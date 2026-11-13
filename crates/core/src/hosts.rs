//! The services a project lives on: GitHub and GitLab for code, issues and
//! merge requests, Linear for issues.
//!
//! GitHub and GitLab are reached through their own command-line tools, `gh`
//! and `glab`. That way the core never holds their passwords or tokens: the
//! user signs in once with the tool, and the core asks the tool. Linear has no
//! such tool, so it is asked over its API with a key the user gives.
//!
//! Everything here blocks and may use the network. Call it from `spawn_blocking`.

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// How long a request to Linear may take.
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HostKind {
    GitHub,
    GitLab,
}

impl HostKind {
    pub fn name(self) -> &'static str {
        match self {
            HostKind::GitHub => "GitHub",
            HostKind::GitLab => "GitLab",
        }
    }
    /// What a request to merge a branch is called there.
    pub fn request_word(self) -> &'static str {
        match self {
            HostKind::GitHub => "pull request",
            HostKind::GitLab => "merge request",
        }
    }
}

/// Where a repository lives, read from the address of its remote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Remote {
    pub kind: HostKind,
    /// The server, such as `github.com`.
    pub host: String,
    /// The owner and the repository: `owner/name`, or `group/subgroup/name` on GitLab.
    pub slug: String,
}

/// What moving an issue fails with when the Linear key may only read.
pub const READ_ONLY_KEY: &str = "the Linear key kept here may only read";

/// Reads `git@github.com:owner/repo.git`, `https://gitlab.com/group/repo` and the like.
/// `None` for an address that is not on GitHub or GitLab, such as a folder on disk.
pub fn parse_remote(address: &str) -> Option<Remote> {
    let address = address.trim();
    let rest = address.strip_prefix("ssh://").unwrap_or(address);
    let (host, path) = if let Some(after) = rest.strip_prefix("https://").or_else(|| rest.strip_prefix("http://")) {
        let (host, path) = after.split_once('/')?;
        // A name and password before the host are not part of where it lives.
        (host.rsplit('@').next()?, path)
    } else if let Some((user_host, path)) =
        rest.split_once(':').filter(|(left, _)| left.contains('@') && !left.contains('/'))
    {
        (user_host.rsplit('@').next()?, path)
    } else if let Some((user_host, path)) = rest.split_once('/').filter(|(left, _)| left.contains('@')) {
        (user_host.rsplit('@').next()?, path)
    } else {
        return None;
    };
    let host = host.split(':').next()?.to_lowercase();
    let slug = path.trim_matches('/').trim_end_matches(".git").to_string();
    if slug.is_empty() || !slug.contains('/') {
        return None;
    }
    let kind = if host == "github.com" || host.starts_with("github.") {
        HostKind::GitHub
    } else if host.contains("gitlab") {
        HostKind::GitLab
    } else {
        return None;
    };
    Some(Remote { kind, host, slug })
}

/// Where the repository in `root` lives, by its `origin` remote (or its first remote).
/// The address is read as it is written in the configuration, before any rewriting rules.
pub fn remote_of(root: &Path) -> Option<Remote> {
    let ask = |args: &[&str]| -> Option<String> {
        let output = Command::new("git").arg("-C").arg(root).args(args).output().ok()?;
        output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
    };
    let address =
        ask(&["config", "--get", "remote.origin.url"]).filter(|address| !address.is_empty()).or_else(|| {
            // "remote.<name>.url <address>" for every remote; the first one will do.
            let all = ask(&["config", "--get-regexp", r"^remote\..*\.url$"])?;
            all.lines().next()?.split_once(' ').map(|(_, address)| address.to_string())
        })?;
    parse_remote(&address)
}

/// Something to do, from a tracker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Issue {
    /// How people call it: `#12` on GitHub and GitLab, `APP-214` in Linear.
    pub key: String,
    /// What the tracker's own commands need: the number, or Linear's id.
    pub id: String,
    pub title: String,
    pub url: String,
    /// The tracker's name for where it stands, such as "open" or "In progress".
    pub state: String,
    pub body: String,
    pub labels: Vec<String>,
    pub assignee: Option<String>,
    /// When it last changed, in milliseconds.
    pub updated_at: u64,
    /// What is attached to it in the tracker: a title and an address each. Linear
    /// keeps here what other services say about the issue, such as the error in
    /// Sentry it was made from; an issue may have nothing but that.
    #[serde(default)]
    pub links: Vec<IssueLink>,
}

/// Something attached to an issue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueLink {
    pub title: String,
    pub url: String,
}

/// How the automatic checks of a request stand.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Checks {
    /// "none", "pending", "passed" or "failed".
    pub state: &'static str,
    pub passed: usize,
    pub failed: usize,
    pub pending: usize,
}

/// A pull request or merge request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub number: u64,
    pub title: String,
    pub url: String,
    /// "open", "merged" or "closed".
    pub state: &'static str,
    pub draft: bool,
    /// False when it conflicts with its target; `None` when the host has not worked it out yet.
    pub mergeable: Option<bool>,
    pub checks: Checks,
    /// "approved", "changes_requested", "review_required" or "none".
    pub review: &'static str,
}

/// What a reviewer wrote about a line of a pull request or merge request, and nobody has settled yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    /// What the host needs to answer it.
    pub id: String,
    pub author: String,
    /// The file and line it is about, when it is about a line.
    pub path: Option<String>,
    pub line: Option<u64>,
    pub body: String,
    pub url: String,
}

/// The open review threads of a pull request, asked of GitHub in one question.
const GITHUB_THREADS: &str = "query($owner:String!,$name:String!,$number:Int!){repository(owner:$owner,name:$name){pullRequest(number:$number){reviewThreads(first:100){nodes{isResolved comments(first:1){nodes{databaseId author{login} path line originalLine body url}}}}}}}";

/// Review threads as GitHub's GraphQL answers them. Settled threads are left out.
pub fn parse_github_comments(json: &str) -> Result<Vec<Comment>> {
    let value: Value = serde_json::from_str(json).context("GitHub's answer is not JSON")?;
    let threads = value
        .pointer("/data/repository/pullRequest/reviewThreads/nodes")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("GitHub's answer has no review threads in it"))?;
    let mut comments = Vec::new();
    for thread in threads.iter().filter(|thread| thread["isResolved"] != true) {
        let first = &thread["comments"]["nodes"][0];
        let Some(id) = first["databaseId"].as_u64() else { continue };
        comments.push(Comment {
            id: id.to_string(),
            author: first["author"]["login"].as_str().unwrap_or("someone").to_string(),
            path: first["path"].as_str().map(str::to_string),
            line: first["line"].as_u64().or_else(|| first["originalLine"].as_u64()),
            body: first["body"].as_str().unwrap_or_default().trim().to_string(),
            url: first["url"].as_str().unwrap_or_default().to_string(),
        });
    }
    Ok(comments)
}

/// Discussions of a merge request as GitLab's API lists them. Settled ones and
/// GitLab's own notes ("changed the description") are left out.
pub fn parse_gitlab_comments(json: &str) -> Result<Vec<Comment>> {
    let value: Value = serde_json::from_str(json).context("GitLab's answer is not JSON")?;
    let discussions = value.as_array().ok_or_else(|| anyhow!("GitLab's answer is not a list of discussions"))?;
    let mut comments = Vec::new();
    for discussion in discussions {
        let first = &discussion["notes"][0];
        if first["system"] == true || first["resolvable"] != true || first["resolved"] == true {
            continue;
        }
        let Some(id) = discussion["id"].as_str() else { continue };
        comments.push(Comment {
            id: id.to_string(),
            author: first["author"]["username"].as_str().unwrap_or("someone").to_string(),
            path: first["position"]["new_path"].as_str().map(str::to_string),
            line: first["position"]["new_line"].as_u64().or_else(|| first["position"]["old_line"].as_u64()),
            body: first["body"].as_str().unwrap_or_default().trim().to_string(),
            url: String::new(),
        });
    }
    Ok(comments)
}

/// A GitLab project's path as its API wants it: `group/sub/name` with the slashes written out.
fn gitlab_project(slug: &str) -> String {
    slug.replace('/', "%2F")
}

impl Request {
    /// Open, no conflicts, checks green or absent, and nobody asked for changes.
    pub fn ready(&self) -> bool {
        self.state == "open"
            && !self.draft
            && self.mergeable != Some(false)
            && matches!(self.checks.state, "passed" | "none")
            && self.review != "changes_requested"
    }
}

/// Milliseconds since 1970 for a time like `2026-11-12T19:05:30Z`. Zero when it cannot be read.
pub(crate) fn parse_time(text: &str) -> u64 {
    let digits = |from: usize, len: usize| text.get(from..from + len).and_then(|part| part.parse::<i64>().ok());
    let (Some(year), Some(month), Some(day)) = (digits(0, 4), digits(5, 2), digits(8, 2)) else {
        return 0;
    };
    let (hour, minute, second) = (digits(11, 2).unwrap_or(0), digits(14, 2).unwrap_or(0), digits(17, 2).unwrap_or(0));
    // Days from 1970-01-01, by the usual civil-calendar arithmetic.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let year_of_era = y.rem_euclid(400);
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    let seconds = days * 86_400 + hour * 3600 + minute * 60 + second;
    u64::try_from(seconds).unwrap_or(0) * 1000
}

fn text_of(value: &Value, key: &str) -> String {
    value.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn names(value: &Value, key: &str, field: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                // GitHub gives objects with a name; GitLab gives plain words.
                .filter_map(|item| item.as_str().or_else(|| item.get(field).and_then(Value::as_str)))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Issues as `gh issue list --json …` prints them.
pub fn parse_github_issues(json: &str) -> Result<Vec<Issue>> {
    let list: Vec<Value> = serde_json::from_str(json).context("GitHub's answer could not be read")?;
    Ok(list
        .iter()
        .map(|item| {
            let number = item.get("number").and_then(Value::as_u64).unwrap_or(0);
            Issue {
                key: format!("#{number}"),
                id: number.to_string(),
                title: text_of(item, "title"),
                url: text_of(item, "url"),
                state: text_of(item, "state").to_lowercase(),
                body: text_of(item, "body"),
                labels: names(item, "labels", "name"),
                assignee: names(item, "assignees", "login").into_iter().next(),
                updated_at: parse_time(&text_of(item, "updatedAt")),
                links: vec![],
            }
        })
        .collect())
}

/// Issues as `glab issue list --output json` prints them.
pub fn parse_gitlab_issues(json: &str) -> Result<Vec<Issue>> {
    let list: Vec<Value> = serde_json::from_str(json).context("GitLab's answer could not be read")?;
    Ok(list
        .iter()
        .map(|item| {
            let number = item.get("iid").and_then(Value::as_u64).unwrap_or(0);
            let state = text_of(item, "state");
            Issue {
                key: format!("#{number}"),
                id: number.to_string(),
                title: text_of(item, "title"),
                url: text_of(item, "web_url"),
                state: if state == "opened" { "open".into() } else { state },
                body: text_of(item, "description"),
                labels: names(item, "labels", "name"),
                assignee: names(item, "assignees", "username").into_iter().next(),
                updated_at: parse_time(&text_of(item, "updated_at")),
                links: vec![],
            }
        })
        .collect())
}

/// A pull request as `gh pr view --json …` prints it.
pub fn parse_github_request(json: &str) -> Result<Request> {
    let item: Value = serde_json::from_str(json).context("GitHub's answer could not be read")?;
    let mut checks = Checks::default();
    for check in item.get("statusCheckRollup").and_then(Value::as_array).into_iter().flatten() {
        // A check run has a status and, once done, a conclusion; an older "status" has only a state.
        let done = check.get("status").and_then(Value::as_str).is_none_or(|status| status == "COMPLETED");
        let result = check.get("conclusion").or_else(|| check.get("state")).and_then(Value::as_str).unwrap_or_default();
        match (done, result) {
            (false, _) | (true, "PENDING" | "EXPECTED" | "") => checks.pending += 1,
            (true, "SUCCESS" | "NEUTRAL" | "SKIPPED") => checks.passed += 1,
            (true, _) => checks.failed += 1,
        }
    }
    checks.state = match (checks.failed, checks.pending, checks.passed) {
        (0, 0, 0) => "none",
        (0, 0, _) => "passed",
        (0, _, _) => "pending",
        _ => "failed",
    };
    Ok(Request {
        number: item.get("number").and_then(Value::as_u64).unwrap_or(0),
        title: text_of(&item, "title"),
        url: text_of(&item, "url"),
        state: match text_of(&item, "state").as_str() {
            "MERGED" => "merged",
            "CLOSED" => "closed",
            _ => "open",
        },
        draft: item.get("isDraft").and_then(Value::as_bool).unwrap_or(false),
        mergeable: match text_of(&item, "mergeable").as_str() {
            "MERGEABLE" => Some(true),
            "CONFLICTING" => Some(false),
            _ => None,
        },
        checks,
        review: match text_of(&item, "reviewDecision").as_str() {
            "APPROVED" => "approved",
            "CHANGES_REQUESTED" => "changes_requested",
            "REVIEW_REQUIRED" => "review_required",
            _ => "none",
        },
    })
}

/// A merge request as `glab mr view --output json` prints it.
pub fn parse_gitlab_request(json: &str) -> Result<Request> {
    let item: Value = serde_json::from_str(json).context("GitLab's answer could not be read")?;
    let pipeline = item
        .get("head_pipeline")
        .or_else(|| item.get("pipeline"))
        .and_then(|p| p.get("status"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut checks = Checks::default();
    checks.state = match pipeline {
        "" => "none",
        "success" | "skipped" | "manual" => {
            checks.passed = 1;
            "passed"
        }
        "failed" | "canceled" => {
            checks.failed = 1;
            "failed"
        }
        _ => {
            checks.pending = 1;
            "pending"
        }
    };
    let merge_status = text_of(&item, "detailed_merge_status");
    Ok(Request {
        number: item.get("iid").and_then(Value::as_u64).unwrap_or(0),
        title: text_of(&item, "title"),
        url: text_of(&item, "web_url"),
        state: match text_of(&item, "state").as_str() {
            "merged" => "merged",
            "closed" | "locked" => "closed",
            _ => "open",
        },
        draft: item.get("draft").and_then(Value::as_bool).unwrap_or(false),
        mergeable: match item.get("has_conflicts").and_then(Value::as_bool) {
            Some(true) => Some(false),
            Some(false) if merge_status != "checking" && merge_status != "unchecked" => Some(true),
            _ => None,
        },
        checks,
        review: match merge_status.as_str() {
            "not_approved" => "review_required",
            "requested_changes" => "changes_requested",
            _ => "none",
        },
    })
}

/// Issues as Linear's API returns them.
pub fn parse_linear_issues(answer: &Value) -> Result<Vec<Issue>> {
    if let Some(errors) = answer.get("errors").and_then(Value::as_array).filter(|list| !list.is_empty()) {
        bail!("Linear refused: {}", text_of(&errors[0], "message"));
    }
    // A team's issues, the ones assigned to the person whose key it is, or one issue on its own.
    let one = answer.pointer("/data/issue").filter(|issue| issue.is_object()).map(std::slice::from_ref);
    let nodes = answer
        .pointer("/data/issues/nodes")
        .or_else(|| answer.pointer("/data/viewer/assignedIssues/nodes"))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .or(one)
        .ok_or_else(|| anyhow!("Linear's answer has no issues in it"))?;
    Ok(nodes
        .iter()
        .map(|item| Issue {
            key: text_of(item, "identifier"),
            id: text_of(item, "id"),
            title: text_of(item, "title"),
            url: text_of(item, "url"),
            state: item.pointer("/state/name").and_then(Value::as_str).unwrap_or_default().to_string(),
            body: text_of(item, "description"),
            labels: item
                .pointer("/labels/nodes")
                .and_then(Value::as_array)
                .map(|list| list.iter().map(|label| text_of(label, "name")).collect())
                .unwrap_or_default(),
            assignee: item.pointer("/assignee/name").and_then(Value::as_str).map(str::to_string),
            updated_at: parse_time(&text_of(item, "updatedAt")),
            links: item
                .pointer("/attachments/nodes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|link| IssueLink { title: text_of(link, "title"), url: text_of(link, "url") })
                .filter(|link| !link.url.is_empty())
                .collect(),
        })
        .collect())
}

/// Whether one of the command-line tools is there and signed in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolState {
    pub installed: bool,
    pub signed_in: bool,
    /// What to do when it is not ready, in words for the user.
    pub hint: Option<String>,
}

/// The way out to the services. Tests swap the tools for scripts and Linear for a local server.
#[derive(Debug, Clone)]
pub struct Hosts {
    gh: PathBuf,
    glab: PathBuf,
    docker: PathBuf,
    linear_url: String,
}

impl Default for Hosts {
    fn default() -> Self {
        Self::detect()
    }
}

impl Hosts {
    pub fn detect() -> Self {
        let find = |name: &str| which::which(name).unwrap_or_else(|_| PathBuf::from(name));
        Self {
            gh: find("gh"),
            glab: find("glab"),
            docker: find("docker"),
            linear_url: std::env::var("AE_LINEAR_URL").unwrap_or_else(|_| "https://api.linear.app/graphql".into()),
        }
    }

    pub fn with(gh: PathBuf, glab: PathBuf, docker: PathBuf, linear_url: String) -> Self {
        Self { gh, glab, docker, linear_url }
    }

    pub fn docker(&self) -> &Path {
        &self.docker
    }

    fn tool(&self, kind: HostKind) -> &Path {
        match kind {
            HostKind::GitHub => &self.gh,
            HostKind::GitLab => &self.glab,
        }
    }

    /// Runs one of the tools against one server and returns what it printed, or its complaint as the error. Without it `gh` asks github.com and `glab` asks gitlab.com,
    /// whatever server the project lives on: a company's own GitLab answers "not found" and "not signed in".
    pub(crate) fn run_at(&self, kind: HostKind, host: Option<&str>, args: &[&str]) -> Result<String> {
        let tool = self.tool(kind);
        let name = if kind == HostKind::GitHub { "gh" } else { "glab" };
        let mut command = Command::new(tool);
        if let Some(host) = host {
            command.env(if kind == HostKind::GitHub { "GH_HOST" } else { "GITLAB_HOST" }, host);
        }
        let output = command
            .args(args)
            // Never stop to ask; plain text without colours, so it can be read by code.
            .env("GH_PROMPT_DISABLED", "1")
            .env("NO_COLOR", "1")
            .env("GLAB_NO_PROMPT", "1")
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|_| {
                anyhow!("The `{name}` command was not found. Install it to work with {} from here.", kind.name())
            })?;
        if !output.status.success() {
            let complaint = String::from_utf8_lossy(&output.stderr);
            let complaint =
                if complaint.trim().is_empty() { String::from_utf8_lossy(&output.stdout) } else { complaint };
            bail!("{}", complaint.trim());
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Whether the tool for a host is installed and signed in to every server the projects live on.
    /// With no project on such a server, signed in anywhere is enough.
    pub fn tool_state(&self, kind: HostKind, servers: &[String]) -> ToolState {
        let name = if kind == HostKind::GitHub { "gh" } else { "glab" };
        let installed = self.tool(kind).is_file() || which::which(self.tool(kind)).is_ok();
        if !installed {
            return ToolState {
                installed: false,
                signed_in: false,
                hint: Some(format!("Install the `{name}` command, then run `{name} auth login` in a terminal.")),
            };
        }
        if servers.is_empty() {
            let signed_in = self.signed_in(kind, None);
            return ToolState {
                installed: true,
                signed_in,
                hint: (!signed_in)
                    .then(|| format!("Run `{name} auth login` in a terminal to sign in to {}.", kind.name())),
            };
        }
        let out: Vec<&String> = servers.iter().filter(|server| !self.signed_in(kind, Some(server))).collect();
        let hint = match out.as_slice() {
            [] => None,
            [one] => Some(format!("Run `{name} auth login --hostname {one}` in a terminal to sign in to {one}.")),
            many => Some(format!(
                "Sign in to {} with `{name} auth login --hostname <server>` in a terminal.",
                many.iter().map(|server| server.as_str()).collect::<Vec<_>>().join(" and ")
            )),
        };
        ToolState { installed: true, signed_in: out.is_empty(), hint }
    }

    /// Whether the tool is signed in to one server, or to any.
    ///
    /// `auth status` without a server checks every server the tool has heard of and fails when any one of them
    /// is not signed in, so someone signed in to their company's GitLab only would read as signed out. Asked
    /// about the project's own server, the answer is about that server.
    fn signed_in(&self, kind: HostKind, server: Option<&str>) -> bool {
        let mut args = vec!["auth", "status"];
        if let Some(server) = server {
            args.extend(["--hostname", server]);
        }
        let Ok(output) = Command::new(self.tool(kind))
            .args(&args)
            .env("NO_COLOR", "1")
            .env("GH_PROMPT_DISABLED", "1")
            .env("GLAB_NO_PROMPT", "1")
            .stdin(std::process::Stdio::null())
            .output()
        else {
            return false;
        };
        if output.status.success() {
            return true;
        }
        // Signed in somewhere, with another server failing: for the question "any", that is a yes.
        let said = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        server.is_none() && said.contains("Logged in to")
    }

    /// Whether Docker is installed and its engine is running.
    pub fn docker_state(&self) -> ToolState {
        let installed = self.docker.is_file() || which::which(&self.docker).is_ok();
        if !installed {
            return ToolState {
                installed: false,
                signed_in: false,
                hint: Some("Docker is not installed on this machine.".into()),
            };
        }
        let running = Command::new(&self.docker)
            .args(["info", "--format", "{{.ServerVersion}}"])
            .stdin(std::process::Stdio::null())
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false);
        ToolState {
            installed: true,
            signed_in: running,
            hint: (!running).then(|| "Docker is installed but not running. Start Docker Desktop.".into()),
        }
    }

    // ----- issues -----

    pub fn issues(&self, remote: &Remote) -> Result<Vec<Issue>> {
        match remote.kind {
            HostKind::GitHub => {
                let fields = "number,title,url,state,body,labels,assignees,updatedAt";
                let json = self.run_at(
                    HostKind::GitHub,
                    Some(&remote.host),
                    &["issue", "list", "-R", &remote.slug, "--state", "open", "--limit", "100", "--json", fields],
                )?;
                parse_github_issues(&json)
            }
            HostKind::GitLab => {
                let json = self.run_at(
                    HostKind::GitLab,
                    Some(&remote.host),
                    &["issue", "list", "-R", &remote.slug, "--per-page", "100", "--output", "json"],
                )?;
                parse_gitlab_issues(&json)
            }
        }
    }

    fn linear(&self, key: &str, query: &str, variables: Value) -> Result<Value> {
        let mut response = ureq::post(&self.linear_url)
            .header("Authorization", key)
            .config()
            .timeout_global(Some(HTTP_TIMEOUT))
            .http_status_as_error(false)
            .build()
            .send_json(json!({ "query": query, "variables": variables }))
            .map_err(|error| anyhow!("Linear could not be reached: {error}"))?;
        let status = response.status().as_u16();
        if status == 401 || status == 403 {
            bail!("Linear did not accept the key. Check it in the Issues section.");
        }
        let answer: Value =
            response.body_mut().read_json().map_err(|_| anyhow!("Linear's answer could not be read"))?;
        if status >= 400 && answer.get("errors").is_none() {
            bail!("Linear answered with an error ({status})");
        }
        Ok(answer)
    }

    /// The open issues of a Linear team, newest change first.
    pub fn linear_issues(&self, key: &str, team: &str) -> Result<Vec<Issue>> {
        let query = "query($team: String!) { issues(first: 100, orderBy: updatedAt, filter: { \
                     team: { key: { eq: $team } }, state: { type: { nin: [\"completed\", \"canceled\"] } } }) { \
                     nodes { id identifier title url description updatedAt state { name type } \
                     assignee { name } labels { nodes { name } } } } }";
        parse_linear_issues(&self.linear(key, query, json!({ "team": team }))?)
    }

    /// The open issues assigned to the person whose key it is, in every team, newest change first.
    pub fn linear_mine(&self, key: &str) -> Result<Vec<Issue>> {
        let query = "query { viewer { assignedIssues(first: 100, orderBy: updatedAt, filter: { \
                     state: { type: { nin: [\"completed\", \"canceled\"] } } }) { \
                     nodes { id identifier title url description updatedAt state { name type } \
                     assignee { name } labels { nodes { name } } } } } }";
        parse_linear_issues(&self.linear(key, query, json!({}))?)
    }

    /// One issue, by the name people call it ("APP-214").
    pub fn linear_issue(&self, key: &str, identifier: &str) -> Result<Issue> {
        // One issue is asked for with what is attached to it. The lists are not: a hundred
        // issues with their attachments is more than Linear answers in one question.
        let query = "query($id: String!) { issue(id: $id) { id identifier title url description updatedAt \
                     state { name type } assignee { name } labels { nodes { name } } \
                     attachments(first: 10) { nodes { title url } } } }";
        parse_linear_issues(&self.linear(key, query, json!({ "id": identifier }))?)?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("Linear has no issue {identifier}"))
    }

    /// Who the key belongs to and in which workspace, and the teams there, for the screen to offer.
    pub fn linear_account(&self, key: &str) -> Result<Value> {
        let answer = self.linear(
            key,
            "query { viewer { name email } organization { name } teams(first: 100) { nodes { key name } } }",
            json!({}),
        )?;
        if let Some(error) = answer.pointer("/errors/0/message").and_then(Value::as_str) {
            bail!("Linear refused: {error}");
        }
        let teams: Vec<Value> = answer
            .pointer("/data/teams/nodes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|team| json!({ "key": text_of(team, "key"), "name": text_of(team, "name") }))
            .collect();
        Ok(json!({
            "name": answer.pointer("/data/viewer/name").and_then(Value::as_str),
            "workspace": answer.pointer("/data/organization/name").and_then(Value::as_str),
            "teams": teams,
        }))
    }

    /// Moves a Linear issue to the team's first state of a kind: "started" or "completed".
    pub fn linear_move(&self, key: &str, team: &str, issue_id: &str, kind: &str) -> Result<String> {
        let states = self.linear(
            key,
            "query($team: String!) { workflowStates(filter: { team: { key: { eq: $team } } }) { nodes { id name type position } } }",
            json!({ "team": team }),
        )?;
        let mut of_kind: Vec<&Value> = states
            .pointer("/data/workflowStates/nodes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|state| state.get("type").and_then(Value::as_str) == Some(kind))
            .collect();
        of_kind.sort_by(|a, b| {
            let position = |state: &Value| state.get("position").and_then(Value::as_f64).unwrap_or(0.0);
            position(a).total_cmp(&position(b))
        });
        let state = of_kind.first().ok_or_else(|| anyhow!("Team {team} has no state of the kind \"{kind}\""))?;
        let answer = self.linear(
            key,
            "mutation($id: String!, $state: String!) { issueUpdate(id: $id, input: { stateId: $state }) { success } }",
            json!({ "id": issue_id, "state": text_of(state, "id") }),
        )?;
        if answer.pointer("/data/issueUpdate/success").and_then(Value::as_bool) != Some(true) {
            let reason = answer.pointer("/errors/0/message").and_then(Value::as_str).unwrap_or("no reason given");
            // A key that may only read is the user's to replace, and is told apart from every other refusal.
            if reason.contains("Invalid scope") {
                bail!("{READ_ONLY_KEY}");
            }
            bail!("Linear did not move the issue: {reason}");
        }
        Ok(text_of(state, "name"))
    }

    // ----- merge requests -----

    /// The request for a branch, if there is one.
    pub fn request(&self, remote: &Remote, branch: &str) -> Result<Option<Request>> {
        match remote.kind {
            HostKind::GitHub => {
                let fields = "number,title,url,state,isDraft,mergeable,reviewDecision,statusCheckRollup";
                match self.run_at(
                    HostKind::GitHub,
                    Some(&remote.host),
                    &["pr", "view", branch, "-R", &remote.slug, "--json", fields],
                ) {
                    Ok(json) => parse_github_request(&json).map(Some),
                    Err(error) if error.to_string().contains("no pull requests found") => Ok(None),
                    Err(error) => Err(error),
                }
            }
            HostKind::GitLab => {
                match self.run_at(
                    HostKind::GitLab,
                    Some(&remote.host),
                    &["mr", "view", branch, "-R", &remote.slug, "--output", "json"],
                ) {
                    Ok(json) => parse_gitlab_request(&json).map(Some),
                    Err(error) => {
                        let text = error.to_string().to_lowercase();
                        if text.contains("no open merge request")
                            || text.contains("no merge request")
                            || text.contains("404")
                        {
                            Ok(None)
                        } else {
                            Err(error)
                        }
                    }
                }
            }
        }
    }

    /// Opens a request to merge `branch` into `base`. The branch must be pushed already.
    pub fn create_request(
        &self,
        remote: &Remote,
        branch: &str,
        base: &str,
        title: &str,
        body: &str,
    ) -> Result<Request> {
        match remote.kind {
            HostKind::GitHub => {
                self.run_at(
                    HostKind::GitHub,
                    Some(&remote.host),
                    &[
                        "pr",
                        "create",
                        "-R",
                        &remote.slug,
                        "--head",
                        branch,
                        "--base",
                        base,
                        "--title",
                        title,
                        "--body",
                        body,
                    ],
                )?;
            }
            HostKind::GitLab => {
                self.run_at(
                    HostKind::GitLab,
                    Some(&remote.host),
                    &[
                        "mr",
                        "create",
                        "-R",
                        &remote.slug,
                        "--source-branch",
                        branch,
                        "--target-branch",
                        base,
                        "--title",
                        title,
                        "--description",
                        body,
                        "--yes",
                    ],
                )?;
            }
        }
        self.request(remote, branch)?
            .ok_or_else(|| anyhow!("The {} was opened but cannot be read back yet", remote.kind.request_word()))
    }

    /// Merges a request. With `delete_branch` the branch is removed on the host afterwards.
    /// What reviewers wrote about lines of a request and nobody has settled yet.
    pub fn comments(&self, remote: &Remote, number: u64) -> Result<Vec<Comment>> {
        match remote.kind {
            HostKind::GitHub => {
                let (owner, name) = remote.slug.split_once('/').ok_or_else(|| anyhow!("Odd repository name"))?;
                let json = self.run_at(
                    HostKind::GitHub,
                    Some(&remote.host),
                    &[
                        "api",
                        "graphql",
                        "--hostname",
                        &remote.host,
                        "-f",
                        &format!("query={GITHUB_THREADS}"),
                        "-F",
                        &format!("owner={owner}"),
                        "-F",
                        &format!("name={name}"),
                        "-F",
                        &format!("number={number}"),
                    ],
                )?;
                parse_github_comments(&json)
            }
            HostKind::GitLab => {
                let path = format!(
                    "projects/{}/merge_requests/{number}/discussions?per_page=100",
                    gitlab_project(&remote.slug)
                );
                parse_gitlab_comments(&self.run_at(
                    HostKind::GitLab,
                    Some(&remote.host),
                    &["api", "--hostname", &remote.host, &path],
                )?)
            }
        }
    }

    /// Answers a reviewer's comment in its own thread.
    pub fn reply(&self, remote: &Remote, number: u64, comment_id: &str, body: &str) -> Result<()> {
        if !comment_id.chars().all(|c| c.is_ascii_alphanumeric()) {
            bail!("No such comment");
        }
        let text = format!("body={body}");
        match remote.kind {
            HostKind::GitHub => {
                let path = format!("repos/{}/pulls/{number}/comments/{comment_id}/replies", remote.slug);
                self.run_at(
                    HostKind::GitHub,
                    Some(&remote.host),
                    &["api", "--hostname", &remote.host, "-X", "POST", &path, "-f", &text],
                )?;
            }
            HostKind::GitLab => {
                let path = format!(
                    "projects/{}/merge_requests/{number}/discussions/{comment_id}/notes",
                    gitlab_project(&remote.slug)
                );
                self.run_at(
                    HostKind::GitLab,
                    Some(&remote.host),
                    &["api", "--hostname", &remote.host, "-X", "POST", &path, "-f", &text],
                )?;
            }
        }
        Ok(())
    }

    pub fn merge_request(&self, remote: &Remote, number: u64, squash: bool, delete_branch: bool) -> Result<()> {
        let number = number.to_string();
        match remote.kind {
            HostKind::GitHub => {
                let mut args =
                    vec!["pr", "merge", &number, "-R", &remote.slug, if squash { "--squash" } else { "--merge" }];
                if delete_branch {
                    args.push("--delete-branch");
                }
                self.run_at(HostKind::GitHub, Some(&remote.host), &args)?;
            }
            HostKind::GitLab => {
                let mut args = vec!["mr", "merge", &number, "-R", &remote.slug, "--yes"];
                if squash {
                    args.push("--squash");
                }
                if delete_branch {
                    args.push("--remove-source-branch");
                }
                self.run_at(HostKind::GitLab, Some(&remote.host), &args)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn review_comments_are_read_and_settled_ones_left_out() {
        let github = r#"{"data":{"repository":{"pullRequest":{"reviewThreads":{"nodes":[
            {"isResolved":false,"comments":{"nodes":[{"databaseId":55,"author":{"login":"anna"},"path":"src/a.ts","line":5,"originalLine":4,"body":" Should the limit come from config? ","url":"https://github.com/o/n/pull/1#discussion_r55"}]}},
            {"isResolved":true,"comments":{"nodes":[{"databaseId":56,"author":{"login":"anna"},"path":"src/b.ts","line":1,"body":"done","url":""}]}},
            {"isResolved":false,"comments":{"nodes":[{"databaseId":57,"author":null,"path":"src/c.ts","line":null,"originalLine":12,"body":"On a line that is gone","url":""}]}}
        ]}}}}}"#;
        let comments = super::parse_github_comments(github).unwrap();
        assert_eq!(comments.len(), 2);
        assert_eq!((comments[0].id.as_str(), comments[0].author.as_str()), ("55", "anna"));
        assert_eq!((comments[0].path.as_deref(), comments[0].line), (Some("src/a.ts"), Some(5)));
        assert_eq!(comments[0].body, "Should the limit come from config?");
        assert_eq!((comments[1].author.as_str(), comments[1].line), ("someone", Some(12)));
        assert!(super::parse_github_comments("{}").is_err());

        let gitlab = r#"[
            {"id":"abc123","notes":[{"system":false,"resolvable":true,"resolved":false,"author":{"username":"boris"},"body":"Put the limiter on /register too.","position":{"new_path":"routes/auth.ts","new_line":12}}]},
            {"id":"sys","notes":[{"system":true,"resolvable":false,"body":"changed the description"}]},
            {"id":"done","notes":[{"system":false,"resolvable":true,"resolved":true,"author":{"username":"boris"},"body":"ok"}]}
        ]"#;
        let comments = super::parse_gitlab_comments(gitlab).unwrap();
        assert_eq!(comments.len(), 1);
        assert_eq!((comments[0].id.as_str(), comments[0].author.as_str()), ("abc123", "boris"));
        assert_eq!((comments[0].path.as_deref(), comments[0].line), (Some("routes/auth.ts"), Some(12)));
        assert_eq!(super::gitlab_project("group/sub/name"), "group%2Fsub%2Fname");
    }

    use super::*;

    #[test]
    fn reads_where_a_repository_lives_from_its_address() {
        let github = Remote { kind: HostKind::GitHub, host: "github.com".into(), slug: "mrx/shop".into() };
        assert_eq!(parse_remote("git@github.com:mrx/shop.git"), Some(github.clone()));
        assert_eq!(parse_remote("https://github.com/mrx/shop"), Some(github.clone()));
        assert_eq!(parse_remote("https://user:secret@github.com/mrx/shop.git\n"), Some(github.clone()));
        assert_eq!(parse_remote("ssh://git@github.com/mrx/shop.git"), Some(github));
        assert_eq!(
            parse_remote("git@gitlab.example.org:group/sub/shop.git"),
            Some(Remote { kind: HostKind::GitLab, host: "gitlab.example.org".into(), slug: "group/sub/shop".into() })
        );
        assert_eq!(
            parse_remote("https://gitlab.com:8443/group/shop.git"),
            Some(Remote { kind: HostKind::GitLab, host: "gitlab.com".into(), slug: "group/shop".into() })
        );
        // A folder on disk, another host, an address without an owner: not a known host.
        assert_eq!(parse_remote("/tmp/remote.git"), None);
        assert_eq!(parse_remote("git@bitbucket.org:mrx/shop.git"), None);
        assert_eq!(parse_remote("https://github.com/shop"), None);
        assert_eq!(parse_remote(""), None);
    }

    #[test]
    fn reads_times_as_the_hosts_write_them() {
        assert_eq!(parse_time("1970-01-01T00:00:00Z"), 0);
        assert_eq!(parse_time("2000-03-01T00:00:00Z"), 951_868_800_000);
        assert_eq!(parse_time("2026-11-12T19:05:30.123Z"), 1_794_510_330_000);
        assert_eq!(parse_time("not a time"), 0);
    }

    #[test]
    fn reads_github_issues() {
        let json = r##"[{"number":12,"title":"Login allows unlimited attempts","url":"https://github.com/mrx/shop/issues/12",
            "state":"OPEN","body":"Five a minute is enough.","labels":[{"name":"bug"},{"name":"auth"}],
            "assignees":[{"login":"mrx"}],"updatedAt":"2026-11-12T19:05:30Z"},
            {"number":13,"title":"No body","url":"u","state":"OPEN","body":"","labels":[],"assignees":[],"updatedAt":""}]"##;
        let issues = parse_github_issues(json).unwrap();
        assert_eq!(issues.len(), 2);
        assert_eq!(issues[0].key, "#12");
        assert_eq!(issues[0].state, "open");
        assert_eq!(issues[0].labels, vec!["bug", "auth"]);
        assert_eq!(issues[0].assignee.as_deref(), Some("mrx"));
        assert_eq!(issues[0].updated_at, 1_794_510_330_000);
        assert_eq!((issues[1].assignee.clone(), issues[1].updated_at), (None, 0));
        assert!(parse_github_issues("gh: not logged in").is_err());
    }

    #[test]
    fn reads_gitlab_issues() {
        let json = r##"[{"iid":7,"title":"Search ignores accents","web_url":"https://gitlab.com/g/p/-/issues/7","state":"opened",
            "description":"Cafe and café should match.","labels":["search"],"assignees":[{"username":"ann"}],
            "updated_at":"2026-11-12T19:05:30.000Z"}]"##;
        let issues = parse_gitlab_issues(json).unwrap();
        assert_eq!((issues[0].key.as_str(), issues[0].state.as_str()), ("#7", "open"));
        assert_eq!(issues[0].labels, vec!["search"]);
        assert_eq!(issues[0].assignee.as_deref(), Some("ann"));
    }

    #[test]
    fn reads_a_github_pull_request_and_knows_when_it_can_merge() {
        let open = |checks: &str, review: &str, mergeable: &str| {
            format!(
                r#"{{"number":128,"title":"Limit logins","url":"https://github.com/mrx/shop/pull/128","state":"OPEN",
                "isDraft":false,"mergeable":"{mergeable}","reviewDecision":"{review}","statusCheckRollup":[{checks}]}}"#
            )
        };
        let green = r#"{"__typename":"CheckRun","status":"COMPLETED","conclusion":"SUCCESS"},{"__typename":"StatusContext","state":"SUCCESS"}"#;
        let request = parse_github_request(&open(green, "APPROVED", "MERGEABLE")).unwrap();
        assert_eq!((request.number, request.state, request.review), (128, "open", "approved"));
        assert_eq!(request.checks, Checks { state: "passed", passed: 2, failed: 0, pending: 0 });
        assert!(request.ready());

        let running = r#"{"__typename":"CheckRun","status":"IN_PROGRESS","conclusion":""},{"__typename":"CheckRun","status":"COMPLETED","conclusion":"SUCCESS"}"#;
        let request = parse_github_request(&open(running, "", "UNKNOWN")).unwrap();
        assert_eq!((request.checks.state, request.checks.pending, request.mergeable), ("pending", 1, None));
        assert!(!request.ready());

        let red = r#"{"__typename":"CheckRun","status":"COMPLETED","conclusion":"FAILURE"}"#;
        assert_eq!(parse_github_request(&open(red, "", "MERGEABLE")).unwrap().checks.state, "failed");
        // No checks at all is not a reason to wait; a conflict and a requested change are.
        assert!(parse_github_request(&open("", "", "MERGEABLE")).unwrap().ready());
        assert!(!parse_github_request(&open("", "", "CONFLICTING")).unwrap().ready());
        assert!(!parse_github_request(&open("", "CHANGES_REQUESTED", "MERGEABLE")).unwrap().ready());

        let merged = r#"{"number":5,"title":"t","url":"u","state":"MERGED","isDraft":false,"mergeable":"UNKNOWN","reviewDecision":"","statusCheckRollup":[]}"#;
        let request = parse_github_request(merged).unwrap();
        assert_eq!(request.state, "merged");
        assert!(!request.ready());
    }

    #[test]
    fn reads_a_gitlab_merge_request() {
        let json = r#"{"iid":66,"title":"Speed up search","web_url":"https://gitlab.com/g/p/-/merge_requests/66","state":"opened",
            "draft":false,"has_conflicts":false,"detailed_merge_status":"mergeable","head_pipeline":{"status":"success"}}"#;
        let request = parse_gitlab_request(json).unwrap();
        assert_eq!(
            (request.number, request.state, request.mergeable, request.checks.state),
            (66, "open", Some(true), "passed")
        );
        assert!(request.ready());

        let waiting = r#"{"iid":67,"title":"t","web_url":"u","state":"opened","draft":false,"has_conflicts":false,
            "detailed_merge_status":"not_approved","head_pipeline":{"status":"running"}}"#;
        let request = parse_gitlab_request(waiting).unwrap();
        assert_eq!((request.review, request.checks.state), ("review_required", "pending"));
        let conflict = r#"{"iid":68,"title":"t","web_url":"u","state":"opened","has_conflicts":true,"detailed_merge_status":"conflict"}"#;
        assert_eq!(parse_gitlab_request(conflict).unwrap().mergeable, Some(false));
        assert_eq!(parse_gitlab_request(r#"{"iid":1,"state":"merged"}"#).unwrap().state, "merged");
    }

    #[test]
    fn reads_linear_issues_and_its_refusals() {
        let answer = json!({ "data": { "issues": { "nodes": [{
            "id": "uuid-1", "identifier": "APP-214", "title": "Login allows unlimited attempts",
            "url": "https://linear.app/x/issue/APP-214", "description": "Limit it.", "updatedAt": "2026-11-12T19:05:30.000Z",
            "state": { "name": "In Progress", "type": "started" }, "assignee": { "name": "Ann" },
            "labels": { "nodes": [{ "name": "Auth" }] }
        }] } } });
        let issues = parse_linear_issues(&answer).unwrap();
        assert_eq!(
            (issues[0].key.as_str(), issues[0].id.as_str(), issues[0].state.as_str()),
            ("APP-214", "uuid-1", "In Progress")
        );
        assert_eq!(issues[0].labels, vec!["Auth"]);
        assert_eq!(issues[0].assignee.as_deref(), Some("Ann"));

        let refused = json!({ "errors": [{ "message": "Authentication required" }] });
        assert!(parse_linear_issues(&refused).unwrap_err().to_string().contains("Authentication required"));
        assert!(parse_linear_issues(&json!({ "data": {} })).is_err());
    }
}
