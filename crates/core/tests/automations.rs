//! Automations: what they are, turning them on, and the ones that go by the clock.
//!
//! Only the practice agent runs here.

mod common;

use ae_core::automations::{Then, When};
use common::*;

const DAY: u64 = 24 * 60 * 60 * 1000;
const MINUTE: u64 = 60 * 1000;
/// Midnight UTC of some day, far from any real date the tests could run on.
const SOME_DAY: u64 = 20_000 * DAY;

fn daily(at: &str) -> When {
    When::Daily { at: at.into() }
}
fn start(text: &str) -> Then {
    Then::StartTask { agent_id: "mock".into(), text: text.into() }
}

#[tokio::test]
async fn the_built_in_ones_are_there_and_off_and_the_users_own_are_kept() {
    let sandbox = Sandbox::new("automations");
    let manager = sandbox.manager();
    let project = manager.add_project(&sandbox.project().to_string_lossy(), None).unwrap();

    let all = manager.automations();
    let brief: Vec<(&str, bool, bool)> = all.iter().map(|one| (one.id.as_str(), one.built_in, one.on)).collect();
    assert_eq!(brief, [("clean-up", true, false), ("e2e", true, false)]);
    assert_eq!(all[0].when, When::Merged);
    assert_eq!(all[0].then, Then::CleanUp);

    manager.automation_turn("clean-up", true).unwrap();
    assert!(manager.automations()[0].on);
    assert!(manager.automation_remove("clean-up").is_err(), "a built-in one can only be turned off");
    assert!(manager.automation_turn("nope", true).is_err());

    let id = manager
        .automation_save(
            None,
            " Update dependencies ",
            daily("03:00"),
            start("Update the dependencies"),
            vec![project.id.clone()],
        )
        .unwrap();
    let own = manager.automations().into_iter().find(|one| one.id == id).unwrap();
    assert_eq!((own.name.as_str(), own.on, own.built_in), ("Update dependencies", true, false));
    assert_eq!(own.projects, vec![project.id.clone()]);
    // Changing it keeps its id; a built-in one cannot be changed this way.
    manager.automation_save(Some(&id), "Nightly update", daily("04:30"), start("Update them"), vec![]).unwrap();
    let own = manager.automations().into_iter().find(|one| one.id == id).unwrap();
    assert_eq!((own.name.as_str(), own.when.clone()), ("Nightly update", daily("04:30")));
    assert!(manager.automation_save(Some("clean-up"), "x", When::Merged, Then::CleanUp, vec![]).is_err());

    // What makes no sense is refused with a reason.
    for (name, when, then, projects, why) in [
        ("", daily("03:00"), start("x"), vec![], "Give the automation a name"),
        ("n", daily("25:00"), start("x"), vec![], "hours and minutes"),
        ("n", When::IssueLabeled { label: " ".into() }, start("x"), vec![], "label"),
        ("n", daily("03:00"), start("  "), vec![], "what the agent should do"),
        ("n", daily("03:00"), Then::StartTask { agent_id: "nobody".into(), text: "x".into() }, vec![], "No such agent"),
        ("n", daily("03:00"), Then::CleanUp, vec![], "do not go together"),
        ("n", When::Merged, start("x"), vec![], "do not go together"),
        ("n", daily("03:00"), start("x"), vec!["no-such-project".to_string()], "No such project"),
    ] {
        let error = manager.automation_save(None, name, when, then, projects).unwrap_err().to_string();
        assert!(error.contains(why), "{error}");
    }

    manager.automation_remove(&id).unwrap();
    assert_eq!(manager.automations().len(), 2);
    // The choices survive a restart of the core.
    drop(manager);
    assert!(sandbox.manager().automations()[0].on);
}

#[tokio::test]
async fn a_daily_automation_starts_its_task_once_a_day_at_its_time() {
    let sandbox = Sandbox::new("automations-daily");
    let manager = sandbox.manager();
    let project = manager.add_project(&sandbox.project().to_string_lossy(), None).unwrap();
    let id = manager
        .automation_save(None, "Morning look", daily("03:00"), start("What does the limiter do?"), vec![])
        .unwrap();
    let tasks = || manager.list().unwrap().len();
    let mut events = manager.subscribe();

    // Before its time nothing happens.
    manager.automations_tick(SOME_DAY + 179 * MINUTE).await;
    assert_eq!(tasks(), 0);

    // At its time a task is started, like one the user starts, and the run is written down.
    manager.automations_tick(SOME_DAY + 180 * MINUTE).await;
    assert_eq!(tasks(), 1);
    let task = manager.list().unwrap().remove(0);
    assert_eq!(
        (task.row.project_id.as_str(), task.row.title.as_str()),
        (project.id.as_str(), "What does the limiter do?")
    );
    until(&mut events, &task.row.id, |e| state_is(e, "idle")).await;
    let own = manager.automations().into_iter().find(|one| one.id == id).unwrap();
    assert!(own.last_run.is_some());
    assert_eq!(own.runs_that_day, 1);
    assert_eq!(own.last_note.as_deref(), Some("Started “What does the limiter do?” in project"));

    // Later the same day it does not run again; the next day it does, also when the core was off at its time.
    manager.automations_tick(SOME_DAY + 600 * MINUTE).await;
    assert_eq!(tasks(), 1);
    manager.automations_tick(SOME_DAY + DAY + 700 * MINUTE).await;
    assert_eq!(tasks(), 2);

    // Turned off, it stays quiet.
    manager.automation_turn(&id, false).unwrap();
    manager.automations_tick(SOME_DAY + 2 * DAY + 200 * MINUTE).await;
    assert_eq!(tasks(), 2);
}

#[tokio::test]
async fn the_time_is_the_time_on_the_users_clock() {
    let sandbox = Sandbox::new("automations-clock");
    let manager = sandbox.manager();
    manager.add_project(&sandbox.project().to_string_lossy(), None).unwrap();
    manager.automation_save(None, "Morning look", daily("03:00"), start("What does the limiter do?"), vec![]).unwrap();
    // The user's clock is two hours ahead: their 03:00 is 01:00 for the core.
    manager.set_clock(120).unwrap();
    assert!(manager.set_clock(99 * 60).is_err());

    manager.automations_tick(SOME_DAY + 59 * MINUTE).await;
    assert_eq!(manager.list().unwrap().len(), 0, "02:59 for the user");
    manager.automations_tick(SOME_DAY + 60 * MINUTE).await;
    assert_eq!(manager.list().unwrap().len(), 1, "03:00 for the user");
}
