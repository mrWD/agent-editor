//! Projects and tasks joined with the services they live on: which tracker a
//! project's issues come from, starting a task from an issue, and the merge
//! request of a task from opening it to cleaning up after the merge.
//!
//! The functions that ask a service block and use the network. Call them from
//! `spawn_blocking`.

use crate::gitops;
use crate::hosts::{HostKind, Issue, Remote, Request, remote_of};
use crate::store::{Project, TaskIssue, TaskRow};
use crate::tasks::{Manager, TaskInfo};
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
}

fn team_setting(project_id: &str) -> String {
    format!("project.{project_id}.linearTeam")
}

/// What an issue becomes as the first message of its task.
fn first_message(issue: &Issue) -> String {
    let body = issue.body.trim();
    if body.is_empty() {
        format!("{}\n\nThis is issue {} ({}).", issue.title, issue.key, issue.url)
    } else {
        format!("{}\n\n{}\n\nThis is issue {} ({}).", issue.title, body, issue.key, issue.url)
    }
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
    fn linear_key(&self) -> Option<String> {
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
        json!({
            "github": self.hosts.tool_state(HostKind::GitHub),
            "gitlab": self.hosts.tool_state(HostKind::GitLab),
            "linear": { "hasKey": self.linear_key().is_some() },
            "docker": self.hosts.docker_state(),
        })
    }

    /// Saves the Linear key, or forgets it. It is kept in the core's own database on this machine.
    pub fn set_linear_key(&self, key: Option<&str>) -> Result<()> {
        let key = key.map(str::trim).filter(|key| !key.is_empty());
        self.store().set_setting("linear.key", key)?;
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

    fn linear_team(&self, project_id: &str) -> Option<String> {
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
            Source::Host(remote) => self.hosts.issues(remote),
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

    /// Starts a task for an issue. The issue's text is the first message to the agent.
    pub async fn start_issue(self: &Arc<Self>, project_id: &str, key: &str, agent_id: &str) -> Result<TaskInfo> {
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
        let attached = TaskIssue {
            key: issue.key.clone(),
            id: issue.id.clone(),
            url: issue.url.clone(),
            title: issue.title.clone(),
            source: source.to_string(),
        };
        let task = self.create_for(project_id, agent_id, &first_message(&issue), Some(attached)).await?;

        // In Linear a started task means a started issue. A failure here must not undo the task.
        if source == "linear" {
            let manager = self.clone();
            let (project, issue_id, task_id) = (project_id.to_string(), issue.id.clone(), task.row.id.clone());
            tokio::task::spawn_blocking(move || manager.move_linear_issue(&task_id, &project, &issue_id, "started"));
        }
        Ok(task)
    }

    /// Moves a task's Linear issue and says so in the conversation. Failures are said there too.
    fn move_linear_issue(&self, task_id: &str, project_id: &str, issue_id: &str, kind: &str) {
        let (Some(key), Some(team)) = (self.linear_key(), self.linear_team(project_id)) else {
            return;
        };
        match self.hosts.linear_move(&key, &team, issue_id, kind) {
            Ok(state) => self.emit(task_id, json!({ "type": "tracker", "text": format!("The issue is now “{state}” in Linear.") })),
            Err(error) => self.emit(
                task_id,
                json!({ "type": "tracker", "text": format!("The issue could not be moved in Linear: {error:#}"), "failed": true }),
            ),
        }
    }

    // ----- merge requests -----

    /// The working copy of a task, its branch and where its repository lives.
    fn request_place(&self, task_id: &str) -> Result<(TaskRow, PathBuf, String, Remote)> {
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

    fn remember_request(&self, task_id: &str, request: Option<Request>) {
        self.remembered.requests.lock().unwrap().insert(task_id.to_string(), (Instant::now(), request));
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
            body.push_str(&format!("Closes {}.\n", issue.key));
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
                let request = self.hosts.request(&remote, &branch)?;
                self.remember_request(task_id, request.clone());
                request
            }
        };
        let (title, body) = self.request_defaults(&task, &root);
        Ok(json!({
            "host": { "kind": remote.kind, "name": remote.kind.name(), "word": remote.kind.request_word(), "slug": remote.slug },
            "request": request,
            "ready": request.as_ref().is_some_and(Request::ready),
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
        self.remember_request(task_id, Some(request.clone()));
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
        self.remember_request(task_id, Some(merged.clone()));
        self.emit(
            task_id,
            json!({ "type": "git", "action": "merged", "subject": format!("{word} {}", merged.number), "short": "" }),
        );
        if let Some(issue) = task.issue.as_ref().filter(|issue| issue.source == "linear") {
            self.move_linear_issue(task_id, &task.project_id, &issue.id, "completed");
        }
        self.broadcast("mr.changed", json!({ "taskId": task_id }));
        Ok(merged)
    }

    /// The merge request of every open task that has pushed its branch. Hosts are asked
    /// side by side, and only for tasks whose state was not read a moment ago.
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
                                    let request = self.hosts.request(remote, branch).unwrap_or(None);
                                    self.remember_request(&task.id, request.clone());
                                    request
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
}
