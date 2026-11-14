//! The background jobs of a project: what there is to run, and one run with its output.
//!
//! Much of what a service does happens away from any request: a task on a
//! queue, a command run by a timer. Such work has no page and no address, so
//! its preview is to start one by hand with the arguments it takes and watch
//! what it prints.
//!
//! The jobs are found by reading the project's files, without running any of
//! them:
//!
//! - **Celery** tasks (`@shared_task`, `@app.task`) and **Dramatiq** actors;
//! - **Django** management commands and **Laravel** console commands;
//! - **Sidekiq** workers and **Active Job** classes;
//! - scripts of `package.json` whose names say they are jobs (`job:…`, `cron:…`, `seed`).
//!
//! A job is started the way a person would start it from a terminal of the
//! project: `celery call`, `manage.py`, `rails runner`. Arguments for a queue
//! are given as JSON and reach the program through the environment, so no
//! quoting of them is left to a shell. Each kind has a command line the
//! project can replace, for one that runs its workers inside Docker.
//!
//! The run happens in a terminal of the task, with the task's environment.
//! A job put on a queue is done by the project's worker: its output is in the
//! Run tab, where the worker runs.

use crate::gitops;
use crate::runner::{self, python_prefix, quote};
use crate::server::AppState;
use crate::terminals::{Terminals, strip_codes};
use anyhow::{Result, anyhow, bail};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How many files of a project are read, and how long one may be.
const MOST_FILES: usize = 6000;
const MAX_FILE: u64 = 400 * 1024;
/// How many jobs are listed.
const MOST_JOBS: usize = 400;
/// How much of what a job printed is given: its end.
const MOST_SAID: usize = 12_000;
/// The starts of the names of package scripts that are jobs.
const SCRIPT_STARTS: [&str; 9] = ["job", "cron", "task", "seed", "sync", "import", "export", "backfill", "cleanup"];
/// The kinds whose arguments are JSON for a queue. The others take the rest of a command line.
const QUEUED: [&str; 4] = ["celery", "dramatiq", "sidekiq", "activejob"];

/// One thing of the project that runs in the background.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    /// Its kind and its name: `celery:shop.tasks.send_receipt`.
    pub id: String,
    /// "celery", "dramatiq", "django", "artisan", "sidekiq", "activejob" or "script".
    pub kind: &'static str,
    /// What it is called where it is started.
    pub name: String,
    /// Where it is written, within the task's folder, and on which line.
    pub file: String,
    pub line: usize,
    /// What it takes, as written: `order_id`, `force=False`.
    pub takes: Vec<String>,
    /// Whether the task touched its file.
    pub changed: bool,
    /// The folder its command runs in, within the task's folder. Empty for the task's folder itself.
    pub folder: String,
    /// For Python: the module it is in and its name there, to import it by.
    #[serde(skip)]
    module: String,
    #[serde(skip)]
    function: String,
}

fn is_word(sign: char) -> bool {
    sign.is_alphanumeric() || sign == '_'
}

/// The names between the brackets of a `def`, each as written, without `self` and the like.
fn parameters(inside: &str) -> Vec<String> {
    let mut found = vec![String::new()];
    let mut depth = 0;
    for sign in inside.chars() {
        match sign {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => {
                found.push(String::new());
                continue;
            }
            _ => {}
        }
        found.last_mut().unwrap().push(sign);
    }
    found
        .into_iter()
        .map(|one| one.split_whitespace().collect::<Vec<_>>().join(" "))
        // A type after the name says nothing a person filling it in needs.
        .map(|one| match (one.split_once(':'), one.split_once('=')) {
            (Some((name, _)), Some((_, default))) if !name.contains('=') => {
                format!("{}={}", name.trim(), default.trim())
            }
            (Some((name, _)), None) => name.trim().to_string(),
            _ => one.replace(" = ", "="),
        })
        .filter(|one| !one.is_empty() && !matches!(one.as_str(), "self" | "cls" | "*" | "/"))
        .collect()
}

/// The text in the first pair of quotes after `key=` or `key:`.
fn named(text: &str, key: &str) -> Option<String> {
    let at = text.find(key)?;
    if text[..at].chars().next_back().is_some_and(is_word) {
        return None;
    }
    let rest = text[at + key.len()..].trim_start();
    let rest = rest.strip_prefix('=').or_else(|| rest.strip_prefix(':'))?.trim_start();
    let quote = rest.chars().next().filter(|sign| *sign == '"' || *sign == '\'')?;
    rest[1..].split(quote).next().map(str::to_string)
}

/// The folder a Python file is imported from, and the module it is there: the folder above the
/// topmost one that is a package.
fn python_module(root: &Path, file: &str) -> (String, String) {
    let parts: Vec<&str> = file.trim_end_matches(".py").split('/').collect();
    let mut top = parts.len() - 1;
    while top > 0 && root.join(parts[..top].join("/")).join("__init__.py").is_file() {
        top -= 1;
    }
    // No package at all: the file is imported from the project's top, or from `src`.
    if top == parts.len() - 1 {
        top = usize::from(parts.first() == Some(&"src") && parts.len() > 1);
    }
    let module: Vec<&str> = parts[top..].iter().copied().filter(|part| *part != "__init__").collect();
    (parts[..top].join("/"), module.join("."))
}

/// The jobs written in one Python file: tasks of Celery and actors of Dramatiq.
fn python_jobs(root: &Path, file: &str, text: &str, out: &mut Vec<Job>) {
    let lower = text.to_lowercase();
    let (celery, dramatiq) = (lower.contains("celery"), lower.contains("dramatiq"));
    if !celery && !dramatiq {
        return;
    }
    let lines: Vec<&str> = text.lines().collect();
    let mut at = 0;
    while at < lines.len() {
        let line = lines[at].trim_start();
        at += 1;
        let Some(decorator) = line.strip_prefix('@') else { continue };
        let head: String = decorator.chars().take_while(|sign| is_word(*sign) || *sign == '.').collect();
        let kind = match head.as_str() {
            "shared_task" | "celery.shared_task" => "celery",
            "dramatiq.actor" | "actor" if dramatiq => "dramatiq",
            head if celery && (head.ends_with(".task") || head == "task") => "celery",
            _ => continue,
        };
        // The decorator may go on for lines, and others may stand between it and the function.
        let mut said = decorator.to_string();
        let mut found = None;
        for (ahead, next) in lines.iter().enumerate().skip(at).take(40) {
            let next = next.trim_start();
            if let Some(rest) = next.strip_prefix("async def ").or_else(|| next.strip_prefix("def ")) {
                found = Some((ahead, rest));
                break;
            }
            if next.starts_with("class ") {
                break;
            }
            said.push_str(next);
        }
        let Some((line_at, rest)) = found else { continue };
        let function: String = rest.chars().take_while(|sign| is_word(*sign)).collect();
        // The brackets of a long `def` close some lines down.
        let whole = lines[line_at..(line_at + 30).min(lines.len())].join(" ");
        let open = whole.find('(').map_or(0, |open| open + 1);
        // Up to the bracket that closes the one after the name: a default or a type may hold brackets of its own.
        let mut depth = 1;
        let mut close = whole.len();
        for (at, sign) in whole[open..].char_indices() {
            match sign {
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth -= 1,
                _ => {}
            }
            if depth == 0 {
                close = open + at;
                break;
            }
        }
        let mut takes = parameters(&whole[open..close]);
        // A bound task is handed itself first, under whatever name; `self` is left out already.
        let first = whole[open..close].split(',').next().unwrap_or_default().trim();
        if said.replace(' ', "").contains("bind=True") && !takes.is_empty() && !matches!(first, "self" | "cls") {
            takes.remove(0);
        }
        let (folder, module) = python_module(root, file);
        let decorated = said.split_once('(').map_or("", |(_, args)| args);
        let given = if kind == "celery" { named(decorated, "name") } else { named(decorated, "actor_name") };
        let name =
            given.unwrap_or_else(|| if kind == "celery" { format!("{module}.{function}") } else { function.clone() });
        out.push(Job {
            id: format!("{kind}:{name}"),
            kind,
            name,
            file: file.to_string(),
            line: line_at + 1,
            takes,
            changed: false,
            folder,
            module,
            function,
        });
        at = line_at + 1;
    }
}

/// The texts given to `add_argument` in a Django command: what it takes on its command line.
fn django_arguments(text: &str) -> Vec<String> {
    text.split("add_argument(")
        .skip(1)
        .filter_map(|rest| {
            let quote = rest.trim_start().chars().next().filter(|sign| *sign == '"' || *sign == '\'')?;
            rest.trim_start()[1..].split(quote).next().map(str::to_string)
        })
        .collect()
}

/// The jobs written in one Ruby file: a Sidekiq worker or an Active Job class.
fn ruby_jobs(file: &str, text: &str, out: &mut Vec<Job>) {
    let sidekiq = text.contains("Sidekiq::Job") || text.contains("Sidekiq::Worker");
    let active = text.contains("< ApplicationJob") || text.contains("< ActiveJob::Base");
    if !sidekiq && !active {
        return;
    }
    let mut modules: Vec<String> = Vec::new();
    for (at, line) in text.lines().enumerate() {
        let line = line.trim_start();
        if let Some(name) = line.strip_prefix("module ") {
            modules.push(name.trim().to_string());
        }
        let Some(rest) = line.strip_prefix("class ") else { continue };
        let class: String = rest.chars().take_while(|sign| is_word(*sign) || *sign == ':').collect();
        if class == "ApplicationJob" {
            return;
        }
        let name = modules.iter().cloned().chain([class]).collect::<Vec<_>>().join("::");
        let takes = text
            .split("def perform(")
            .nth(1)
            .and_then(|rest| rest.split(')').next())
            .map(parameters)
            .unwrap_or_default();
        let kind = if sidekiq { "sidekiq" } else { "activejob" };
        out.push(Job {
            id: format!("{kind}:{name}"),
            kind,
            name,
            file: file.to_string(),
            line: at + 1,
            takes,
            changed: false,
            folder: String::new(),
            module: String::new(),
            function: String::new(),
        });
        return;
    }
}

/// The folder a file's command runs in: the one that holds `mark`, looked for from the file upward.
fn folder_with(root: &Path, file: &str, mark: &str) -> String {
    let mut parts: Vec<&str> = file.split('/').collect();
    while parts.pop().is_some() {
        if root.join(parts.join("/")).join(mark).is_file() {
            return parts.join("/");
        }
    }
    String::new()
}

/// Every job of a project, read from its files.
pub fn scan(root: &Path) -> Vec<Job> {
    let mut found: Vec<Job> = Vec::new();
    let walk = ignore::WalkBuilder::new(root).hidden(true).max_depth(Some(12)).build();
    let job = |kind: &'static str, name: String, file: &str, line: usize, takes: Vec<String>, folder: String| Job {
        id: format!("{kind}:{name}"),
        kind,
        name,
        file: file.to_string(),
        line,
        takes,
        changed: false,
        folder,
        module: String::new(),
        function: String::new(),
    };
    for entry in walk.flatten().filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file())).take(MOST_FILES) {
        let path = entry.path();
        let Ok(file) = path.strip_prefix(root).map(|inside| inside.to_string_lossy().replace('\\', "/")) else {
            continue;
        };
        let name = file.rsplit('/').next().unwrap_or_default();
        let ours = name.ends_with(".py") || name.ends_with(".rb") || name.ends_with(".php") || name == "package.json";
        if !ours || file.contains("node_modules/") || entry.metadata().map_or(true, |meta| meta.len() > MAX_FILE) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(path) else { continue };
        if name == "package.json" {
            let scripts = serde_json::from_str::<Value>(&text).ok().and_then(|package| package.get("scripts").cloned());
            let folder = file.strip_suffix("package.json").unwrap_or_default().trim_end_matches('/').to_string();
            for (script, _) in scripts.as_ref().and_then(Value::as_object).into_iter().flatten() {
                let is_job = SCRIPT_STARTS.iter().any(|start| {
                    script
                        .strip_prefix(start)
                        .is_some_and(|rest| rest.is_empty() || rest.starts_with([':', '-', '_', 's']))
                });
                if is_job {
                    found.push(job("script", script.clone(), &file, 1, vec![], folder.clone()));
                }
            }
        } else if file.contains("management/commands/")
            && name.ends_with(".py")
            && !name.starts_with('_')
            && text.contains("BaseCommand")
        {
            let command = name.trim_end_matches(".py").to_string();
            found.push(job(
                "django",
                command,
                &file,
                1,
                django_arguments(&text),
                folder_with(root, &file, "manage.py"),
            ));
        } else if name.ends_with(".py") {
            python_jobs(root, &file, &text, &mut found);
        } else if name.ends_with(".rb") {
            ruby_jobs(&file, &text, &mut found);
        } else if file.contains("Console/Commands/") {
            let line = text.lines().position(|line| line.contains("$signature")).unwrap_or(0);
            if let Some(signature) = named(text.lines().nth(line).unwrap_or_default(), "$signature") {
                let mut words = signature.split_whitespace();
                let command = words.next().unwrap_or_default().to_string();
                let takes = words.map(|word| word.trim_matches(['{', '}']).to_string()).collect();
                found.push(job("artisan", command, &file, line + 1, takes, folder_with(root, &file, "artisan")));
            }
        }
    }
    // A name becomes part of a command line. One that holds anything a shell would read as more
    // than a name is not a job to start from here.
    let plain =
        |text: &str, more: &str| text.chars().all(|sign| sign.is_alphanumeric() || sign == '_' || more.contains(sign));
    found.retain(|job| {
        !job.name.is_empty() && plain(&job.name, ".:-/") && plain(&job.module, ".") && plain(&job.function, "")
    });
    found.sort_by(|a, b| (a.kind, &a.name).cmp(&(b.kind, &b.name)));
    found.dedup_by(|a, b| a.id == b.id);
    found.truncate(MOST_JOBS);
    found
}

/// The files that say a folder holds a Python project.
const PYTHON_MARKS: [&str; 8] =
    ["pyproject.toml", "requirements.txt", "setup.py", "setup.cfg", "Pipfile", "manage.py", "uv.lock", "poetry.lock"];
/// How long an answer about a project's Celery app is believed: the Run tab asks often, and reading
/// every file each time would be felt.
const APP_BELIEVED: Duration = Duration::from_secs(20);

/// The answer for each folder asked about, with when it was given.
type Believed = HashMap<PathBuf, (Instant, Option<(String, String)>)>;

/// Where the project makes its Celery app, as `celery -A` wants it: `shop.celery:app`. Looked for
/// only in a folder that holds a Python project, or has one right under it.
/// Gives the folder it is imported from, within `root`, and the app.
pub fn celery_app(root: &Path) -> Option<(String, String)> {
    static KNOWN: OnceLock<Mutex<Believed>> = OnceLock::new();
    let known = KNOWN.get_or_init(Mutex::default);
    if let Some((at, found)) = known.lock().unwrap().get(root)
        && at.elapsed() < APP_BELIEVED
    {
        return found.clone();
    }
    let python = |folder: &Path| PYTHON_MARKS.iter().any(|mark| folder.join(mark).is_file());
    let is_python = python(root)
        || std::fs::read_dir(root)
            .is_ok_and(|all| all.flatten().take(200).any(|one| one.path().is_dir() && python(&one.path())));
    let found = if is_python { find_celery_app(root) } else { None };
    known.lock().unwrap().insert(root.to_path_buf(), (Instant::now(), found.clone()));
    found
}

/// The same, read from the files now, with nothing believed from before.
fn find_celery_app(root: &Path) -> Option<(String, String)> {
    let walk = ignore::WalkBuilder::new(root).hidden(true).max_depth(Some(8)).build();
    let mut found: Vec<(String, String)> = Vec::new();
    for entry in walk.flatten().take(MOST_FILES) {
        let path = entry.path();
        if path.extension().is_none_or(|end| end != "py") || entry.metadata().map_or(true, |meta| meta.len() > MAX_FILE)
        {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(path) else { continue };
        for line in text.lines().filter(|line| line.contains("Celery(") && !line.trim_start().starts_with('#')) {
            let Some((name, _)) = line.split_once('=').filter(|(_, made)| made.trim_start().starts_with("Celery("))
            else {
                continue;
            };
            let name = name.trim();
            if name.is_empty() || !name.chars().all(is_word) {
                continue;
            }
            let Ok(file) = path.strip_prefix(root).map(|inside| inside.to_string_lossy().replace('\\', "/")) else {
                continue;
            };
            let (folder, module) = python_module(root, &file);
            found.push((folder, format!("{module}:{name}")));
        }
    }
    // The shortest way in is the one a person would write.
    found.sort_by_key(|(_, app)| (app.len(), app.clone()));
    found.into_iter().next()
}

fn template_key(project_id: &str, kind: &str) -> String {
    format!("project.{project_id}.jobs.{kind}")
}

/// The command line of each kind of job as it is unless the project says otherwise. `{name}` is the
/// job, `{args}` and `{kwargs}` its arguments as JSON in quotes, `{tail}` the rest of a command line.
pub fn default_templates(root: &Path, folder: &str, app: Option<&str>) -> BTreeMap<&'static str, String> {
    let at = root.join(folder);
    let python = python_prefix(&at, "python");
    let celery = format!("{} -A {}", python_prefix(&at, "celery"), app.unwrap_or("app"));
    let import = "import json, os; from {module} import {function} as job; print(job.send(*json.loads(os.environ['AE_ARGS']), **json.loads(os.environ['AE_KWARGS'])))";
    BTreeMap::from([
        (
            "celery",
            format!(
                "id=$({celery} call {{name}} --args {{args}} --kwargs {{kwargs}}) && echo \"Sent as $id. Waiting for the result:\" && {celery} result \"$id\""
            ),
        ),
        ("dramatiq", format!("{python} -c {}", quote(import))),
        ("django", format!("{python} manage.py {{name}} {{tail}}")),
        ("artisan", "php artisan {name} {tail}".to_string()),
        (
            "sidekiq",
            format!("bin/rails runner {}", quote("puts {name}.perform_async(*JSON.parse(ENV.fetch('AE_ARGS')))")),
        ),
        (
            "activejob",
            format!(
                "bin/rails runner {}",
                quote("puts {name}.perform_later(*JSON.parse(ENV.fetch('AE_ARGS'))).job_id")
            ),
        ),
        ("script", format!("{} run {{name}} -- {{tail}}", runner::package_manager(&at))),
    ])
}

/// The arguments of a queued job as the user gave them: a JSON list and a JSON object, each on one line.
fn json_arguments(args: &str, kwargs: &str) -> Result<(String, String)> {
    let args = if args.trim().is_empty() { "[]" } else { args.trim() };
    let kwargs = if kwargs.trim().is_empty() { "{}" } else { kwargs.trim() };
    let list: Value = serde_json::from_str(args).map_err(|error| anyhow!("The arguments are not JSON: {error}"))?;
    let named: Value =
        serde_json::from_str(kwargs).map_err(|error| anyhow!("The named arguments are not JSON: {error}"))?;
    if !list.is_array() {
        bail!("The arguments are a JSON list, such as [42, \"tea\"]");
    }
    if !named.is_object() {
        bail!("The named arguments are a JSON object, such as {{\"force\": true}}");
    }
    Ok((list.to_string(), named.to_string()))
}

/// The command line that starts a job, and what it is given in its environment.
pub fn command_line(job: &Job, template: &str, args: &str, kwargs: &str) -> Result<(String, Vec<(String, String)>)> {
    let mut line =
        template.replace("{name}", &job.name).replace("{module}", &job.module).replace("{function}", &job.function);
    let mut env = Vec::new();
    if QUEUED.contains(&job.kind) {
        let (args, kwargs) = json_arguments(args, kwargs)?;
        if job.kind != "celery" && job.kind != "dramatiq" && kwargs != "{}" {
            bail!("Jobs of this kind take a list of arguments only");
        }
        line = line.replace("{args}", &quote(&args)).replace("{kwargs}", &quote(&kwargs));
        env.extend([("AE_ARGS".to_string(), args), ("AE_KWARGS".to_string(), kwargs)]);
    } else {
        if args.contains('\n') {
            bail!("The rest of the command line is one line");
        }
        line = line.replace("{tail}", args.trim());
    }
    let line = line.trim().trim_end_matches(" --").trim().to_string();
    Ok((line, env))
}

fn purpose(id: &str) -> String {
    format!("job:{id}")
}

fn text<'a>(params: &'a Value, name: &str) -> Result<&'a str> {
    params.get(name).and_then(Value::as_str).ok_or_else(|| anyhow!("Missing parameter: {name}"))
}

/// How the last run of a job stands: whether it still runs, how it ended, and the end of what it printed.
fn state_of(terminals: &Terminals, task_id: &str, id: &str, secrets: &[String]) -> Value {
    let purpose = purpose(id);
    let running = terminals.find(task_id, &purpose);
    let printed = match &running {
        Some(info) => {
            terminals.scrollback(info.term_id).ok().map(|(bytes, _)| strip_codes(&String::from_utf8_lossy(&bytes)))
        }
        None => terminals.last_output(task_id, &purpose),
    };
    let printed = printed.map(|text| runner::env::scrub(text.trim_end(), secrets));
    let tail = printed.map(|text| {
        let cut = text.len().saturating_sub(MOST_SAID);
        let from = (cut..=text.len()).find(|at| text.is_char_boundary(*at)).unwrap_or(text.len());
        text[from..].to_string()
    });
    json!({
        "running": running.is_some(),
        "termId": running.map(|info| info.term_id),
        "exit": terminals.last_exit(task_id, &purpose),
        "output": tail
    })
}

fn handle(
    state_manager: &crate::tasks::Manager,
    terminals: &Arc<Terminals>,
    method: &str,
    params: &Value,
) -> Result<Value> {
    let task_id = text(params, "taskId")?;
    let (place, project_id) = runner::place_of(state_manager, task_id)?;
    let root = place.root.as_path();
    let kept =
        |kind: &str| state_manager.store().setting(&template_key(&project_id, kind)).ok().flatten().unwrap_or_default();
    match method {
        "jobs.get" => {
            let task = state_manager.store().task(task_id)?;
            let mut jobs = scan(root);
            let start = task
                .branch
                .as_ref()
                .and_then(|_| gitops::resolve_base(root, task.base_branch.as_deref()))
                .and_then(|base| gitops::fork_point(root, &base));
            if let Some(changed) = start.and_then(|start| gitops::changed_since(root, &start).ok()) {
                let touched: HashSet<&str> =
                    changed.iter().filter(|(_, kind)| *kind != "deleted").map(|(path, _)| path.as_str()).collect();
                for job in &mut jobs {
                    job.changed = touched.contains(job.file.as_str());
                }
            }
            // What the task touched comes first; the rest keeps its order.
            jobs.sort_by_key(|job| !job.changed);
            // Tasks of Celery were found, so there is an app to find: it is looked for whatever the folder looks like.
            let app = jobs
                .iter()
                .any(|job| job.kind == "celery")
                .then(|| find_celery_app(root))
                .flatten()
                .map(|(_, app)| app);
            let folder = jobs.first().map(|job| job.folder.clone()).unwrap_or_default();
            let defaults = default_templates(root, &folder, app.as_deref());
            let settings: BTreeMap<&str, String> = defaults.keys().map(|kind| (*kind, kept(kind))).collect();
            Ok(json!({ "jobs": jobs, "celeryApp": app, "templates": defaults, "settings": settings }))
        }
        "jobs.set" => {
            let kind = text(params, "kind")?;
            if !default_templates(root, "", None).contains_key(kind) {
                bail!("There are no jobs of the kind {kind}");
            }
            let line = text(params, "template")?.trim();
            if line.len() > 2000 || line.contains('\n') {
                bail!("A command is one line");
            }
            state_manager.store().set_setting(&template_key(&project_id, kind), (!line.is_empty()).then_some(line))?;
            Ok(json!({}))
        }
        "jobs.run" => {
            let id = text(params, "id")?;
            let job = scan(root)
                .into_iter()
                .find(|job| job.id == id)
                .ok_or_else(|| anyhow!("This project has no job {id}"))?;
            if terminals.find(task_id, &purpose(id)).is_some() {
                bail!("{} is running already. Stop it first, or wait for it to end.", job.name);
            }
            let set = kept(job.kind);
            let template = match set.is_empty() {
                true => {
                    let app = (job.kind == "celery").then(|| find_celery_app(root)).flatten().map(|(_, app)| app);
                    default_templates(root, &job.folder, app.as_deref()).remove(job.kind).unwrap_or_default()
                }
                false => set,
            };
            let args = params.get("args").and_then(Value::as_str).unwrap_or_default();
            let kwargs = params.get("kwargs").and_then(Value::as_str).unwrap_or_default();
            let (line, mut env) = command_line(&job, &template, args, kwargs)?;
            env.extend(place.env.iter().cloned());
            let cwd: PathBuf = root.join(&job.folder);
            let info = terminals.run(task_id, &cwd, &line, &purpose(id), &env)?;
            Ok(json!({ "termId": info.term_id, "command": line }))
        }
        "jobs.state" => Ok(state_of(terminals, task_id, text(params, "id")?, &place.secrets)),
        "jobs.stop" => {
            if let Some(info) = terminals.find(task_id, &purpose(text(params, "id")?)) {
                terminals.stop(info.term_id)?;
            }
            Ok(json!({}))
        }
        other => bail!("Unknown method: {other}"),
    }
}

/// The commands of the Jobs view.
///
/// - `jobs.get { taskId }` gives the jobs of the project, the ones the task touched first, and the
///   command line of each kind.
/// - `jobs.set { taskId, kind, template }` keeps the project's own command line for a kind.
/// - `jobs.run { taskId, id, args?, kwargs? }` starts a job in a terminal of the task.
/// - `jobs.state { taskId, id }` says whether it still runs, how it ended and what it printed.
/// - `jobs.stop { taskId, id }` stops it.
///
/// They read many files, so they run off the event loop.
pub async fn command(state: &AppState, method: &str, params: &Value) -> Result<Value> {
    let (manager, terminals) = (state.manager.clone(), state.terminals.clone());
    let (method, params) = (method.to_string(), params.clone());
    tokio::task::spawn_blocking(move || handle(&manager, &terminals, &method, &params)).await?
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Folder(PathBuf);

    impl Folder {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("ae-jobs-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn write(&self, path: &str, text: &str) {
            let file = self.0.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, text).unwrap();
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn named_jobs(jobs: &[Job]) -> Vec<(&str, &str)> {
        jobs.iter().map(|job| (job.kind, job.name.as_str())).collect()
    }

    #[test]
    fn the_tasks_of_celery_and_the_actors_of_dramatiq_are_found_by_their_decorators() {
        let project = Folder::new("python");
        project.write("backend/shop/__init__.py", "");
        project.write("backend/pyproject.toml", "");
        project.write("backend/shop/celery.py", "from celery import Celery\n\napp = Celery(\"shop\")\n");
        project.write(
            "backend/shop/tasks.py",
            r#"from celery import shared_task
from .celery import app


@shared_task
def send_receipt(order_id: int, copy_to: str | None = None):
    ...


@app.task(
    bind=True,
    name="shop.rebuild_search",
    max_retries=3,
)
@other_decorator
async def rebuild(self, full=False, *, since: str = "2026-01-01"):
    ...


@app.route("/health")
def not_a_task():
    ...
"#,
        );
        project.write(
            "backend/shop/actors.py",
            "import dramatiq\n\n@dramatiq.actor(max_retries=0)\ndef resize(image_id, width=200):\n    ...\n",
        );
        project.write("backend/shop/plain.py", "@task\ndef not_ours():\n    ...\n");
        // A typed signature, a call with brackets right under it, and a name no shell should ever see.
        project.write(
            "backend/shop/typed.py",
            "from celery import shared_task\n\n@shared_task\ndef typed(order_id: int, tags: list[str] = (\"a\",)) -> None:\n    if check(order_id):\n        ...\n\n@shared_task(name=\"x; rm -rf ~\")\ndef odd():\n    ...\n",
        );
        let jobs = scan(&project.0);
        assert_eq!(
            named_jobs(&jobs),
            [
                ("celery", "shop.rebuild_search"),
                ("celery", "shop.tasks.send_receipt"),
                ("celery", "shop.typed.typed"),
                ("dramatiq", "resize")
            ]
        );
        assert_eq!(jobs[2].takes, ["order_id", "tags=(\"a\",)"]);
        assert_eq!(jobs[0].takes, ["full=False", "since=\"2026-01-01\""], "a bound task is not asked for itself");
        assert_eq!(jobs[1].takes, ["order_id", "copy_to=None"]);
        assert_eq!(
            (jobs[1].file.as_str(), jobs[1].line, jobs[1].folder.as_str()),
            ("backend/shop/tasks.py", 6, "backend")
        );
        assert_eq!((jobs[3].module.as_str(), jobs[3].function.as_str()), ("shop.actors", "resize"));
        assert_eq!(celery_app(&project.0), Some(("backend".to_string(), "shop.celery:app".to_string())));
    }

    #[test]
    fn commands_workers_and_scripts_that_are_jobs_are_found_too() {
        let project = Folder::new("others");
        project.write("manage.py", "");
        project.write(
            "shop/management/commands/expire_carts.py",
            "from django.core.management.base import BaseCommand\n\nclass Command(BaseCommand):\n    def add_arguments(self, parser):\n        parser.add_argument('--older-than', type=int)\n        parser.add_argument(\"--dry-run\", action='store_true')\n",
        );
        project.write("shop/management/commands/_helpers.py", "from django.core.management.base import BaseCommand\n");
        project.write("app/sidekiq/receipt_job.rb", "module Shop\n  class ReceiptJob\n    include Sidekiq::Job\n\n    def perform(order_id, copy_to = nil)\n    end\n  end\nend\n");
        project.write("app/jobs/application_job.rb", "class ApplicationJob < ActiveJob::Base\nend\n");
        project
            .write("app/jobs/cleanup_job.rb", "class CleanupJob < ApplicationJob\n  def perform(days)\n  end\nend\n");
        project.write("app/Console/Commands/SendDigest.php", "<?php\nclass SendDigest extends Command {\n    protected $signature = 'digest:send {user} {--force}';\n}\n");
        project.write(
            "package.json",
            r#"{ "scripts": { "dev": "vite", "job:prices": "node jobs/prices.js", "seed": "node seed.js", "jobs": "node all.js", "taskfile": "x", "importer": "y" } }"#,
        );
        project.write("node_modules/x/package.json", r#"{ "scripts": { "seed": "x" } }"#);
        let jobs = scan(&project.0);
        assert_eq!(
            named_jobs(&jobs),
            [
                ("activejob", "CleanupJob"),
                ("artisan", "digest:send"),
                ("django", "expire_carts"),
                ("script", "job:prices"),
                ("script", "jobs"),
                ("script", "seed"),
                ("sidekiq", "Shop::ReceiptJob"),
            ]
        );
        assert_eq!(jobs[0].takes, ["days"]);
        assert_eq!(jobs[1].takes, ["user", "--force"]);
        assert_eq!(jobs[2].takes, ["--older-than", "--dry-run"]);
        assert_eq!(jobs[6].takes, ["order_id", "copy_to=nil"]);
    }

    fn job(kind: &'static str, name: &str) -> Job {
        Job {
            id: format!("{kind}:{name}"),
            kind,
            name: name.into(),
            file: String::new(),
            line: 1,
            takes: vec![],
            changed: false,
            folder: String::new(),
            module: "shop.actors".into(),
            function: "resize".into(),
        }
    }

    #[test]
    fn a_job_is_started_the_way_a_person_would_start_it() {
        let project = Folder::new("lines");
        project.write("uv.lock", "");
        let templates = default_templates(&project.0, "", Some("shop.celery:app"));
        let (line, env) =
            command_line(&job("celery", "shop.tasks.send_receipt"), &templates["celery"], "[42, \"it's\"]", "")
                .unwrap();
        assert_eq!(
            line,
            "id=$(uv run celery -A shop.celery:app call shop.tasks.send_receipt --args '[42,\"it'\\''s\"]' --kwargs '{}') \
             && echo \"Sent as $id. Waiting for the result:\" && uv run celery -A shop.celery:app result \"$id\""
        );
        assert_eq!(
            env,
            [("AE_ARGS".to_string(), "[42,\"it's\"]".to_string()), ("AE_KWARGS".to_string(), "{}".to_string())]
        );

        // Arguments for a queue never pass through a shell's quoting: the program reads them from its environment.
        let (line, env) =
            command_line(&job("dramatiq", "resize"), &templates["dramatiq"], "[7]", "{\"width\": 100}").unwrap();
        assert!(
            line.starts_with("uv run python -c 'import json, os; from shop.actors import resize as job;"),
            "{line}"
        );
        assert_eq!(env[1].1, "{\"width\":100}");
        let (line, _) = command_line(&job("sidekiq", "Shop::ReceiptJob"), &templates["sidekiq"], "[1]", "").unwrap();
        assert_eq!(
            line,
            "bin/rails runner 'puts Shop::ReceiptJob.perform_async(*JSON.parse(ENV.fetch('\\''AE_ARGS'\\'')))'"
        );

        // A command takes the rest of its command line as typed.
        let (line, env) =
            command_line(&job("django", "expire_carts"), &templates["django"], " --older-than 30 ", "").unwrap();
        assert_eq!((line.as_str(), env.len()), ("uv run python manage.py expire_carts --older-than 30", 0));
        let (line, _) = command_line(&job("script", "job:prices"), &templates["script"], "", "").unwrap();
        assert_eq!(line, "npm run job:prices");

        // The project's own line, for workers that live in a container.
        let own = "docker compose exec worker celery call {name} --args {args}";
        let (line, _) = command_line(&job("celery", "t"), own, "", "").unwrap();
        assert_eq!(line, "docker compose exec worker celery call t --args '[]'");
    }

    #[test]
    fn arguments_that_are_not_what_a_queue_takes_are_refused_in_words() {
        let celery = job("celery", "t");
        let refused =
            |args: &str, kwargs: &str| command_line(&celery, "{args} {kwargs}", args, kwargs).unwrap_err().to_string();
        assert!(refused("[1,", "").starts_with("The arguments are not JSON"));
        assert_eq!(refused("42", ""), "The arguments are a JSON list, such as [42, \"tea\"]");
        assert_eq!(refused("", "[1]"), "The named arguments are a JSON object, such as {\"force\": true}");
        let ruby = command_line(&job("sidekiq", "X"), "{args}", "[1]", "{\"a\": 1}").unwrap_err().to_string();
        assert_eq!(ruby, "Jobs of this kind take a list of arguments only");
    }
}
