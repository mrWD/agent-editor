//! An issue or a merge request in full: what it says, who is on it, and what was said under it.
//!
//! The lists of issues and the card of a request show a line each. To decide
//! whether to start on an issue, or to see what a reviewer wrote under a
//! request, the whole of it is wanted: the description, the people, the
//! comments in the order they were written, and the pictures in them.
//!
//! Each tracker is asked the way it is asked for everything else: GitHub
//! through `gh`, GitLab through `glab api`, Linear with the user's key.
//!
//! A picture in a description is an address at the tracker that only a
//! signed-in person may fetch. The interface cannot show it by itself, so the
//! core fetches it with the user's own sign-in and hands the bytes on. It does
//! that only for addresses that are the tracker's own places for uploads: a
//! text may name any address, and the sign-in must not be sent to it.
//!
//! Nothing is written here: no comment is added and nothing is changed.

use crate::hosts::{HostKind, Hosts, IssueLink, Remote, parse_time};
use crate::tasks::Manager;
use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;
use serde_json::{Value, json};
use std::io::Read;
use std::time::Duration;

/// How many comments of one issue or request are read.
const MOST_NOTES: usize = 100;
/// A picture larger than this is not handed on.
const MAX_PICTURE: u64 = 20 * 1024 * 1024;
/// How long a tracker may take to give a picture.
const PICTURE_PATIENCE: Duration = Duration::from_secs(20);
/// Where Linear keeps what is uploaded to an issue.
const LINEAR_UPLOADS: &str = "https://uploads.linear.app/";

/// Something written under an issue or a request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Note {
    pub author: String,
    /// When it was written, in milliseconds.
    pub at: u64,
    /// What it says, in Markdown.
    pub body: String,
    /// "comment", or "review" for the word a reviewer gave on a request as a whole.
    pub kind: &'static str,
    /// For a review: "approved", "asked for changes". For a comment on a line: the file. Else empty.
    pub about: String,
}

/// An issue or a request, whole.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Detail {
    /// "github", "gitlab" or "linear".
    pub source: &'static str,
    /// How people call it: `#12`, `!48`, `APP-214`.
    pub key: String,
    pub title: String,
    pub url: String,
    /// The tracker's own word for where it stands.
    pub state: String,
    /// A request that is not yet meant to be merged.
    pub draft: bool,
    pub author: String,
    pub assignees: Vec<String>,
    /// Who was asked to review a request.
    pub reviewers: Vec<String>,
    pub labels: Vec<String>,
    pub created_at: u64,
    pub updated_at: u64,
    /// The description, in Markdown.
    pub body: String,
    /// What is attached to it in the tracker.
    pub links: Vec<IssueLink>,
    /// What was written under it, oldest first.
    pub notes: Vec<Note>,
    /// For a request: the branch it comes from and the one it goes into.
    pub branch: Option<String>,
    pub base: Option<String>,
}

fn text(item: &Value, name: &str) -> String {
    item.get(name).and_then(Value::as_str).unwrap_or_default().to_string()
}

/// The name of a person as a tracker gives one: GitHub a login, GitLab and Linear a name.
fn person(item: Option<&Value>) -> String {
    let Some(item) = item else { return String::new() };
    ["name", "login", "username", "displayName"]
        .iter()
        .map(|name| text(item, name))
        .find(|name| !name.is_empty())
        .unwrap_or_default()
}

fn people(item: &Value, name: &str) -> Vec<String> {
    let list = item
        .get(name)
        .and_then(Value::as_array)
        .or_else(|| item.pointer(&format!("/{name}/nodes")).and_then(Value::as_array));
    list.into_iter().flatten().map(|one| person(Some(one))).filter(|name| !name.is_empty()).collect()
}

fn labels(item: &Value) -> Vec<String> {
    let list = item
        .get("labels")
        .and_then(Value::as_array)
        .or_else(|| item.pointer("/labels/nodes").and_then(Value::as_array));
    // GitHub and Linear give objects with a name; GitLab gives plain words.
    list.into_iter()
        .flatten()
        .filter_map(|label| {
            label.as_str().map(str::to_string).or_else(|| Some(text(label, "name")).filter(|name| !name.is_empty()))
        })
        .collect()
}

fn sorted(mut notes: Vec<Note>) -> Vec<Note> {
    notes.sort_by_key(|note| note.at);
    notes.truncate(MOST_NOTES);
    notes
}

/// An issue or a pull request as `gh issue view --json` and `gh pr view --json` give one.
pub fn parse_github(json: &str, request: bool) -> Result<Detail> {
    let item: Value = serde_json::from_str(json).context("GitHub's answer could not be read")?;
    let mut notes: Vec<Note> = item
        .get("comments")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|comment| Note {
            author: person(comment.get("author")),
            at: parse_time(&text(comment, "createdAt")),
            body: text(comment, "body"),
            kind: "comment",
            about: String::new(),
        })
        .collect();
    // The word a reviewer gave on the request as a whole. One who only left lines said nothing here.
    for review in item.get("reviews").and_then(Value::as_array).into_iter().flatten() {
        let about = match text(review, "state").as_str() {
            "APPROVED" => "approved",
            "CHANGES_REQUESTED" => "asked for changes",
            "DISMISSED" => "review dismissed",
            _ if text(review, "body").trim().is_empty() => continue,
            _ => "reviewed",
        };
        notes.push(Note {
            author: person(review.get("author")),
            at: parse_time(&text(review, "submittedAt")),
            body: text(review, "body"),
            kind: "review",
            about: about.to_string(),
        });
    }
    let number = item.get("number").and_then(Value::as_u64).unwrap_or(0);
    Ok(Detail {
        source: "github",
        key: format!("#{number}"),
        title: text(&item, "title"),
        url: text(&item, "url"),
        state: text(&item, "state").to_lowercase(),
        draft: item.get("isDraft").and_then(Value::as_bool).unwrap_or(false),
        author: person(item.get("author")),
        assignees: people(&item, "assignees"),
        reviewers: people(&item, "reviewRequests"),
        labels: labels(&item),
        created_at: parse_time(&text(&item, "createdAt")),
        updated_at: parse_time(&text(&item, "updatedAt")),
        body: text(&item, "body"),
        links: Vec::new(),
        notes: sorted(notes),
        branch: request.then(|| text(&item, "headRefName")),
        base: request.then(|| text(&item, "baseRefName")),
    })
}

/// An issue or a merge request of GitLab, with the notes under it, as its API gives them.
pub fn parse_gitlab(item: &str, notes: &str, request: bool) -> Result<Detail> {
    let item: Value = serde_json::from_str(item).context("GitLab's answer could not be read")?;
    let listed: Vec<Value> =
        serde_json::from_str(notes).context("GitLab's answer about the comments could not be read")?;
    let notes = listed
        .iter()
        // "added 2 commits", "changed the description": what GitLab notes by itself is not a comment.
        .filter(|note| note.get("system").and_then(Value::as_bool) != Some(true))
        .map(|note| Note {
            author: person(note.get("author")),
            at: parse_time(&text(note, "created_at")),
            body: text(note, "body"),
            kind: "comment",
            // A note on a line of the change says which file it is about.
            about: note.pointer("/position/new_path").and_then(Value::as_str).unwrap_or_default().to_string(),
        })
        .collect();
    let number = item.get("iid").and_then(Value::as_u64).unwrap_or(0);
    Ok(Detail {
        source: "gitlab",
        key: format!("{}{number}", if request { '!' } else { '#' }),
        title: text(&item, "title"),
        url: text(&item, "web_url"),
        state: text(&item, "state"),
        draft: item.get("draft").or_else(|| item.get("work_in_progress")).and_then(Value::as_bool).unwrap_or(false),
        author: person(item.get("author")),
        assignees: people(&item, "assignees"),
        reviewers: people(&item, "reviewers"),
        labels: labels(&item),
        created_at: parse_time(&text(&item, "created_at")),
        updated_at: parse_time(&text(&item, "updated_at")),
        body: text(&item, "description"),
        links: Vec::new(),
        notes: sorted(notes),
        branch: request.then(|| text(&item, "source_branch")),
        base: request.then(|| text(&item, "target_branch")),
    })
}

/// One issue of Linear with its comments, as its API answers.
pub fn parse_linear(answer: &Value) -> Result<Detail> {
    if let Some(errors) = answer.get("errors").and_then(Value::as_array).filter(|list| !list.is_empty()) {
        bail!("Linear refused: {}", text(&errors[0], "message"));
    }
    let item = answer
        .pointer("/data/issue")
        .filter(|issue| issue.is_object())
        .ok_or_else(|| anyhow!("Linear has no such issue"))?;
    let notes = item
        .pointer("/comments/nodes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|comment| Note {
            // A comment made by an integration has no person; it says what made it.
            author: Some(person(comment.get("user")))
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| person(comment.get("botActor"))),
            at: parse_time(&text(comment, "createdAt")),
            body: text(comment, "body"),
            kind: "comment",
            about: String::new(),
        })
        .collect();
    Ok(Detail {
        source: "linear",
        key: text(item, "identifier"),
        title: text(item, "title"),
        url: text(item, "url"),
        state: item.pointer("/state/name").and_then(Value::as_str).unwrap_or_default().to_string(),
        draft: false,
        author: person(item.get("creator")),
        assignees: item
            .get("assignee")
            .map(|one| person(Some(one)))
            .filter(|name| !name.is_empty())
            .into_iter()
            .collect(),
        reviewers: Vec::new(),
        labels: labels(item),
        created_at: parse_time(&text(item, "createdAt")),
        updated_at: parse_time(&text(item, "updatedAt")),
        body: text(item, "description"),
        links: item
            .pointer("/attachments/nodes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|link| IssueLink { title: text(link, "title"), url: text(link, "url") })
            .filter(|link| !link.url.is_empty())
            .collect(),
        notes: sorted(notes),
        branch: None,
        base: None,
    })
}

fn gitlab_project(slug: &str) -> String {
    slug.replace('/', "%2F")
}

const LINEAR_DETAIL: &str = "query($id: String!) { issue(id: $id) { id identifier title url description createdAt updatedAt \
     state { name type } creator { name } assignee { name } labels { nodes { name } } \
     attachments(first: 20) { nodes { title url } } \
     comments(first: 100) { nodes { body createdAt user { name } botActor { name } } } } }";

impl Hosts {
    /// One issue of a repository, whole.
    pub fn issue_detail(&self, remote: &Remote, number: u64) -> Result<Detail> {
        let number = number.to_string();
        match remote.kind {
            HostKind::GitHub => {
                let fields = "number,title,body,state,author,assignees,labels,createdAt,updatedAt,url,comments";
                let args = ["issue", "view", &number, "-R", &remote.slug, "--json", fields];
                parse_github(&self.run_at(HostKind::GitHub, Some(&remote.host), &args)?, false)
            }
            HostKind::GitLab => {
                let path = format!("projects/{}/issues/{number}", gitlab_project(&remote.slug));
                self.gitlab_detail(remote, &path, false)
            }
        }
    }

    /// One request of a repository, whole.
    pub fn request_detail(&self, remote: &Remote, number: u64) -> Result<Detail> {
        let number = number.to_string();
        match remote.kind {
            HostKind::GitHub => {
                let fields = "number,title,body,state,isDraft,author,assignees,reviewRequests,reviews,labels,\
                              createdAt,updatedAt,url,comments,baseRefName,headRefName";
                let args = ["pr", "view", &number, "-R", &remote.slug, "--json", fields];
                parse_github(&self.run_at(HostKind::GitHub, Some(&remote.host), &args)?, true)
            }
            HostKind::GitLab => {
                let path = format!("projects/{}/merge_requests/{number}", gitlab_project(&remote.slug));
                self.gitlab_detail(remote, &path, true)
            }
        }
    }

    fn gitlab_detail(&self, remote: &Remote, path: &str, request: bool) -> Result<Detail> {
        let ask =
            |path: &str| self.run_at(HostKind::GitLab, Some(&remote.host), &["api", "--hostname", &remote.host, path]);
        let item = ask(path)?;
        let notes = ask(&format!("{path}/notes?sort=asc&order_by=created_at&per_page={MOST_NOTES}"))?;
        parse_gitlab(&item, &notes, request)
    }

    /// One issue of Linear, whole, by the name people call it.
    pub fn linear_detail(&self, key: &str, identifier: &str) -> Result<Detail> {
        parse_linear(&self.linear_ask(key, LINEAR_DETAIL, json!({ "id": identifier }))?)
    }
}

// ----- pictures -----

/// How a picture named in a text is fetched. `None` for an address that is not the tracker's own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fetch {
    /// With the user's Linear key, from Linear's uploads.
    Linear(String),
    /// With the token `gh` is signed in with, from GitHub's places for attachments.
    GitHub(String),
    /// Through GitLab's API: the upload of a project by its secret and its name.
    GitLab(String),
}

fn plain(text: &str, more: &str) -> bool {
    !text.is_empty() && text.chars().all(|sign| sign.is_ascii_alphanumeric() || more.contains(sign))
}

/// How the picture at `src` is fetched for a project that lives at `remote`, with Linear's uploads
/// at `linear_uploads`. Only the trackers' own places for uploads are fetched from.
pub fn fetch_plan(remote: Option<&Remote>, linear_uploads: &str, src: &str) -> Option<Fetch> {
    let src = src.trim();
    if src.contains(char::is_whitespace) || src.contains("..") {
        return None;
    }
    if src.starts_with(LINEAR_UPLOADS) || (!linear_uploads.is_empty() && src.starts_with(linear_uploads)) {
        return Some(Fetch::Linear(src.to_string()));
    }
    let remote = remote?;
    match remote.kind {
        HostKind::GitHub => {
            let own = [
                format!("https://{}/user-attachments/assets/", remote.host),
                format!("https://{}/{}/assets/", remote.host, remote.slug),
                "https://user-images.githubusercontent.com/".to_string(),
                "https://private-user-images.githubusercontent.com/".to_string(),
            ];
            own.iter().any(|start| src.starts_with(start.as_str())).then(|| Fetch::GitHub(src.to_string()))
        }
        HostKind::GitLab => {
            // `/uploads/<secret>/<name>` as the text writes it, or the same under the project's address.
            let own = format!("https://{}/", remote.host);
            if !src.starts_with('/') && !src.starts_with(&own) {
                return None;
            }
            let (_, rest) = src.split_once("/uploads/")?;
            let (secret, name) = rest.split_once('/')?;
            let name = name.split(['?', '#']).next().unwrap_or_default();
            (plain(secret, "") && plain(name, "._-%+"))
                .then(|| Fetch::GitLab(format!("projects/{}/uploads/{secret}/{name}", gitlab_project(&remote.slug))))
        }
    }
}

/// What kind of picture some bytes are, by how they begin. `None` for anything that is not a
/// picture a page can show safely: a drawing in SVG can hold a script.
pub fn picture_kind(bytes: &[u8]) -> Option<&'static str> {
    match bytes {
        [0x89, b'P', b'N', b'G', ..] => Some("image/png"),
        [0xFF, 0xD8, 0xFF, ..] => Some("image/jpeg"),
        [b'G', b'I', b'F', b'8', ..] => Some("image/gif"),
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => Some("image/webp"),
        [_, _, _, _, b'f', b't', b'y', b'p', b'a', b'v', b'i', b'f', ..] => Some("image/avif"),
        [b'B', b'M', ..] => Some("image/bmp"),
        _ => None,
    }
}

fn get(url: &str, authorization: &str) -> Result<Vec<u8>> {
    let mut response = ureq::get(url)
        .header("Authorization", authorization)
        .config()
        .timeout_global(Some(PICTURE_PATIENCE))
        .http_status_as_error(false)
        .build()
        .call()
        .map_err(|error| anyhow!("The picture could not be reached: {error}"))?;
    if response.status().as_u16() != 200 {
        bail!("The tracker did not give the picture ({})", response.status().as_u16());
    }
    let mut bytes = Vec::new();
    response.body_mut().as_reader().take(MAX_PICTURE + 1).read_to_end(&mut bytes)?;
    Ok(bytes)
}

impl Manager {
    /// An issue of a project, whole. `source` is "linear" for one of the user's own Linear issues,
    /// whatever tracker the project itself uses.
    pub fn issue_detail(&self, project_id: &str, key: &str, source: Option<&str>) -> Result<Detail> {
        let project = self.store().project(project_id)?;
        let linear = source == Some("linear") || self.linear_team(project_id).is_some();
        if linear {
            let token =
                self.linear_key().ok_or_else(|| anyhow!("Linear is not connected yet. Paste a key in Issues."))?;
            return self.hosts.linear_detail(&token, key);
        }
        let remote = crate::hosts::remote_of(std::path::Path::new(&project.path))
            .ok_or_else(|| anyhow!("This project's remote is not on GitHub or GitLab"))?;
        let number: u64 =
            key.trim_start_matches('#').parse().map_err(|_| anyhow!("{key} is not the number of an issue"))?;
        self.hosts.issue_detail(&remote, number).map_err(|error| crate::hosts::said_plainly(&remote, error))
    }

    /// The request of a task, whole.
    pub fn request_detail(&self, task_id: &str) -> Result<Detail> {
        let (_, _, branch, remote) = self.request_place(task_id)?;
        let number = match self.request_number(task_id) {
            Some(number) => number,
            None => self
                .hosts
                .request(&remote, &branch)
                .map_err(|error| crate::hosts::said_plainly(&remote, error))?
                .map(|request| request.number)
                .ok_or_else(|| anyhow!("This task has no {} yet", remote.kind.request_word()))?,
        };
        self.hosts.request_detail(&remote, number).map_err(|error| crate::hosts::said_plainly(&remote, error))
    }

    /// A picture named in an issue or a request of a project: its bytes and what kind of picture
    /// they are. Refused for an address that is not the tracker's own place for uploads.
    pub fn tracker_picture(&self, project_id: &str, src: &str) -> Result<(Vec<u8>, &'static str)> {
        let project = self.store().project(project_id)?;
        let remote = crate::hosts::remote_of(std::path::Path::new(&project.path));
        let plan = fetch_plan(remote.as_ref(), &self.hosts.linear_uploads(), src)
            .ok_or_else(|| anyhow!("That picture is not kept by the project's tracker, so it is not fetched"))?;
        let bytes = match plan {
            Fetch::Linear(url) => {
                let key = self.linear_key().ok_or_else(|| anyhow!("Linear is not connected"))?;
                get(&url, &key)?
            }
            Fetch::GitHub(url) => {
                let host = remote.as_ref().map(|remote| remote.host.clone()).unwrap_or_default();
                let token =
                    self.hosts.run_at(HostKind::GitHub, Some(&host), &["auth", "token", "--hostname", &host])?;
                get(&url, &format!("Bearer {}", token.trim()))?
            }
            Fetch::GitLab(path) => {
                let host = remote.as_ref().map(|remote| remote.host.clone()).unwrap_or_default();
                self.hosts.run_bytes_at(HostKind::GitLab, Some(&host), &["api", "--hostname", &host, &path])?
            }
        };
        if bytes.len() as u64 > MAX_PICTURE {
            bail!("The picture is too large to show here");
        }
        let kind =
            picture_kind(&bytes).ok_or_else(|| anyhow!("What the tracker gave is not a picture that can be shown"))?;
        Ok((bytes, kind))
    }
}

/// The commands for an issue or a request in full.
///
/// - `issues.detail { projectId, key, source? }` gives an issue.
/// - `mr.detail { taskId }` gives the request of a task.
///
/// They wait for a tracker, so they run off the event loop.
pub async fn command(manager: &std::sync::Arc<Manager>, method: &str, params: &Value) -> Result<Value> {
    let word = |name: &str| -> Result<String> {
        params.get(name).and_then(Value::as_str).map(str::to_string).ok_or_else(|| anyhow!("Missing parameter: {name}"))
    };
    let manager = manager.clone();
    match method {
        "issues.detail" => {
            let (project, key) = (word("projectId")?, word("key")?);
            let source = params.get("source").and_then(Value::as_str).map(str::to_string);
            Ok(json!(
                tokio::task::spawn_blocking(move || manager.issue_detail(&project, &key, source.as_deref())).await??
            ))
        }
        "mr.detail" => {
            let task = word("taskId")?;
            Ok(json!(tokio::task::spawn_blocking(move || manager.request_detail(&task)).await??))
        }
        other => bail!("Unknown method: {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_github_issue_is_read_with_its_comments_oldest_first() {
        let json = r#"{"number":12,"title":"Login allows unlimited attempts","body":"Five a minute.\n\n![shot](https://github.com/user-attachments/assets/abc)",
            "state":"OPEN","author":{"login":"ann"},"assignees":[{"login":"bob"}],"labels":[{"name":"auth"}],
            "createdAt":"2026-11-10T08:00:00Z","updatedAt":"2026-11-12T19:05:30Z","url":"https://github.com/x/shop/issues/12",
            "comments":[{"author":{"login":"bob"},"body":"Second","createdAt":"2026-11-12T10:00:00Z"},
                        {"author":{"login":"ann"},"body":"First","createdAt":"2026-11-11T10:00:00Z"}]}"#;
        let issue = parse_github(json, false).unwrap();
        assert_eq!((issue.key.as_str(), issue.state.as_str(), issue.author.as_str()), ("#12", "open", "ann"));
        assert_eq!(
            (issue.assignees.clone(), issue.labels.clone()),
            (vec!["bob".to_string()], vec!["auth".to_string()])
        );
        assert_eq!(issue.notes.iter().map(|note| note.body.as_str()).collect::<Vec<_>>(), ["First", "Second"]);
        assert_eq!((issue.branch, issue.draft), (None, false));
    }

    #[test]
    fn a_pull_request_carries_its_branches_and_what_reviewers_said_of_it_as_a_whole() {
        let json = r#"{"number":128,"title":"Limit logins","body":"","state":"OPEN","isDraft":true,"author":{"login":"ann"},
            "assignees":[],"reviewRequests":[{"login":"cy"},{"name":"platform"}],"labels":[],
            "createdAt":"2026-11-10T08:00:00Z","updatedAt":"2026-11-12T19:05:30Z","url":"https://github.com/x/shop/pull/128",
            "baseRefName":"main","headRefName":"task/limit","comments":[{"author":{"login":"ann"},"body":"Ready","createdAt":"2026-11-11T10:00:00Z"}],
            "reviews":[{"author":{"login":"cy"},"state":"CHANGES_REQUESTED","body":"See line 3","submittedAt":"2026-11-11T12:00:00Z"},
                       {"author":{"login":"cy"},"state":"COMMENTED","body":"","submittedAt":"2026-11-11T12:30:00Z"},
                       {"author":{"login":"dee"},"state":"APPROVED","body":"","submittedAt":"2026-11-12T09:00:00Z"}]}"#;
        let request = parse_github(json, true).unwrap();
        assert!(request.draft);
        assert_eq!((request.branch.as_deref(), request.base.as_deref()), (Some("task/limit"), Some("main")));
        assert_eq!(request.reviewers, ["cy", "platform"]);
        let said: Vec<_> =
            request.notes.iter().map(|note| (note.author.as_str(), note.kind, note.about.as_str())).collect();
        assert_eq!(
            said,
            [("ann", "comment", ""), ("cy", "review", "asked for changes"), ("dee", "review", "approved")]
        );
    }

    #[test]
    fn a_gitlab_request_leaves_out_what_gitlab_notes_by_itself() {
        let item = r#"{"iid":48,"title":"Limit logins","description":"![shot](/uploads/0123456789abcdef0123456789abcdef/shot.png)",
            "state":"opened","draft":false,"author":{"name":"Ann Lee","username":"ann"},"assignees":[{"name":"Bob"}],
            "reviewers":[{"name":"Cy"}],"labels":["auth","backend"],"created_at":"2026-11-10T08:00:00.000Z",
            "updated_at":"2026-11-12T19:05:30.000Z","web_url":"https://gitlab.example.com/g/shop/-/merge_requests/48",
            "source_branch":"task/limit","target_branch":"main"}"#;
        let notes = r#"[{"body":"added 2 commits","system":true,"author":{"name":"Ann Lee"},"created_at":"2026-11-10T09:00:00.000Z"},
            {"body":"Why five?","system":false,"author":{"name":"Cy"},"created_at":"2026-11-11T09:00:00.000Z","position":{"new_path":"src/limit.ts"}},
            {"body":"Because of the spec.","system":false,"author":{"name":"Ann Lee"},"created_at":"2026-11-11T10:00:00.000Z"}]"#;
        let request = parse_gitlab(item, notes, true).unwrap();
        assert_eq!((request.key.as_str(), request.author.as_str(), request.labels.len()), ("!48", "Ann Lee", 2));
        assert_eq!(
            (request.reviewers.clone(), request.branch.as_deref()),
            (vec!["Cy".to_string()], Some("task/limit"))
        );
        let said: Vec<_> = request.notes.iter().map(|note| (note.body.as_str(), note.about.as_str())).collect();
        assert_eq!(said, [("Why five?", "src/limit.ts"), ("Because of the spec.", "")]);
        assert_eq!(parse_gitlab(item, "[]", false).unwrap().key, "#48");
    }

    #[test]
    fn a_linear_issue_is_read_with_what_is_attached_and_what_was_said() {
        let answer = json!({ "data": { "issue": {
            "identifier": "APP-214", "title": "Unreachable UI", "url": "https://linear.app/x/issue/APP-214", "description": "Remove it.",
            "createdAt": "2026-11-10T08:00:00.000Z", "updatedAt": "2026-11-12T19:05:30.000Z",
            "state": { "name": "In Progress" }, "creator": { "name": "Ann" }, "assignee": { "name": "Bob" },
            "labels": { "nodes": [{ "name": "frontend" }] },
            "attachments": { "nodes": [{ "title": "Sentry", "url": "https://sentry.io/issues/1" }, { "title": "", "url": "" }] },
            "comments": { "nodes": [
                { "body": "From the integration", "createdAt": "2026-11-12T08:00:00.000Z", "user": null, "botActor": { "name": "GitLab" } },
                { "body": "Agreed", "createdAt": "2026-11-11T08:00:00.000Z", "user": { "name": "Cy" } }
            ] }
        } } });
        let issue = parse_linear(&answer).unwrap();
        assert_eq!(
            (issue.key.as_str(), issue.state.as_str(), issue.author.as_str()),
            ("APP-214", "In Progress", "Ann")
        );
        assert_eq!(
            (issue.assignees.clone(), issue.labels.clone(), issue.links.len()),
            (vec!["Bob".to_string()], vec!["frontend".to_string()], 1)
        );
        let said: Vec<_> = issue.notes.iter().map(|note| (note.author.as_str(), note.body.as_str())).collect();
        assert_eq!(said, [("Cy", "Agreed"), ("GitLab", "From the integration")]);
        let refused = parse_linear(&json!({ "errors": [{ "message": "Entity not found" }] })).unwrap_err().to_string();
        assert_eq!(refused, "Linear refused: Entity not found");
    }

    #[test]
    fn a_picture_is_fetched_only_from_the_trackers_own_places_for_uploads() {
        let github = Remote { kind: HostKind::GitHub, host: "github.com".into(), slug: "x/shop".into() };
        let gitlab = Remote { kind: HostKind::GitLab, host: "gitlab.example.com".into(), slug: "g/sub/shop".into() };
        let plan = |remote: Option<&Remote>, src: &str| fetch_plan(remote, "", src);

        assert_eq!(
            plan(None, "https://uploads.linear.app/a/b/c"),
            Some(Fetch::Linear("https://uploads.linear.app/a/b/c".into()))
        );
        assert_eq!(
            plan(Some(&github), "https://github.com/user-attachments/assets/abc-123"),
            Some(Fetch::GitHub("https://github.com/user-attachments/assets/abc-123".into()))
        );
        assert!(plan(Some(&github), "https://github.com/x/shop/assets/1/abc").is_some());
        assert_eq!(
            plan(Some(&gitlab), "/uploads/0123456789abcdef0123456789abcdef/shot%201.png"),
            Some(Fetch::GitLab("projects/g%2Fsub%2Fshop/uploads/0123456789abcdef0123456789abcdef/shot%201.png".into()))
        );
        assert!(plan(Some(&gitlab), "https://gitlab.example.com/g/sub/shop/uploads/abc/shot.png?x=1").is_some());

        // Anywhere else, the user's sign-in is not sent: a text may name any address.
        for (remote, src) in [
            (Some(&github), "https://evil.example.com/user-attachments/assets/abc"),
            (Some(&github), "https://github.com.evil.example.com/user-attachments/assets/abc"),
            (Some(&github), "https://github.com/other/repo/assets/1/abc"),
            (Some(&gitlab), "https://evil.example.com/g/shop/uploads/abc/shot.png"),
            (Some(&gitlab), "/uploads/abc/../../../users"),
            (Some(&gitlab), "/uploads/abc/a/b.png"),
            (None, "https://uploads.linear.app.evil.example.com/a"),
            (None, "/uploads/abc/shot.png"),
            (Some(&github), "/uploads/abc/shot.png"),
        ] {
            assert_eq!(plan(remote, src), None, "{src}");
        }
    }

    #[test]
    fn only_bytes_that_begin_as_a_picture_are_handed_on() {
        assert_eq!(picture_kind(&[0x89, b'P', b'N', b'G', 0x0D]), Some("image/png"));
        assert_eq!(picture_kind(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));
        assert_eq!(picture_kind(b"GIF89a"), Some("image/gif"));
        assert_eq!(picture_kind(b"RIFF\x00\x00\x00\x00WEBPVP8 "), Some("image/webp"));
        assert_eq!(picture_kind(b"<svg xmlns='http://www.w3.org/2000/svg'><script>"), None);
        assert_eq!(picture_kind(b"<!DOCTYPE html>"), None);
        assert_eq!(picture_kind(b""), None);
    }
}
