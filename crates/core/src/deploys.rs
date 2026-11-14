//! Where a task's branch is deployed.
//!
//! A branch that is pushed is often put somewhere to look at: a preview on
//! Vercel or Netlify, a review app made by the pipeline, a storybook. The
//! host knows where. Preview shows those addresses next to what runs on this
//! computer, so "what does it look like out there" is one press away.
//!
//! - **GitHub** keeps deployments: each has an environment and a row of
//!   statuses, the newest of which says how it stands and at what address.
//!   The ones for the newest commit of the branch and the ones made for the
//!   branch by name are asked for.
//! - **GitLab** keeps environments ("review/fix-login") and deployments to
//!   them. A deployment for a merge request is made for the request's own ref,
//!   not the branch, and an environment is named after the branch as the
//!   pipeline spells it, so both are asked for: the environments whose name
//!   holds the branch, and the newest deployments for the branch or its request.
//!
//! Everything goes through `gh` and `glab`, as with requests and pipelines.
//! Nothing is deployed, stopped or changed from here.

use crate::hosts::{HostKind, Hosts, Remote};
use crate::tasks::Manager;
use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;
use serde_json::{Value, json};
use std::sync::Arc;

/// How many places are shown for one branch.
const MOST: usize = 8;

/// One place a branch is deployed to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Deploy {
    /// What the host calls the place: "Preview", "review/fix-login".
    pub name: String,
    /// Where it is. Empty when the host knows of the place and gave no address.
    pub url: String,
    /// "ready", "going" (being deployed now), "failed", or "known" when the host says only that the place exists.
    pub state: &'static str,
    /// When it was deployed, in milliseconds. Zero when the host did not say.
    pub at: u64,
    /// The commit that is there, shortened. Empty when the host did not say.
    pub commit: String,
}

fn text(item: &Value, name: &str) -> String {
    item.get(name).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn short(sha: &str) -> String {
    sha.chars().take(8).collect()
}

/// An address the interface can put behind a link: one of the web. A host can be made to say anything here.
fn web(address: String) -> String {
    let ok = address.starts_with("https://") || address.starts_with("http://");
    if ok && !address.contains(char::is_whitespace) { address } else { String::new() }
}

/// A piece of an address, with everything that would mean something else made plain.
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

/// A branch as GitLab spells it for the names of things (its `CI_COMMIT_REF_SLUG`): small letters and
/// digits, everything else a dash, sixty-three signs at most, no dash at either end.
pub fn ref_slug(branch: &str) -> String {
    let dashed: String =
        branch.to_lowercase().chars().map(|sign| if sign.is_ascii_alphanumeric() { sign } else { '-' }).collect();
    dashed.chars().take(63).collect::<String>().trim_matches('-').to_string()
}

fn github_state(status: &str) -> Option<&'static str> {
    match status {
        "success" => Some("ready"),
        "failure" | "error" => Some("failed"),
        "in_progress" | "queued" | "pending" | "waiting" => Some("going"),
        // "inactive": something newer took its place.
        _ => None,
    }
}

fn gitlab_state(status: &str) -> Option<&'static str> {
    match status {
        "success" => Some("ready"),
        "failed" => Some("failed"),
        "created" | "running" | "blocked" => Some("going"),
        // "canceled", "skipped": nothing was put there.
        _ => None,
    }
}

/// The deployments of GitHub that are worth asking further about: the newest of each environment.
/// Gives their numbers with the environment and the commit of each.
pub fn newest_github(lists: &[&str]) -> Result<Vec<(u64, String, String, u64)>> {
    let mut all: Vec<Value> = Vec::new();
    for list in lists {
        let more: Vec<Value> =
            serde_json::from_str(list).context("GitHub's answer about the deployments could not be read")?;
        all.extend(more);
    }
    // Newest first, whichever question it came from.
    all.sort_by_key(|one| std::cmp::Reverse(one.get("id").and_then(Value::as_u64).unwrap_or(0)));
    let mut newest: Vec<(u64, String, String, u64)> = Vec::new();
    for one in all {
        let (Some(id), environment) = (one.get("id").and_then(Value::as_u64), text(&one, "environment")) else {
            continue;
        };
        if newest.iter().any(|(_, known, _, _)| *known == environment) {
            continue;
        }
        let at = crate::hosts::parse_time(&text(&one, "created_at"));
        newest.push((id, environment, short(&text(&one, "sha")), at));
    }
    newest.truncate(MOST);
    Ok(newest)
}

/// How one deployment of GitHub stands: its newest status, as `gh api …/statuses` gives it.
pub fn github_status(json: &str) -> Result<Option<(&'static str, String)>> {
    let list: Vec<Value> =
        serde_json::from_str(json).context("GitHub's answer about a deployment could not be read")?;
    let Some(newest) = list.first() else { return Ok(Some(("going", String::new()))) };
    Ok(github_state(&text(newest, "state")).map(|state| (state, web(text(newest, "environment_url")))))
}

/// The places of GitLab a branch is deployed to: the environments found by name, joined with the newest
/// deployments that were made for the branch or for its merge request.
pub fn parse_gitlab(environments: &str, deployments: &str, branch: &str, request: Option<u64>) -> Result<Vec<Deploy>> {
    let read = |json: &str, what: &str| -> Result<Vec<Value>> {
        serde_json::from_str(json).with_context(|| format!("GitLab's answer about the {what} could not be read"))
    };
    let of_request = request.map(|number| format!("refs/merge-requests/{number}/"));
    let mine = |one: &Value| {
        let made_for = text(one, "ref");
        made_for == branch || of_request.as_ref().is_some_and(|start| made_for.starts_with(start))
    };
    let mut found: Vec<(u64, Deploy)> = Vec::new();
    // Newest first, as asked: the first deployment met for an environment is how it stands now.
    for one in read(deployments, "deployments")?.iter().filter(|one| mine(one)) {
        let Some(environment) = one.get("environment") else { continue };
        let id = environment.get("id").and_then(Value::as_u64).unwrap_or(0);
        if found.iter().any(|(known, _)| *known == id) {
            continue;
        }
        let Some(state) = gitlab_state(&text(one, "status")) else { continue };
        found.push((
            id,
            Deploy {
                name: text(environment, "name"),
                url: web(text(environment, "external_url")),
                state,
                at: crate::hosts::parse_time(&text(one, "created_at")),
                commit: short(&text(one, "sha")),
            },
        ));
    }
    // An environment of that name with no deployment among the newest: it is there, and that is all that is known.
    for one in read(environments, "environments")? {
        let id = one.get("id").and_then(Value::as_u64).unwrap_or(0);
        let url = web(text(&one, "external_url"));
        if found.iter().any(|(known, _)| *known == id) || url.is_empty() {
            continue;
        }
        found.push((id, Deploy { name: text(&one, "name"), url, state: "known", at: 0, commit: String::new() }));
    }
    let mut deploys: Vec<Deploy> = found.into_iter().map(|(_, deploy)| deploy).collect();
    deploys.truncate(MOST);
    Ok(deploys)
}

impl Hosts {
    /// Where a branch is deployed, as its host knows. `commit` is the newest commit of the branch on
    /// this computer; `request` the number of its merge request, when it has one.
    pub fn deploys(
        &self,
        remote: &Remote,
        branch: &str,
        commit: Option<&str>,
        request: Option<u64>,
    ) -> Result<Vec<Deploy>> {
        match remote.kind {
            HostKind::GitHub => {
                let ask = |question: String| {
                    self.run_at(
                        HostKind::GitHub,
                        Some(&remote.host),
                        &["api", &format!("repos/{}/{question}", remote.slug)],
                    )
                };
                // A service that deploys every commit names the commit, not the branch: both are asked for.
                let mut lists = vec![ask(format!("deployments?ref={}&per_page=20", in_path(branch)))?];
                if let Some(commit) = commit.filter(|commit| commit.bytes().all(|byte| byte.is_ascii_hexdigit())) {
                    lists.push(ask(format!("deployments?sha={commit}&per_page=20"))?);
                }
                let lists: Vec<&str> = lists.iter().map(String::as_str).collect();
                let mut deploys = Vec::new();
                for (id, name, commit, at) in newest_github(&lists)? {
                    let Some((state, url)) = github_status(&ask(format!("deployments/{id}/statuses?per_page=1"))?)?
                    else {
                        continue;
                    };
                    deploys.push(Deploy { name, url, state, at, commit });
                }
                Ok(deploys)
            }
            HostKind::GitLab => {
                let ask = |question: String| {
                    let endpoint = format!("projects/{}/{question}", in_path(&remote.slug));
                    self.run_at(HostKind::GitLab, Some(&remote.host), &["api", &endpoint])
                };
                let slug = ref_slug(branch);
                // GitLab looks for names of three signs and more.
                let environments = match slug.len() >= 3 {
                    true => ask(format!("environments?states=available&per_page=20&search={}", in_path(&slug)))?,
                    false => "[]".to_string(),
                };
                let deployments = ask("deployments?order_by=created_at&sort=desc&per_page=100".to_string())?;
                parse_gitlab(&environments, &deployments, branch, request)
            }
        }
    }
}

impl Manager {
    /// Where a task's branch is deployed. A task with no branch, or on neither host, has no such place:
    /// that is an answer, not a failure.
    pub fn deploys_of(&self, task_id: &str) -> Result<Value> {
        let Ok((_, root, branch, remote)) = self.request_place(task_id) else {
            return Ok(json!({ "host": Value::Null, "branch": Value::Null, "deploys": [] }));
        };
        let commit = crate::gitops::head_commit(&root);
        let deploys = self
            .hosts
            .deploys(&remote, &branch, commit.as_deref(), self.request_number(task_id))
            .map_err(|error| crate::hosts::said_plainly(&remote, error))?;
        Ok(json!({ "host": remote.kind.name(), "branch": branch, "deploys": deploys }))
    }
}

/// The command of the interface: `deploys.get { taskId }` gives
/// `{ host, branch, deploys: [{ name, url, state, at, commit }] }`.
pub async fn command(manager: &Arc<Manager>, method: &str, params: &Value) -> Result<Value> {
    if method != "deploys.get" {
        bail!("Unknown method: {method}");
    }
    let task_id =
        params.get("taskId").and_then(Value::as_str).ok_or_else(|| anyhow!("Missing parameter: taskId"))?.to_string();
    let manager = manager.clone();
    tokio::task::spawn_blocking(move || manager.deploys_of(&task_id)).await?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_branch_is_spelled_the_way_gitlab_names_things_after_it() {
        assert_eq!(ref_slug("fix/APP-12706_employee.attribute"), "fix-app-12706-employee-attribute");
        assert_eq!(ref_slug("/Ünïcode/"), "n-code");
        assert_eq!(ref_slug(&"a".repeat(80)).len(), 63);
        assert_eq!(ref_slug(&format!("{}/x", "a".repeat(62))), "a".repeat(62), "no dash is left at the end of the cut");
    }

    #[test]
    fn the_newest_deployment_of_each_environment_is_the_one_asked_about() {
        let by_branch = r#"[
            {"id":11,"environment":"Preview","sha":"aaaaaaaaaaaa","created_at":"2026-11-10T09:00:00Z"},
            {"id":14,"environment":"Preview","sha":"bbbbbbbbbbbb","created_at":"2026-11-12T09:00:00Z"}
        ]"#;
        let by_commit = r#"[
            {"id":14,"environment":"Preview","sha":"bbbbbbbbbbbb","created_at":"2026-11-12T09:00:00Z"},
            {"id":13,"environment":"storybook","sha":"bbbbbbbbbbbb","created_at":"2026-11-12T08:00:00Z"}
        ]"#;
        let newest = newest_github(&[by_branch, by_commit]).unwrap();
        let told: Vec<(u64, &str, &str)> =
            newest.iter().map(|(id, name, sha, _)| (*id, name.as_str(), sha.as_str())).collect();
        assert_eq!(told, [(14, "Preview", "bbbbbbbb"), (13, "storybook", "bbbbbbbb")]);
        assert!(newest_github(&["[]"]).unwrap().is_empty());
        assert!(newest_github(&["not json"]).is_err());
    }

    #[test]
    fn a_deployment_of_github_stands_as_its_newest_status_says() {
        let ready =
            r#"[{"state":"success","environment_url":"https://shop-git-fix.vercel.app"},{"state":"in_progress"}]"#;
        assert_eq!(github_status(ready).unwrap(), Some(("ready", "https://shop-git-fix.vercel.app".to_string())));
        assert_eq!(
            github_status(r#"[{"state":"failure","environment_url":""}]"#).unwrap(),
            Some(("failed", String::new()))
        );
        // No status yet: it was only just made.
        assert_eq!(github_status("[]").unwrap(), Some(("going", String::new())));
        // Something newer took its place: it is not a place any more.
        assert_eq!(github_status(r#"[{"state":"inactive","environment_url":"https://old"}]"#).unwrap(), None);
        // What is no address of the web is not put behind a link.
        let odd = r#"[{"state":"success","environment_url":"javascript:alert(1)"}]"#;
        assert_eq!(github_status(odd).unwrap(), Some(("ready", String::new())));
    }

    #[test]
    fn the_places_of_gitlab_are_the_ones_made_for_the_branch_or_its_request() {
        // As a real project answers: a deployment for a merge request names the request's own ref.
        let deployments = r#"[
            {"id":9,"ref":"refs/merge-requests/3256/merge","sha":"cccccccccccc","status":"success","created_at":"2026-11-13T11:16:28.825Z",
             "environment":{"id":201,"name":"storybook/fix-app-12706","external_url":"https://3256-ui.preview.example.com/"}},
            {"id":8,"ref":"refs/merge-requests/3256/merge","sha":"cccccccccccc","status":"running","created_at":"2026-11-13T11:15:46.316Z",
             "environment":{"id":200,"name":"review/fix-app-12706","external_url":"https://3256.preview.example.com"}},
            {"id":7,"ref":"refs/merge-requests/3256/merge","sha":"bbbbbbbbbbbb","status":"success","created_at":"2026-11-12T10:00:00Z",
             "environment":{"id":200,"name":"review/fix-app-12706","external_url":"https://3256.preview.example.com"}},
            {"id":6,"ref":"refs/merge-requests/1894/merge","sha":"dddddddddddd","status":"success","created_at":"2026-11-13T10:57:16Z",
             "environment":{"id":150,"name":"review/renovate","external_url":"https://1894.preview.example.com"}},
            {"id":5,"ref":"fix/app-12706","sha":"aaaaaaaaaaaa","status":"skipped","created_at":"2026-11-11T10:00:00Z",
             "environment":{"id":202,"name":"linear/fix-app-12706","external_url":"https://linear.app/x/issue/APP-12706"}},
            {"id":4,"ref":"main","sha":"eeeeeeeeeeee","status":"success","created_at":"2026-11-13T17:26:27Z",
             "environment":{"id":64,"name":"main","external_url":"https://dev.example.com"}}
        ]"#;
        // Found by the branch's name: one that is among the deployments already, one that is not, one without an address.
        let environments = r#"[
            {"id":200,"name":"review/fix-app-12706","external_url":"https://3256.preview.example.com","state":"available"},
            {"id":202,"name":"linear/fix-app-12706","external_url":"https://linear.app/x/issue/APP-12706","state":"available"},
            {"id":203,"name":"docs/fix-app-12706","external_url":null,"state":"available"}
        ]"#;
        let deploys = parse_gitlab(environments, deployments, "fix/app-12706", Some(3256)).unwrap();
        let told: Vec<(&str, &str, &str, &str)> =
            deploys.iter().map(|one| (one.name.as_str(), one.state, one.url.as_str(), one.commit.as_str())).collect();
        assert_eq!(
            told,
            [
                ("storybook/fix-app-12706", "ready", "https://3256-ui.preview.example.com/", "cccccccc"),
                // Its newest deployment is the one that counts, and that one is still going.
                ("review/fix-app-12706", "going", "https://3256.preview.example.com", "cccccccc"),
                // Skipped the last time, so nothing says how it stands: it is there, by its name.
                ("linear/fix-app-12706", "known", "https://linear.app/x/issue/APP-12706", ""),
            ]
        );
        assert_eq!(deploys[0].at, 1_794_568_588_000);
        // Without a request only what was made for the branch itself counts.
        let alone = parse_gitlab("[]", deployments, "fix/app-12706", None).unwrap();
        assert!(alone.is_empty(), "{alone:?}");
        assert!(parse_gitlab("[]", "{}", "x", None).is_err());
    }
}
