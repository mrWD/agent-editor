//! Rules of a project: kept by the editor and told to every agent that works in it.

mod common;

use ae_core::rules::ALL_PROJECTS;
use common::*;
use serde_json::Value;

fn thought(events: &[Value]) -> String {
    events
        .iter()
        .filter(|e| e["update"]["sessionUpdate"] == "agent_thought_chunk")
        .filter_map(|e| e["update"]["content"]["text"].as_str())
        .collect()
}

#[tokio::test]
async fn rules_are_kept_per_project_and_for_all_projects() {
    let sandbox = Sandbox::new("rules");
    let manager = sandbox.manager();
    let project = manager.add_project(&sandbox.project().to_string_lossy(), None).unwrap();

    let rule = manager.rule_add(&project.id, "  Use pnpm,\n never npm. ", "you", vec![]).unwrap();
    assert_eq!((rule.text.as_str(), rule.from.as_str()), ("Use pnpm, never npm.", "you"));
    manager.rule_add(ALL_PROJECTS, "Write a failing test before fixing a bug.", "edit", vec!["mock".into()]).unwrap();

    // What cannot be: an empty rule, the same rule twice, a project or an agent that is not there.
    assert!(manager.rule_add(&project.id, "   ", "you", vec![]).unwrap_err().to_string().contains("empty"));
    assert!(
        manager
            .rule_add(&project.id, "Use pnpm, never npm.", "you", vec![])
            .unwrap_err()
            .to_string()
            .contains("already")
    );
    assert!(manager.rule_add("no-such-project", "x", "you", vec![]).is_err());
    assert!(
        manager
            .rule_add(&project.id, "x", "you", vec!["nobody".into()])
            .unwrap_err()
            .to_string()
            .contains("No such agent")
    );

    let lists = manager.all_rules().unwrap();
    assert_eq!(lists["lists"][0]["scope"], ALL_PROJECTS);
    assert_eq!(lists["lists"][0]["rules"][0]["from"], "edit");
    assert_eq!(lists["lists"][1]["rules"][0]["text"], "Use pnpm, never npm.");

    manager.rule_remove(&project.id, &rule.id).unwrap();
    assert!(manager.rule_remove(&project.id, &rule.id).is_err());
    assert!(manager.rules(&project.id).is_empty());

    // The question after a correction can be turned off for a project.
    assert!(manager.rule_offers(&project.id));
    manager.set_rule_offers(&project.id, false).unwrap();
    assert!(!manager.rule_offers(&project.id));
}

#[tokio::test]
async fn an_agent_is_told_the_rules_once_and_again_when_they_change() {
    let sandbox = Sandbox::new("rules-told");
    let manager = sandbox.manager();
    let project = manager.add_project(&sandbox.project().to_string_lossy(), None).unwrap();
    manager.rule_add(&project.id, "Use pnpm, never npm.", "you", vec![]).unwrap();
    manager.rule_add(ALL_PROJECTS, "Only for another agent.", "you", vec!["mock-load".into()]).unwrap();

    let mut events = manager.subscribe();
    let task = manager.create(&project.id, "mock", "What does the limiter do?").await.unwrap();
    let id = task.row.id.clone();
    let seen = until(&mut events, &id, |e| state_is(e, "idle")).await;
    let told = thought(&seen);
    assert!(told.contains("the user has set these rules for this project"), "{told}");
    assert!(told.contains("- Use pnpm, never npm."), "{told}");
    assert!(!told.contains("Only for another agent"), "a rule for one agent is not told to another: {told}");
    // The user sees only what they typed.
    assert_eq!(seen.iter().find(|e| is(e, "user_message")).unwrap()["text"], "What does the limiter do?");

    // The same session is not told twice.
    manager.prompt(&id, "And who calls it?".into()).await.unwrap();
    let seen = until(&mut events, &id, |e| state_is(e, "idle")).await;
    assert_eq!(thought(&seen), "The user asked: And who calls it?");

    // A new rule is told with the next message, together with the ones that stand.
    manager.rule_add(&project.id, "Never hardcode limits.", "edit", vec![]).unwrap();
    manager.prompt(&id, "Thanks".into()).await.unwrap();
    let told = thought(&until(&mut events, &id, |e| state_is(e, "idle")).await);
    assert!(told.contains("- Use pnpm, never npm.") && told.contains("- Never hardcode limits."), "{told}");

    // A new session of the agent knows nothing: it is told again.
    manager.stop_agent(&id).await.unwrap();
    until(&mut events, &id, |e| state_is(e, "asleep")).await;
    manager.prompt(&id, "Once more".into()).await.unwrap();
    let told = thought(&until(&mut events, &id, |e| state_is(e, "idle")).await);
    assert!(told.contains("- Never hardcode limits."), "{told}");

    // With no rules left, nothing is said about rules.
    for rule in manager.rules(&project.id) {
        manager.rule_remove(&project.id, &rule.id).unwrap();
    }
    manager.prompt(&id, "Last one".into()).await.unwrap();
    assert_eq!(thought(&until(&mut events, &id, |e| state_is(e, "idle")).await), "The user asked: Last one");
}
