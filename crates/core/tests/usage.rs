//! What the agents cost, the daily budget, and handing a task to another agent.
//!
//! Only practice agents run here. One of them says each turn cost a quarter; nothing is charged.

mod common;

use common::*;
use serde_json::Value;

fn agent<'a>(summary: &'a Value, id: &str) -> &'a Value {
    summary["agents"].as_array().unwrap().iter().find(|agent| agent["id"] == id).expect("the agent is listed")
}

#[tokio::test]
async fn cost_is_added_up_by_agent_task_and_day() {
    let sandbox = Sandbox::new("usage");
    let manager = sandbox.manager();
    let project = manager.add_project(&sandbox.project().to_string_lossy(), None).unwrap();
    let mut events = manager.subscribe();
    let task = manager.create(&project.id, "mock-paid", "What does the limiter do?").await.unwrap();
    let id = task.row.id.clone();
    until(&mut events, &id, |e| state_is(e, "idle")).await;
    manager.prompt(&id, "And who calls it?".into()).await.unwrap();
    until(&mut events, &id, |e| state_is(e, "idle")).await;

    // Two turns of one session: the session says 0.25, then 0.50. Spent: 0.50, not 0.75.
    let summary = manager.usage_summary(0, 0).unwrap();
    let paid = agent(&summary, "mock-paid");
    assert_eq!(
        (paid["today"].as_f64(), paid["week"].as_f64(), paid["total"].as_f64()),
        (Some(0.5), Some(0.5), Some(0.5))
    );
    assert_eq!(
        (paid["tasks"].as_u64(), paid["working"].as_u64(), paid["spendsTokens"].as_bool()),
        (Some(1), Some(0), Some(true))
    );
    assert_eq!(summary["today"], 0.5);
    assert_eq!(summary["tasks"][0]["taskId"], id.as_str());
    assert_eq!(summary["tasks"][0]["today"], 0.5);
    assert_eq!(summary["tasks"][0]["agents"], serde_json::json!(["mock-paid"]));
    assert_eq!(agent(&summary, "mock")["today"], 0.0);

    // A new session counts from zero again; what the old one cost is not lost.
    manager.stop_agent(&id).await.unwrap();
    until(&mut events, &id, |e| state_is(e, "asleep")).await;
    manager.prompt(&id, "Thanks".into()).await.unwrap();
    until(&mut events, &id, |e| state_is(e, "idle")).await;
    let summary = manager.usage_summary(0, 0).unwrap();
    assert_eq!(agent(&summary, "mock-paid")["total"], 0.75);

    // "Today" and "this week" are counted from the moments the interface names.
    let later = u64::MAX / 2;
    let summary = manager.usage_summary(later, 0).unwrap();
    assert_eq!((summary["today"].as_f64(), agent(&summary, "mock-paid")["week"].as_f64()), (Some(0.0), Some(0.75)));
    assert!(summary["tasks"].as_array().unwrap().is_empty(), "only tasks that cost something today are listed");

    // An archived task still counts: the money was spent.
    manager.archive(&id).await.unwrap();
    let summary = manager.usage_summary(0, 0).unwrap();
    assert_eq!(summary["today"], 0.75);
    assert_eq!(summary["tasks"][0]["archived"], true);
    assert_eq!(agent(&summary, "mock-paid")["tasks"], 0);
}

#[tokio::test]
async fn the_daily_budget_is_kept() {
    let sandbox = Sandbox::new("budget");
    let manager = sandbox.manager();
    assert_eq!(manager.budget(), serde_json::json!({ "daily": null, "ask": true }));
    manager.set_budget(Some(Some(5.5)), Some(false)).unwrap();
    assert_eq!(manager.budget(), serde_json::json!({ "daily": 5.5, "ask": false }));
    // Leaving one out leaves it as it is; `null` takes the budget away.
    manager.set_budget(None, Some(true)).unwrap();
    assert_eq!(manager.budget(), serde_json::json!({ "daily": 5.5, "ask": true }));
    manager.set_budget(Some(None), None).unwrap();
    assert_eq!(manager.budget()["daily"], Value::Null);
    assert!(manager.set_budget(Some(Some(-1.0)), None).is_err());
    assert_eq!(manager.usage_summary(0, 0).unwrap()["budget"]["ask"], true);
}

#[tokio::test]
async fn a_task_is_handed_to_another_agent_which_is_told_what_was_said() {
    let sandbox = Sandbox::new("handover");
    let manager = sandbox.manager();
    let project = manager.add_project(&sandbox.project().to_string_lossy(), None).unwrap();
    let mut events = manager.subscribe();
    let task = manager.create(&project.id, "mock-paid", "What does the limiter do?").await.unwrap();
    let id = task.row.id.clone();
    until(&mut events, &id, |e| state_is(e, "idle")).await;

    // What cannot be: an agent that is not there, one that is not installed, a handover in the middle of a turn.
    assert!(manager.switch_agent(&id, "nobody").await.unwrap_err().to_string().contains("No such agent"));
    let error = manager.switch_agent(&id, "mock-missing").await.unwrap_err().to_string();
    assert!(error.contains("not available on this computer: Install it first"), "{error}");
    manager.prompt(&id, "Fix the limit".into()).await.unwrap();
    until(&mut events, &id, |e| is(e, "permission_request")).await;
    let error = manager.switch_agent(&id, "mock").await.unwrap_err().to_string();
    assert!(error.contains("Stop its turn first"), "{error}");
    manager.cancel(&id).unwrap();
    until(&mut events, &id, |e| state_is(e, "idle")).await;

    let mut windows = manager.subscribe();
    let handed = manager.switch_agent(&id, "mock").await.unwrap();
    assert_eq!(handed.row.agent_id, "mock");
    let seen = until(&mut events, &id, |e| is(e, "agent_switched")).await;
    let told = seen.last().unwrap();
    assert_eq!((told["from"].as_str(), told["toName"].as_str()), (Some("mock-paid"), Some("Practice agent")));
    assert_eq!(told["fromName"], "Paid practice agent");
    let mut updated = false;
    while let Ok(message) = windows.try_recv() {
        updated |= message["event"] == "task.updated" && message["data"]["agentId"] == "mock";
    }
    assert!(updated, "every window learns who has the task now");
    assert_eq!(manager.task(&id).unwrap().state, ae_core::tasks::TaskState::Asleep);

    // The new agent starts with the next message and is told the conversation so far.
    manager.prompt(&id, "And who calls it?".into()).await.unwrap();
    let seen = until(&mut events, &id, |e| state_is(e, "idle")).await;
    let started = seen.iter().find(|e| is(e, "agent_started")).unwrap();
    assert_eq!((started["agentId"].as_str(), started["loaded"].as_bool()), (Some("mock"), Some(false)));
    let thought: String = seen
        .iter()
        .filter(|e| e["update"]["sessionUpdate"] == "agent_thought_chunk")
        .filter_map(|e| e["update"]["content"]["text"].as_str())
        .collect();
    assert!(thought.contains("continues an earlier conversation"), "{thought}");
    assert!(thought.contains("User: What does the limiter do?"), "{thought}");

    // The bill is split by who did the work: the first agent's quarters stay its own.
    let summary = manager.usage_summary(0, 0).unwrap();
    assert!(agent(&summary, "mock-paid")["total"].as_f64().unwrap() >= 0.25);
    assert_eq!(agent(&summary, "mock")["total"], 0.0);
    // Handing a task to the agent it has changes nothing.
    manager.switch_agent(&id, "mock").await.unwrap();
    assert_eq!(manager.history(&id).unwrap().iter().filter(|e| is(e, "agent_switched")).count(), 1);
}
