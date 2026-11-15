//! The infrastructure of a project: what a task changes in it, read and not run.
//!
//! A change to a Terraform file or a manifest does nothing a page or a request
//! can show: it takes effect when somebody applies it, often far from the
//! person who wrote it. Its preview is to say what it would do.
//!
//! The infrastructure is found by the files a project keeps:
//!
//! - **Terraform**: `*.tf` and the values in `*.tfvars`;
//! - **Helm**: a `Chart.yaml`, with the values and the templates next to it;
//! - **Kubernetes**: any other YAML that has an `apiVersion` and a `kind`;
//! - **Docker**: a `Dockerfile` and a compose file;
//! - **CI**: `.gitlab-ci.yml` and the workflows under `.github/workflows`.
//!
//! The files the task touched since its branch left the base are listed, each
//! with what its change does (see [`read`]). Nothing of that starts a program.
//!
//! Three checks can be started by hand, each in a terminal of the task and
//! only when its tool is on this computer: `terraform validate`,
//! `terraform plan` (without locking or refreshing the state) and
//! `helm template`. Nothing here ever applies anything.

pub mod read;

use crate::gitops;
use crate::runner::{self, quote};
use crate::server::AppState;
use crate::terminals::Terminals;
use crate::tools::{self, Tools};
use anyhow::{Result, anyhow, bail};
use read::Step;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// How many files of a project are looked at, and how long one that is read may be.
const MOST_FILES: usize = 6000;
const MAX_FILE: u64 = 400 * 1024;
/// How much of a YAML file is read to tell whether it is a manifest.
const HEAD: usize = 64 * 1024;
/// How many changed files are read, and how many checks are offered.
const MOST_CHANGED: usize = 200;
const MOST_CHECKS: usize = 24;

/// The tools, in the order they are shown.
pub const TOOLS: [&str; 6] = ["terraform", "kubernetes", "helm", "docker", "compose", "ci"];

/// The tool a file belongs to, judged by its name and place alone. `None` for a file that is none
/// of them by name: a YAML file may still be a manifest, which only its text says.
fn tool_by_name(path: &str, charts: &BTreeSet<String>) -> Option<&'static str> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let lower = name.to_lowercase();
    let yaml = lower.ends_with(".yaml") || lower.ends_with(".yml");
    if lower.ends_with(".tf") || lower.ends_with(".tfvars") {
        return Some("terraform");
    }
    if path == ".gitlab-ci.yml" || (path.starts_with(".github/workflows/") && yaml) {
        return Some("ci");
    }
    if lower == "dockerfile"
        || lower == "containerfile"
        || lower.starts_with("dockerfile.")
        || lower.ends_with(".dockerfile")
    {
        return Some("docker");
    }
    let compose = |start: &str| lower.strip_prefix(start).is_some_and(|rest| rest.starts_with(['.', '-']));
    if yaml && (compose("docker-compose") || compose("compose")) {
        return Some("compose");
    }
    if name == "Chart.yaml" {
        return Some("helm");
    }
    // The values and the templates of a chart are the chart's.
    let folder = path.rsplit_once('/').map_or("", |(folder, _)| folder);
    if yaml && lower.starts_with("values") && charts.contains(folder) {
        return Some("helm");
    }
    let template = yaml || lower.ends_with(".tpl");
    if template
        && charts.iter().any(|chart| {
            path.strip_prefix(chart.as_str()).is_some_and(|rest| rest.trim_start_matches('/').starts_with("templates/"))
        })
    {
        return Some("helm");
    }
    None
}

fn is_yaml(path: &str) -> bool {
    let lower = path.to_lowercase();
    lower.ends_with(".yaml") || lower.ends_with(".yml")
}

/// Whether a YAML text is a Kubernetes manifest: one of its documents says its `apiVersion` and its `kind`.
fn is_manifest(text: &str) -> bool {
    let has = |key: &str| text.lines().any(|line| line.strip_prefix(key).is_some_and(|rest| rest.starts_with(':')));
    has("apiVersion") && has("kind")
}

/// The start of a file, as text. Empty when it cannot be read.
fn head_of(file: &Path) -> String {
    use std::io::Read;
    let mut bytes = Vec::new();
    if let Ok(open) = std::fs::File::open(file) {
        let _ = open.take(HEAD as u64).read_to_end(&mut bytes);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// The infrastructure files of a project, each with its tool, and the folders that are charts.
#[derive(Debug, Default)]
pub struct Found {
    pub files: Vec<(String, &'static str)>,
    pub charts: BTreeSet<String>,
}

impl Found {
    /// The folders that hold Terraform files.
    fn terraform_folders(&self) -> BTreeSet<String> {
        self.files
            .iter()
            .filter(|(path, tool)| *tool == "terraform" && path.ends_with(".tf"))
            .map(|(path, _)| path.rsplit_once('/').map_or(String::new(), |(folder, _)| folder.to_string()))
            .collect()
    }
}

/// The files of a project that may be infrastructure, by name: within its folder, hidden ones aside,
/// with the pipelines, which are kept in hidden places, added by hand.
fn candidates(root: &Path) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    if root.join(".gitlab-ci.yml").is_file() {
        found.push(".gitlab-ci.yml".into());
    }
    for entry in std::fs::read_dir(root.join(".github/workflows")).into_iter().flatten().flatten().take(200) {
        if entry.path().is_file() {
            found.push(format!(".github/workflows/{}", entry.file_name().to_string_lossy()));
        }
    }
    let walk = ignore::WalkBuilder::new(root).hidden(true).max_depth(Some(12)).build();
    for entry in walk.flatten().filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file())).take(MOST_FILES) {
        let Ok(path) = entry.path().strip_prefix(root).map(|inside| inside.to_string_lossy().replace('\\', "/")) else {
            continue;
        };
        if !path.contains("node_modules/") && !path.contains("vendor/") {
            found.push(path);
        }
    }
    found.sort();
    found
}

/// The folders among some files that are charts: the ones that hold a `Chart.yaml`.
fn charts_among<'a>(paths: impl IntoIterator<Item = &'a str>) -> BTreeSet<String> {
    paths
        .into_iter()
        .filter(|path| path.rsplit('/').next() == Some("Chart.yaml"))
        .map(|path| path.rsplit_once('/').map_or(String::new(), |(folder, _)| folder.to_string()))
        .collect()
}

/// Every infrastructure file of a project, found by reading names and, for YAML, the start of the file.
pub fn scan(root: &Path) -> Found {
    let paths = candidates(root);
    let charts = charts_among(paths.iter().map(String::as_str));
    let files = paths
        .into_iter()
        .filter_map(|path| {
            let tool = tool_by_name(&path, &charts)
                .or_else(|| (is_yaml(&path) && is_manifest(&head_of(&root.join(&path)))).then_some("kubernetes"))?;
            Some((path, tool))
        })
        .collect();
    Found { files, charts }
}

/// Whether a project has any infrastructure file at all. Stops at the first one.
pub fn has_any(root: &Path) -> bool {
    let paths = candidates(root);
    let charts = charts_among(paths.iter().map(String::as_str));
    paths
        .iter()
        .any(|path| tool_by_name(path, &charts).is_some() || (is_yaml(path) && is_manifest(&head_of(&root.join(path)))))
}

/// One infrastructure file the task touched, with what its change does.
#[derive(Debug, Serialize)]
pub struct Changed {
    pub path: String,
    pub tool: &'static str,
    /// "added", "modified" or "deleted".
    pub change: &'static str,
    pub steps: Vec<Step>,
}

/// The infrastructure files that differ from the commit `start`, each read before and after.
pub fn changed(root: &Path, start: &str, found: &Found) -> Result<Vec<Changed>> {
    let all = gitops::changed_since(root, start)?;
    // A chart the task removed is a chart still, for the files that went with it.
    let mut charts = found.charts.clone();
    charts.extend(charts_among(all.iter().map(|(path, _)| path.as_str())));
    let mut files: Vec<Changed> = Vec::new();
    // Each file as it was and as it is, to tell one that was only moved.
    let mut texts: Vec<(Option<String>, Option<String>)> = Vec::new();
    for (path, change) in all {
        let was = match change {
            "added" => None,
            _ => gitops::bytes_at(root, start, &path)?,
        };
        let long = was.as_ref().is_some_and(|bytes| bytes.len() as u64 > MAX_FILE);
        let before = was.filter(|_| !long).map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
        let on_disk = root.join(&path);
        let short = std::fs::metadata(&on_disk).is_ok_and(|meta| meta.len() <= MAX_FILE);
        let after = match change {
            "deleted" => None,
            _ if short => std::fs::read(&on_disk).ok().map(|bytes| String::from_utf8_lossy(&bytes).into_owned()),
            _ => None,
        };
        let manifest =
            || is_yaml(&path) && [&before, &after].iter().any(|text| text.as_deref().is_some_and(is_manifest));
        let Some(tool) = tool_by_name(&path, &charts).or_else(|| manifest().then_some("kubernetes")) else {
            continue;
        };
        let steps = if long || (change != "deleted" && after.is_none()) {
            read::too_long()
        } else {
            read::read(tool, &path, before.as_deref(), after.as_deref())
        };
        files.push(Changed { path, tool, change, steps });
        texts.push((before, after));
        if files.len() >= MOST_CHANGED {
            break;
        }
    }
    settle_moves(&mut files, &texts);
    let order = |tool: &str| TOOLS.iter().position(|one| *one == tool).unwrap_or(TOOLS.len());
    files.sort_by(|a, b| (order(a.tool), &a.path).cmp(&(order(b.tool), &b.path)));
    Ok(files)
}

fn folder_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(folder, _)| folder)
}

fn name_of(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Says of what was only moved that it was moved, so that it is not read as removed in one place and
/// new in another. `texts` holds each file as it was and as it is.
fn settle_moves(files: &mut [Changed], texts: &[(Option<String>, Option<String>)]) {
    for from in 0..files.len() {
        for to in 0..files.len() {
            if from == to || files[from].tool != files[to].tool {
                continue;
            }
            let (left, came) = match from < to {
                true => {
                    let (head, tail) = files.split_at_mut(to);
                    (&mut head[from], &mut tail[0])
                }
                false => {
                    let (head, tail) = files.split_at_mut(from);
                    (&mut tail[0], &mut head[to])
                }
            };
            if left.tool == "terraform" {
                // Terraform reads a folder as one: what counts is whether a block left the folder.
                if folder_of(&left.path) == folder_of(&came.path) {
                    read::settle_moves(&mut left.steps, name_of(&left.path), &mut came.steps, name_of(&came.path));
                }
            } else if left.change == "deleted"
                && came.change == "added"
                && texts[from].0.is_some()
                && texts[from].0 == texts[to].1
            {
                (left.steps, came.steps) = read::moved_whole(&left.path, &came.path);
            }
        }
    }
}

// ----- the checks -----

/// A check of a tool that can be started by hand.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    /// What it is and where: `terraform-plan:infra/prod`.
    pub id: String,
    pub tool: &'static str,
    /// What a person would call it: `terraform plan`.
    pub name: String,
    /// The folder it runs in, within the task's folder. Empty for the task's folder itself.
    pub folder: String,
    /// The command line it is started with.
    pub command: String,
    /// Whether its tool is on this computer.
    pub ready: bool,
    /// What stands in its way, or what it needs first.
    pub note: Option<String>,
    /// Whether the task touched a file of its folder.
    pub changed: bool,
}

/// The kinds of checks: the start of the id, the tool, the program and what it is given.
/// A plan is asked not to lock the state and not to ask the cloud how things stand, and never to ask a question.
const KINDS: [(&str, &str, &str, &str); 3] = [
    ("terraform-validate", "terraform", "terraform", "validate -no-color"),
    ("terraform-plan", "terraform", "terraform", "plan -lock=false -refresh=false -input=false -no-color"),
    ("helm-template", "helm", "helm", "template ."),
];

/// Where a tool is on this computer. One that was named (by a test, or by `AE_TERRAFORM` and
/// `AE_HELM`) is the only one used, even when it is not there.
fn program(tools: &Tools, name: &str) -> Option<PathBuf> {
    let named = if name == "terraform" { &tools.terraform } else { &tools.helm };
    match named {
        Some(path) => path.is_file().then(|| path.clone()),
        // The desktop app is not started from a shell, so the usual places are looked in too.
        None => which::which(name).ok().or_else(|| {
            tools::first_file(["/opt/homebrew/bin", "/usr/local/bin"].iter().map(|folder| Path::new(folder).join(name)))
        }),
    }
}

/// The checks a project offers: two for each folder of Terraform files, one for each chart.
/// The folders the task touched come first.
pub fn checks(root: &Path, found: &Found, touched: &[Changed], tools: &Tools) -> Vec<Check> {
    let within = |folder: &str, path: &str| match folder {
        "" => true,
        folder => path.strip_prefix(folder).is_some_and(|rest| rest.starts_with('/')),
    };
    let mut all = Vec::new();
    for (head, tool, name, args) in KINDS {
        let found_program = program(tools, name);
        let folders = if tool == "terraform" { found.terraform_folders() } else { found.charts.clone() };
        for folder in folders {
            let changed = touched.iter().any(|file| match tool {
                // A Terraform file belongs to the folder it is in, and to no folder above it.
                "terraform" => {
                    file.tool == "terraform" && file.path.rsplit_once('/').map_or("", |(at, _)| at) == folder
                }
                _ => file.tool == "helm" && within(&folder, &file.path),
            });
            let note = match (&found_program, tool) {
                (None, "terraform") => Some("Install Terraform on this computer to run it here.".to_string()),
                (None, _) => Some("Install Helm on this computer to run it here.".to_string()),
                (Some(_), "terraform") if !root.join(&folder).join(".terraform").is_dir() => {
                    Some("This folder was never set up: run `terraform init` in it first.".to_string())
                }
                _ => None,
            };
            let run = found_program.as_ref().map_or(name.to_string(), |path| quote(&path.to_string_lossy()));
            all.push(Check {
                id: format!("{head}:{folder}"),
                tool,
                name: format!("{name} {}", args.split(' ').next().unwrap_or_default()),
                folder,
                command: format!("{run} {args}"),
                ready: found_program.is_some(),
                note,
                changed,
            });
        }
    }
    // What the task touched comes first; the rest keeps its order.
    all.sort_by_key(|check| !check.changed);
    all.truncate(MOST_CHECKS);
    all
}

fn purpose(id: &str) -> String {
    format!("infra:{id}")
}

fn text<'a>(params: &'a Value, name: &str) -> Result<&'a str> {
    params.get(name).and_then(Value::as_str).ok_or_else(|| anyhow!("Missing parameter: {name}"))
}

/// Where the task's branch left its base, when both are known.
fn start_of(state_manager: &crate::tasks::Manager, task_id: &str, root: &Path) -> Result<Option<String>> {
    let task = state_manager.store().task(task_id)?;
    Ok(task
        .branch
        .as_ref()
        .and_then(|_| gitops::resolve_base(root, task.base_branch.as_deref()))
        .and_then(|base| gitops::fork_point(root, &base)))
}

fn handle(
    state_manager: &crate::tasks::Manager,
    terminals: &Arc<Terminals>,
    tools: &Tools,
    method: &str,
    params: &Value,
) -> Result<Value> {
    let task_id = text(params, "taskId")?;
    let (place, _) = runner::place_of(state_manager, task_id)?;
    let root = place.root.as_path();
    // What a project has and what the task touched in it, as every command here needs them.
    let look = || -> Result<(Found, Vec<Changed>, Option<String>)> {
        let found = scan(root);
        Ok(match start_of(state_manager, task_id, root)? {
            Some(start) => {
                let touched = changed(root, &start, &found)?;
                (found, touched, None)
            }
            None => {
                let note = "This task has no base branch, so what it changed cannot be told from the rest.";
                (found, Vec::new(), Some(note.to_string()))
            }
        })
    };
    match method {
        "infra.has" => Ok(json!({ "has": has_any(root) })),
        "infra.get" => {
            let (found, files, note) = look()?;
            let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
            for (_, tool) in &found.files {
                *counts.entry(tool).or_default() += 1;
            }
            let tools_found: Vec<Value> = TOOLS
                .iter()
                .filter_map(|tool| counts.get(tool).map(|files| json!({ "kind": tool, "files": files })))
                .collect();
            let checks = checks(root, &found, &files, tools);
            Ok(json!({ "tools": tools_found, "files": files, "note": note, "checks": checks }))
        }
        "infra.run" => {
            let id = text(params, "id")?;
            let (found, files, _) = look()?;
            let check = checks(root, &found, &files, tools)
                .into_iter()
                .find(|check| check.id == id)
                .ok_or_else(|| anyhow!("This project has no check {id}"))?;
            if !check.ready {
                bail!("{}", check.note.unwrap_or_else(|| format!("{} cannot be run here", check.name)));
            }
            if terminals.find(task_id, &purpose(id)).is_some() {
                bail!("{} is running already. Stop it first, or wait for it to end.", check.name);
            }
            let info = terminals.run(task_id, &root.join(&check.folder), &check.command, &purpose(id), &place.env)?;
            Ok(json!({ "termId": info.term_id, "command": check.command }))
        }
        "infra.state" => Ok(crate::jobs::run_state(terminals, task_id, &purpose(text(params, "id")?), &place.secrets)),
        "infra.stop" => {
            if let Some(info) = terminals.find(task_id, &purpose(text(params, "id")?)) {
                terminals.stop(info.term_id)?;
            }
            Ok(json!({}))
        }
        other => bail!("Unknown method: {other}"),
    }
}

/// The commands of the Infra view.
///
/// - `infra.has { taskId }` says whether the project has any infrastructure file.
/// - `infra.get { taskId }` gives the tools the project has, the infrastructure files the task
///   touched with what each change does, and the checks that can be started.
/// - `infra.run { taskId, id }` starts a check in a terminal of the task.
/// - `infra.state { taskId, id }` says whether it still runs, how it ended and what it printed.
/// - `infra.stop { taskId, id }` stops it.
///
/// They read many files, so they run off the event loop.
pub async fn command(state: &AppState, method: &str, params: &Value) -> Result<Value> {
    let (manager, terminals) = (state.manager.clone(), state.terminals.clone());
    let tools = state.previews.tools().clone();
    let (method, params) = (method.to_string(), params.clone());
    tokio::task::spawn_blocking(move || handle(&manager, &terminals, &tools, &method, &params)).await?
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Folder(PathBuf);

    impl Folder {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("ae-infra-{name}-{}", std::process::id()));
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

    const MANIFEST: &str = "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: settings\n";

    #[test]
    fn the_infrastructure_of_a_project_is_told_from_its_files() {
        let project = Folder::new("scan");
        assert!(!has_any(&project.0), "an empty folder has none");
        project.write("src/app.ts", "export {};\n");
        project.write("config/app.yaml", "name: shop\nkind: of its own\n");
        assert!(!has_any(&project.0), "a YAML file is not a manifest by being YAML");

        project.write("infra/main.tf", "");
        project.write("infra/prod.tfvars", "");
        project.write("infra/modules/net/main.tf", "");
        project.write("k8s/settings.yml", MANIFEST);
        project.write("k8s/many.yaml", &format!("# Two of them\n---\n{MANIFEST}---\n{MANIFEST}"));
        project.write("deploy/shop/Chart.yaml", "name: shop\n");
        project.write("deploy/shop/values.yaml", "");
        project.write("deploy/shop/values-prod.yaml", "");
        project.write("deploy/shop/templates/web.yaml", "kind: {{ .Values.kind }}\n");
        project.write("deploy/shop/templates/_helpers.tpl", "");
        project.write("deploy/shop/README.md", "");
        project.write("deploy/values.yaml", "a: 1\n");
        project.write("Dockerfile", "");
        project.write("docker/worker.Dockerfile", "");
        project.write("docker/Dockerfile.dev", "");
        project.write("compose.yaml", "");
        project.write("docker-compose.override.yml", "");
        project.write("composer.json", "{}");
        project.write(".gitlab-ci.yml", "");
        project.write(".github/workflows/ci.yml", "");
        project.write(".github/dependabot.yml", "");
        project.write("node_modules/x/Dockerfile", "");
        let found = scan(&project.0);
        let of = |tool: &str| -> Vec<&str> {
            found.files.iter().filter(|(_, one)| *one == tool).map(|(path, _)| path.as_str()).collect()
        };
        assert_eq!(of("terraform"), ["infra/main.tf", "infra/modules/net/main.tf", "infra/prod.tfvars"]);
        assert_eq!(of("kubernetes"), ["k8s/many.yaml", "k8s/settings.yml"]);
        assert_eq!(
            of("helm"),
            [
                "deploy/shop/Chart.yaml",
                "deploy/shop/templates/_helpers.tpl",
                "deploy/shop/templates/web.yaml",
                "deploy/shop/values-prod.yaml",
                "deploy/shop/values.yaml"
            ]
        );
        assert_eq!(of("docker"), ["Dockerfile", "docker/Dockerfile.dev", "docker/worker.Dockerfile"]);
        assert_eq!(of("compose"), ["compose.yaml", "docker-compose.override.yml"]);
        assert_eq!(of("ci"), [".github/workflows/ci.yml", ".gitlab-ci.yml"]);
        assert_eq!(found.files.len(), 17, "nothing else is infrastructure: {:?}", found.files);
        assert!(has_any(&project.0));
        assert_eq!(found.terraform_folders().into_iter().collect::<Vec<_>>(), ["infra", "infra/modules/net"]);
    }

    #[test]
    fn a_check_is_offered_for_each_folder_of_terraform_and_each_chart() {
        let project = Folder::new("checks");
        project.write("infra/main.tf", "");
        project.write("infra/.terraform/providers", "");
        project.write("infra/modules/net/main.tf", "");
        project.write("deploy/shop/Chart.yaml", "name: shop\n");
        project.write("tools/terraform", "");
        let found = scan(&project.0);
        let touched = [Changed {
            path: "infra/modules/net/main.tf".into(),
            tool: "terraform",
            change: "modified",
            steps: vec![],
        }];
        // Terraform is here; Helm is named and is not there.
        let tools = Tools {
            terraform: Some(project.0.join("tools/terraform")),
            helm: Some(PathBuf::from("/nonexistent/ae-test/helm")),
            ..Tools::default()
        };
        let all = checks(&project.0, &found, &touched, &tools);
        let terraform = quote(&project.0.join("tools/terraform").to_string_lossy());
        let ids: Vec<(&str, bool, bool)> =
            all.iter().map(|check| (check.id.as_str(), check.ready, check.changed)).collect();
        assert_eq!(
            ids,
            [
                ("terraform-validate:infra/modules/net", true, true),
                ("terraform-plan:infra/modules/net", true, true),
                ("terraform-validate:infra", true, false),
                ("terraform-plan:infra", true, false),
                ("helm-template:deploy/shop", false, false),
            ]
        );
        assert_eq!(all[1].name, "terraform plan");
        assert_eq!(all[1].command, format!("{terraform} plan -lock=false -refresh=false -input=false -no-color"));
        assert_eq!(all[0].note.as_deref(), Some("This folder was never set up: run `terraform init` in it first."));
        assert_eq!(all[2].note, None, "a folder that was set up needs nothing first");
        assert_eq!(all[4].command, "helm template .");
        assert_eq!(all[4].note.as_deref(), Some("Install Helm on this computer to run it here."));
        // No check ever applies anything.
        assert!(all.iter().all(|check| !check.command.contains("apply") && !check.command.contains("install")));
    }
}
