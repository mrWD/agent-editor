//! Running a project: its dev commands and its Docker services, per task.
//!
//! What can be started is read from the project itself: scripts in
//! `package.json` and services in a compose file. Commands run in terminals of
//! their own, so their output can be watched. Docker services run under a
//! compose project that belongs to the task alone, and a port that is already
//! taken on this machine is moved to a free one, so two tasks never fight over
//! a port, a container name or a volume.
//!
//! Everything is started with the task's environment (see `env`). Packages are
//! installed first when they are missing or the lock file changed since.
//!
//! The functions block and may run Docker. Call them from `spawn_blocking`.

mod commands;
pub mod env;
pub mod resources;

pub use commands::command;
pub(crate) use commands::place_of;

use crate::terminals::{Terminals, strip_codes};
use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

/// The names a compose file goes by, in the order Docker itself looks for them.
const COMPOSE_FILES: [&str; 4] = ["compose.yaml", "compose.yml", "docker-compose.yml", "docker-compose.yaml"];
/// Scripts worth offering as things to run. Others are still reachable from the terminal.
const SCRIPT_NAMES: [&str; 6] = ["dev", "start", "serve", "api", "worker", "preview"];
/// Lock files, each with the program that writes it. The order is the one a project with two of them is judged by.
const LOCK_FILES: [(&str, &str); 6] = [
    ("pnpm-lock.yaml", "pnpm"),
    ("yarn.lock", "yarn"),
    ("bun.lockb", "bun"),
    ("bun.lock", "bun"),
    ("package-lock.json", "npm"),
    ("npm-shrinkwrap.json", "npm"),
];
/// What package managers write into `node_modules` when an install is done.
const INSTALL_MARKS: [&str; 4] = [".package-lock.json", ".modules.yaml", ".yarn-integrity", ".yarn-state.yml"];
/// A lock file this much newer than the install is a changed one. Less than that
/// is the install itself: some package managers write the lock file last.
const SAME_INSTALL: Duration = Duration::from_secs(2);
/// Images that are databases: their data is worth a volume, and their port is not a web page.
const DATABASE_IMAGES: [&str; 14] = [
    "postgres",
    "postgresql",
    "postgis",
    "pgvector",
    "timescaledb",
    "mysql",
    "mysql-server",
    "mariadb",
    "percona",
    "mongo",
    "mongodb",
    "mongodb-community-server",
    "cockroach",
    "clickhouse-server",
];
/// Other images whose port speaks something a browser cannot show.
const NOT_WEB_IMAGES: [&str; 4] = ["redis", "valkey", "memcached", "keydb"];
/// Where the usual database images look for scripts to fill a new, empty database.
const SEED_FOLDER: &str = "/docker-entrypoint-initdb.d";

/// What the terminal that installs packages is for.
pub(crate) const INSTALL: &str = "deps:install";
/// What the terminal that starts the Docker services is for.
const UP: &str = "docker:up";
/// The profile every project has: everything that was read from its files.
pub const FULL_STACK: &str = "Full stack";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Service {
    pub name: String,
    /// "command" for something run on this machine, "docker" for a container.
    pub kind: &'static str,
    /// The command line, or the image.
    pub runs: String,
    /// "stopped", "starting", "running", "failed" or "exited".
    pub state: &'static str,
    /// Where it can be reached, such as `localhost:3000`.
    pub address: Option<String>,
    /// What went wrong, or what was changed to make it start.
    pub note: Option<String>,
    /// The terminal that shows its output right now, if one is open.
    pub term_id: Option<u32>,
    /// False for a database or a cache: its address is not a page to open.
    pub web: bool,
    /// Whether the chosen profile starts it.
    pub in_profile: bool,
}

/// Commands and services that are started together, under a name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub name: String,
    pub services: Vec<String>,
    /// True for "Full stack", which is not kept anywhere: it is whatever the files say.
    #[serde(default, skip_deserializing)]
    pub built_in: bool,
}

/// The project's packages: what installs them and whether that is done.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Dependencies {
    /// What installs them, such as `npm ci`.
    pub command: String,
    /// The lock file the command installs from, if the project has one.
    pub lock_file: Option<&'static str>,
    /// "none" when the project names no packages; else "missing", "stale",
    /// "installing", "failed" or "installed".
    pub state: &'static str,
    /// When they were last installed, in milliseconds.
    pub installed_at: Option<u64>,
    /// How the last install failed.
    pub note: Option<String>,
    /// The terminal the install runs in right now.
    pub term_id: Option<u32>,
}

impl Dependencies {
    /// Whether something that needs the packages has to wait for an install first.
    fn needs_install(&self) -> bool {
        matches!(self.state, "missing" | "stale" | "failed")
    }
}

/// A named volume a database keeps its data in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DbVolume {
    /// The volume as Docker knows it.
    pub name: String,
    /// False when the compose file gives the volume a fixed name: then every task shares it.
    pub own: bool,
}

/// A database among the Docker services, with where it keeps its data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Database {
    /// The service's name.
    pub name: String,
    pub image: String,
    /// Its named volumes. Empty when the data lives inside the container only.
    pub volumes: Vec<DbVolume>,
    /// Whether the compose file gives it scripts to fill a new database with.
    pub seed: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunState {
    pub services: Vec<Service>,
    /// The compose file the Docker services were read from.
    pub compose_file: Option<String>,
    /// The compose project that belongs to this task.
    pub compose_project: String,
    /// False when Docker is needed and is not installed or not running.
    pub docker_ready: bool,
    pub docker_hint: Option<String>,
    /// What could not be read, in words for the user.
    pub problem: Option<String>,
    /// The project's launch profiles, "Full stack" first.
    pub profiles: Vec<Profile>,
    /// The profile "Start all" starts.
    pub profile: String,
    /// `None` for a project without a `package.json`.
    pub dependencies: Option<Dependencies>,
    pub databases: Vec<Database>,
}

/// A task as the runner sees it: where it works and what its programs are given.
#[derive(Debug, Clone)]
pub struct Place {
    pub task_id: String,
    /// The project's name, which the compose project is named after.
    pub project: String,
    /// The task's folder.
    pub root: PathBuf,
    /// The task's environment: the project's `.env` with the task's own values on top.
    pub env: Vec<(String, String)>,
    /// The values among them that must not be shown, to clean messages of.
    pub secrets: Vec<String>,
}

impl Place {
    /// A task without its environment: enough for stopping and removing.
    pub fn bare(task_id: &str, project: &str, root: &Path) -> Self {
        Self {
            task_id: task_id.into(),
            project: project.into(),
            root: root.to_path_buf(),
            env: vec![],
            secrets: vec![],
        }
    }

    pub(crate) fn compose_project(&self) -> String {
        compose_project(&self.project, &self.task_id)
    }
}

/// One thing the project can run on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Script {
    pub name: String,
    pub command: String,
}

/// The lock file of a project and the program that wrote it.
fn lock_file(root: &Path) -> Option<(&'static str, &'static str)> {
    LOCK_FILES.iter().copied().find(|(name, _)| root.join(name).is_file())
}

/// The program that runs a project's scripts, judged by its lock file.
pub(crate) fn package_manager(root: &Path) -> &'static str {
    lock_file(root).map_or("npm", |(_, manager)| manager)
}

fn package_json(root: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(root.join("package.json")).ok()?).ok()
}

/// What runs a Python program of a project: the project's own environment when it has one.
pub(crate) fn python_prefix(folder: &Path, program: &str) -> String {
    if folder.join("uv.lock").is_file() {
        return format!("uv run {program}");
    }
    if folder.join("poetry.lock").is_file() {
        return format!("poetry run {program}");
    }
    if folder.join("Pipfile").is_file() {
        return format!("pipenv run {program}");
    }
    for env in [".venv", "venv"] {
        if folder.join(env).join("bin").join(program).is_file() {
            return format!("{env}/bin/{program}");
        }
    }
    program.to_string()
}

/// The files a Python web app is usually made in, with the module each is imported as.
const PYTHON_APPS: [(&str, &str); 7] = [
    ("main.py", "main"),
    ("app.py", "app"),
    ("app/main.py", "app.main"),
    ("src/main.py", "src.main"),
    ("api/main.py", "api.main"),
    ("app/__init__.py", "app"),
    ("wsgi.py", "wsgi"),
];

/// The name a file gives to what `made(` makes, as in `app = FastAPI(`. With the module it is in.
fn python_app(root: &Path, made: &str) -> Option<String> {
    PYTHON_APPS.iter().find_map(|(file, module)| {
        let text = std::fs::read_to_string(root.join(file)).ok()?;
        let line = text
            .lines()
            .find(|line| line.split_once('=').is_some_and(|(_, right)| right.trim_start().starts_with(made)))?;
        let name = line.split('=').next()?.split(':').next()?.trim();
        (!name.is_empty() && name.chars().all(|sign| sign.is_alphanumeric() || sign == '_'))
            .then(|| format!("{module}:{name}"))
    })
}

/// The lines of a `Procfile`: a name and the command that runs under it.
fn procfile(root: &Path) -> Vec<Script> {
    let Ok(text) = std::fs::read_to_string(root.join("Procfile")) else { return vec![] };
    text.lines()
        .filter_map(|line| line.split_once(':'))
        .map(|(name, command)| (name.trim(), command.trim()))
        // "release" is run once at a deploy; it is not something that stays up.
        .filter(|(name, command)| {
            !name.is_empty()
                && !command.is_empty()
                && *name != "release"
                && name.chars().all(|sign| sign.is_alphanumeric() || "-_".contains(sign))
        })
        .map(|(name, command)| Script { name: name.to_string(), command: command.to_string() })
        .collect()
}

/// What a project that is a service in one of the usual frameworks is started with, when it does
/// not say so itself. The port can be moved with `PORT` in the task's own values, so two tasks of
/// one project can run side by side.
fn framework(root: &Path) -> Vec<Script> {
    let has = |name: &str| root.join(name).is_file();
    let script = |name: &str, command: String| Script { name: name.to_string(), command };
    let mut found = Vec::new();
    if has("manage.py") {
        found.push(script("web", format!("{} manage.py runserver ${{PORT:-8000}}", python_prefix(root, "python"))));
    } else if let Some(app) = python_app(root, "FastAPI(") {
        found.push(script("api", format!("{} {app} --reload --port ${{PORT:-8000}}", python_prefix(root, "uvicorn"))));
    } else if let Some(app) = python_app(root, "Flask(") {
        found.push(script(
            "web",
            format!("{} --app {app} run --debug --port ${{PORT:-5000}}", python_prefix(root, "flask")),
        ));
    } else if has("bin/rails") {
        found.push(script("web", "bin/rails server -p ${PORT:-3000}".into()));
    } else if has("artisan") {
        found.push(script("web", "php artisan serve --port=${PORT:-8000}".into()));
    } else if has("go.mod") && has("main.go") {
        found.push(script("server", "go run .".into()));
    }
    if let Some((folder, app)) = crate::jobs::celery_app(root) {
        // The app is imported from the folder its package lives in, which may be one below the top.
        let line = format!("{} -A {app} worker --loglevel=info", python_prefix(&root.join(&folder), "celery"));
        found.push(script("worker", if folder.is_empty() { line } else { format!("cd {} && {line}", quote(&folder)) }));
    }
    found
}

/// What of a project can be started and stays up: the scripts of `package.json` that start
/// something long-running, the lines of a `Procfile`, and, for a service in one of the usual
/// frameworks that names neither, the command the framework is started with.
pub fn scripts(root: &Path) -> Vec<Script> {
    let manager = package_manager(root);
    let mut found: Vec<Script> = package_json(root)
        .as_ref()
        .and_then(|package| package.get("scripts"))
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter(|(name, _)| {
            // "dev", "api:dev", "dev:web" and the like.
            name.split(':').any(|part| SCRIPT_NAMES.contains(&part))
        })
        .map(|(name, _)| Script {
            name: name.clone(),
            command: if manager == "npm" { format!("npm run {name}") } else { format!("{manager} {name}") },
        })
        .collect();
    // "dev" first: it is what people start most.
    found.sort_by_key(|script| (script.name != "dev", script.name.clone()));
    // What the project says itself comes before what is guessed from its framework, and a name is taken once.
    let said = procfile(root);
    let more = if said.is_empty() { framework(root) } else { said };
    for script in more {
        if !found.iter().any(|known| known.name == script.name) {
            found.push(script);
        }
    }
    found
}

/// What installs a project's packages. With a lock file it installs exactly
/// what the lock file says and leaves the file alone: running a project must
/// not change what git tracks.
pub fn install_command(root: &Path) -> String {
    match lock_file(root) {
        Some((_, "pnpm")) => "pnpm install --frozen-lockfile".into(),
        // The newer Yarn has its own word for the same promise.
        Some((_, "yarn")) if root.join(".yarnrc.yml").is_file() => "yarn install --immutable".into(),
        Some((_, "yarn")) => "yarn install --frozen-lockfile".into(),
        Some((_, "bun")) => "bun install --frozen-lockfile".into(),
        Some(_) => "npm ci".into(),
        None => "npm install".into(),
    }
}

/// Whether a `package.json` names anything to install.
fn wants_packages(package: &Value) -> bool {
    let named = |list: &str| package.get(list).and_then(Value::as_object).is_some_and(|names| !names.is_empty());
    // A workspace root may name nothing itself: its packages do.
    named("dependencies")
        || named("devDependencies")
        || named("optionalDependencies")
        || package.get("workspaces").is_some()
}

/// When the packages were last installed: the newest of the marks package
/// managers leave in `node_modules`, or of the folder itself. Without the
/// folder, the map Yarn writes when it keeps packages elsewhere counts as
/// the install. `None` when there is neither.
fn installed_at(root: &Path) -> Option<SystemTime> {
    let changed = |path: PathBuf| path.metadata().ok()?.modified().ok();
    let folder = root.join("node_modules");
    if !folder.is_dir() {
        return changed(root.join(".pnp.cjs"));
    }
    let made = changed(folder.clone())?;
    Some(INSTALL_MARKS.iter().filter_map(|mark| changed(folder.join(mark))).fold(made, SystemTime::max))
}

fn millis(time: SystemTime) -> u64 {
    time.duration_since(SystemTime::UNIX_EPOCH).map(|since| since.as_millis() as u64).unwrap_or(0)
}

pub fn compose_file(root: &Path) -> Option<PathBuf> {
    COMPOSE_FILES.iter().map(|name| root.join(name)).find(|path| path.is_file())
}

/// A name Docker accepts for a compose project: lower-case letters, digits and dashes.
pub fn compose_project(project_name: &str, task_id: &str) -> String {
    let mut slug: String =
        project_name.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    slug = slug.trim_matches('-').chars().take(24).collect();
    if slug.is_empty() {
        slug.push_str("project");
    }
    format!("ae-{slug}-{}", task_id[..task_id.len().min(4)].to_lowercase())
}

fn port_is_free(port: u16) -> bool {
    // Free means nothing listens on it on any address of this machine.
    TcpListener::bind(("0.0.0.0", port)).is_ok() && TcpListener::bind(("127.0.0.1", port)).is_ok()
}

/// Gives each published port of a resolved compose file a port that is free on
/// this machine. Returns what was moved: (service, wanted, given).
pub fn free_ports(config: &mut Value, is_free: &dyn Fn(u16) -> bool) -> Vec<(String, u16, u16)> {
    let mut moved = Vec::new();
    let mut taken: HashSet<u16> = HashSet::new();
    let Some(services) = config.get_mut("services").and_then(Value::as_object_mut) else {
        return moved;
    };
    for (name, service) in services.iter_mut() {
        let Some(ports) = service.get_mut("ports").and_then(Value::as_array_mut) else { continue };
        for port in ports.iter_mut() {
            let wanted = match port.get("published") {
                Some(Value::String(text)) => text.parse::<u16>().ok(),
                Some(Value::Number(number)) => number.as_u64().and_then(|n| u16::try_from(n).ok()),
                _ => None,
            };
            let Some(wanted) = wanted else { continue };
            let given = (wanted..=u16::MAX)
                .find(|candidate| !taken.contains(candidate) && is_free(*candidate))
                .unwrap_or(wanted);
            taken.insert(given);
            if given != wanted {
                port["published"] = json!(given.to_string());
                moved.push((name.clone(), wanted, given));
            }
        }
    }
    moved
}

/// The name of an image without where it comes from and which version it is:
/// `ghcr.io/acme/postgres:16` is `postgres`.
pub(crate) fn image_name(image: &str) -> &str {
    let last = image.rsplit('/').next().unwrap_or(image);
    last.split([':', '@']).next().unwrap_or(last)
}

fn is_database(image: &str) -> bool {
    DATABASE_IMAGES.contains(&image_name(image))
}

/// The databases among the services of a resolved compose file, each with its
/// named volumes. `project` is the task's compose project: a volume that
/// carries its name belongs to the task alone.
pub fn databases(config: &Value, project: &str) -> Vec<Database> {
    let mut found = Vec::new();
    for (name, service) in config.get("services").and_then(Value::as_object).into_iter().flatten() {
        let Some(image) = service.get("image").and_then(Value::as_str).filter(|image| is_database(image)) else {
            continue;
        };
        let mounts: Vec<&Value> = service.get("volumes").and_then(Value::as_array).into_iter().flatten().collect();
        let target = |mount: &Value| mount.get("target").and_then(Value::as_str).unwrap_or_default().to_string();
        let volumes = mounts
            .iter()
            .filter(|mount| mount.get("type").and_then(Value::as_str) == Some("volume"))
            .filter_map(|mount| mount.get("source").and_then(Value::as_str))
            .map(|key| {
                let declared = config.pointer(&format!("/volumes/{}", key.replace('~', "~0").replace('/', "~1")));
                let external = declared.and_then(|v| v.get("external")).and_then(Value::as_bool).unwrap_or(false);
                let own_name = format!("{project}_{key}");
                let name = declared.and_then(|v| v.get("name")).and_then(Value::as_str).unwrap_or(&own_name);
                DbVolume { name: name.to_string(), own: !external && name == own_name }
            })
            .collect();
        found.push(Database {
            name: name.clone(),
            image: image.to_string(),
            volumes,
            seed: mounts.iter().any(|mount| target(mount).starts_with(SEED_FOLDER)),
        });
    }
    found
}

/// Starts and stops what a task runs, and says how each part stands.
pub struct Runner {
    pub(crate) terminals: Arc<Terminals>,
    pub(crate) docker: PathBuf,
    pub(crate) data_dir: PathBuf,
    /// The scripts of each task that are started once its install has ended.
    waiting: Arc<Mutex<HashMap<String, Vec<String>>>>,
    /// Tells every interface that what a task runs has changed. Set by whoever
    /// can reach them; needed where something starts later, with no command to answer.
    told: Arc<OnceLock<Told>>,
}

/// A way to say "what this task runs has changed".
pub type Told = Box<dyn Fn(&str) + Send + Sync>;

impl Runner {
    pub fn new(terminals: Arc<Terminals>, docker: PathBuf, data_dir: PathBuf) -> Self {
        Self { terminals, docker, data_dir, waiting: Arc::default(), told: Arc::default() }
    }

    /// Gives the runner its way to tell the interfaces of a change, unless it has one already.
    pub fn tell_with(&self, make: impl FnOnce() -> Told) {
        self.told.get_or_init(make);
    }

    pub(crate) fn docker(&self, args: &[&str], cwd: &Path) -> Result<String> {
        self.docker_with(args, cwd, &[])
    }

    /// Runs Docker with more in its environment: a compose file may use the task's values.
    fn docker_with(&self, args: &[&str], cwd: &Path, env: &[(String, String)]) -> Result<String> {
        let output = Command::new(&self.docker)
            .args(args)
            .current_dir(cwd)
            .envs(env.iter().map(|(name, value)| (name, value)))
            .stdin(Stdio::null())
            .output()
            .map_err(|_| anyhow!("Docker is not installed on this machine"))?;
        if !output.status.success() {
            bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    pub(crate) fn docker_running(&self) -> (bool, Option<String>) {
        let installed = self.docker.is_file() || which::which(&self.docker).is_ok();
        if !installed {
            return (false, Some("Docker is not installed on this machine.".into()));
        }
        let running = Command::new(&self.docker)
            .args(["info", "--format", "{{.ServerVersion}}"])
            .stdin(Stdio::null())
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false);
        (
            running,
            (!running).then(|| "Docker is installed but not running. Start Docker Desktop, then try again.".into()),
        )
    }

    /// The compose file as Docker understands it, with every default and path
    /// filled in. It is read as the task's own compose project, so the names
    /// Docker gives its volumes and its network carry the task's name, and
    /// with the task's environment, which the file may refer to.
    pub(crate) fn resolved(&self, place: &Place, file: &Path) -> Result<Value> {
        let project = place.compose_project();
        let args = ["compose", "-p", &project, "-f", &file.to_string_lossy(), "config", "--format", "json"];
        let text = self.docker_with(&args, &place.root, &place.env)?;
        serde_json::from_str(&text).context("Docker's reading of the compose file could not be understood")
    }

    /// Where the core keeps what it needs for running one task.
    pub(crate) fn task_folder(&self, task_id: &str) -> PathBuf {
        self.data_dir.join("run").join(task_id)
    }

    /// Where the task's own copy of the compose file is kept.
    fn task_file(&self, task_id: &str) -> PathBuf {
        self.task_folder(task_id).join("compose.json")
    }

    /// The compose arguments that point at the task's project.
    pub(crate) fn compose_args(&self, project: &str, task_id: &str) -> Vec<String> {
        vec![
            "compose".into(),
            "-p".into(),
            project.into(),
            "-f".into(),
            self.task_file(task_id).to_string_lossy().into_owned(),
        ]
    }

    /// A compose command for the task's project, as one line for a shell.
    fn compose_line(&self, place: &Place, rest: &str) -> String {
        let args = self.compose_args(&place.compose_project(), &place.task_id);
        let args = args.iter().map(|arg| quote(arg)).collect::<Vec<_>>().join(" ");
        format!("{} {args} {rest}", quote(&self.docker.to_string_lossy()))
    }

    /// The containers of a compose project, as `docker compose ps` lists them.
    pub(crate) fn containers(&self, project: &str, root: &Path) -> Vec<Value> {
        let Ok(text) = self.docker(&["compose", "-p", project, "ps", "--all", "--format", "json"], root) else {
            return vec![];
        };
        // Newer versions print one object per line; older ones print a list.
        if let Ok(Value::Array(list)) = serde_json::from_str::<Value>(text.trim()) {
            return list;
        }
        text.lines().filter_map(|line| serde_json::from_str(line).ok()).collect()
    }

    fn is_running(&self, place: &Place, service: &str) -> bool {
        self.containers(&place.compose_project(), &place.root).iter().any(|container| {
            container.get("Service").and_then(Value::as_str) == Some(service)
                && container.get("State").and_then(Value::as_str) == Some("running")
        })
    }

    /// The scripts of a task that wait for its install to end.
    fn waiting_for_install(&self, task_id: &str) -> Vec<String> {
        self.waiting.lock().unwrap().get(task_id).cloned().unwrap_or_default()
    }

    /// The project's packages and whether they are installed in this task's folder.
    pub fn dependencies(&self, place: &Place) -> Option<Dependencies> {
        let (task_id, root) = (&place.task_id, &place.root);
        let package = package_json(root)?;
        let command = install_command(root);
        let lock = lock_file(root).map(|(name, _)| name);
        let installing = self.terminals.find(task_id, INSTALL);
        let failed = self.terminals.last_exit(task_id, INSTALL).filter(|code| *code != 0);
        let installed = installed_at(root);
        let lock_changed = lock.and_then(|name| root.join(name).metadata().ok()?.modified().ok());
        let stale = installed.zip(lock_changed).is_some_and(|(done, changed)| changed > done + SAME_INSTALL);

        let state = match (wants_packages(&package), &installing, installed, stale) {
            (false, ..) => "none",
            (_, Some(_), ..) => "installing",
            (_, _, Some(_), false) => "installed",
            // The install was tried and stopped half way: saying only "missing" would hide why.
            _ if failed.is_some() => "failed",
            (_, _, None, _) => "missing",
            _ => "stale",
        };
        let note = (state == "failed").then(|| {
            let code = failed.unwrap_or(1);
            // Its last words are the best guess at why.
            let said = self.terminals.last_output(task_id, INSTALL).unwrap_or_default();
            let last = said.lines().rev().map(str::trim).find(|line| !line.is_empty());
            let last: String = last.unwrap_or_default().chars().take(240).collect();
            let why = if last.is_empty() { String::new() } else { format!(" Its last words: {last}") };
            env::scrub(&format!("It stopped with code {code}.{why}"), &place.secrets)
        });
        Some(Dependencies {
            command,
            lock_file: lock,
            state,
            installed_at: installed.filter(|_| state != "none").map(millis),
            note,
            term_id: installing.map(|info| info.term_id),
        })
    }

    /// Everything the Run tab shows for a task.
    pub fn state(&self, place: &Place) -> RunState {
        let (task_id, root) = (place.task_id.as_str(), place.root.as_path());
        let project = place.compose_project();
        let mut services = Vec::new();
        let dependencies = self.dependencies(place);

        let waiting = self.waiting_for_install(task_id);
        for script in scripts(root) {
            let purpose = format!("run:{}", script.name);
            let running = self.terminals.find(task_id, &purpose);
            let exit = self.terminals.last_exit(task_id, &purpose);
            let (state, note) = match (&running, exit) {
                (Some(_), _) => ("running", None),
                (None, _) if waiting.contains(&script.name) => {
                    let install = dependencies.as_ref().map_or("the install", |deps| deps.command.as_str());
                    ("starting", Some(format!("Waiting for {install} to finish.")))
                }
                (None, Some(0)) => ("exited", None),
                (None, Some(code)) => ("failed", Some(format!("It stopped with code {code}. The log says why."))),
                (None, None) => ("stopped", None),
            };
            services.push(Service {
                name: script.name,
                kind: "command",
                runs: script.command,
                state,
                address: running.as_ref().and_then(|info| self.terminals.address(info.term_id)),
                note,
                term_id: running.map(|info| info.term_id),
                web: true,
                in_profile: true,
            });
        }

        let file = compose_file(root);
        let mut problem = None;
        let mut found_databases = Vec::new();
        let (mut docker_ready, mut docker_hint) = (true, None);
        if let Some(file) = &file {
            let (ready, hint) = self.docker_running();
            docker_ready = ready;
            docker_hint = hint;
            match self.resolved(place, file) {
                Ok(config) => {
                    found_databases = databases(&config, &project);
                    let containers = if ready { self.containers(&project, root) } else { vec![] };
                    let starting = self.terminals.find(task_id, UP);
                    let last_start = self.terminals.last_exit(task_id, UP);
                    let moved = self.moved_ports(task_id);
                    for (name, service) in config.get("services").and_then(Value::as_object).into_iter().flatten() {
                        let container =
                            containers.iter().find(|c| c.get("Service").and_then(Value::as_str) == Some(name));
                        let running = container.and_then(|c| c.get("State")).and_then(Value::as_str) == Some("running");
                        let state = match (running, &starting, container, last_start) {
                            (true, ..) => "running",
                            (false, Some(_), ..) => "starting",
                            (false, None, Some(_), _) | (false, None, None, Some(1..)) => "failed",
                            _ => "stopped",
                        };
                        let address = container
                            .and_then(|c| c.get("Publishers"))
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                            .filter_map(|p| p.get("PublishedPort").and_then(Value::as_u64))
                            .find(|port| *port > 0)
                            .map(|port| format!("localhost:{port}"));
                        // A moved port is worth saying only while the service is up on it.
                        let mut note = moved
                            .iter()
                            .find(|(service, ..)| service == name && matches!(state, "running" | "starting"))
                            .map(|(_, wanted, given)| format!("Port {wanted} was taken, so this task uses {given}."));
                        if state == "failed" {
                            note = Some(env::scrub(&self.why_failed(task_id, name, container), &place.secrets));
                        }
                        let logs = self.terminals.find(task_id, &format!("logs:{name}")).or(starting.clone());
                        let image = service.get("image").and_then(Value::as_str);
                        services.push(Service {
                            name: name.clone(),
                            kind: "docker",
                            // An image may be named with one of the task's values in it.
                            runs: image
                                .map(|image| env::scrub(image, &place.secrets))
                                .unwrap_or_else(|| "built from a Dockerfile".into()),
                            state,
                            address,
                            note,
                            term_id: logs.map(|info| info.term_id),
                            web: !image
                                .is_some_and(|image| is_database(image) || NOT_WEB_IMAGES.contains(&image_name(image))),
                            in_profile: true,
                        });
                    }
                }
                Err(error) => {
                    let said = env::scrub(&format!("{error:#}"), &place.secrets);
                    problem = Some(format!("The compose file could not be read: {said}"));
                }
            }
        }

        RunState {
            services,
            compose_file: file.and_then(|path| path.file_name().map(|name| name.to_string_lossy().into_owned())),
            compose_project: project,
            docker_ready,
            docker_hint,
            problem,
            profiles: vec![],
            profile: FULL_STACK.into(),
            dependencies,
            databases: found_databases,
        }
    }

    /// The ports that were moved when the task's services were last started.
    fn moved_ports(&self, task_id: &str) -> Vec<(String, u16, u16)> {
        let path = self.task_file(task_id).with_file_name("moved.json");
        std::fs::read_to_string(path).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
    }

    /// Why a container is not running, in a sentence: Docker's own words when it gave any.
    fn why_failed(&self, task_id: &str, service: &str, container: Option<&Value>) -> String {
        if let Some(code) = container.and_then(|c| c.get("ExitCode")).and_then(Value::as_i64).filter(|code| *code != 0)
        {
            return format!("It stopped with code {code}. The log says why.");
        }
        // Docker's complaint is in what the start printed; the line naming this service is the one to show.
        let printed = self
            .terminals
            .find(task_id, UP)
            .and_then(|info| self.terminals.scrollback(info.term_id).ok())
            .map(|(bytes, _)| strip_codes(&String::from_utf8_lossy(&bytes)))
            .or_else(|| self.terminals.last_output(task_id, UP))
            .unwrap_or_default();
        printed
            .lines()
            .rev()
            .map(str::trim)
            .find(|line| line.to_lowercase().contains("error") && (line.contains(service) || !line.contains("service")))
            .map(|line| line.trim_start_matches("Error response from daemon: ").to_string())
            .unwrap_or_else(|| "It did not start. The log says why.".into())
    }

    /// Starts what is named in `only`, or everything when it is `None`.
    pub fn start(&self, place: &Place, only: Option<&[String]>) -> Result<()> {
        self.launch(place, only, false)
    }

    /// Stops and starts again what is named in `only`, or everything when it is `None`.
    pub fn restart(&self, place: &Place, only: Option<&[String]>) -> Result<()> {
        let task_id = &place.task_id;
        let mut ending = Vec::new();
        for info in self.terminals.list(task_id) {
            let wanted = info
                .purpose
                .strip_prefix("run:")
                .is_some_and(|name| only.is_none_or(|names| names.iter().any(|wanted| wanted == name)));
            if wanted && self.terminals.stop(info.term_id).is_ok() {
                ending.push(info.purpose);
            }
        }
        // A program is gone a moment after it is told to end; its place is free only then.
        for _ in 0..100 {
            if ending.iter().all(|purpose| self.terminals.find(task_id, purpose).is_none()) {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        self.launch(place, only, true)
    }

    /// Starts scripts and services. `anew` also makes the containers anew, which is how a service is restarted.
    fn launch(&self, place: &Place, only: Option<&[String]>, anew: bool) -> Result<()> {
        let (task_id, root) = (place.task_id.as_str(), place.root.as_path());
        let named = |name: &str| only.is_none_or(|names| names.iter().any(|wanted| wanted == name));
        let all_scripts = scripts(root);
        let wanted_scripts: Vec<&Script> = all_scripts.iter().filter(|script| named(&script.name)).collect();
        let to_run: Vec<Script> = wanted_scripts
            .iter()
            .filter(|script| self.terminals.find(task_id, &format!("run:{}", script.name)).is_none())
            .map(|script| (*script).clone())
            .collect();
        if !to_run.is_empty() {
            // A script without its packages fails at once: they are installed first.
            let install = self.dependencies(place).filter(|deps| deps.needs_install() || deps.state == "installing");
            match install {
                Some(deps) => self.start_after_install(place, &deps.command, to_run)?,
                None => {
                    for script in &to_run {
                        self.terminals.run(
                            task_id,
                            root,
                            &script.command,
                            &format!("run:{}", script.name),
                            &place.env,
                        )?;
                    }
                }
            }
        }
        let Some(file) = compose_file(root) else {
            if wanted_scripts.is_empty() && only.is_none() {
                bail!("There is nothing to start here: no dev script in package.json and no compose file");
            }
            return Ok(());
        };
        // Everything that was asked for by name is a script: Docker has no part in it.
        if only.is_some_and(|names| names.iter().all(|name| all_scripts.iter().any(|script| &script.name == name))) {
            return Ok(());
        }
        let mut config = self.resolved(place, &file)?;
        let in_docker: Vec<&String> = only
            .into_iter()
            .flatten()
            .filter(|name| config.get("services").and_then(|services| services.get(name.as_str())).is_some())
            .collect();
        if only.is_some() && in_docker.is_empty() {
            return Ok(());
        }
        let (ready, hint) = self.docker_running();
        if !ready {
            bail!("{}", hint.unwrap_or_else(|| "Docker is not ready".into()));
        }

        // Ports this task's own containers hold already are its own, not "taken".
        let project = place.compose_project();
        let mine: HashSet<u16> = self
            .containers(&project, root)
            .iter()
            .flat_map(|c| c.get("Publishers").and_then(Value::as_array).cloned().unwrap_or_default())
            .filter_map(|p| p.get("PublishedPort").and_then(Value::as_u64).and_then(|n| u16::try_from(n).ok()))
            .collect();
        let moved = free_ports(&mut config, &|port| mine.contains(&port) || port_is_free(port));

        let task_file = self.task_file(task_id);
        std::fs::create_dir_all(task_file.parent().context("The data folder has no place for this task")?)?;
        // The file holds the task's values wherever the compose file uses them: it is for this user only.
        env::write_private(&task_file, &serde_json::to_string_pretty(&config)?)?;
        std::fs::write(task_file.with_file_name("moved.json"), serde_json::to_string(&moved)?)?;
        // Kept so the containers and volumes can still be found once the task itself is gone.
        std::fs::write(task_file.with_file_name("project"), &project)?;

        if self.terminals.find(task_id, UP).is_some() {
            bail!("The services are being started already");
        }
        let mut rest = String::from("up -d --remove-orphans");
        if anew {
            rest.push_str(" --force-recreate");
        }
        for name in in_docker {
            rest.push(' ');
            rest.push_str(&quote(name));
        }
        self.terminals.run(task_id, root, &self.compose_line(place, &rest), UP, &place.env)?;
        Ok(())
    }

    /// Installs the project's packages in a terminal of its own.
    pub fn install(&self, place: &Place) -> Result<u32> {
        let Some(deps) = self.dependencies(place) else {
            bail!("There is no package.json in this task's folder, so there is nothing to install");
        };
        if let Some(term_id) = deps.term_id {
            return Ok(term_id);
        }
        Ok(self.terminals.run(&place.task_id, &place.root, &deps.command, INSTALL, &place.env)?.term_id)
    }

    /// Installs the packages, then starts the scripts that need them. The
    /// scripts are started only when the install ended well.
    fn start_after_install(&self, place: &Place, command: &str, scripts_to_run: Vec<Script>) -> Result<()> {
        let task_id = &place.task_id;
        if self.terminals.find(task_id, INSTALL).is_none() {
            self.terminals.run(task_id, &place.root, command, INSTALL, &place.env)?;
        }
        let watched = {
            let mut waiting = self.waiting.lock().unwrap();
            let watched = waiting.contains_key(task_id);
            let names = waiting.entry(task_id.clone()).or_default();
            for script in scripts_to_run {
                if !names.contains(&script.name) {
                    names.push(script.name);
                }
            }
            watched
        };
        if watched {
            // Someone waits for this install already and will start these scripts too.
            return Ok(());
        }
        let (terminals, waiting, place) = (self.terminals.clone(), self.waiting.clone(), place.clone());
        let told = self.told.clone();
        std::thread::Builder::new()
            .name("deps-wait".into())
            .spawn(move || {
                let task_id = &place.task_id;
                while terminals.find(task_id, INSTALL).is_some() {
                    std::thread::sleep(Duration::from_millis(100));
                }
                // Read now, not before: a script that was stopped while it waited is no longer wanted.
                let names = waiting.lock().unwrap().get(task_id).cloned().unwrap_or_default();
                // An install that failed, or was stopped, is not followed by a start.
                if terminals.last_exit(task_id, INSTALL) == Some(0) {
                    for script in scripts(&place.root).into_iter().filter(|script| names.contains(&script.name)) {
                        let purpose = format!("run:{}", script.name);
                        if terminals.find(task_id, &purpose).is_none() {
                            let _ = terminals.run(task_id, &place.root, &script.command, &purpose, &place.env);
                        }
                    }
                }
                // Forgotten last, so the scripts never read as "stopped" between the install and their start.
                waiting.lock().unwrap().remove(task_id);
                // The end of the install was announced before the scripts were started: say it again now.
                if let Some(tell) = told.get() {
                    tell(task_id);
                }
            })
            .context("Could not wait for the install to end")?;
        Ok(())
    }

    /// Stops one service, or everything when `only` is `None`. Volumes are kept.
    pub fn stop(&self, place: &Place, only: Option<&str>) -> Result<()> {
        let (task_id, root) = (place.task_id.as_str(), place.root.as_path());
        {
            let mut waiting = self.waiting.lock().unwrap();
            match (only, waiting.get_mut(task_id)) {
                (Some(name), Some(names)) => names.retain(|waits| waits != name),
                (None, Some(names)) => names.clear(),
                _ => {}
            }
        }
        for info in self.terminals.list(task_id) {
            let of_service = ["run:", "logs:", "shell:"].iter().any(|kind| {
                info.purpose.strip_prefix(kind).is_some_and(|name| only.is_none_or(|wanted| wanted == name))
            });
            if of_service || (only.is_none() && (info.purpose == UP || info.purpose == INSTALL)) {
                let _ = self.terminals.stop(info.term_id);
            }
        }
        if !self.task_file(task_id).is_file() {
            return Ok(());
        }
        let project = place.compose_project();
        let mut args = self.compose_args(&project, task_id);
        match only {
            Some(name) if scripts(root).iter().any(|script| script.name == name) => return Ok(()),
            Some(name) => args.extend(["stop".into(), name.into()]),
            None => args.push("down".into()),
        }
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        self.docker(&args, root)?;
        Ok(())
    }

    /// Stops everything of a task and removes its containers, network and volumes.
    /// Used when the task is finished for good.
    pub fn remove(&self, task_id: &str, project_name: &str, root: &Path) {
        let place = Place::bare(task_id, project_name, root);
        let _ = self.stop(&place, None);
        let mut gone = true;
        if self.task_file(task_id).is_file() {
            let mut args = self.compose_args(&place.compose_project(), task_id);
            args.extend(["down".into(), "--volumes".into(), "--remove-orphans".into()]);
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            let cwd = if root.is_dir() { root } else { self.data_dir.as_path() };
            gone = self.docker(&args, cwd).is_ok();
        }
        let folder = self.task_folder(task_id);
        if gone {
            let _ = std::fs::remove_dir_all(folder);
        } else {
            // Docker could not be asked, so the containers and volumes are still there. What it
            // takes to remove them later stays; the task's own values, which may be secrets, do not.
            let _ = std::fs::remove_file(folder.join("env.json"));
        }
    }

    /// The terminal that shows a service's output, opening one for a container's log when needed.
    pub fn logs(&self, place: &Place, service: &str) -> Result<u32> {
        let (task_id, root) = (place.task_id.as_str(), place.root.as_path());
        if let Some(info) = self.terminals.find(task_id, &format!("run:{service}")) {
            return Ok(info.term_id);
        }
        let purpose = format!("logs:{service}");
        if let Some(info) = self.terminals.find(task_id, &purpose) {
            return Ok(info.term_id);
        }
        if scripts(root).iter().any(|script| script.name == service) {
            bail!("{service} is not running, so it has no output to show. Start it first.");
        }
        if !self.task_file(task_id).is_file() {
            bail!("{service} has not been started in this task yet");
        }
        let line = self.compose_line(place, &format!("logs --follow --tail 200 {}", quote(service)));
        Ok(self.terminals.run(task_id, root, &line, &purpose, &[])?.term_id)
    }

    /// A terminal inside a service's container: `bash` when the image has it, `sh` otherwise.
    pub fn shell(&self, place: &Place, service: &str) -> Result<u32> {
        let (task_id, root) = (place.task_id.as_str(), place.root.as_path());
        let purpose = format!("shell:{service}");
        if let Some(info) = self.terminals.find(task_id, &purpose) {
            return Ok(info.term_id);
        }
        if scripts(root).iter().any(|script| script.name == service) {
            bail!("{service} runs on this machine, not in a container. The Terminal tab is its shell.");
        }
        if !self.task_file(task_id).is_file() || !self.is_running(place, service) {
            bail!("{service} is not running, so there is no container to open a shell in. Start it first.");
        }
        let pick = "if command -v bash >/dev/null 2>&1; then exec bash; else exec sh; fi";
        let line = self.compose_line(place, &format!("exec {} sh -c {}", quote(service), quote(pick)));
        Ok(self.terminals.run(task_id, root, &line, &purpose, &[])?.term_id)
    }

    /// Throws away what a database of the task holds: its container and its
    /// volumes go, and it is started again if it was running. A new, empty
    /// volume is filled by the image itself, from the seed when there is one.
    pub fn reset_database(&self, place: &Place, service: &str) -> Result<()> {
        let (task_id, root) = (place.task_id.as_str(), place.root.as_path());
        let file = compose_file(root).ok_or_else(|| anyhow!("This project has no compose file"))?;
        let (ready, hint) = self.docker_running();
        if !ready {
            bail!("{}", hint.unwrap_or_else(|| "Docker is not ready".into()));
        }
        let config = self.resolved(place, &file)?;
        let database = databases(&config, &place.compose_project())
            .into_iter()
            .find(|database| database.name == service)
            .ok_or_else(|| anyhow!("{service} is not a database of this project"))?;
        if let Some(shared) = database.volumes.iter().find(|volume| !volume.own) {
            bail!(
                "{service} keeps its data in the volume {}, which the compose file names itself, so other \
                 tasks may use it too. It is not reset from here.",
                shared.name
            );
        }
        let was_running = self.is_running(place, service);
        for info in self.terminals.list(task_id) {
            if info.purpose == format!("logs:{service}") || info.purpose == format!("shell:{service}") {
                let _ = self.terminals.stop(info.term_id);
            }
        }
        // The container goes first: a volume that is in use cannot be removed.
        if self.task_file(task_id).is_file() {
            let mut args = self.compose_args(&place.compose_project(), task_id);
            args.extend(["rm".into(), "--stop".into(), "--force".into(), "--volumes".into(), service.into()]);
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            self.docker(&args, root)?;
        }
        for volume in &database.volumes {
            if let Err(error) = self.docker(&["volume", "rm", &volume.name], root) {
                // A volume that was never made is already as empty as it gets.
                if !format!("{error:#}").to_lowercase().contains("no such volume") {
                    return Err(error.context(format!("The volume {} could not be removed", volume.name)));
                }
            }
        }
        if was_running {
            self.start(place, Some(&[service.to_string()]))?;
        }
        Ok(())
    }
}

/// Wraps a word so a shell reads it as one word.
pub(crate) fn quote(word: &str) -> String {
    if !word.is_empty() && word.chars().all(|c| c.is_ascii_alphanumeric() || "-_./:=".contains(c)) {
        return word.to_string();
    }
    if cfg!(windows) {
        format!("\"{}\"", word.replace('"', "\\\""))
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ae-runner-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn offers_the_scripts_that_start_something_with_the_projects_own_package_manager() {
        let dir = folder("scripts");
        std::fs::write(
            dir.join("package.json"),
            r#"{"scripts":{"build":"vite build","api:dev":"node api.js","dev":"vite","test":"vitest","lint":"eslint ."}}"#,
        )
        .unwrap();
        let found = scripts(&dir);
        assert_eq!(
            found,
            vec![
                Script { name: "dev".into(), command: "npm run dev".into() },
                Script { name: "api:dev".into(), command: "npm run api:dev".into() }
            ]
        );
        std::fs::write(dir.join("pnpm-lock.yaml"), "").unwrap();
        assert_eq!(scripts(&dir)[0].command, "pnpm dev");

        // No package.json, or one that is not JSON: nothing to offer, and no failure.
        std::fs::write(dir.join("package.json"), "not json").unwrap();
        assert!(scripts(&dir).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_service_without_a_package_json_is_started_the_way_its_framework_is() {
        let named =
            |dir: &Path| scripts(dir).into_iter().map(|script| (script.name, script.command)).collect::<Vec<_>>();
        let pair = |name: &str, command: &str| (name.to_string(), command.to_string());

        let fastapi = folder("fastapi");
        std::fs::create_dir_all(fastapi.join("app")).unwrap();
        std::fs::write(fastapi.join("app/__init__.py"), "").unwrap();
        std::fs::write(
            fastapi.join("app/main.py"),
            "from fastapi import FastAPI\n\napi: FastAPI = FastAPI(title='Shop')\n",
        )
        .unwrap();
        std::fs::write(fastapi.join("app/queue.py"), "from celery import Celery\ncelery = Celery('shop')\n").unwrap();
        std::fs::write(fastapi.join("uv.lock"), "").unwrap();
        assert_eq!(crate::jobs::celery_app(&fastapi), Some((String::new(), "app.queue:celery".to_string())));

        // A service one folder down is started from that folder, with that folder's own environment.
        let split = folder("split");
        std::fs::create_dir_all(split.join("backend/shop")).unwrap();
        std::fs::write(split.join("backend/poetry.lock"), "").unwrap();
        std::fs::write(split.join("backend/shop/__init__.py"), "").unwrap();
        std::fs::write(split.join("backend/shop/celery.py"), "from celery import Celery\napp = Celery('shop')\n")
            .unwrap();
        assert_eq!(
            named(&split),
            [pair("worker", "cd backend && poetry run celery -A shop.celery:app worker --loglevel=info")]
        );
        std::fs::remove_dir_all(split).unwrap();
        assert_eq!(
            named(&fastapi),
            [
                pair("api", "uv run uvicorn app.main:api --reload --port ${PORT:-8000}"),
                pair("worker", "uv run celery -A app.queue:celery worker --loglevel=info")
            ]
        );

        let django = folder("django");
        std::fs::write(django.join("manage.py"), "").unwrap();
        assert_eq!(named(&django), [pair("web", "python manage.py runserver ${PORT:-8000}")]);

        // What the project says itself stands instead of the guess, and beside its package scripts.
        std::fs::write(
            django.join("Procfile"),
            "web: gunicorn shop.wsgi\nrelease: python manage.py migrate\n\nworker:celery -A shop worker\n",
        )
        .unwrap();
        std::fs::write(django.join("package.json"), r#"{"scripts":{"dev":"vite","worker":"node worker.js"}}"#).unwrap();
        assert_eq!(
            named(&django),
            [pair("dev", "npm run dev"), pair("worker", "npm run worker"), pair("web", "gunicorn shop.wsgi")]
        );

        let go = folder("go");
        std::fs::write(go.join("go.mod"), "module shop\n").unwrap();
        assert!(named(&go).is_empty(), "a module without a main is a library");
        std::fs::write(go.join("main.go"), "package main\n").unwrap();
        assert_eq!(named(&go), [pair("server", "go run .")]);
        for dir in [fastapi, django, go] {
            std::fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn finds_the_compose_file_by_any_of_its_names() {
        let dir = folder("compose");
        assert_eq!(compose_file(&dir), None);
        std::fs::write(dir.join("docker-compose.yml"), "services: {}").unwrap();
        assert_eq!(compose_file(&dir), Some(dir.join("docker-compose.yml")));
        std::fs::write(dir.join("compose.yaml"), "services: {}").unwrap();
        assert_eq!(compose_file(&dir), Some(dir.join("compose.yaml")), "the newer name wins");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn each_task_gets_a_compose_project_of_its_own() {
        assert_eq!(compose_project("demo-shop", "aa7840a2-1799"), "ae-demo-shop-aa78");
        assert_eq!(compose_project("My Shop (v2)!", "6D938e86"), "ae-my-shop--v2-6d93");
        assert_eq!(compose_project("", "abcd"), "ae-project-abcd");
        assert_ne!(compose_project("shop", "1111aaaa"), compose_project("shop", "2222bbbb"));
    }

    #[test]
    fn a_taken_port_is_moved_to_the_next_free_one() {
        let mut config = json!({ "services": {
            "db": { "image": "postgres:16", "ports": [{ "target": 5432, "published": "5432", "protocol": "tcp" }] },
            "cache": { "image": "redis:7", "ports": [{ "target": 6379, "published": "6379" }] },
            "mail": { "image": "mailpit", "ports": [{ "target": 8025, "published": 5432 }, { "target": 1025 }] },
            "worker": { "image": "worker" }
        } });
        // 5432 and 5433 are taken on this machine; everything else is free.
        let moved = free_ports(&mut config, &|port| port != 5432 && port != 5433);
        assert_eq!(moved, vec![("db".to_string(), 5432, 5434), ("mail".to_string(), 5432, 5435)]);
        assert_eq!(config["services"]["db"]["ports"][0]["published"], "5434");
        assert_eq!(config["services"]["cache"]["ports"][0]["published"], "6379", "a free port stays as written");
        assert_eq!(config["services"]["mail"]["ports"][0]["published"], "5435", "two services never get the same port");
        assert_eq!(config["services"]["mail"]["ports"][1], json!({ "target": 1025 }));
    }

    #[test]
    fn words_are_quoted_for_the_shell_only_when_needed() {
        assert_eq!(quote("docker"), "docker");
        assert_eq!(quote("/usr/local/bin/docker"), "/usr/local/bin/docker");
        if !cfg!(windows) {
            assert_eq!(quote("/Users/me/Application Support/x.json"), "'/Users/me/Application Support/x.json'");
            assert_eq!(quote("it's"), r"'it'\''s'");
        }
    }

    #[test]
    fn the_install_is_the_one_that_keeps_to_the_lock_file() {
        let dir = folder("install");
        assert_eq!(install_command(&dir), "npm install", "without a lock file there is nothing to keep to");
        std::fs::write(dir.join("package-lock.json"), "{}").unwrap();
        assert_eq!(install_command(&dir), "npm ci");
        std::fs::write(dir.join("bun.lock"), "").unwrap();
        assert_eq!(install_command(&dir), "bun install --frozen-lockfile");
        std::fs::write(dir.join("yarn.lock"), "").unwrap();
        assert_eq!(install_command(&dir), "yarn install --frozen-lockfile");
        std::fs::write(dir.join(".yarnrc.yml"), "").unwrap();
        assert_eq!(install_command(&dir), "yarn install --immutable");
        std::fs::write(dir.join("pnpm-lock.yaml"), "").unwrap();
        assert_eq!(install_command(&dir), "pnpm install --frozen-lockfile");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_package_file_that_names_nothing_wants_no_install() {
        assert!(!wants_packages(&json!({ "name": "shop", "scripts": { "dev": "vite" }, "dependencies": {} })));
        assert!(wants_packages(&json!({ "devDependencies": { "vite": "^8" } })));
        assert!(wants_packages(&json!({ "workspaces": ["packages/*"] })), "the packages of a workspace name theirs");
    }

    #[test]
    fn packages_are_installed_as_of_the_newest_mark_in_node_modules() {
        let dir = folder("marks");
        assert_eq!(installed_at(&dir), None);
        std::fs::create_dir_all(dir.join("node_modules")).unwrap();
        let made = installed_at(&dir).expect("the folder itself is a mark");
        let later = made + Duration::from_secs(90);
        let mark = std::fs::File::create(dir.join("node_modules/.modules.yaml")).unwrap();
        mark.set_modified(later).unwrap();
        assert_eq!(installed_at(&dir), Some(later));

        // Yarn may keep the packages outside the project and leave only a map of them.
        std::fs::remove_dir_all(dir.join("node_modules")).unwrap();
        assert_eq!(installed_at(&dir), None);
        std::fs::File::create(dir.join(".pnp.cjs")).unwrap().set_modified(later).unwrap();
        assert_eq!(installed_at(&dir), Some(later));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_database_is_known_by_its_image_and_its_volume_by_the_tasks_project() {
        assert_eq!(image_name("ghcr.io/acme/postgres:16"), "postgres");
        assert_eq!(image_name("localhost:5000/mysql@sha256:abc"), "mysql");
        assert!(is_database("postgres") && is_database("docker.io/library/mariadb:11"));
        assert!(!is_database("mongo-express:1") && !is_database("redis:7") && !is_database("postgres-exporter"));

        let config = json!({
            "services": {
                "db": { "image": "postgres:16", "volumes": [
                    { "type": "volume", "source": "dbdata", "target": "/var/lib/postgresql/data" },
                    { "type": "bind", "source": "/shop/seed", "target": "/docker-entrypoint-initdb.d" }
                ] },
                "legacy": { "image": "mysql:8", "volumes": [{ "type": "volume", "source": "kept", "target": "/var/lib/mysql" }] },
                "bare": { "image": "mongo:8", "volumes": [{ "type": "volume", "target": "/data/db" }] },
                "cache": { "image": "redis:7" },
                "web": { "build": { "context": "." } }
            },
            "volumes": { "dbdata": { "name": "ae-shop-1234_dbdata" }, "kept": { "name": "shop-data" } }
        });
        assert_eq!(
            databases(&config, "ae-shop-1234"),
            vec![
                // No named volume: the data lives in the container.
                Database { name: "bare".into(), image: "mongo:8".into(), volumes: vec![], seed: false },
                Database {
                    name: "db".into(),
                    image: "postgres:16".into(),
                    volumes: vec![DbVolume { name: "ae-shop-1234_dbdata".into(), own: true }],
                    seed: true
                },
                // A name the compose file fixes is the same volume in every task.
                Database {
                    name: "legacy".into(),
                    image: "mysql:8".into(),
                    volumes: vec![DbVolume { name: "shop-data".into(), own: false }],
                    seed: false
                },
            ]
        );
    }
}
