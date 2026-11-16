//! Projects and tasks joined with the services they live on: which tracker a
//! project's issues come from, starting a task from an issue, and the merge
//! request of a task from opening it to cleaning up after the merge.
//!
//! The functions that ask a service block and use the network. Call them from
//! `spawn_blocking`.

use crate::gitops;
use crate::hosts::{HostKind, Issue, Remote, Request, RequestBranch, remote_of};
use crate::store::{Project, TaskIssue, TaskRow};
use crate::tasks::{Manager, OnBranch, TaskInfo};
use anyhow::{Result, anyhow, bail};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long a merge request's state is trusted before the host is asked again.
const REQUEST_TTL: Duration = Duration::from_secs(45);
/// How many hosts are asked at once when many tasks are looked up together.
const AT_ONCE: usize = 6;

/// What was last read from the services.
#[derive(Default)]
pub struct Remembered {
    /// Per project: the issues last listed, so starting one does not ask the tracker again.
    issues: Mutex<HashMap<String, Vec<Issue>>>,
    /// Per task: when its merge request was last read, and what it was.
    requests: Mutex<HashMap<String, (Instant, Option<Request>)>>,
}

/// Where a project's issues come from.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Source {
    Linear { team: String },
    Host(Remote),
    None,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueRow {
    #[serde(flatten)]
    pub issue: Issue,
    /// The open task that works on this issue, if there is one.
    pub task_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectIssues {
    pub project_id: String,
    /// "github", "gitlab", "linear" or "none".
    pub source: &'static str,
    /// What the source is called on screen: "GitHub mrx/shop", "Linear team APP".
    pub label: String,
    /// The Linear team set for this project, whether or not Linear is the source in use.
    pub linear_team: Option<String>,
    pub issues: Vec<IssueRow>,
    /// Why the issues could not be listed, in words for the user.
    pub problem: Option<String>,
    /// How a request of this project's repository is called in two letters, "PR" on GitHub and
    /// "MR" on GitLab, whatever tracker its issues come from. `None` for a project on neither.
    pub request_short: Option<&'static str>,
}

fn team_setting(project_id: &str) -> String {
    format!("project.{project_id}.linearTeam")
}

/// What an issue becomes as the first message of its task.
fn first_message(issue: &Issue) -> String {
    let mut message = issue.title.clone();
    let body = issue.body.trim();
    if !body.is_empty() {
        message.push_str(&format!("\n\n{body}"));
    }
    // What is attached may be all there is to go on: an issue made from an error
    // in Sentry has no text of its own, only the way to that error.
    if !issue.links.is_empty() {
        message.push_str("\n\nAttached to the issue:");
        for link in &issue.links {
            let title = link.title.trim();
            if title.is_empty() || title == link.url {
                message.push_str(&format!("\n- {}", link.url));
            } else {
                message.push_str(&format!("\n- {title}: {}", link.url));
            }
        }
    }
    message.push_str(&format!("\n\nThis is issue {} ({}).", issue.key, issue.url));
    message
}

/// Where it is kept that the Linear key may only read, once Linear has said so.
const LINEAR_READ_ONLY: &str = "linear.readOnly";

/// Where it is kept that a task's request merges by itself, with how.
fn auto_merge_key(task_id: &str) -> String {
    format!("task.{task_id}.autoMerge")
}

impl Manager {
    /// Where the Docker program is, for the part of the core that runs projects.
    pub fn docker_path(&self) -> PathBuf {
        self.hosts.docker().to_path_buf()
    }

    /// A project's name, or a plain word when the project is gone.
    pub fn project_name(&self, project_id: &str) -> String {
        self.store().project(project_id).map(|project| project.name).unwrap_or_else(|_| "project".into())
    }

    /// Tells every interface that what a task runs has changed.
    pub fn broadcast_run_changed(&self, task_id: &str) {
        self.broadcast("run.changed", json!({ "taskId": task_id }));
    }

    // ----- what is connected -----

    /// The Linear key: the one the user saved, else the one in the environment.
    pub(crate) fn linear_key(&self) -> Option<String> {
        self.store()
            .setting("linear.key")
            .ok()
            .flatten()
            .or_else(|| std::env::var("LINEAR_API_KEY").ok())
            .map(|key| key.trim().to_string())
            .filter(|key| !key.is_empty())
    }

    /// Which services can be used right now, and what to do about those that cannot.
    pub fn integrations(&self) -> Value {
        let servers = self.code_servers();
        json!({
            "github": self.hosts.tool_state(HostKind::GitHub, &servers[0]),
            "gitlab": self.hosts.tool_state(HostKind::GitLab, &servers[1]),
            "linear": { "hasKey": self.linear_key().is_some(), "readOnly": self.linear_read_only() },
            "docker": self.hosts.docker_state(),
        })
    }

    /// The servers the projects live on, GitHub's first and GitLab's second: a company's own GitLab is asked
    /// about itself, not gitlab.com.
    pub(crate) fn code_servers(&self) -> [Vec<String>; 2] {
        let mut servers: [Vec<String>; 2] = [vec![], vec![]];
        for project in self.store().projects().unwrap_or_default() {
            if let Some(remote) = crate::hosts::remote_of(std::path::Path::new(&project.path)) {
                let list = &mut servers[usize::from(remote.kind == HostKind::GitLab)];
                if !list.contains(&remote.host) {
                    list.push(remote.host);
                }
            }
        }
        servers
    }

    /// Saves the Linear key, or forgets it. It is kept in the core's own database on this machine.
    pub fn set_linear_key(&self, key: Option<&str>) -> Result<()> {
        let key = key.map(str::trim).filter(|key| !key.is_empty());
        self.store().set_setting("linear.key", key)?;
        // What was found out about the old key says nothing about the new one.
        self.store().set_setting(LINEAR_READ_ONLY, None)?;
        self.remembered.issues.lock().unwrap().clear();
        self.broadcast("integrations.changed", json!({}));
        Ok(())
    }

    /// Says which Linear team a project's issues come from; `None` goes back to the code host.
    pub fn set_linear_team(&self, project_id: &str, team: Option<&str>) -> Result<()> {
        self.store().project(project_id)?;
        let team = team.map(|team| team.trim().to_uppercase()).filter(|team| !team.is_empty());
        self.store().set_setting(&team_setting(project_id), team.as_deref())?;
        self.remembered.issues.lock().unwrap().remove(project_id);
        self.broadcast("integrations.changed", json!({}));
        Ok(())
    }

    pub(crate) fn linear_team(&self, project_id: &str) -> Option<String> {
        self.store().setting(&team_setting(project_id)).ok().flatten()
    }

    fn source_of(&self, project: &Project) -> Source {
        if let Some(team) = self.linear_team(&project.id) {
            return Source::Linear { team };
        }
        match remote_of(Path::new(&project.path)) {
            Some(remote) => Source::Host(remote),
            None => Source::None,
        }
    }

    // ----- issues -----

    /// The open issues of one project, each with the task that works on it.
    pub fn project_issues(&self, project_id: &str) -> Result<ProjectIssues> {
        let project = self.store().project(project_id)?;
        let source = self.source_of(&project);
        let listed: Result<Vec<Issue>> = match &source {
            Source::Linear { team } => match self.linear_key() {
                Some(key) => self.hosts.linear_issues(&key, team),
                None => Err(anyhow!("Linear is not connected yet. Paste a key above.")),
            },
            Source::Host(remote) => {
                self.hosts.issues(remote).map_err(|error| crate::hosts::said_plainly(remote, error))
            }
            Source::None => Ok(vec![]),
        };
        let (kind, label) = match &source {
            Source::Linear { team } => ("linear", format!("Linear team {team}")),
            Source::Host(remote) => (
                if remote.kind == HostKind::GitHub { "github" } else { "gitlab" },
                format!("{} {}", remote.kind.name(), remote.slug),
            ),
            Source::None => ("none", "No tracker".to_string()),
        };
        let (issues, problem) = match listed {
            Ok(issues) => (issues, None),
            Err(error) => (vec![], Some(format!("{error:#}"))),
        };
        if problem.is_none() {
            self.remembered.issues.lock().unwrap().insert(project_id.to_string(), issues.clone());
        }

        let tasks: Vec<TaskRow> =
            self.store().tasks()?.into_iter().filter(|task| task.project_id == project_id).collect();
        let rows = issues
            .into_iter()
            .map(|issue| IssueRow {
                task_id: tasks
                    .iter()
                    .find(|task| task.issue.as_ref().is_some_and(|mine| mine.key == issue.key && mine.source == kind))
                    .map(|task| task.id.clone()),
                issue,
            })
            .collect();
        Ok(ProjectIssues {
            project_id: project_id.to_string(),
            source: kind,
            label,
            linear_team: self.linear_team(project_id),
            issues: rows,
            problem,
            request_short: remote_of(Path::new(&project.path)).map(|remote| match remote.kind {
                HostKind::GitHub => "PR",
                HostKind::GitLab => "MR",
            }),
        })
    }

    /// The issues of every project, asked side by side.
    pub fn all_issues(self: &Arc<Self>) -> Result<Vec<ProjectIssues>> {
        let projects = self.store().projects()?;
        let mut all = Vec::with_capacity(projects.len());
        for group in projects.chunks(AT_ONCE) {
            let results: Vec<Result<ProjectIssues>> = std::thread::scope(|scope| {
                let handles: Vec<_> =
                    group.iter().map(|project| scope.spawn(|| self.project_issues(&project.id))).collect();
                handles
                    .into_iter()
                    .map(|handle| handle.join().unwrap_or_else(|_| Err(anyhow!("Listing issues failed"))))
                    .collect()
            });
            for result in results {
                all.push(result?);
            }
        }
        Ok(all)
    }

    /// Looks up an issue by its key among those last listed for the project.
    fn remembered_issue(&self, project_id: &str, key: &str) -> Option<(Issue, &'static str)> {
        let issue =
            self.remembered.issues.lock().unwrap().get(project_id)?.iter().find(|issue| issue.key == key)?.clone();
        let project = self.store().project(project_id).ok()?;
        let source = match self.source_of(&project) {
            Source::Linear { .. } => "linear",
            Source::Host(remote) if remote.kind == HostKind::GitHub => "github",
            Source::Host(_) => "gitlab",
            Source::None => return None,
        };
        Some((issue, source))
    }

    /// The open requests of this project's repository that an issue points at: the work on it that
    /// is under way already, which a task can go on with. For an issue of Linear they are what is
    /// attached to it there; for an issue of the code host, the requests the host joins to it (the
    /// pull requests that close it on GitHub, the related merge requests on GitLab). A request of
    /// another repository or from a fork, one that is merged or closed, and one that could not be
    /// read are left out. Empty for a project that is on neither host, and for an issue nothing
    /// points from.
    pub fn issue_requests(&self, project_id: &str, key: &str, source: Option<&str>) -> Result<Vec<RequestBranch>> {
        let project = self.store().project(project_id)?;
        let Some(remote) = crate::hosts::remote_of(Path::new(&project.path)) else { return Ok(vec![]) };
        let linear = source == Some("linear") || self.linear_team(project_id).is_some();
        let links: Vec<String> = match (linear, self.linear_key()) {
            // The list has not what is attached: the issue is read by itself.
            (true, Some(token)) => {
                self.hosts.linear_issue(&token, key)?.links.into_iter().map(|link| link.url).collect()
            }
            (true, None) => vec![],
            // Not being told is no reason to stop a task from starting: it is then taken that nothing is joined.
            (false, _) => match key.trim_start_matches('#').parse::<u64>() {
                Ok(number) => self.hosts.issue_request_links(&remote, number).unwrap_or_default(),
                Err(_) => vec![],
            },
        };
        let mut numbers: Vec<u64> =
            links.iter().filter_map(|url| crate::hosts::request_number_in(&remote, url)).collect();
        numbers.sort_unstable();
        numbers.dedup();
        let mut found = Vec::new();
        for number in numbers {
            match self.hosts.request_by_number(&remote, number) {
                Ok(request) if request.state == "open" && request.own && !request.branch.is_empty() => {
                    found.push(request)
                }
                Ok(_) => {}
                Err(error) => tracing::debug!("Could not read request {number} of {}: {error:#}", remote.slug),
            }
        }
        Ok(found)
    }

    /// The open requests of the issue a task works on, other than the task's own: work on the
    /// issue that is under way on another branch. Empty for a task without an issue.
    pub fn task_issue_requests(&self, task_id: &str) -> Result<Vec<RequestBranch>> {
        let task = self.store().task(task_id)?;
        let Some(issue) = &task.issue else { return Ok(vec![]) };
        let source = (issue.source == "linear").then_some("linear");
        let mut found = self.issue_requests(&task.project_id, &issue.key, source)?;
        found.retain(|request| Some(&request.branch) != task.branch.as_ref());
        Ok(found)
    }

    /// Takes a task that works on a branch of its own over to the branch of an open request: the
    /// work on its issue that was under way before the task. Its working copy goes to that branch
    /// as it stands, and from then on the request is the task's.
    ///
    /// Nothing is lost by it, so it is refused where something would be: while the agent works,
    /// while files are not committed, and while the task's branch holds commits that are neither
    /// on the request's branch nor on the base. The branch the task leaves is removed when it
    /// holds nothing of its own, and is left otherwise.
    pub async fn join_request(self: &Arc<Self>, task_id: &str, number: u64) -> Result<TaskInfo> {
        let task = self.store().task(task_id)?;
        let Some(before) = task.branch.clone() else {
            bail!("This task works in a folder without git, so it has no branch to change");
        };
        let cwd = PathBuf::from(&task.cwd);
        if !cwd.starts_with(self.data_root().join("worktrees")) {
            bail!("This task works in the project's own folder: its branch is yours to change, not the editor's");
        }
        if matches!(
            self.state_of(task_id),
            crate::tasks::TaskState::Running | crate::tasks::TaskState::WaitingPermission
        ) {
            bail!("The agent is working. Wait for its turn to end, or stop it, then go over to the request.");
        }
        let (on, _) = self.on_request(&task.project_id, number).await?;
        if on.branch == before {
            bail!("This task is on the branch of that request already");
        }
        let dirty = crate::git::dirty_count(&cwd).await?;
        if dirty > 0 {
            bail!(
                "{dirty} {} not committed in this task. Commit or undo {} first: going over to another branch must not carry {} along.",
                if dirty == 1 { "file is" } else { "files are" },
                if dirty == 1 { "it" } else { "them" },
                if dirty == 1 { "it" } else { "them" }
            );
        }
        let own = crate::git::commits_of_its_own(&cwd, &on.branch, task.base_branch.as_deref()).await;
        if own > 0 {
            bail!(
                "This task has {own} {} of its own on {before} that the request's branch does not have. They would be left behind: push them as a request of their own, or start a new task on the request.",
                if own == 1 { "commit" } else { "commits" }
            );
        }
        crate::git::switch_worktree_to(&cwd, &on.branch).await?;
        self.store().set_task_branch(task_id, &on.branch, on.base.as_deref())?;
        // Nothing of its own was on it: it goes. git keeps it if that is not so.
        let project = self.store().project(&task.project_id)?;
        crate::git::drop_merged_branch(Path::new(&project.path), &before).await;
        // What was known of the task's request was of the branch it left.
        self.remembered.requests.lock().unwrap().remove(task_id);
        self.emit(
            task_id,
            json!({
                "type": "tracker",
                "text": format!("This task went over to the branch {} of request {number}: what is committed there is where it goes on.", on.branch)
            }),
        );
        let info = self.task(task_id)?;
        self.broadcast("task.updated", json!(info));
        self.broadcast("mr.changed", json!({ "taskId": task_id }));
        Ok(info)
    }

    /// The branch of an open request of a project's repository, for a task that goes on with it,
    /// and the words that tell the agent so.
    async fn on_request(self: &Arc<Self>, project_id: &str, number: u64) -> Result<(OnBranch, String)> {
        let project = self.store().project(project_id)?;
        let remote = crate::hosts::remote_of(Path::new(&project.path))
            .ok_or_else(|| anyhow!("This project's remote is not on GitHub or GitLab"))?;
        let hosts = self.hosts.clone();
        let asked = remote.clone();
        let request = tokio::task::spawn_blocking(move || hosts.request_by_number(&asked, number))
            .await?
            .map_err(|error| crate::hosts::said_plainly(&remote, error))?;
        if request.state != "open" {
            bail!(
                "{} is {}: there is nothing to go on with. Start the task on a new branch.",
                request.name,
                request.state
            );
        }
        if !request.own || request.branch.is_empty() {
            bail!("{} comes from another repository: its branch is not in this one.", request.name);
        }
        let note = format!(
            "\n\nThe work on this goes on in {} ({}), on its branch `{}`: you are on that branch, and what is \
             committed there already is part of this task.",
            request.name, request.url, request.branch
        );
        Ok((OnBranch { branch: request.branch, base: Some(request.base).filter(|base| !base.is_empty()) }, note))
    }

    /// Starts a task for an issue. The issue's text is the first message to the agent. With the
    /// number of a request, the task goes on with that request's branch instead of a new one.
    pub async fn start_issue(
        self: &Arc<Self>,
        project_id: &str,
        key: &str,
        agent_id: &str,
        request: Option<u64>,
    ) -> Result<TaskInfo> {
        let already = self
            .store()
            .tasks()?
            .into_iter()
            .any(|task| task.project_id == project_id && task.issue.as_ref().is_some_and(|issue| issue.key == key));
        if already {
            bail!("A task for {key} is already open");
        }
        let (issue, source) = match self.remembered_issue(project_id, key) {
            Some(found) => found,
            None => {
                // Not listed yet in this run of the core: list once, then look again.
                let manager = self.clone();
                let project = project_id.to_string();
                tokio::task::spawn_blocking(move || manager.project_issues(&project)).await??;
                self.remembered_issue(project_id, key)
                    .ok_or_else(|| anyhow!("There is no open issue {key} in this project"))?
            }
        };
        // A Linear issue is read once more by itself: the list has neither what is
        // attached to it nor what was written since. If that fails, the list's is used.
        let issue = match (source == "linear", self.linear_key()) {
            (true, Some(token)) => {
                let (hosts, wanted) = (self.hosts.clone(), issue.key.clone());
                match tokio::task::spawn_blocking(move || hosts.linear_issue(&token, &wanted)).await? {
                    // Only the very issue that was asked for takes the place of the list's.
                    Ok(read) if read.key == issue.key => read,
                    _ => issue,
                }
            }
            _ => issue,
        };
        let attached = TaskIssue {
            key: issue.key.clone(),
            id: issue.id.clone(),
            url: issue.url.clone(),
            title: issue.title.clone(),
            source: source.to_string(),
        };
        let (on, note) = match request {
            Some(number) => self.on_request(project_id, number).await.map(|(on, note)| (Some(on), note))?,
            None => (None, String::new()),
        };
        let text = format!("{}{note}", first_message(&issue));
        let task = self.create_on(project_id, agent_id, &text, Some(attached), &[], on).await?;

        self.linear_started(&task, project_id);
        Ok(task)
    }

    /// In Linear a started task means a started issue. A failure here must not undo the task.
    fn linear_started(self: &Arc<Self>, task: &TaskInfo, project_id: &str) {
        let Some(issue) = task.row.issue.clone().filter(|issue| issue.source == "linear") else { return };
        let manager = self.clone();
        let (project, task_id) = (project_id.to_string(), task.row.id.clone());
        tokio::task::spawn_blocking(move || manager.move_linear_issue(&task_id, &project, &issue, "started"));
    }

    /// The open Linear issues assigned to whoever the key belongs to, in every team, each with the task working
    /// on it. They need no project set up: a task for one starts in whichever project the user picks.
    pub fn linear_mine(&self) -> Value {
        let Some(key) = self.linear_key() else { return Value::Null };
        let tasks = self.store().tasks().unwrap_or_default();
        match self.hosts.linear_mine(&key) {
            Ok(issues) => {
                let rows: Vec<Value> = issues
                    .into_iter()
                    .map(|issue| {
                        let task = tasks.iter().find(|task| {
                            task.issue.as_ref().is_some_and(|mine| mine.source == "linear" && mine.key == issue.key)
                        });
                        let mut row = json!(issue);
                        row["taskId"] = json!(task.map(|task| task.id.clone()));
                        row["projectId"] = json!(task.map(|task| task.project_id.clone()));
                        row
                    })
                    .collect();
                json!({ "issues": rows, "problem": null })
            }
            Err(error) => json!({ "issues": [], "problem": format!("{error:#}") }),
        }
    }

    /// Who the Linear key belongs to, and the teams of that workspace.
    pub fn linear_account(&self) -> Result<Value> {
        let key = self.linear_key().ok_or_else(|| anyhow!("Linear is not connected yet"))?;
        self.hosts.linear_account(&key)
    }

    /// Starts a task in a project for a Linear issue, whatever tracker the project itself uses.
    pub async fn start_linear_issue(
        self: &Arc<Self>,
        project_id: &str,
        key: &str,
        agent_id: &str,
        request: Option<u64>,
    ) -> Result<TaskInfo> {
        self.store().project(project_id)?;
        let already = self
            .store()
            .tasks()?
            .into_iter()
            .any(|task| task.issue.as_ref().is_some_and(|issue| issue.source == "linear" && issue.key == key));
        if already {
            bail!("A task for {key} is already open");
        }
        let token = self.linear_key().ok_or_else(|| anyhow!("Linear is not connected yet"))?;
        let (hosts, wanted) = (self.hosts.clone(), key.to_string());
        let issue = tokio::task::spawn_blocking(move || hosts.linear_issue(&token, &wanted)).await??;
        let attached = TaskIssue {
            key: issue.key.clone(),
            id: issue.id.clone(),
            url: issue.url.clone(),
            title: issue.title.clone(),
            source: "linear".into(),
        };
        let (on, note) = match request {
            Some(number) => self.on_request(project_id, number).await.map(|(on, note)| (Some(on), note))?,
            None => (None, String::new()),
        };
        let text = format!("{}{note}", first_message(&issue));
        let task = self.create_on(project_id, agent_id, &text, Some(attached), &[], on).await?;
        self.linear_started(&task, project_id);
        Ok(task)
    }

    /// Moves a task's Linear issue and says so in the conversation. Failures are said there too.
    /// The team is the project's, or else the one the issue's name starts with ("APP-214" is team APP's).
    fn move_linear_issue(&self, task_id: &str, project_id: &str, issue: &TaskIssue, kind: &str) {
        let team = self
            .linear_team(project_id)
            .or_else(|| issue.key.split_once('-').map(|(team, _)| team.to_string()).filter(|team| !team.is_empty()));
        let (Some(key), Some(team)) = (self.linear_key(), team) else {
            return;
        };
        // A key that may only read is known after its first refusal: Linear is not asked again, and it is
        // said in a line, not as a failure, for nothing is broken.
        if self.linear_read_only() {
            self.emit(
                task_id,
                json!({ "type": "tracker", "text": "The issue stays as it is in Linear: the key kept here may only read." }),
            );
            return;
        }
        let issue_id = issue.id.as_str();
        match self.hosts.linear_move(&key, &team, issue_id, kind) {
            Ok(state) => self.emit(task_id, json!({ "type": "tracker", "text": format!("The issue is now “{state}” in Linear.") })),
            Err(error) if error.to_string().contains(crate::hosts::READ_ONLY_KEY) => {
                let _ = self.store().set_setting(LINEAR_READ_ONLY, Some("yes"));
                self.broadcast("integrations.changed", json!({}));
                self.emit(
                    task_id,
                    json!({
                        "type": "tracker",
                        "text": "The issue was not moved in Linear: the key kept here may only read. To have issues moved \
                                 by themselves, make a personal API key with write access (Linear, Settings, Security and \
                                 access) and connect it in Issues.",
                        "failed": true
                    }),
                );
            }
            Err(error) => self.emit(
                task_id,
                json!({ "type": "tracker", "text": format!("The issue could not be moved in Linear: {error:#}"), "failed": true }),
            ),
        }
    }

    /// Whether Linear has refused to change anything with the key kept here.
    pub(crate) fn linear_read_only(&self) -> bool {
        self.store().setting(LINEAR_READ_ONLY).ok().flatten().as_deref() == Some("yes")
    }

    // ----- merge requests -----

    /// The working copy of a task, its branch and where its repository lives.
    /// The number of a task's merge request, when it was read not long ago. Nobody is asked for it here.
    pub(crate) fn request_number(&self, task_id: &str) -> Option<u64> {
        let known = self.remembered.requests.lock().unwrap();
        known.get(task_id).and_then(|(_, request)| request.as_ref()).map(|request| request.number)
    }

    pub(crate) fn request_place(&self, task_id: &str) -> Result<(TaskRow, PathBuf, String, Remote)> {
        let task = self.store().task(task_id)?;
        let Some(branch) = task.branch.clone() else {
            bail!("This task works in a folder without git, so it has no branch to merge");
        };
        let root = PathBuf::from(&task.cwd);
        let Some(remote) = remote_of(&root) else {
            bail!("This project's remote is not on GitHub or GitLab, so there is nowhere to open a merge request");
        };
        Ok((task, root, branch, remote))
    }

    /// Keeps what a host said about a task's request. `asked` is when the host
    /// was asked: commands run side by side, and an answer that was asked for
    /// before something newer became known (the request was just opened, say)
    /// must not be put on top of it.
    pub(crate) fn remember_request(&self, task_id: &str, request: Option<Request>, asked: Instant) {
        let mut known = self.remembered.requests.lock().unwrap();
        if known.get(task_id).is_some_and(|(at, _)| *at > asked) {
            return;
        }
        known.insert(task_id.to_string(), (Instant::now(), request.clone()));
        drop(known);
        // The user may want to be told that the checks failed or that the request can merge now.
        self.request_learned(task_id, request.as_ref());
    }

    /// The title and description a new request starts with: written from the commits and the issue.
    fn request_defaults(&self, task: &TaskRow, root: &Path) -> (String, String) {
        let commits = gitops::state(root, task.base_branch.as_deref()).map(|state| state.commits).unwrap_or_default();
        let title = match commits.as_slice() {
            [only] => only.subject.clone(),
            _ => task.issue.as_ref().map(|issue| issue.title.clone()).unwrap_or_else(|| task.title.clone()),
        };
        let mut body = String::new();
        if commits.len() > 1 {
            // Oldest first: the order the work was done in.
            for commit in commits.iter().rev() {
                body.push_str(&format!("- {}\n", commit.subject));
            }
        }
        if let Some(issue) = &task.issue {
            if !body.is_empty() {
                body.push('\n');
            }
            // Sentry resolves an issue when a commit that says "Fixes" with its name reaches it.
            let word = if issue.source == "sentry" { "Fixes" } else { "Closes" };
            body.push_str(&format!("{word} {}.\n", issue.key));
        }
        (title, body)
    }

    /// Everything the merge request card shows for a task.
    pub fn request_of(&self, task_id: &str, fresh: bool) -> Result<Value> {
        let task = self.store().task(task_id)?;
        let root = PathBuf::from(&task.cwd);
        let (Some(branch), Some(remote)) = (task.branch.clone(), remote_of(&root)) else {
            return Ok(json!({ "host": Value::Null, "request": Value::Null }));
        };
        let cached = self.remembered.requests.lock().unwrap().get(task_id).cloned();
        let request = match cached {
            Some((at, request)) if !fresh && at.elapsed() < REQUEST_TTL => request,
            _ => {
                let asked = Instant::now();
                let request =
                    self.hosts.request(&remote, &branch).map_err(|error| crate::hosts::said_plainly(&remote, error))?;
                self.remember_request(task_id, request.clone(), asked);
                // What is known may be newer than this answer: that is what is shown.
                self.remembered.requests.lock().unwrap().get(task_id).map_or(request, |(_, known)| known.clone())
            }
        };
        let (title, body) = self.request_defaults(&task, &root);
        Ok(json!({
            "host": { "kind": remote.kind, "name": remote.kind.name(), "word": remote.kind.request_word(), "slug": remote.slug },
            "request": request,
            "ready": request.as_ref().is_some_and(Request::ready),
            // Set when the user asked for the request to merge by itself once everything is green.
            "auto": self.auto_merge(task_id),
            "defaults": { "title": title, "body": body, "base": gitops::resolve_base(&root, task.base_branch.as_deref()) },
        }))
    }

    /// Pushes the branch and opens a request to merge it into the branch the task started from.
    pub fn open_request(&self, task_id: &str, title: &str, body: &str) -> Result<Request> {
        let (task, root, branch, remote) = self.request_place(task_id)?;
        if title.trim().is_empty() {
            bail!("Give the {} a title", remote.kind.request_word());
        }
        let Some(base) = gitops::resolve_base(&root, task.base_branch.as_deref()) else {
            bail!("It is not known which branch this task started from");
        };
        if gitops::state(&root, Some(&base))?.commits.is_empty() {
            bail!("There are no commits on this branch yet. Commit something first.");
        }
        gitops::push(&root)?;
        let request = self.hosts.create_request(&remote, &branch, &base, title.trim(), body)?;
        self.remember_request(task_id, Some(request.clone()), Instant::now());
        self.emit(
            task_id,
            json!({ "type": "git", "action": "opened", "subject": format!("{} {}", remote.kind.request_word(), request.number), "short": "" }),
        );
        self.broadcast("git.changed", json!({ "taskId": task_id }));
        self.broadcast("mr.changed", json!({ "taskId": task_id }));
        Ok(request)
    }

    /// Merges a task's request on the host. Cleaning up afterwards is `clean_up`.
    pub fn merge_request(&self, task_id: &str, squash: bool, delete_branch: bool) -> Result<Request> {
        let (task, _, branch, remote) = self.request_place(task_id)?;
        let word = remote.kind.request_word();
        let Some(request) = self.hosts.request(&remote, &branch)? else {
            bail!("This task has no {word} yet");
        };
        match request.state {
            "merged" => bail!("This {word} is already merged"),
            "closed" => bail!("This {word} was closed without merging"),
            _ => {}
        }
        if request.mergeable == Some(false) {
            bail!("This {word} conflicts with its target. Update the branch first.");
        }
        self.hosts.merge_request(&remote, request.number, squash, delete_branch)?;
        let merged = Request { state: "merged", ..request };
        self.remember_request(task_id, Some(merged.clone()), Instant::now());
        self.emit(
            task_id,
            json!({ "type": "git", "action": "merged", "subject": format!("{word} {}", merged.number), "short": "" }),
        );
        if let Some(issue) = task.issue.as_ref().filter(|issue| issue.source == "linear") {
            self.move_linear_issue(task_id, &task.project_id, issue, "completed");
        }
        self.broadcast("mr.changed", json!({ "taskId": task_id }));
        Ok(merged)
    }

    /// The merge request of every open task that has pushed its branch. Hosts are asked
    /// side by side, and only for tasks whose state was not read a moment ago.
    /// What reviewers wrote about lines of a task's request and nobody has settled yet.
    pub fn request_comments(&self, task_id: &str) -> Result<Value> {
        let (_, _, branch, remote) = self.request_place(task_id)?;
        let Some(request) = self.hosts.request(&remote, &branch)? else {
            return Ok(json!({ "comments": [] }));
        };
        Ok(json!({ "comments": self.hosts.comments(&remote, request.number)? }))
    }

    /// Answers a reviewer's comment in its thread, in the user's name.
    pub fn reply_to_comment(&self, task_id: &str, comment_id: &str, body: &str) -> Result<()> {
        if body.trim().is_empty() {
            bail!("The reply is empty");
        }
        let (_, _, branch, remote) = self.request_place(task_id)?;
        let word = remote.kind.request_word();
        let Some(request) = self.hosts.request(&remote, &branch)? else {
            bail!("This task has no {word} yet");
        };
        self.hosts.reply(&remote, request.number, comment_id, body.trim())
    }

    /// How a task's request is to be merged by itself, if the user asked for that.
    pub fn auto_merge(&self, task_id: &str) -> Option<Value> {
        let text = self.store().setting(&auto_merge_key(task_id)).ok().flatten()?;
        serde_json::from_str(&text).ok()
    }

    /// Turns "merge by itself when everything is green" on, with what to do
    /// at the merge (`options`), or off (`None`).
    pub fn set_auto_merge(&self, task_id: &str, options: Option<Value>) -> Result<()> {
        let (task, _, _, remote) = self.request_place(task_id)?;
        let word = remote.kind.request_word();
        let was = self.auto_merge(task_id).is_some();
        self.store().set_setting(&auto_merge_key(task_id), options.as_ref().map(Value::to_string).as_deref())?;
        if options.is_some() != was {
            let text = if options.is_some() {
                format!("The {word} will merge by itself when everything is green")
            } else {
                format!("The {word} will not merge by itself any more")
            };
            self.emit(&task.id, json!({ "type": "tracker", "text": text, "failed": false }));
        }
        self.broadcast("mr.changed", json!({ "taskId": task_id }));
        Ok(())
    }

    /// The tasks whose request was asked to merge by itself and can merge now,
    /// each with what to do at the merge. A request that can never merge this
    /// way any more (it was merged or closed elsewhere, or its checks failed)
    /// is taken off the list, and the task is told why.
    pub fn auto_merge_due(&self) -> Vec<(String, Value)> {
        let mut due = Vec::new();
        for task in self.store().tasks().unwrap_or_default() {
            let Some(options) = self.auto_merge(&task.id) else { continue };
            let root = PathBuf::from(&task.cwd);
            let (Some(branch), Some(remote)) = (task.branch.clone(), remote_of(&root)) else { continue };
            // A host that cannot be reached now is asked again in the next round.
            let asked = Instant::now();
            let Ok(request) = self.hosts.request(&remote, &branch) else { continue };
            self.remember_request(&task.id, request.clone(), asked);
            let word = remote.kind.request_word();
            let stop = |why: String| {
                let _ = self.store().set_setting(&auto_merge_key(&task.id), None);
                self.emit(&task.id, json!({ "type": "tracker", "text": why, "failed": true }));
                self.broadcast("mr.changed", json!({ "taskId": task.id }));
            };
            match request {
                None => stop(format!("The {word} is gone, so it will not merge by itself")),
                Some(request) if request.state != "open" => {
                    // Merged or closed by someone else: there is nothing left to wait for.
                    let _ = self.store().set_setting(&auto_merge_key(&task.id), None);
                    self.broadcast("mr.changed", json!({ "taskId": task.id }));
                }
                Some(request) if request.checks.state == "failed" => {
                    stop(format!("The checks of the {word} failed, so it will not merge by itself"))
                }
                Some(request) if request.mergeable == Some(false) => {
                    stop(format!("The {word} conflicts with its target, so it will not merge by itself"))
                }
                Some(request) if request.ready() => due.push((task.id.clone(), options)),
                Some(_) => {}
            }
        }
        due
    }

    /// Looks again at every request that was open when last seen, and returns
    /// the tasks whose request is merged now. Used to notice a merge that was
    /// made on the host, not from here.
    pub fn merged_since_last_look(&self) -> Vec<String> {
        let mut merged = Vec::new();
        for task in self.store().tasks().unwrap_or_default() {
            let known = self.remembered.requests.lock().unwrap().get(&task.id).cloned();
            let Some((_, Some(last))) = known else { continue };
            if last.state == "merged" {
                merged.push(task.id);
                continue;
            }
            if last.state != "open" {
                continue;
            }
            let root = PathBuf::from(&task.cwd);
            let (Some(branch), Some(remote)) = (task.branch.clone(), remote_of(&root)) else { continue };
            let asked = Instant::now();
            let Ok(request) = self.hosts.request(&remote, &branch) else { continue };
            let is_merged = request.as_ref().is_some_and(|request| request.state == "merged");
            self.remember_request(&task.id, request, asked);
            if is_merged {
                self.broadcast("mr.changed", json!({ "taskId": task.id }));
                merged.push(task.id);
            }
        }
        merged
    }

    /// Called when a request that was due did not merge after all.
    pub fn auto_merge_failed(&self, task_id: &str, why: &str) {
        let _ = self.store().set_setting(&auto_merge_key(task_id), None);
        if self.store().task(task_id).is_ok() {
            self.emit(
                task_id,
                json!({ "type": "tracker", "text": format!("It could not merge by itself: {why}"), "failed": true }),
            );
            self.broadcast("mr.changed", json!({ "taskId": task_id }));
        }
    }

    /// Called when a request merged by itself: nothing is left to wait for.
    pub fn auto_merge_done(&self, task_id: &str) {
        let _ = self.store().set_setting(&auto_merge_key(task_id), None);
    }

    pub fn requests_overview(self: &Arc<Self>) -> Result<Vec<Value>> {
        let mut wanted: Vec<(TaskRow, String, Remote)> = Vec::new();
        for task in self.store().tasks()? {
            let Some(branch) = task.branch.clone() else { continue };
            let root = PathBuf::from(&task.cwd);
            // A branch that was never pushed cannot have a request; do not ask the host about it.
            if !gitops::is_pushed(&root, &branch) {
                continue;
            }
            if let Some(remote) = remote_of(&root) {
                wanted.push((task, branch, remote));
            }
        }

        let mut overview = Vec::new();
        for group in wanted.chunks(AT_ONCE) {
            let results: Vec<(String, Option<Request>)> = std::thread::scope(|scope| {
                let handles: Vec<_> = group
                    .iter()
                    .map(|(task, branch, remote)| {
                        scope.spawn(move || {
                            let cached = self.remembered.requests.lock().unwrap().get(&task.id).cloned();
                            let request = match cached {
                                Some((at, request)) if at.elapsed() < REQUEST_TTL => request,
                                _ => {
                                    let asked = Instant::now();
                                    let request = self.hosts.request(remote, branch).unwrap_or(None);
                                    self.remember_request(&task.id, request.clone(), asked);
                                    // Something newer may have become known meanwhile: that is what is told.
                                    let known = self.remembered.requests.lock().unwrap().get(&task.id).cloned();
                                    known.map_or(request, |(_, known)| known)
                                }
                            };
                            (task.id.clone(), request)
                        })
                    })
                    .collect();
                handles.into_iter().filter_map(|handle| handle.join().ok()).collect()
            });
            for (task_id, request) in results {
                if let Some(request) = request {
                    overview.push(json!({ "taskId": task_id, "ready": request.ready(), "request": request }));
                }
            }
        }
        Ok(overview)
    }

    /// Finishes with a task: its agent is stopped, and as asked its working copy and
    /// branch are removed and the task leaves the lists.
    pub async fn clean_up(&self, task_id: &str, delete_copy: bool, archive: bool) -> Result<()> {
        let task = self.store().task(task_id)?;
        self.stop_agent(task_id).await?;
        if delete_copy && let Some(branch) = &task.branch {
            let project = self.store().project(&task.project_id)?;
            // Only a working copy the core made itself is removed, never the project's own folder.
            let inside = Path::new(&task.cwd).starts_with(self.data_root().join("worktrees"));
            if inside {
                crate::git::remove_worktree(Path::new(&project.path), Path::new(&task.cwd), branch).await;
            }
            // The task's checkpoints go with its working copy: nothing is left to rewind.
            self.drop_checkpoints(Path::new(&project.path), task_id);
        }
        if archive {
            self.archive(task_id).await?;
        }
        self.remembered.requests.lock().unwrap().remove(task_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_issue_becomes_the_first_message_of_its_task() {
        let mut issue = Issue {
            key: "#12".into(),
            id: "12".into(),
            title: "Login allows unlimited attempts".into(),
            url: "https://github.com/x/shop/issues/12".into(),
            state: "open".into(),
            body: "  Five a minute is enough.\n".into(),
            labels: vec![],
            assignee: None,
            updated_at: 0,
            links: vec![],
        };
        assert_eq!(
            first_message(&issue),
            "Login allows unlimited attempts\n\nFive a minute is enough.\n\nThis is issue #12 (https://github.com/x/shop/issues/12)."
        );
        issue.body.clear();
        assert_eq!(
            first_message(&issue),
            "Login allows unlimited attempts\n\nThis is issue #12 (https://github.com/x/shop/issues/12)."
        );
    }

    #[test]
    fn what_is_attached_to_an_issue_goes_to_the_agent_even_when_the_issue_says_nothing_else() {
        // An issue Sentry made: no text, only the way to the error.
        let issue = Issue {
            key: "APP-7".into(),
            id: "uuid-7".into(),
            title: "Error: No error message".into(),
            url: "https://linear.app/x/issue/APP-7".into(),
            state: "Todo".into(),
            body: String::new(),
            labels: vec![],
            assignee: None,
            updated_at: 0,
            links: vec![
                crate::hosts::IssueLink {
                    title: "Error: No error message".into(),
                    url: "https://sentry.io/organizations/x/issues/1/".into(),
                },
                crate::hosts::IssueLink { title: " ".into(), url: "https://example.com/log".into() },
            ],
        };
        assert_eq!(
            first_message(&issue),
            "Error: No error message\n\nAttached to the issue:\n\
             - Error: No error message: https://sentry.io/organizations/x/issues/1/\n\
             - https://example.com/log\n\n\
             This is issue APP-7 (https://linear.app/x/issue/APP-7)."
        );
    }
}
