//! Running a project: its dev commands and its Docker services, per task.
//!
//! What can be started is read from the project itself: scripts in
//! `package.json` and services in a compose file. Commands run in terminals of
//! their own, so their output can be watched. Docker services run under a
//! compose project that belongs to the task alone, and a port that is already
//! taken on this machine is moved to a free one, so two tasks never fight over
//! a port, a container name or a volume.
//!
//! The functions block and may run Docker. Call them from `spawn_blocking`.

use crate::terminals::{Terminals, strip_codes};
use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

/// The names a compose file goes by, in the order Docker itself looks for them.
const COMPOSE_FILES: [&str; 4] = ["compose.yaml", "compose.yml", "docker-compose.yml", "docker-compose.yaml"];
/// Scripts worth offering as things to run. Others are still reachable from the terminal.
const SCRIPT_NAMES: [&str; 6] = ["dev", "start", "serve", "api", "worker", "preview"];

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
    /// Where it can be opened, such as `localhost:3000`.
    pub address: Option<String>,
    /// What went wrong, or what was changed to make it start.
    pub note: Option<String>,
    /// The terminal that shows its output right now, if one is open.
    pub term_id: Option<u32>,
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
}

/// One thing the project can run on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Script {
    pub name: String,
    pub command: String,
}

/// The program that runs a project's scripts, judged by its lock file.
fn package_manager(root: &Path) -> &'static str {
    if root.join("pnpm-lock.yaml").is_file() {
        "pnpm"
    } else if root.join("yarn.lock").is_file() {
        "yarn"
    } else if root.join("bun.lockb").is_file() || root.join("bun.lock").is_file() {
        "bun"
    } else {
        "npm"
    }
}

/// The scripts of `package.json` that start something long-running.
pub fn scripts(root: &Path) -> Vec<Script> {
    let Ok(text) = std::fs::read_to_string(root.join("package.json")) else {
        return vec![];
    };
    let Ok(package) = serde_json::from_str::<Value>(&text) else {
        return vec![];
    };
    let manager = package_manager(root);
    let mut found: Vec<Script> = package
        .get("scripts")
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
    found
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

/// Starts and stops what a task runs, and says how each part stands.
pub struct Runner {
    terminals: Arc<Terminals>,
    docker: PathBuf,
    data_dir: PathBuf,
}

impl Runner {
    pub fn new(terminals: Arc<Terminals>, docker: PathBuf, data_dir: PathBuf) -> Self {
        Self { terminals, docker, data_dir }
    }

    fn docker(&self, args: &[&str], cwd: &Path) -> Result<String> {
        let output = Command::new(&self.docker)
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .output()
            .map_err(|_| anyhow!("Docker is not installed on this machine"))?;
        if !output.status.success() {
            bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    fn docker_running(&self) -> (bool, Option<String>) {
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

    /// The compose file as Docker understands it, with every default and path filled in.
    fn resolved(&self, root: &Path, file: &Path) -> Result<Value> {
        let text = self.docker(&["compose", "-f", &file.to_string_lossy(), "config", "--format", "json"], root)?;
        serde_json::from_str(&text).context("Docker's reading of the compose file could not be understood")
    }

    /// Where the task's own copy of the compose file is kept.
    fn task_file(&self, task_id: &str) -> PathBuf {
        self.data_dir.join("run").join(task_id).join("compose.json")
    }

    /// The compose arguments that point at the task's project.
    fn compose_args(&self, project: &str, task_id: &str) -> Vec<String> {
        vec![
            "compose".into(),
            "-p".into(),
            project.into(),
            "-f".into(),
            self.task_file(task_id).to_string_lossy().into_owned(),
        ]
    }

    /// The containers of a compose project, as `docker compose ps` lists them.
    fn containers(&self, project: &str, root: &Path) -> Vec<Value> {
        let Ok(text) = self.docker(&["compose", "-p", project, "ps", "--all", "--format", "json"], root) else {
            return vec![];
        };
        // Newer versions print one object per line; older ones print a list.
        if let Ok(Value::Array(list)) = serde_json::from_str::<Value>(text.trim()) {
            return list;
        }
        text.lines().filter_map(|line| serde_json::from_str(line).ok()).collect()
    }

    /// Everything the Run tab shows for a task.
    pub fn state(&self, task_id: &str, project_name: &str, root: &Path) -> RunState {
        let project = compose_project(project_name, task_id);
        let mut services = Vec::new();

        for script in scripts(root) {
            let purpose = format!("run:{}", script.name);
            let running = self.terminals.find(task_id, &purpose);
            let exit = self.terminals.last_exit(task_id, &purpose);
            let (state, note) = match (&running, exit) {
                (Some(_), _) => ("running", None),
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
            });
        }

        let file = compose_file(root);
        let mut problem = None;
        let (mut docker_ready, mut docker_hint) = (true, None);
        if let Some(file) = &file {
            let (ready, hint) = self.docker_running();
            docker_ready = ready;
            docker_hint = hint;
            match self.resolved(root, file) {
                Ok(config) => {
                    let containers = if ready { self.containers(&project, root) } else { vec![] };
                    let starting = self.terminals.find(task_id, "docker:up");
                    let last_start = self.terminals.last_exit(task_id, "docker:up");
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
                            note = Some(self.why_failed(task_id, name, container));
                        }
                        let logs = self.terminals.find(task_id, &format!("logs:{name}")).or(starting.clone());
                        services.push(Service {
                            name: name.clone(),
                            kind: "docker",
                            runs: service
                                .get("image")
                                .and_then(Value::as_str)
                                .map(str::to_string)
                                .unwrap_or_else(|| "built from a Dockerfile".into()),
                            state,
                            address,
                            note,
                            term_id: logs.map(|info| info.term_id),
                        });
                    }
                }
                Err(error) => problem = Some(format!("The compose file could not be read: {error:#}")),
            }
        }

        RunState {
            services,
            compose_file: file.and_then(|path| path.file_name().map(|name| name.to_string_lossy().into_owned())),
            compose_project: project,
            docker_ready,
            docker_hint,
            problem,
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
            .find(task_id, "docker:up")
            .and_then(|info| self.terminals.scrollback(info.term_id).ok())
            .map(|(bytes, _)| strip_codes(&String::from_utf8_lossy(&bytes)))
            .or_else(|| std::fs::read_to_string(self.task_file(task_id).with_file_name("last-start.log")).ok())
            .unwrap_or_default();
        printed
            .lines()
            .rev()
            .map(str::trim)
            .find(|line| line.to_lowercase().contains("error") && (line.contains(service) || !line.contains("service")))
            .map(|line| line.trim_start_matches("Error response from daemon: ").to_string())
            .unwrap_or_else(|| "It did not start. The log says why.".into())
    }

    /// Starts one service, or all of them when `only` is `None`.
    pub fn start(&self, task_id: &str, project_name: &str, root: &Path, only: Option<&str>) -> Result<()> {
        let wanted_scripts: Vec<Script> =
            scripts(root).into_iter().filter(|script| only.is_none_or(|name| name == script.name)).collect();
        for script in &wanted_scripts {
            let purpose = format!("run:{}", script.name);
            if self.terminals.find(task_id, &purpose).is_none() {
                self.terminals.run(task_id, root, &script.command, &purpose, &[])?;
            }
        }
        let Some(file) = compose_file(root) else {
            if wanted_scripts.is_empty() {
                bail!("There is nothing to start here: no dev script in package.json and no compose file");
            }
            return Ok(());
        };
        let mut config = self.resolved(root, &file)?;
        let docker_service = only.filter(|name| config.pointer(&format!("/services/{name}")).is_some());
        if only.is_some() && docker_service.is_none() {
            // The one thing asked for was a script, and it is started.
            return Ok(());
        }
        let (ready, hint) = self.docker_running();
        if !ready {
            bail!("{}", hint.unwrap_or_else(|| "Docker is not ready".into()));
        }

        // Ports this task's own containers hold already are its own, not "taken".
        let project = compose_project(project_name, task_id);
        let mine: HashSet<u16> = self
            .containers(&project, root)
            .iter()
            .flat_map(|c| c.get("Publishers").and_then(Value::as_array).cloned().unwrap_or_default())
            .filter_map(|p| p.get("PublishedPort").and_then(Value::as_u64).and_then(|n| u16::try_from(n).ok()))
            .collect();
        let moved = free_ports(&mut config, &|port| mine.contains(&port) || port_is_free(port));

        let task_file = self.task_file(task_id);
        std::fs::create_dir_all(task_file.parent().context("The data folder has no place for this task")?)?;
        std::fs::write(&task_file, serde_json::to_string_pretty(&config)?)?;
        std::fs::write(task_file.with_file_name("moved.json"), serde_json::to_string(&moved)?)?;

        if self.terminals.find(task_id, "docker:up").is_some() {
            bail!("The services are being started already");
        }
        let mut line = format!(
            "{} {}",
            quote(&self.docker.to_string_lossy()),
            self.compose_args(&project, task_id).iter().map(|a| quote(a)).collect::<Vec<_>>().join(" ")
        );
        line.push_str(" up -d --remove-orphans");
        if let Some(name) = docker_service {
            line.push(' ');
            line.push_str(&quote(name));
        }
        self.terminals.run(task_id, root, &line, "docker:up", &[])?;
        Ok(())
    }

    /// Stops one service, or everything when `only` is `None`. Volumes are kept.
    pub fn stop(&self, task_id: &str, project_name: &str, root: &Path, only: Option<&str>) -> Result<()> {
        for info in self.terminals.list(task_id) {
            let is_run = info.purpose.strip_prefix("run:").is_some_and(|name| only.is_none_or(|wanted| wanted == name));
            let is_logs =
                info.purpose.strip_prefix("logs:").is_some_and(|name| only.is_none_or(|wanted| wanted == name));
            if is_run || is_logs || (only.is_none() && info.purpose == "docker:up") {
                let _ = self.terminals.stop(info.term_id);
            }
        }
        if !self.task_file(task_id).is_file() {
            return Ok(());
        }
        let project = compose_project(project_name, task_id);
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
        let _ = self.stop(task_id, project_name, root, None);
        if self.task_file(task_id).is_file() {
            let project = compose_project(project_name, task_id);
            let mut args = self.compose_args(&project, task_id);
            args.extend(["down".into(), "--volumes".into(), "--remove-orphans".into()]);
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            let cwd = if root.is_dir() { root } else { self.data_dir.as_path() };
            let _ = self.docker(&args, cwd);
        }
        let _ = std::fs::remove_dir_all(self.data_dir.join("run").join(task_id));
    }

    /// The terminal that shows a service's output, opening one for a container's log when needed.
    pub fn logs(&self, task_id: &str, project_name: &str, root: &Path, service: &str) -> Result<u32> {
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
        let project = compose_project(project_name, task_id);
        let line = format!(
            "{} {} logs --follow --tail 200 {}",
            quote(&self.docker.to_string_lossy()),
            self.compose_args(&project, task_id).iter().map(|a| quote(a)).collect::<Vec<_>>().join(" "),
            quote(service)
        );
        Ok(self.terminals.run(task_id, root, &line, &purpose, &[])?.term_id)
    }
}

/// Wraps a word so a shell reads it as one word.
fn quote(word: &str) -> String {
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
}
