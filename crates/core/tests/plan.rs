//! End-to-end tests of a task's plan: its commands, building a block, drafting,
//! proposals, and what an agent is told about the plan.
//!
//! Like the other tests, these run against the practice agent only.

mod common;

use ae_core::server::{AppState, dispatch};
use common::*;
use serde_json::{Value, json};
use std::time::Duration;
use tokio::sync::broadcast::Receiver;

const DEMO: &str = "src/middleware/rateLimit.ts";
const PLAIN_ANSWER: &str = "The limiter allows five attempts a minute and answers 429 after that. Nothing was changed.";

/// A core with one project and one task whose first turn has ended.
struct Bench {
    _sandbox: Sandbox,
    state: AppState,
    events: Receiver<Value>,
    id: String,
    /// The question of the agent that nobody has answered yet.
    open_question: Option<String>,
}

impl Bench {
    async fn new(name: &str, git: bool, first: &str) -> Self {
        let sandbox = Sandbox::new(name);
        let state = AppState::new(sandbox.manager());
        let folder = if git { git_project(&sandbox) } else { sandbox.project() };
        let project = state.manager.add_project(&folder.to_string_lossy(), None).unwrap();
        let mut events = state.manager.subscribe();
        let task = state.manager.create(&project.id, "mock", first).await.unwrap();
        let id = task.row.id.clone();
        until(&mut events, &id, |e| state_is(e, "idle")).await;
        Self { _sandbox: sandbox, state, events, id, open_question: None }
    }

    /// Sends one command of the plan for this task and returns the plan it answers with.
    async fn plan(&self, method: &str, more: Value) -> Value {
        self.ask(method, more).await.unwrap_or_else(|error| panic!("{method}: {error:#}"))
    }

    /// The same for a command that is expected to be refused: the words of the refusal.
    async fn refusal(&self, method: &str, more: Value) -> String {
        self.ask(method, more).await.expect_err("refused").to_string()
    }

    async fn ask(&self, method: &str, more: Value) -> anyhow::Result<Value> {
        let mut params = more;
        params["taskId"] = json!(self.id);
        dispatch(&self.state, &format!("plan.{method}"), &params).await
    }

    async fn until(&mut self, done: impl Fn(&Value) -> bool) -> Vec<Value> {
        let seen = until(&mut self.events, &self.id, done).await;
        for event in &seen {
            if is(event, "permission_request") {
                self.open_question = event["requestId"].as_str().map(str::to_string);
            } else if is(event, "permission_resolved") {
                self.open_question = None;
            }
        }
        seen
    }

    /// Answers the agent's question about its edit, waiting for the question
    /// first when it has not come yet, and then waits for the end of the turn.
    async fn answer(&mut self, option: &str) -> Vec<Value> {
        let mut seen = Vec::new();
        if self.open_question.is_none() {
            seen = self.until(|e| is(e, "permission_request")).await;
        }
        let request = self.open_question.take().expect("the agent asked");
        self.state.manager.answer_permission(&self.id, &request, Some(option.into())).unwrap();
        seen.extend(self.until(|e| state_is(e, "idle")).await);
        seen
    }

    /// Sends a plain message and returns what the agent got and what it answered.
    async fn say(&mut self, text: &str) -> (String, String) {
        self.state.manager.prompt(&self.id, text.into()).await.unwrap();
        let seen = self.until(|e| state_is(e, "idle")).await;
        assert_eq!(seen.iter().find(|e| is(e, "user_message")).unwrap()["text"], text, "the user sees what they typed");
        (thought(&seen), answer_text(&seen))
    }
}

/// What the practice agent says it was sent: it repeats the whole message in its thought.
fn thought(events: &[Value]) -> String {
    let text: String = events
        .iter()
        .filter(|e| e["update"]["sessionUpdate"] == "agent_thought_chunk")
        .filter_map(|e| e["update"]["content"]["text"].as_str())
        .collect();
    text.strip_prefix("The user asked: ").unwrap_or(&text).to_string()
}

fn sent(events: &[Value]) -> String {
    events.iter().find(|e| is(e, "user_message")).expect("a message was sent")["text"].as_str().unwrap().to_string()
}

fn block<'a>(plan: &'a Value, id: &str) -> &'a Value {
    plan["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|block| block["id"] == id)
        .unwrap_or_else(|| panic!("no {id}: {plan}"))
}

fn states(plan: &Value) -> Vec<(String, String)> {
    let blocks = plan["blocks"].as_array().unwrap();
    blocks.iter().map(|b| (b["text"].as_str().unwrap().to_string(), b["state"].as_str().unwrap().to_string())).collect()
}

fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter().map(|(text, state)| (text.to_string(), state.to_string())).collect()
}

#[tokio::test]
async fn a_block_is_built_by_one_turn_and_keeps_the_files_that_turn_changed() {
    let mut bench = Bench::new("plan-build", true, "What does the limiter do?").await;

    // A task starts with an empty plan, and there is nothing to build in it.
    assert_eq!(
        bench.plan("get", json!({})).await,
        json!({
            "rev": 0,
            "blocks": [],
            "arrows": [],
            "only": true,
            "problem": null,
            "busy": null,
            "next": null,
            "nextWhy": "The plan has no blocks yet. Add a step first.",
            "counts": { "built": 0, "total": 0, "building": 0, "planned": 0, "proposed": 0 }
        })
    );
    assert_eq!(bench.refusal("build", json!({})).await, "The plan has no blocks yet. Add a step first.");

    let plan = bench.plan("add", json!({ "kind": "step", "text": "Answer 429" })).await;
    assert_eq!(plan["blockId"], "b1");
    let plan =
        bench.plan("add", json!({ "kind": "step", "text": "Tell the form how long to wait", "after": "b1" })).await;
    assert_eq!(plan["arrows"], json!([{ "from": "b1", "to": "b2", "label": null }]));
    assert_eq!((&plan["next"], &plan["counts"]["total"]), (&json!("b1"), &json!(2)));

    // "Build the next block": the block is marked, and the agent gets the plan and the one block.
    let plan = bench.plan("build", json!({})).await;
    assert_eq!(plan["busy"], json!({ "kind": "build", "blockId": "b1" }));
    assert_eq!(
        (&block(&plan, "b1")["state"], &block(&plan, "b1")["byName"]),
        (&json!("building"), &json!("Practice agent"))
    );
    assert_eq!(plan["next"], Value::Null, "what comes after it has to wait");
    let seen = bench.until(|e| is(e, "permission_request")).await;
    let message = "Build one block of this task's plan, and nothing else. The plan, in the editor's flow format (`*` marks a block that is built already):\n\n```flow\nstep b1: Answer 429\nstep b2: Tell the form how long to wait\nb1 -> b2\n```\n\nBuild only this block: `b1`, “Answer 429”.\n\nIf you think a block is missing from the plan, do not build it. End your answer with one line `PROPOSE: <what the block should do>` for each such block.";
    assert_eq!(sent(&seen), message);
    // The message holds the plan and the rule, so no note about the plan is put in front of it.
    assert_eq!(thought(&seen), message);

    // While a block is being built nothing else can be started, and the block cannot be changed.
    let busy = "Practice agent is building “Answer 429”. Wait for it or stop its turn.";
    assert_eq!(bench.refusal("build", json!({ "blockId": "b2" })).await, busy);
    assert_eq!(bench.refusal("draft", json!({})).await, busy);
    assert!(bench.refusal("edit", json!({ "blockId": "b1", "text": "Answer 418" })).await.contains("being built"));
    assert!(bench.refusal("remove", json!({ "blockId": "b1" })).await.contains("being built"));

    // The turn ends well: the block is built, by the agent, with the file the turn changed.
    let seen = bench.answer("allow-once").await;
    assert!(answer_text(&seen).starts_with("Done."), "{}", answer_text(&seen));
    let plan = bench.plan("get", json!({})).await;
    let built = block(&plan, "b1");
    assert_eq!(
        (&built["state"], &built["by"], &built["byName"]),
        (&json!("built"), &json!("mock"), &json!("Practice agent"))
    );
    assert_eq!(built["files"], json!([{ "path": DEMO, "line": null }]));
    assert!(built["at"].as_u64().is_some_and(|at| at > 0));
    assert_eq!((&plan["busy"], &plan["next"]), (&Value::Null, &json!("b2")));
    assert_eq!(plan["counts"], json!({ "built": 1, "total": 2, "building": 0, "planned": 1, "proposed": 0 }));
    let folder = bench.state.manager.cwd(&bench.id).unwrap();
    assert!(std::fs::read_to_string(folder.join(DEMO)).unwrap().contains("limit: limits.login,"));

    // The next block now. The plan it is sent marks what is built.
    bench.plan("build", json!({})).await;
    let seen = bench.answer("allow-once").await;
    assert!(
        sent(&seen).contains("```flow\nstep* b1: Answer 429\nstep b2: Tell the form how long to wait\nb1 -> b2\n```")
    );
    assert!(sent(&seen).contains("Build only this block: `b2`, “Tell the form how long to wait”."));
    let plan = bench.plan("get", json!({})).await;
    assert_eq!(states(&plan), pairs(&[("Answer 429", "built"), ("Tell the form how long to wait", "built")]));
    assert_eq!(block(&plan, "b2")["files"], json!([{ "path": DEMO, "line": null }]));
    assert_eq!(
        bench.refusal("build", json!({})).await,
        "Nothing is left to build: every block of the plan is built or yours."
    );

    // A built block whose text changes goes back to planned, and is told apart when it is built again.
    let plan = bench.plan("edit", json!({ "blockId": "b1", "details": "Say in Retry-After how long to wait." })).await;
    let again = block(&plan, "b1");
    assert_eq!((&again["state"], &again["by"], &again["files"]), (&json!("planned"), &Value::Null, &json!([])));
    assert_eq!(plan["next"], "b1");
    bench.plan("build", json!({ "blockId": "b1" })).await;
    let seen = bench.answer("allow-once").await;
    assert!(
        sent(&seen).contains(
            "Build only this block: `b1`, “Answer 429”.\nWhat it should do: Say in Retry-After how long to wait.\nIt was built before, when it read: Answer 429\nChange what was built so that it fits how the block reads now.\n\n"
        ),
        "{}",
        sent(&seen)
    );
    let plan = bench.plan("get", json!({})).await;
    assert_eq!(block(&plan, "b1")["state"], "built");

    // What is not a block of the plan is not built, and a task that is not there has no plan.
    assert_eq!(bench.refusal("build", json!({ "blockId": "b9" })).await, "The plan has no such block");
    let nobody = dispatch(&bench.state, "plan.get", &json!({ "taskId": "nope" })).await.unwrap_err().to_string();
    assert!(!nobody.is_empty());
}

#[tokio::test]
async fn a_turn_that_is_stopped_or_fails_puts_the_block_back() {
    let mut bench = Bench::new("plan-stop", true, "What does the limiter do?").await;
    bench.plan("add", json!({ "kind": "step", "text": "Answer 429" })).await;

    // Stopped by the user while the agent waits for an answer.
    bench.plan("build", json!({})).await;
    bench.until(|e| is(e, "permission_request")).await;
    bench.state.manager.cancel(&bench.id).unwrap();
    let seen = bench.until(|e| state_is(e, "idle")).await;
    assert!(seen.iter().any(|e| is(e, "turn_end") && e["stopReason"] == "cancelled"));
    let plan = bench.plan("get", json!({})).await;
    let back = block(&plan, "b1");
    assert_eq!((&back["state"], &back["by"], &plan["busy"]), (&json!("planned"), &Value::Null, &Value::Null));
    assert_eq!(plan["next"], "b1");

    // The agent's process ends in the middle of the turn.
    bench.plan("build", json!({})).await;
    bench.until(|e| is(e, "permission_request")).await;
    bench.state.manager.stop_agent(&bench.id).await.unwrap();
    bench.until(|e| state_is(e, "asleep")).await;
    let mut plan = Value::Null;
    for _ in 0..200 {
        plan = bench.plan("get", json!({})).await;
        if plan["busy"].is_null() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!((&block(&plan, "b1")["state"], &plan["busy"]), (&json!("planned"), &Value::Null));

    // A turn that ends by itself counts as built even when the agent changed nothing: the block says so.
    bench.plan("build", json!({})).await;
    let seen = bench.answer("reject-once").await;
    assert!(answer_text(&seen).contains("the edit was not allowed"));
    let plan = bench.plan("get", json!({})).await;
    assert_eq!((&block(&plan, "b1")["state"], &block(&plan, "b1")["files"]), (&json!("built"), &json!([])));

    // "I will do it myself": the block is the user's and no agent builds it.
    let plan = bench.plan("take", json!({ "blockId": "b1" })).await;
    assert_eq!((&block(&plan, "b1")["state"], &block(&plan, "b1")["by"]), (&json!("yours"), &Value::Null));
    assert_eq!(
        bench.refusal("build", json!({ "blockId": "b1" })).await,
        "You took this block for yourself. Give it back first."
    );
    let plan = bench.plan("take", json!({ "blockId": "b1", "mine": false })).await;
    assert_eq!(block(&plan, "b1")["state"], "planned");
}

#[tokio::test]
async fn in_a_folder_without_git_the_files_are_those_the_agents_edits_name() {
    let mut bench = Bench::new("plan-plain", false, "What is in this folder?").await;
    bench.plan("add", json!({ "kind": "step", "text": "Keep notes" })).await;
    bench.plan("build", json!({})).await;
    bench.answer("allow-once").await;
    let plan = bench.plan("get", json!({})).await;
    assert_eq!(block(&plan, "b1")["state"], "built");
    assert_eq!(block(&plan, "b1")["files"], json!([{ "path": "NOTES.md", "line": null }]));
}

#[tokio::test]
async fn a_draft_arrives_as_proposed_blocks_and_a_drawing_of_the_code_as_what_was_there() {
    let mut bench = Bench::new("plan-draft", true, "Describe the limiter of /login\n\nFive attempts a minute.").await;

    let plan = bench.plan("draft", json!({})).await;
    assert_eq!(plan["busy"], json!({ "kind": "draft" }));
    assert_eq!(
        bench.refusal("build", json!({})).await,
        "Practice agent is drawing the plan. Wait for it or stop its turn."
    );
    let seen = bench.until(|e| state_is(e, "idle")).await;
    let request = sent(&seen);
    assert!(request.starts_with("Draft a plan for this task as a flow chart: what the task is meant to become, step by step. Do not change any file.\n\nThe task, as it was first described:\n> Describe the limiter of /login\n> \n> Five attempts a minute.\n\nAnswer with one block in exactly this format"), "{request}");
    assert!(!seen.iter().any(|e| is(e, "permission_request")), "a draft changes no file");

    // The flow the practice agent draws is in the plan, and every block of it waits for a yes.
    let plan = bench.plan("get", json!({})).await;
    assert_eq!(
        states(&plan),
        pairs(&[
            ("Request to POST /login", "proposed"),
            ("Over the limit?", "proposed"),
            ("Respond 429", "proposed"),
            ("Check the password", "proposed"),
        ])
    );
    assert_eq!(block(&plan, "b2")["kind"], "ask");
    assert_eq!((&block(&plan, "b2")["from"], &block(&plan, "b2")["by"]), (&json!("draft"), &json!("mock")));
    assert_eq!(
        plan["arrows"],
        json!([
            { "from": "b1", "to": "b2", "label": null },
            { "from": "b2", "to": "b3", "label": "yes" },
            { "from": "b2", "to": "b4", "label": "no" }
        ])
    );
    assert_eq!(plan["counts"], json!({ "built": 0, "total": 4, "building": 0, "planned": 0, "proposed": 4 }));
    assert_eq!((&plan["busy"], &plan["problem"], &plan["next"]), (&Value::Null, &Value::Null, &Value::Null));
    assert_eq!(
        bench.refusal("build", json!({})).await,
        "The blocks that are left wait for your yes. Add them to the plan first."
    );
    assert!(bench.refusal("build", json!({ "blockId": "b1" })).await.contains("not in the plan yet"));

    // One by one, or all at once.
    let plan = bench.plan("accept", json!({ "blockId": "b1" })).await;
    assert_eq!((&block(&plan, "b1")["state"], &block(&plan, "b1")["from"]), (&json!("planned"), &Value::Null));
    assert_eq!((&plan["next"], &plan["counts"]["proposed"]), (&json!("b1"), &json!(3)));
    let plan = bench.plan("decline", json!({ "blockId": "b4" })).await;
    assert_eq!(plan["blocks"].as_array().unwrap().len(), 3);
    assert_eq!(plan["arrows"].as_array().unwrap().len(), 2, "its arrow went with it");
    let plan = bench.plan("accept", json!({})).await;
    assert_eq!(plan["counts"], json!({ "built": 0, "total": 3, "building": 0, "planned": 3, "proposed": 0 }));

    // "Draw from the code": what is there now, each step with its place, and nothing of it to build.
    bench.plan("draft", json!({ "from": "code" })).await;
    let seen = bench.until(|e| state_is(e, "idle")).await;
    assert!(sent(&seen).starts_with("Draw the code this task is about as a flow chart: what happens in it now"));
    // By now the task has a plan, and the agent is shown it with this message as with any other.
    assert!(thought(&seen).starts_with("Note from the editor: this task is built from a plan"), "{}", thought(&seen));
    assert!(thought(&seen).contains("\n\nDraw the code this task is about"));
    let plan = bench.plan("get", json!({})).await;
    let drawn: Vec<&Value> = plan["blocks"].as_array().unwrap().iter().filter(|b| b["from"] == "code").collect();
    assert_eq!(drawn.len(), 4);
    assert!(drawn.iter().all(|b| b["state"] == "before"));
    assert_eq!(drawn[1]["files"], json!([{ "path": DEMO, "line": 3 }]));
    assert_eq!(plan["counts"]["total"], 3, "what was there before is not counted as work of the plan");
}

#[tokio::test]
async fn a_block_the_agent_proposes_waits_for_a_yes_or_a_no() {
    let mut bench = Bench::new("plan-propose", true, "What does the limiter do?").await;
    bench.plan("add", json!({ "kind": "step", "text": "Count against the limit" })).await;

    // Building a block about a limit, the practice agent misses a block and says so.
    bench.plan("build", json!({})).await;
    let seen = bench.answer("allow-once").await;
    assert!(
        answer_text(&seen).ends_with("\n\nPROPOSE: Reset the count after a correct password"),
        "{}",
        answer_text(&seen)
    );
    let plan = bench.plan("get", json!({})).await;
    assert_eq!(
        states(&plan),
        pairs(&[("Count against the limit", "built"), ("Reset the count after a correct password", "proposed")])
    );
    let proposed = block(&plan, "b2");
    assert_eq!(
        (&proposed["from"], &proposed["by"], &proposed["kind"]),
        (&json!("build"), &json!("mock"), &json!("step"))
    );
    assert_eq!(
        plan["arrows"],
        json!([{ "from": "b1", "to": "b2", "label": null }]),
        "it hangs after the block that was built"
    );
    assert_eq!(plan["counts"], json!({ "built": 1, "total": 2, "building": 0, "planned": 0, "proposed": 1 }));
    assert_eq!(plan["next"], Value::Null, "nothing is built until the user says yes");

    // "Not needed": it leaves the plan.
    let plan = bench.plan("decline", json!({ "blockId": "b2" })).await;
    assert_eq!(states(&plan), pairs(&[("Count against the limit", "built")]));
    assert_eq!(plan["arrows"], json!([]));

    // The same session does not propose it a second time.
    bench.plan("build", json!({ "blockId": "b1" })).await;
    let seen = bench.answer("allow-once").await;
    assert!(!answer_text(&seen).contains("PROPOSE"));
    assert_eq!(bench.plan("get", json!({})).await["blocks"].as_array().unwrap().len(), 1);

    // A new session of the agent does. "Add it to the plan": it is planned, and next.
    bench.state.manager.stop_agent(&bench.id).await.unwrap();
    bench.until(|e| state_is(e, "asleep")).await;
    bench.plan("build", json!({ "blockId": "b1" })).await;
    bench.answer("allow-once").await;
    let plan = bench.plan("get", json!({})).await;
    assert_eq!(block(&plan, "b3")["state"], "proposed");
    let plan = bench.plan("accept", json!({ "blockId": "b3" })).await;
    assert_eq!((&block(&plan, "b3")["state"], &plan["next"]), (&json!("planned"), &json!("b3")));
    // A block that is in the plan is not proposed again.
    bench.state.manager.stop_agent(&bench.id).await.unwrap();
    bench.until(|e| state_is(e, "asleep")).await;
    bench.plan("build", json!({ "blockId": "b1" })).await;
    let seen = bench.answer("allow-once").await;
    assert!(answer_text(&seen).contains("PROPOSE: Reset the count"));
    assert_eq!(bench.plan("get", json!({})).await["blocks"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn an_agent_is_told_the_plan_only_while_the_switch_is_on() {
    let mut bench = Bench::new("plan-note", true, "What does the limiter do?").await;

    // No plan yet: nothing is said about one, although the switch is on.
    assert_eq!(bench.say("And who calls it?").await.0, "And who calls it?");

    bench.plan("add", json!({ "kind": "step", "text": "Answer 429" })).await;
    bench.plan("add", json!({ "kind": "note", "text": "Never hardcode the limit" })).await;
    let (told, answer) = bench.say("What else is there?").await;
    assert_eq!(
        told,
        "Note from the editor: this task is built from a plan, shown here in the editor's flow format (`*` marks a block that is built already). Build nothing that is not a block of this plan; if something is missing, do not build it, and end your answer with a line `PROPOSE: <what the block should do>` instead.\n```flow\nstep b1: Answer 429\n```\nNotes on the plan:\n- Never hardcode the limit\n\nWhat else is there?"
    );
    // The plan in front of a message is something to keep to, not a request to draw a flow.
    assert_eq!(answer, PLAIN_ANSWER);

    // Off: the same plan, and not a word about it.
    let plan = bench.plan("only", json!({ "on": false })).await;
    assert_eq!(plan["only"], false);
    assert_eq!(bench.say("And now?").await.0, "And now?");
    // A block can still be built: the message that builds it holds the plan itself.
    bench.plan("build", json!({})).await;
    let seen = bench.answer("allow-once").await;
    assert!(thought(&seen).starts_with("Build one block of this task's plan, and nothing else."));

    // On again, with a plan that has no step in it: there is nothing to hold an agent to.
    let plan = bench.plan("only", json!({ "on": true })).await;
    assert_eq!(plan["only"], true);
    assert!(bench.say("Once more?").await.0.starts_with("Note from the editor: this task is built from a plan"));
    bench.plan("remove", json!({ "blockId": "b1" })).await;
    assert_eq!(bench.say("And last?").await.0, "And last?");
}

#[tokio::test]
async fn every_change_is_told_to_all_windows_and_survives_the_core() {
    let sandbox = Sandbox::new("plan-kept");
    let state = AppState::new(sandbox.manager());
    let project = state.manager.add_project(&sandbox.project().to_string_lossy(), None).unwrap();
    let mut events = state.manager.subscribe();
    let task = state.manager.create(&project.id, "mock", "What is in this folder?").await.unwrap();
    let id = task.row.id.clone();
    until(&mut events, &id, |e| state_is(e, "idle")).await;

    /// The next "the plan changed" the core sends out.
    async fn changed(events: &mut Receiver<Value>) -> Value {
        let wait = async {
            loop {
                let message = events.recv().await.expect("event stream stays open");
                if message["event"] == "plan.changed" {
                    return message["data"].clone();
                }
            }
        };
        tokio::time::timeout(Duration::from_secs(20), wait).await.expect("the change is told")
    }
    let ask = |method: &'static str, mut params: Value| {
        params["taskId"] = json!(id);
        let state = state.clone();
        async move { dispatch(&state, method, &params).await }
    };

    // Each command answers with the plan, and the same plan goes out to every window.
    let answered = ask("plan.add", json!({ "kind": "ask", "text": "Over the limit?" })).await.unwrap();
    let told = changed(&mut events).await;
    assert_eq!(told["taskId"], json!(id));
    assert_eq!(told["plan"]["blocks"], answered["blocks"]);
    // Each change makes the plan one newer, so a window never takes an older plan for the newest.
    assert_eq!((&told["plan"]["rev"], &answered["rev"]), (&json!(1), &json!(1)));
    ask("plan.add", json!({ "kind": "step", "text": "Respond 429", "after": "b1", "label": "yes" })).await.unwrap();
    assert_eq!(changed(&mut events).await["plan"]["arrows"], json!([{ "from": "b1", "to": "b2", "label": "yes" }]));
    ask("plan.add", json!({ "kind": "step", "text": "Check the password" })).await.unwrap();
    changed(&mut events).await;
    ask("plan.connect", json!({ "from": "b1", "to": "b3", "label": "no" })).await.unwrap();
    assert_eq!(changed(&mut events).await["plan"]["arrows"].as_array().unwrap().len(), 2);
    ask("plan.disconnect", json!({ "from": "b1", "to": "b3" })).await.unwrap();
    assert_eq!(changed(&mut events).await["plan"]["arrows"].as_array().unwrap().len(), 1);
    ask("plan.edit", json!({ "blockId": "b3", "text": "Check the password hash" })).await.unwrap();
    assert_eq!(changed(&mut events).await["plan"]["blocks"][2]["text"], "Check the password hash");
    ask("plan.remove", json!({ "blockId": "b3" })).await.unwrap();
    assert_eq!(changed(&mut events).await["plan"]["blocks"].as_array().unwrap().len(), 2);

    // What is refused says why, and tells nobody anything.
    for (method, params, why) in [
        ("plan.add", json!({ "kind": "circle" }), "a step, a question"),
        ("plan.add", json!({ "kind": "step", "text": "x".repeat(61) }), "at most 60 characters"),
        ("plan.connect", json!({ "from": "b1", "to": "b1" }), "cannot come after itself"),
        ("plan.connect", json!({ "from": "b1" }), "Missing parameter: to"),
        ("plan.edit", json!({ "blockId": "b7", "text": "x" }), "no such block"),
        ("plan.accept", json!({ "blockId": "b1" }), "in the plan already"),
        ("plan.decline", json!({ "blockId": "b1" }), "remove it instead"),
        ("plan.take", json!({ "blockId": "b1", "mine": false }), "not yours"),
    ] {
        let refused = ask(method, params).await.unwrap_err().to_string();
        assert!(refused.contains(why), "{method}: {refused}");
    }
    assert!(
        dispatch(&state, "plan.get", &json!({})).await.unwrap_err().to_string().contains("Missing parameter: taskId")
    );

    // A core that stopped while a block was being built comes back with the block planned again.
    let mut kept: Value = serde_json::from_str(
        &sandbox.store().setting(&format!("task.{id}.plan")).unwrap().expect("the plan is stored"),
    )
    .unwrap();
    kept["blocks"][0]["state"] = json!("building");
    sandbox.store().set_setting(&format!("task.{id}.plan"), Some(&kept.to_string())).unwrap();
    state.manager.stop_all().await;
    let again = AppState::new(sandbox.manager());
    let plan = dispatch(&again, "plan.get", &json!({ "taskId": id })).await.unwrap();
    assert_eq!(states(&plan), pairs(&[("Over the limit?", "planned"), ("Respond 429", "planned")]));
    assert_eq!(plan["busy"], Value::Null);
}
