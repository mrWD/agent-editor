//! What running things use: memory by service and by task, and Docker's space on disk.
//!
//! Every number here is asked for: Docker is asked about its containers,
//! images and volumes, and the system is asked about the processes of the
//! commands. Where neither gives a number, there is none.
//!
//! Asking Docker takes a moment, so this is a command of its own and not part
//! of the Run tab's state.

use super::{Place, Runner, compose_project};
use crate::tasks::{Manager, TaskState};
use anyhow::{Result, bail};
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};

/// The label Docker Compose puts on everything of a project.
const PROJECT_LABEL: &str = "com.docker.compose.project";
const SERVICE_LABEL: &str = "com.docker.compose.service";

/// Reads a size as Docker prints it: "120.4MiB", "2.576GB", "163.8kB", "89B".
pub fn bytes(text: &str) -> Option<u64> {
    let text = text.trim();
    let split = text.find(|c: char| !(c.is_ascii_digit() || c == '.'))?;
    let (number, unit) = text.split_at(split);
    let number: f64 = number.parse().ok()?;
    let kib = 1024.0_f64;
    let factor = match unit.trim() {
        "B" => 1.0,
        "kB" | "KB" => 1e3,
        "MB" => 1e6,
        "GB" => 1e9,
        "TB" => 1e12,
        "KiB" => kib,
        "MiB" => kib.powi(2),
        "GiB" => kib.powi(3),
        "TiB" => kib.powi(4),
        _ => return None,
    };
    Some((number * factor).round() as u64)
}

/// One label out of Docker's `name=value,name=value` list.
fn label<'a>(labels: &'a str, name: &str) -> Option<&'a str> {
    labels.split(',').find_map(|pair| pair.strip_prefix(name)?.strip_prefix('='))
}

/// The lines of a Docker listing, each a JSON object.
fn json_lines(text: &str) -> Vec<Value> {
    text.lines().filter_map(|line| serde_json::from_str(line.trim()).ok()).collect()
}

/// A running container of some compose project.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Container {
    project: String,
    service: String,
    /// What it holds in memory, when Docker said.
    memory: Option<u64>,
}

/// Puts the list of running containers together with what each uses.
/// `listed` is what `docker ps` printed, `used` what `docker stats` printed.
fn containers(listed: &str, used: &str) -> Vec<Container> {
    let memory: HashMap<String, u64> = json_lines(used)
        .iter()
        .filter_map(|line| {
            let usage = line.get("MemUsage")?.as_str()?.split('/').next()?;
            Some((line.get("ID")?.as_str()?.to_string(), bytes(usage)?))
        })
        .collect();
    json_lines(listed)
        .iter()
        .filter_map(|line| {
            let labels = line.get("Labels")?.as_str()?;
            let id = line.get("ID").and_then(Value::as_str).unwrap_or_default();
            Some(Container {
                project: label(labels, PROJECT_LABEL)?.to_string(),
                service: label(labels, SERVICE_LABEL)?.to_string(),
                memory: memory.get(id).copied(),
            })
        })
        .collect()
}

/// Every process of this machine: its parent and what it holds in memory.
#[cfg(unix)]
fn processes() -> HashMap<u32, (u32, u64)> {
    // Spelled the way both macOS and Linux take it: every process, three columns, no heading.
    let listed = std::process::Command::new("ps").args(["-A", "-o", "pid=,ppid=,rss="]).output();
    let Ok(listed) = listed else { return HashMap::new() };
    process_table(&String::from_utf8_lossy(&listed.stdout))
}

/// Windows has no `ps`; the commands of a task have no number there.
#[cfg(not(unix))]
fn processes() -> HashMap<u32, (u32, u64)> {
    HashMap::new()
}

/// Reads what `ps` printed: three numbers a line, the last in kilobytes of 1024.
fn process_table(text: &str) -> HashMap<u32, (u32, u64)> {
    text.lines()
        .filter_map(|line| {
            let mut numbers = line.split_whitespace();
            let pid = numbers.next()?.parse().ok()?;
            let parent = numbers.next()?.parse().ok()?;
            let kilobytes: u64 = numbers.next()?.parse().ok()?;
            Some((pid, (parent, kilobytes * 1024)))
        })
        .collect()
}

/// What a process and everything it started hold in memory. `None` for a process that is not there.
fn tree_memory(table: &HashMap<u32, (u32, u64)>, root: u32) -> Option<u64> {
    let mut total = table.get(&root)?.1;
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for (pid, (parent, _)) in table {
        children.entry(*parent).or_default().push(*pid);
    }
    let mut seen: HashSet<u32> = HashSet::from([root]);
    let mut next = vec![root];
    while let Some(pid) = next.pop() {
        for child in children.get(&pid).into_iter().flatten() {
            if seen.insert(*child) {
                total += table[child].1;
                next.push(*child);
            }
        }
    }
    Some(total)
}

/// What a task has running, with what it uses.
struct Running {
    /// The names of its running commands and services.
    names: Vec<String>,
    /// Memory by name, where a number is known.
    memory: Map<String, Value>,
}

impl Running {
    fn total(&self) -> Option<u64> {
        let known: Vec<u64> = self.memory.values().filter_map(Value::as_u64).collect();
        (!known.is_empty()).then(|| known.iter().sum())
    }
}

impl Runner {
    /// The running containers of every compose project, or `None` when Docker cannot be asked.
    fn running_containers(&self) -> Option<Vec<Container>> {
        let listed = self.docker(&["ps", "--format", "json"], &self.data_dir).ok()?;
        // Without the second answer the containers are still known to run; only their numbers are missing.
        let used = self.docker(&["stats", "--no-stream", "--format", "json"], &self.data_dir).unwrap_or_default();
        Some(containers(&listed, &used))
    }

    /// The commands and containers a task has running.
    fn running_of(
        &self,
        task_id: &str,
        project: &str,
        containers: &[Container],
        table: &HashMap<u32, (u32, u64)>,
    ) -> Running {
        let mut running = Running { names: vec![], memory: Map::new() };
        for info in self.terminals.list(task_id) {
            let Some(name) = info.purpose.strip_prefix("run:") else { continue };
            running.names.push(name.to_string());
            if let Some(used) = self.terminals.pid(info.term_id).and_then(|pid| tree_memory(table, pid)) {
                running.memory.insert(name.to_string(), json!(used));
            }
        }
        for container in containers.iter().filter(|container| container.project == project) {
            running.names.push(container.service.clone());
            if let Some(used) = container.memory {
                running.memory.insert(container.service.clone(), json!(used));
            }
        }
        running
    }

    /// Tasks that are gone while their containers and volumes may still be
    /// there, because Docker could not be asked when they went: (task, compose project).
    fn leftovers(&self, open: &HashSet<String>) -> Vec<(String, String)> {
        let Ok(folders) = std::fs::read_dir(self.data_dir.join("run")) else { return vec![] };
        folders
            .filter_map(|folder| {
                let folder = folder.ok()?;
                let task_id = folder.file_name().to_string_lossy().into_owned();
                let project = std::fs::read_to_string(folder.path().join("project")).ok()?;
                let kept = folder.path().join("compose.json").is_file() && !open.contains(&task_id);
                kept.then(|| (task_id, project.trim().to_string()))
            })
            .collect()
    }
}

/// When a task was last busy: its agent's last word or the user's last visit, whichever is later.
fn last_activity(manager: &Manager, task_id: &str) -> Option<u64> {
    let left = manager
        .store()
        .setting(&format!("task.{task_id}.left"))
        .ok()
        .flatten()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|left| left.get("at").and_then(Value::as_u64));
    let said = manager.store().last_event_at(task_id).ok().flatten();
    left.max(said)
}

/// Another task with something running.
struct Other {
    place: Place,
    title: String,
    in_use: bool,
    running: Running,
}

/// Every open task but this one that has something running.
fn others(
    manager: &Manager,
    runner: &Runner,
    this: &str,
    containers: &[Container],
    table: &HashMap<u32, (u32, u64)>,
) -> Vec<Other> {
    let mut names: HashMap<String, String> = HashMap::new();
    let mut found = Vec::new();
    for task in manager.list().unwrap_or_default() {
        if task.row.id == this {
            continue;
        }
        let project_id = &task.row.project_id;
        let name = names.entry(project_id.clone()).or_insert_with(|| manager.project_name(project_id));
        let place = Place::bare(&task.row.id, name, std::path::Path::new(&task.row.cwd));
        let running = runner.running_of(&task.row.id, &compose_project(name, &task.row.id), containers, table);
        if running.names.is_empty() {
            continue;
        }
        found.push(Other {
            place,
            title: task.row.title,
            // A task whose agent is at work, or waits for an answer, is not one to take things away from.
            in_use: matches!(task.state, TaskState::Running | TaskState::WaitingPermission),
            running,
        });
    }
    found
}

/// Docker's space on disk: all its images, and the volumes of this core's
/// tasks. Returns that, and the size of each volume of the compose project `mine`.
fn disk(runner: &Runner, projects: &HashSet<String>, mine: &str) -> Option<(Value, Map<String, Value>)> {
    let summary = runner.docker(&["system", "df", "--format", "json"], &runner.data_dir).ok()?;
    let images = json_lines(&summary)
        .iter()
        .find(|line| line.get("Type").and_then(Value::as_str) == Some("Images"))
        .and_then(|line| bytes(line.get("Size")?.as_str()?));
    let detail = runner.docker(&["system", "df", "-v", "--format", "json"], &runner.data_dir).ok()?;
    let detail: Value = serde_json::from_str(detail.trim()).ok()?;

    let (mut total, mut tasks, mut own) = (0u64, HashSet::new(), Map::new());
    for volume in detail.get("Volumes").and_then(Value::as_array).into_iter().flatten() {
        let project = volume.get("Labels").and_then(Value::as_str).and_then(|labels| label(labels, PROJECT_LABEL));
        let size = volume.get("Size").and_then(Value::as_str).and_then(bytes);
        let (Some(project), Some(size)) = (project, size) else { continue };
        if projects.contains(project) {
            total += size;
            tasks.insert(project.to_string());
        }
        if let Some(name) = volume.get("Name").and_then(Value::as_str).filter(|_| project == mine) {
            own.insert(name.to_string(), json!(size));
        }
    }
    Some((json!({ "images": images, "volumes": total, "tasks": tasks.len() }), own))
}

/// What the Run tab shows about use: memory of this task's services, what runs
/// in other tasks, and, when asked for, Docker's space on disk.
pub fn get(manager: &Manager, runner: &Runner, place: &Place, with_disk: bool) -> Value {
    let containers = runner.running_containers();
    let listed = containers.as_deref().unwrap_or_default();
    let table = processes();
    let mine = place.compose_project();
    let own = runner.running_of(&place.task_id, &mine, listed, &table);

    let mut elsewhere = others(manager, runner, &place.task_id, listed, &table);
    // The heaviest first; those without a number last.
    elsewhere.sort_by(|a, b| b.running.total().cmp(&a.running.total()).then(a.title.cmp(&b.title)));
    let known: Vec<u64> = elsewhere.iter().filter_map(|other| other.running.total()).collect();
    let rows: Vec<Value> = elsewhere
        .iter()
        .map(|other| {
            json!({
                "taskId": other.place.task_id, "title": other.title, "inUse": other.in_use,
                "idleSince": if other.in_use { None } else { last_activity(manager, &other.place.task_id) },
                "memory": other.running.total(), "running": other.running.names,
            })
        })
        .collect();

    let open: HashSet<String> = manager.list().unwrap_or_default().into_iter().map(|task| task.row.id).collect();
    let leftovers = runner.leftovers(&open);
    let mut answer = json!({
        "memory": own.memory,
        "others": rows,
        "othersMemory": (!known.is_empty()).then(|| known.iter().sum::<u64>()),
        "leftovers": leftovers.len(),
    });
    if with_disk {
        let mut projects: HashSet<String> = leftovers.into_iter().map(|(_, project)| project).collect();
        for task in manager.list().unwrap_or_default() {
            projects.insert(compose_project(&manager.project_name(&task.row.project_id), &task.row.id));
        }
        // Docker is asked only when it runs: a compose project needs it, and without one there is nothing to say.
        let found = containers.as_ref().and_then(|_| disk(runner, &projects, &mine));
        let (disk, volumes) = found.unwrap_or_else(|| (Value::Null, Map::new()));
        answer["disk"] = disk;
        answer["volumes"] = Value::Object(volumes);
    }
    answer
}

/// Stops what runs in the other tasks that are idle, and removes the
/// containers and volumes of tasks that are gone. Returns how many of each.
pub fn clean_up(manager: &Manager, runner: &Runner, place: &Place) -> Result<Value> {
    let containers = runner.running_containers();
    let table = processes();
    let mut stopped = 0;
    let mut failed: Option<String> = None;
    for other in others(manager, runner, &place.task_id, containers.as_deref().unwrap_or_default(), &table) {
        if other.in_use {
            continue;
        }
        match runner.stop(&other.place, None) {
            Ok(()) => stopped += 1,
            Err(error) => failed = Some(format!("{} could not be stopped: {error:#}", other.title)),
        }
        manager.broadcast_run_changed(&other.place.task_id);
    }

    let open: HashSet<String> = manager.list()?.into_iter().map(|task| task.row.id).collect();
    let mut removed = 0;
    for (task_id, project) in runner.leftovers(&open) {
        let folder = runner.task_folder(&task_id);
        let file = folder.join("compose.json");
        let args = ["compose", "-p", &project, "-f", &file.to_string_lossy(), "down", "--volumes", "--remove-orphans"];
        match runner.docker(&args, &runner.data_dir) {
            Ok(_) => {
                let _ = std::fs::remove_dir_all(folder);
                removed += 1;
            }
            Err(error) => failed = Some(format!("What a finished task left in Docker could not be removed: {error:#}")),
        }
    }
    if let Some(failed) = failed {
        bail!("{failed}");
    }
    Ok(json!({ "stopped": stopped, "removed": removed }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_read_in_both_of_dockers_spellings() {
        assert_eq!(bytes("89B"), Some(89));
        assert_eq!(bytes("163.8kB"), Some(163_800));
        assert_eq!(bytes("758.4MB"), Some(758_400_000));
        assert_eq!(bytes("2.576GB"), Some(2_576_000_000));
        assert_eq!(bytes("120MiB"), Some(125_829_120));
        assert_eq!(bytes(" 2.376GiB "), Some(2_551_210_574));
        assert_eq!(bytes("0B"), Some(0));
        assert_eq!(bytes("N/A"), None);
        assert_eq!(bytes("12 parsecs"), None);
    }

    #[test]
    fn a_container_is_matched_to_its_project_its_service_and_its_memory() {
        let listed = concat!(
            r#"{"ID":"aaa111","Names":"ae-shop-1a2b-db-1","Labels":"com.docker.compose.project=ae-shop-1a2b,com.docker.compose.service=db,tags=a,b","State":"running"}"#,
            "\n",
            r#"{"ID":"bbb222","Names":"ae-shop-9f9f-db-1","Labels":"com.docker.compose.service=db,com.docker.compose.project=ae-shop-9f9f","State":"running"}"#,
            "\n",
            r#"{"ID":"ccc333","Names":"sonar","Labels":"io.tags=x,y","State":"running"}"#,
            "\n"
        );
        let used = concat!(
            r#"{"ID":"aaa111","Name":"ae-shop-1a2b-db-1","MemUsage":"120MiB / 7.651GiB"}"#,
            "\n",
            r#"{"ID":"ccc333","Name":"sonar","MemUsage":"2.376GiB / 7.651GiB"}"#,
            "\n"
        );
        assert_eq!(
            containers(listed, used),
            vec![
                Container { project: "ae-shop-1a2b".into(), service: "db".into(), memory: Some(125_829_120) },
                // Docker gave no number for this one: it runs, and that is all that is known.
                Container { project: "ae-shop-9f9f".into(), service: "db".into(), memory: None },
            ]
        );
        assert_eq!(containers("", ""), vec![]);
    }

    #[test]
    fn a_command_uses_what_it_and_everything_it_started_use() {
        // 10 is the shell, 11 the package manager it runs, 12 and 13 what that runs; 20 is somebody else's.
        let table =
            process_table("   10     1   2048\n   11    10  10240\n 12 11 51200\n 13 11 1024\n 20 1 99999\nrubbish\n");
        assert_eq!(tree_memory(&table, 10), Some((2048 + 10240 + 51200 + 1024) * 1024));
        assert_eq!(tree_memory(&table, 12), Some(51200 * 1024));
        assert_eq!(tree_memory(&table, 999), None, "a process that is gone uses nothing that can be said");
    }
}
