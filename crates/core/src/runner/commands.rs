//! The commands of the Run tab, as the interface sends them.
//!
//! Every one takes `taskId`. "The state" is `RunState`: the services, the
//! profiles, the packages and the databases.
//!
//! - `run.state` gives the state. So do the ones that change it:
//!   `run.start { service? }`, `run.restart { service? }` and `run.stop { service? }`
//!   (without `service`, start and restart take the chosen profile and stop takes everything),
//!   `run.profile { name }`, `run.saveProfile { name, services, was? }`, `run.removeProfile { name }`,
//!   `deps.install` and `db.reset { service }`.
//! - `run.logs { service }` and `run.shell { service }` give `{ termId }`.
//! - `env.get` and `env.set { name, value }` give the task's environment with
//!   every secret masked; `value: null` takes the task's own value away.
//! - `resources.get { disk? }` gives what running things use; `resources.cleanUp`
//!   gives `{ stopped, removed }`.
//!
//! The kinds differ in their first word on purpose: commands of one kind and
//! task wait for each other, and a slow question to Docker about memory must
//! not hold up a press on "Stop".

use super::env::{self, TaskEnv};
use super::{FULL_STACK, Place, Profile, Runner, resources};
use crate::server::AppState;
use crate::tasks::Manager;
use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};
use std::path::PathBuf;

/// How many profiles a project may keep, and how long a name may be.
const MAX_PROFILES: usize = 20;
const MAX_NAME: usize = 40;

fn text<'a>(params: &'a Value, name: &str) -> Result<&'a str> {
    params.get(name).and_then(Value::as_str).ok_or_else(|| anyhow!("Missing parameter: {name}"))
}

fn profiles_key(project_id: &str) -> String {
    format!("project.{project_id}.runProfiles")
}

fn chosen_key(project_id: &str) -> String {
    format!("project.{project_id}.runProfile")
}

/// The profiles the user made for a project.
fn kept_profiles(manager: &Manager, project_id: &str) -> Vec<Profile> {
    let kept = manager.store().setting(&profiles_key(project_id)).ok().flatten();
    kept.and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

/// The name of the profile "Start all" starts: the chosen one while it still exists.
fn chosen_profile(manager: &Manager, project_id: &str, kept: &[Profile]) -> String {
    let chosen = manager.store().setting(&chosen_key(project_id)).ok().flatten();
    chosen.filter(|name| kept.iter().any(|profile| &profile.name == name)).unwrap_or_else(|| FULL_STACK.into())
}

/// What the chosen profile starts: `None` for everything the project has.
fn wanted(manager: &Manager, project_id: &str) -> Option<Vec<String>> {
    let kept = kept_profiles(manager, project_id);
    let chosen = chosen_profile(manager, project_id, &kept);
    kept.into_iter().find(|profile| profile.name == chosen).map(|profile| profile.services)
}

/// The state of a task's Run tab, with the project's profiles in it.
fn shown(manager: &Manager, runner: &Runner, place: &Place, project_id: &str) -> Value {
    let mut state = runner.state(place);
    let kept = kept_profiles(manager, project_id);
    let chosen = chosen_profile(manager, project_id, &kept);
    if let Some(profile) = kept.iter().find(|profile| profile.name == chosen) {
        for service in &mut state.services {
            service.in_profile = profile.services.contains(&service.name);
        }
    }
    let everything = state.services.iter().map(|service| service.name.clone()).collect();
    state.profiles = vec![Profile { name: FULL_STACK.into(), services: everything, built_in: true }];
    state.profiles.extend(kept);
    state.profile = chosen;
    json!(state)
}

/// Keeps a profile under `name`, in place of the one called `was` when that is given.
fn save_profile(manager: &Manager, runner: &Runner, place: &Place, project_id: &str, params: &Value) -> Result<()> {
    let name = text(params, "name")?.trim().to_string();
    let was = params.get("was").and_then(Value::as_str);
    if name.is_empty() || name.chars().count() > MAX_NAME {
        bail!("Give the profile a name of up to {MAX_NAME} characters");
    }
    if name.eq_ignore_ascii_case(FULL_STACK) {
        bail!("\"{FULL_STACK}\" is the profile that starts everything. Pick another name.");
    }
    let known: Vec<String> = runner.state(place).services.into_iter().map(|service| service.name).collect();
    let mut services: Vec<String> = Vec::new();
    for service in params.get("services").and_then(Value::as_array).into_iter().flatten() {
        let Some(service) = service.as_str() else { continue };
        if !known.iter().any(|name| name == service) {
            bail!("{service} is not something this project runs");
        }
        if !services.iter().any(|name| name == service) {
            services.push(service.to_string());
        }
    }
    if services.is_empty() {
        bail!("Tick at least one thing for the profile to start");
    }

    let mut kept = kept_profiles(manager, project_id);
    let taken = |profile: &Profile| profile.name.eq_ignore_ascii_case(&name) && Some(profile.name.as_str()) != was;
    if kept.iter().any(taken) {
        bail!("There is a profile called {name} already");
    }
    let profile = Profile { name: name.clone(), services, built_in: false };
    match kept.iter().position(|profile| Some(profile.name.as_str()) == was) {
        Some(at) => kept[at] = profile,
        None if kept.len() >= MAX_PROFILES => bail!("A project keeps at most {MAX_PROFILES} profiles"),
        None => kept.push(profile),
    }
    manager.store().set_setting(&profiles_key(project_id), Some(&serde_json::to_string(&kept)?))?;
    // A new profile is made to be used; a renamed one stays the chosen one if it was.
    manager.store().set_setting(&chosen_key(project_id), Some(&name))
}

/// One command, done on a thread that may wait for Docker.
fn handle(manager: &Manager, runner: &Runner, method: &str, params: &Value) -> Result<Value> {
    let task_id = text(params, "taskId")?;
    let task = manager.task(task_id)?;
    let project_id = task.row.project_id.as_str();
    let project = manager.store().project(project_id).ok();
    let root = PathBuf::from(&task.row.cwd);
    // A project that is gone from the list still has the task's folder to read from.
    let project_dir = project.as_ref().map_or_else(|| root.clone(), |project| PathBuf::from(&project.path));
    let task_env = TaskEnv::read(manager.data_dir(), task_id, &root, &project_dir);
    let place = Place {
        task_id: task_id.to_string(),
        project: project.map_or_else(|| "project".into(), |project| project.name),
        root,
        env: task_env.pairs(),
        secrets: task_env.secrets(),
    };
    let service = params.get("service").and_then(Value::as_str);
    let one = service.map(|name| vec![name.to_string()]);

    match method {
        "run.state" => return Ok(shown(manager, runner, &place, project_id)),
        "run.logs" => return Ok(json!({ "termId": runner.logs(&place, text(params, "service")?)? })),
        "env.get" => return Ok(task_env.shown()),
        "resources.get" => {
            let with_disk = params.get("disk").and_then(Value::as_bool).unwrap_or(false);
            return Ok(resources::get(manager, runner, &place, with_disk));
        }
        _ => {}
    }

    // Everything from here on changes what the tab shows, in this window and in the others.
    let done = match method {
        "run.start" => runner.start(&place, one.or_else(|| wanted(manager, project_id)).as_deref()).map(|()| None),
        "run.restart" => runner.restart(&place, one.or_else(|| wanted(manager, project_id)).as_deref()).map(|()| None),
        "run.stop" => runner.stop(&place, service).map(|()| None),
        "run.shell" => runner.shell(&place, text(params, "service")?).map(|term_id| Some(json!({ "termId": term_id }))),
        "run.profile" => {
            let name = text(params, "name")?;
            let kept = kept_profiles(manager, project_id);
            if name != FULL_STACK && !kept.iter().any(|profile| profile.name == name) {
                bail!("There is no profile called {name}");
            }
            manager.store().set_setting(&chosen_key(project_id), (name != FULL_STACK).then_some(name)).map(|()| None)
        }
        "run.saveProfile" => save_profile(manager, runner, &place, project_id, params).map(|()| None),
        "run.removeProfile" => {
            let name = text(params, "name")?;
            let mut kept = kept_profiles(manager, project_id);
            kept.retain(|profile| profile.name != name);
            let text = serde_json::to_string(&kept)?;
            manager
                .store()
                .set_setting(&profiles_key(project_id), (!kept.is_empty()).then_some(text.as_str()))
                .map(|()| None)
        }
        "env.set" => {
            let value = params.get("value").and_then(Value::as_str);
            env::set_own(manager.data_dir(), task_id, text(params, "name")?, value)
                .map(|()| Some(TaskEnv::read(manager.data_dir(), task_id, &place.root, &project_dir).shown()))
        }
        "deps.install" => runner.install(&place).map(|_| None),
        "db.reset" => runner.reset_database(&place, text(params, "service")?).map(|()| None),
        "resources.cleanUp" => resources::clean_up(manager, runner, &place).map(Some),
        other => bail!("Unknown method: {other}"),
    };
    // Told even when the command failed half way: part of it may have happened.
    manager.broadcast_run_changed(task_id);
    match done? {
        Some(answer) => Ok(answer),
        None => Ok(shown(manager, runner, &place, project_id)),
    }
}

/// The commands of the Run tab. They may wait for Docker, so they run off the event loop.
pub async fn command(state: &AppState, method: &str, params: &Value) -> Result<Value> {
    let (manager, runner) = (state.manager.clone(), state.runner.clone());
    let (method, params) = (method.to_string(), params.clone());
    // What starts by itself later, such as a script after its install, has no command to answer.
    runner.tell_with(|| {
        let teller = manager.clone();
        Box::new(move |task_id| teller.broadcast_run_changed(task_id))
    });
    tokio::task::spawn_blocking(move || handle(&manager, &runner, &method, &params)).await?
}
