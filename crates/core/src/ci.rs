//! The pipeline of a task's branch: what runs for it on GitLab or GitHub, job
//! by job, and the few things a person does to it without leaving the editor:
//! run a failed job again, start a manual one, run or cancel the whole thing,
//! start a new one.
//!
//! GitLab has one pipeline for a commit, made of jobs in stages. GitHub has
//! one run for each workflow, made of jobs. Both are shown the same way here:
//! a pipeline of runs, each with its jobs. GitLab's pipeline is one run; its
//! jobs say which stage they are in.
//!
//! Everything goes through the hosts' own programs, as the rest does: `glab
//! api` for GitLab's REST API, `gh run` for GitHub. No token is kept here.

use crate::hosts::{HostKind, Hosts, Remote};
use crate::tasks::Manager;
use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;
use serde_json::{Value, json};
use std::sync::Arc;

/// How many pages of a hundred jobs are read of one pipeline. Past that it is cut: a thousand jobs is a pipeline nobody reads by hand.
const MOST_JOB_PAGES: usize = 10;

/// Where a job, a run or a whole pipeline stands. One of: "passed", "failed",
/// "running", "pending", "manual" (waits for someone to start it),
/// "canceled", "skipped".
pub type State = &'static str;

/// One job of a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub id: String,
    pub name: String,
    /// GitLab's stage; empty on GitHub, where the run is the group.
    pub stage: String,
    pub state: State,
    pub url: String,
    /// True for a job whose failure does not fail the pipeline.
    pub allowed_to_fail: bool,
}

/// A GitLab pipeline, or one workflow run on GitHub.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub id: String,
    /// The workflow's name on GitHub; "Pipeline #123" on GitLab.
    pub name: String,
    pub state: State,
    pub url: String,
    pub jobs: Vec<Job>,
}

/// Everything that runs for the newest commit of a branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Pipeline {
    pub state: State,
    /// The commit it runs for, shortened.
    pub commit: String,
    pub runs: Vec<Run>,
}

/// GitLab's word for where a job or a pipeline stands, as ours.
fn gitlab_state(status: &str) -> State {
    match status {
        "success" => "passed",
        "failed" => "failed",
        "running" => "running",
        "manual" | "blocked" => "manual",
        "canceled" | "canceling" => "canceled",
        "skipped" => "skipped",
        // created, pending, preparing, scheduled, waiting_for_resource and whatever comes next.
        _ => "pending",
    }
}

/// GitHub's two words (how far it is, and how it ended) as ours.
fn github_state(status: &str, conclusion: &str) -> State {
    match (status.to_ascii_lowercase().as_str(), conclusion.to_ascii_lowercase().as_str()) {
        ("completed", "success") => "passed",
        ("completed", "cancelled") => "canceled",
        ("completed", "skipped" | "neutral") => "skipped",
        ("completed", _) => "failed",
        ("in_progress", _) => "running",
        _ => "pending",
    }
}

/// How a pipeline of several runs stands: by its worst run.
fn worst(states: impl Iterator<Item = State>) -> State {
    let rank = |state: State| match state {
        "failed" => 6,
        "running" => 5,
        "pending" => 4,
        "manual" => 3,
        "canceled" => 2,
        "passed" => 1,
        _ => 0,
    };
    states.max_by_key(|state| rank(state)).unwrap_or("skipped")
}

fn text(item: &Value, name: &str) -> String {
    item.get(name).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn number(item: &Value, name: &str) -> String {
    item.get(name).and_then(Value::as_u64).map(|n| n.to_string()).unwrap_or_default()
}

fn short(sha: &str) -> String {
    sha.chars().take(8).collect()
}

/// A pipeline as GitLab's API gives it, with its jobs as a second answer.
/// Jobs come newest first; here they stand in the order they were made, which is the order of the stages.
pub fn parse_gitlab_pipeline(pipeline: &str, jobs: &str) -> Result<Pipeline> {
    let item: Value = serde_json::from_str(pipeline).context("GitLab's answer about the pipeline could not be read")?;
    let list: Vec<Value> = serde_json::from_str(jobs).context("GitLab's answer about the jobs could not be read")?;
    let mut jobs: Vec<(u64, Job)> = list
        .iter()
        .map(|job| {
            let id = job.get("id").and_then(Value::as_u64).unwrap_or(0);
            (
                id,
                Job {
                    id: id.to_string(),
                    name: text(job, "name"),
                    stage: text(job, "stage"),
                    state: gitlab_state(&text(job, "status")),
                    url: text(job, "web_url"),
                    allowed_to_fail: job.get("allow_failure").and_then(Value::as_bool).unwrap_or(false),
                },
            )
        })
        .collect();
    jobs.sort_by_key(|(id, _)| *id);
    let id = number(&item, "id");
    let state = gitlab_state(&text(&item, "status"));
    Ok(Pipeline {
        state,
        commit: short(&text(&item, "sha")),
        runs: vec![Run {
            name: format!("Pipeline #{id}"),
            id,
            state,
            url: text(&item, "web_url"),
            jobs: jobs.into_iter().map(|(_, job)| job).collect(),
        }],
    })
}

/// The runs of a branch as `gh run list --json …` prints them, newest first: the commit they are
/// for, and the runs. Only the ones for the newest commit are a pipeline; older ones are history.
pub fn parse_github_runs(json: &str) -> Result<(String, Vec<Run>)> {
    let list: Vec<Value> = serde_json::from_str(json).context("GitHub's answer about the runs could not be read")?;
    let newest = list.first().map(|run| text(run, "headSha")).unwrap_or_default();
    let mut runs: Vec<Run> = Vec::new();
    for run in list.iter().filter(|run| text(run, "headSha") == newest) {
        let workflow = text(run, "workflowName");
        // A workflow that was run again is listed twice: only its newest run counts.
        if runs.iter().any(|known| known.name == workflow) {
            continue;
        }
        runs.push(Run {
            id: number(run, "databaseId"),
            name: workflow,
            state: github_state(&text(run, "status"), &text(run, "conclusion")),
            url: text(run, "url"),
            jobs: vec![],
        });
    }
    Ok((short(&newest), runs))
}

/// The jobs of one run as `gh run view --json jobs` prints them.
pub fn parse_github_jobs(json: &str) -> Result<Vec<Job>> {
    let item: Value = serde_json::from_str(json).context("GitHub's answer about the jobs could not be read")?;
    Ok(item
        .get("jobs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|job| Job {
            id: number(job, "databaseId"),
            name: text(job, "name"),
            stage: String::new(),
            state: github_state(&text(job, "status"), &text(job, "conclusion")),
            url: text(job, "url"),
            allowed_to_fail: false,
        })
        .collect())
}

/// A piece of an address, with everything that would mean something else made plain: a
/// project is named `group/project` and a branch `fix/x`, and both stand inside a path.
fn in_path(piece: &str) -> String {
    let mut out = String::with_capacity(piece.len());
    for byte in piece.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// An id the interface sent back: digits only, so it can stand in an address or on a command line as it is.
fn id_of(given: &str) -> Result<&str> {
    if given.is_empty() || given.len() > 20 || !given.bytes().all(|byte| byte.is_ascii_digit()) {
        bail!("That is not the number of a job or a run");
    }
    Ok(given)
}

impl Hosts {
    fn gitlab(&self, remote: &Remote, method: &str, path: &str) -> Result<String> {
        let endpoint = format!("projects/{}/{path}", in_path(&remote.slug));
        let mut args = vec!["api"];
        if method != "GET" {
            args.extend(["--method", method]);
        }
        args.push(&endpoint);
        self.run_at(HostKind::GitLab, Some(&remote.host), &args)
    }

    /// What runs for the newest commit of a branch; `None` when nothing ever ran for it. With the
    /// number of the branch's merge request, GitLab is asked for that request's pipelines first:
    /// a project may run its pipelines for requests only, and those are not listed under the branch.
    pub fn pipeline(&self, remote: &Remote, branch: &str, request: Option<u64>) -> Result<Option<Pipeline>> {
        match remote.kind {
            HostKind::GitLab => {
                let newest = |listed: String| -> Result<Option<u64>> {
                    let list: Vec<Value> = serde_json::from_str(&listed)
                        .context("GitLab's answer about the pipelines could not be read")?;
                    Ok(list.first().and_then(|pipeline| pipeline.get("id")).and_then(Value::as_u64))
                };
                let of_request = match request {
                    Some(number) => {
                        newest(self.gitlab(remote, "GET", &format!("merge_requests/{number}/pipelines?per_page=1"))?)?
                    }
                    None => None,
                };
                let id = match of_request {
                    Some(id) => Some(id),
                    None => {
                        newest(self.gitlab(remote, "GET", &format!("pipelines?ref={}&per_page=1", in_path(branch)))?)?
                    }
                };
                let Some(id) = id else { return Ok(None) };
                let pipeline = self.gitlab(remote, "GET", &format!("pipelines/{id}"))?;
                // GitLab hands the jobs out a hundred at a time, newest first. All of them are asked for:
                // the oldest are the first stages, and a pipeline cut short would hide what failed there.
                let mut jobs: Vec<Value> = Vec::new();
                for page in 1..=MOST_JOB_PAGES {
                    let listed =
                        self.gitlab(remote, "GET", &format!("pipelines/{id}/jobs?per_page=100&page={page}"))?;
                    let more: Vec<Value> =
                        serde_json::from_str(&listed).context("GitLab's answer about the jobs could not be read")?;
                    let full = more.len() >= 100;
                    jobs.extend(more);
                    if !full {
                        break;
                    }
                }
                parse_gitlab_pipeline(&pipeline, &Value::Array(jobs).to_string()).map(Some)
            }
            HostKind::GitHub => {
                let fields = "databaseId,status,conclusion,workflowName,url,headSha";
                let listed = self.run_at(
                    HostKind::GitHub,
                    Some(&remote.host),
                    &["run", "list", "--branch", branch, "-R", &remote.slug, "--limit", "20", "--json", fields],
                )?;
                let (commit, found) = parse_github_runs(&listed)?;
                if found.is_empty() {
                    return Ok(None);
                }
                let mut runs = Vec::new();
                for mut run in found {
                    let jobs = self.run_at(
                        HostKind::GitHub,
                        Some(&remote.host),
                        &["run", "view", &run.id, "-R", &remote.slug, "--json", "jobs"],
                    )?;
                    run.jobs = parse_github_jobs(&jobs)?;
                    runs.push(run);
                }
                Ok(Some(Pipeline { state: worst(runs.iter().map(|run| run.state)), commit, runs }))
            }
        }
    }

    /// Does one thing to a job, a run, or the branch. `what` is one of "retryJob", "playJob",
    /// "cancelJob", "retryRun", "retryFailed", "cancelRun", "runNew"; `id` is the job's or the run's.
    pub fn pipeline_act(&self, remote: &Remote, branch: &str, what: &str, id: &str) -> Result<()> {
        let github = |args: &[&str]| {
            let mut all = args.to_vec();
            all.extend(["-R", remote.slug.as_str()]);
            self.run_at(HostKind::GitHub, Some(&remote.host), &all).map(|_| ())
        };
        match (remote.kind, what) {
            (HostKind::GitLab, "retryJob") => {
                self.gitlab(remote, "POST", &format!("jobs/{}/retry", id_of(id)?)).map(|_| ())
            }
            (HostKind::GitLab, "playJob") => {
                self.gitlab(remote, "POST", &format!("jobs/{}/play", id_of(id)?)).map(|_| ())
            }
            (HostKind::GitLab, "cancelJob") => {
                self.gitlab(remote, "POST", &format!("jobs/{}/cancel", id_of(id)?)).map(|_| ())
            }
            // GitLab runs again what failed or was canceled; the jobs that passed are kept.
            (HostKind::GitLab, "retryRun" | "retryFailed") => {
                self.gitlab(remote, "POST", &format!("pipelines/{}/retry", id_of(id)?)).map(|_| ())
            }
            (HostKind::GitLab, "cancelRun") => {
                self.gitlab(remote, "POST", &format!("pipelines/{}/cancel", id_of(id)?)).map(|_| ())
            }
            (HostKind::GitLab, "runNew") => {
                self.gitlab(remote, "POST", &format!("pipeline?ref={}", in_path(branch))).map(|_| ())
            }
            (HostKind::GitHub, "retryJob") => github(&["run", "rerun", "--job", id_of(id)?]),
            (HostKind::GitHub, "retryRun") => github(&["run", "rerun", id_of(id)?]),
            (HostKind::GitHub, "retryFailed") => github(&["run", "rerun", id_of(id)?, "--failed"]),
            (HostKind::GitHub, "cancelRun") => github(&["run", "cancel", id_of(id)?]),
            (HostKind::GitHub, "playJob" | "cancelJob" | "runNew") => {
                bail!("GitHub has no way to do that from here: open the run on GitHub")
            }
            (_, other) => bail!("Unknown thing to do with a pipeline: {other}"),
        }
    }
}

impl Manager {
    /// The pipeline of a task's branch, with what can be done to it on that host.
    pub fn pipeline_of(&self, task_id: &str) -> Result<Value> {
        let (_, _, branch, remote) = self.request_place(task_id)?;
        let pipeline = self.hosts.pipeline(&remote, &branch, self.request_number(task_id))?;
        Ok(json!({
            "host": remote.kind.name(),
            "branch": branch,
            "pipeline": pipeline,
            // GitLab starts a pipeline for a branch on request, and has jobs that wait to be started
            // and jobs that can be stopped one by one. On GitHub a run starts from what its workflow listens to.
            "can": {
                "runNew": remote.kind == HostKind::GitLab,
                "playJob": remote.kind == HostKind::GitLab,
                "cancelJob": remote.kind == HostKind::GitLab,
                "retryFailed": true
            }
        }))
    }

    /// Does one thing to the pipeline of a task's branch and says so in the conversation.
    pub fn pipeline_act(&self, task_id: &str, what: &str, id: &str, name: &str) -> Result<()> {
        let (_, _, branch, remote) = self.request_place(task_id)?;
        self.hosts.pipeline_act(&remote, &branch, what, id)?;
        let name: String = name.chars().filter(|letter| !letter.is_control()).take(80).collect();
        let host = remote.kind.name();
        let said = match what {
            "retryJob" => format!("The job “{name}” was started again on {host}."),
            "playJob" => format!("The job “{name}” was started on {host}."),
            "cancelJob" => format!("The job “{name}” was stopped on {host}."),
            "retryRun" => format!("“{name}” was started again on {host}."),
            "retryFailed" => format!("The failed jobs of “{name}” were started again on {host}."),
            "cancelRun" => format!("“{name}” was stopped on {host}."),
            _ => format!("A new pipeline was started for {branch} on {host}."),
        };
        self.emit(task_id, json!({ "type": "tracker", "text": said }));
        Ok(())
    }
}

/// The commands of the interface that are about pipelines.
pub async fn command(manager: &Arc<Manager>, method: &str, params: &Value) -> Result<Value> {
    let text =
        |name: &str| params.get(name).and_then(Value::as_str).ok_or_else(|| anyhow!("Missing parameter: {name}"));
    let task_id = text("taskId")?.to_string();
    let manager = manager.clone();
    match method {
        "ci.get" => tokio::task::spawn_blocking(move || manager.pipeline_of(&task_id)).await?,
        "ci.act" => {
            let (what, id) = (text("what")?.to_string(), text("id").unwrap_or_default().to_string());
            let name = text("name").unwrap_or_default().to_string();
            tokio::task::spawn_blocking(move || {
                manager.pipeline_act(&task_id, &what, &id, &name)?;
                // What it looks like right after: the host needs a moment, so this may still show the old state.
                manager.pipeline_of(&task_id)
            })
            .await?
        }
        other => bail!("Unknown method: {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gitlab_pipeline_is_one_run_with_its_jobs_in_the_order_of_the_stages() {
        let pipeline = r#"{"id":991,"status":"failed","sha":"0123456789abcdef","web_url":"https://gitlab.example.com/g/p/-/pipelines/991"}"#;
        // Newest first, as GitLab answers.
        let jobs = r#"[
            {"id":5003,"name":"deploy","stage":"deploy","status":"manual","web_url":"u3","allow_failure":false},
            {"id":5002,"name":"unit","stage":"test","status":"failed","web_url":"u2","allow_failure":false},
            {"id":5001,"name":"lint","stage":"test","status":"success","web_url":"u1","allow_failure":true}
        ]"#;
        let read = parse_gitlab_pipeline(pipeline, jobs).unwrap();
        assert_eq!((read.state, read.commit.as_str()), ("failed", "01234567"));
        assert_eq!(read.runs.len(), 1);
        let run = &read.runs[0];
        assert_eq!((run.id.as_str(), run.name.as_str(), run.state), ("991", "Pipeline #991", "failed"));
        let jobs: Vec<(&str, &str, State)> =
            run.jobs.iter().map(|j| (j.name.as_str(), j.stage.as_str(), j.state)).collect();
        assert_eq!(jobs, [("lint", "test", "passed"), ("unit", "test", "failed"), ("deploy", "deploy", "manual")]);
        assert!(run.jobs[0].allowed_to_fail && !run.jobs[1].allowed_to_fail);
    }

    #[test]
    fn gitlabs_words_and_githubs_words_become_the_same_few() {
        for (theirs, ours) in [
            ("success", "passed"),
            ("failed", "failed"),
            ("running", "running"),
            ("created", "pending"),
            ("waiting_for_resource", "pending"),
            ("manual", "manual"),
            ("canceled", "canceled"),
            ("skipped", "skipped"),
            ("a word of next year", "pending"),
        ] {
            assert_eq!(gitlab_state(theirs), ours, "{theirs}");
        }
        for ((status, conclusion), ours) in [
            (("completed", "success"), "passed"),
            (("completed", "failure"), "failed"),
            (("completed", "timed_out"), "failed"),
            (("completed", "cancelled"), "canceled"),
            (("completed", "skipped"), "skipped"),
            (("in_progress", ""), "running"),
            (("queued", ""), "pending"),
            // `gh` prints them in capitals in some of its answers.
            (("COMPLETED", "SUCCESS"), "passed"),
        ] {
            assert_eq!(github_state(status, conclusion), ours, "{status} {conclusion}");
        }
        assert_eq!(worst(["passed", "running", "failed"].into_iter()), "failed");
        assert_eq!(worst(["passed", "pending"].into_iter()), "pending");
        assert_eq!(worst(["passed", "skipped"].into_iter()), "passed");
        assert_eq!(worst(std::iter::empty()), "skipped");
    }

    #[test]
    fn githubs_runs_for_the_newest_commit_are_the_pipeline_each_workflow_once() {
        let listed = r#"[
            {"databaseId":30,"status":"completed","conclusion":"failure","workflowName":"CI","url":"r30","headSha":"aaaa1111bbbb"},
            {"databaseId":29,"status":"completed","conclusion":"success","workflowName":"Docs","url":"r29","headSha":"aaaa1111bbbb"},
            {"databaseId":28,"status":"completed","conclusion":"failure","workflowName":"CI","url":"r28","headSha":"aaaa1111bbbb"},
            {"databaseId":20,"status":"completed","conclusion":"success","workflowName":"CI","url":"r20","headSha":"0000oldcommit"}
        ]"#;
        let (commit, runs) = parse_github_runs(listed).unwrap();
        assert_eq!(commit, "aaaa1111");
        let names: Vec<(&str, &str, State)> =
            runs.iter().map(|run| (run.id.as_str(), run.name.as_str(), run.state)).collect();
        // The older try of CI for the same commit and the run for the older commit are history.
        assert_eq!(names, [("30", "CI", "failed"), ("29", "Docs", "passed")]);
        assert!(parse_github_runs("[]").unwrap().1.is_empty());

        let jobs = parse_github_jobs(
            r#"{"jobs":[{"databaseId":77,"name":"test (ubuntu)","status":"completed","conclusion":"failure","url":"j77"},
                        {"databaseId":78,"name":"lint","status":"in_progress","conclusion":"","url":"j78"}]}"#,
        )
        .unwrap();
        let read: Vec<(&str, &str, State)> = jobs.iter().map(|j| (j.id.as_str(), j.name.as_str(), j.state)).collect();
        assert_eq!(read, [("77", "test (ubuntu)", "failed"), ("78", "lint", "running")]);
    }

    #[test]
    fn a_project_and_a_branch_stand_in_an_address_as_one_piece_each() {
        assert_eq!(in_path("group/sub/project"), "group%2Fsub%2Fproject");
        assert_eq!(in_path("fix/app-12330 hide&seek"), "fix%2Fapp-12330%20hide%26seek");
        assert_eq!(in_path("plain-name_1.2~x"), "plain-name_1.2~x");
    }

    #[test]
    fn only_a_number_is_taken_for_a_job_or_a_run() {
        assert_eq!(id_of("5002").unwrap(), "5002");
        for bad in ["", "12/retry", "-1", "1 2", "../x", "123456789012345678901"] {
            assert!(id_of(bad).is_err(), "{bad}");
        }
    }
}
