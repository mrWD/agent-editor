//! Errors of the running app, from Sentry, as work for an agent.
//!
//! Sentry groups the errors an app sends it into issues. A project of the
//! editor can be joined to a project of Sentry; its unresolved issues are then
//! listed on the Issues screen under the tracker's, the most frequent first.
//! "Start" makes a task of one. Its first message is what a person would look
//! up before starting: how often it happens and since when, in which release
//! and environment, and the newest event itself: the exception, the frames of
//! the app's own code with the lines around each, the request it happened in
//! and the last things the app did before it.
//!
//! Sentry is only read. An issue is not resolved from here: Sentry does that
//! itself when a commit that says "Fixes SHOP-1A" reaches it, which is what
//! the merge request of such a task says by default.
//!
//! The token is kept with the other settings on this computer. It needs to
//! read events and projects (`event:read`, `project:read`, `org:read`).

use crate::store::TaskIssue;
use crate::tasks::{Manager, TaskInfo};
use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

const TOKEN: &str = "sentry.token";
const ORG: &str = "sentry.org";
const URL: &str = "sentry.url";
/// Where Sentry is when the user did not say: the one Sentry itself runs.
const SENTRY_IO: &str = "https://sentry.io";
const PATIENCE: Duration = Duration::from_secs(20);
/// How many issues of a project are listed.
const MOST_ISSUES: usize = 25;
/// How many frames of the app's own code go into a message, counted from where it broke.
const MOST_FRAMES: usize = 12;
/// How many of the last things the app did go into a message.
const MOST_CRUMBS: usize = 8;
/// The longest line of code that is quoted. A minified line runs to thousands of signs.
const LONGEST_LINE: usize = 200;

fn project_setting(project_id: &str) -> String {
    format!("project.{project_id}.sentry")
}

/// Whether text can be the name Sentry gives an organization or a project in an address.
fn is_slug(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 100
        && text.chars().all(|sign| sign.is_ascii_alphanumeric() || "-_.".contains(sign))
}

/// The address of a Sentry, tidied: https, or http on this computer for one of the user's own.
fn tidy_url(text: &str) -> Result<String> {
    let url = text.trim().trim_end_matches('/');
    if url.is_empty() {
        return Ok(SENTRY_IO.to_string());
    }
    let local = ["http://127.0.0.1", "http://localhost"].iter().any(|start| url.starts_with(start));
    let rest = url.strip_prefix("https://").unwrap_or_default();
    if !(local || !rest.is_empty() && !rest.starts_with('/')) || url.contains(char::is_whitespace) {
        bail!("Give the address of your Sentry, beginning with https://, or leave it empty for sentry.io");
    }
    Ok(url.to_string())
}

fn text(item: &Value, name: &str) -> String {
    item.get(name).and_then(Value::as_str).unwrap_or_default().to_string()
}

/// A number Sentry may write as a number or as text.
fn count(item: &Value, name: &str) -> u64 {
    let value = item.get(name);
    value
        .and_then(Value::as_u64)
        .or_else(|| value.and_then(Value::as_str).and_then(|text| text.parse().ok()))
        .unwrap_or(0)
}

/// "2026-11-13" out of a time as Sentry writes it.
fn day(time: &str) -> &str {
    time.get(..10).unwrap_or(time)
}

/// One line of code, short enough to quote.
fn line_of(code: &str) -> String {
    let code = code.trim_end();
    match code.char_indices().nth(LONGEST_LINE) {
        Some((cut, _)) => format!("{}…", &code[..cut]),
        None => code.to_string(),
    }
}

/// An issue as the list shows it.
pub fn listed(issue: &Value) -> Option<Value> {
    let id = text(issue, "id");
    if id.is_empty() || !id.chars().all(|digit| digit.is_ascii_digit()) {
        return None;
    }
    let url = text(issue, "permalink");
    Some(json!({
        "id": id,
        "key": text(issue, "shortId"),
        "title": text(issue, "title"),
        "culprit": text(issue, "culprit"),
        "level": text(issue, "level"),
        "count": count(issue, "count"),
        "users": count(issue, "userCount"),
        "firstSeen": crate::hosts::parse_time(&text(issue, "firstSeen")),
        "lastSeen": crate::hosts::parse_time(&text(issue, "lastSeen")),
        // Only an address of the web goes behind a link.
        "url": if url.starts_with("https://") || url.starts_with("http://") { url } else { String::new() },
    }))
}

/// The frames of one exception as lines of a message: where it broke first, the app's own code with
/// the lines around each place, and a count of what was the libraries'.
fn frames_of(exception: &Value) -> Vec<String> {
    let frames = exception.pointer("/stacktrace/frames").and_then(Value::as_array).cloned().unwrap_or_default();
    // Sentry lists the oldest call first; a person reads from where it broke.
    let newest_first: Vec<&Value> = frames.iter().rev().collect();
    let own = |frame: &&Value| frame.get("inApp").and_then(Value::as_bool) == Some(true);
    // An event without a single frame of the app's own (a minified bundle with no source map) shows what there is.
    let any_own = newest_first.iter().any(own);
    let shown: Vec<&Value> =
        newest_first.iter().filter(|frame| !any_own || own(frame)).take(MOST_FRAMES).copied().collect();
    let mut lines = Vec::new();
    for frame in &shown {
        let file = [text(frame, "filename"), text(frame, "module"), text(frame, "absPath")]
            .into_iter()
            .find(|name| !name.is_empty())
            .unwrap_or_else(|| "(unknown file)".into());
        let line = frame.get("lineNo").and_then(Value::as_u64);
        let place = match (line, frame.get("colNo").and_then(Value::as_u64)) {
            (Some(line), Some(column)) => format!("{file}:{line}:{column}"),
            (Some(line), None) => format!("{file}:{line}"),
            _ => file,
        };
        let function = text(frame, "function");
        lines.push(if function.is_empty() { format!("  at {place}") } else { format!("  at {function} ({place})") });
        // The lines around the place, with the one it broke on marked.
        for pair in frame.get("context").and_then(Value::as_array).into_iter().flatten() {
            let (Some(number), Some(code)) = (pair.get(0).and_then(Value::as_u64), pair.get(1).and_then(Value::as_str))
            else {
                continue;
            };
            // Two lines either side are enough to find the place; the file itself is in the repository.
            if line.is_some_and(|line| number.abs_diff(line) <= 2) {
                let mark = if Some(number) == line { ">" } else { " " };
                // An empty line is told by its number alone, with nothing hanging after the bar.
                lines.push(format!("      {mark} {number:>5} | {}", line_of(code)).trim_end().to_string());
            }
        }
    }
    let left_out = frames.len() - shown.len();
    if left_out > 0 {
        let whose = if any_own { " of libraries and of the app" } else { "" };
        lines.push(format!("  ({left_out} more {}{whose} left out)", if left_out == 1 { "frame" } else { "frames" }));
    }
    lines
}

/// What an agent is told about an issue: Sentry's numbers, then the newest event.
pub fn first_message(issue: &Value, event: Option<&Value>) -> String {
    let title = text(issue, "title");
    let key = text(issue, "shortId");
    let mut out = format!("{title}\n\n");
    let (times, users) = (count(issue, "count"), count(issue, "userCount"));
    out.push_str(&format!(
        "This is Sentry issue {key} ({}). It happened {} {}{}, first on {} and last on {}.",
        text(issue, "permalink"),
        times,
        if times == 1 { "time" } else { "times" },
        match users {
            0 => String::new(),
            1 => " to one user".into(),
            users => format!(" to {users} users"),
        },
        day(&text(issue, "firstSeen")),
        day(&text(issue, "lastSeen")),
    ));
    let culprit = text(issue, "culprit");
    if !culprit.is_empty() {
        out.push_str(&format!(" Sentry names {culprit} as where it comes from."));
    }
    out.push('\n');

    let Some(event) = event else {
        out.push_str("\nSentry gave no event for it, so there is no stack trace here. Open the issue to read one.\n");
        out.push_str(CLOSING);
        return out;
    };
    let tag = |name: &str| {
        let tags = event.get("tags").and_then(Value::as_array).cloned().unwrap_or_default();
        tags.iter().find(|tag| tag["key"] == name).map(|tag| text(tag, "value")).filter(|value| !value.is_empty())
    };
    let known: Vec<String> = [
        ("release", "Release"),
        ("environment", "Environment"),
        ("browser", "Browser"),
        ("os", "System"),
        ("transaction", "Transaction"),
    ]
    .into_iter()
    .filter_map(|(name, word)| tag(name).map(|value| format!("{word}: {value}")))
    .collect();
    if !known.is_empty() {
        out.push_str(&format!("{}.\n", known.join(". ")));
    }
    let entries = event.get("entries").and_then(Value::as_array).cloned().unwrap_or_default();
    let entry = |kind: &str| entries.iter().find(|entry| entry["type"] == kind).map(|entry| entry["data"].clone());
    if let Some(request) = entry("request") {
        let (method, url) = (text(&request, "method"), text(&request, "url"));
        if !url.is_empty() {
            out.push_str(&format!("Request: {} {url}\n", if method.is_empty() { "GET".into() } else { method }));
        }
    }

    out.push_str("\nThe newest event:\n\n");
    let exceptions =
        entry("exception").and_then(|data| data.get("values").and_then(Value::as_array).cloned()).unwrap_or_default();
    if exceptions.is_empty() {
        let message = [text(event, "message"), text(event, "title")].into_iter().find(|said| !said.is_empty());
        out.push_str(&format!(
            "{}\n(The event carries no exception, so there is no stack trace.)\n",
            message.unwrap_or_default()
        ));
    }
    // A chain of exceptions is listed with the cause first; the one that was thrown last is what the user saw.
    for (index, exception) in exceptions.iter().rev().enumerate() {
        if index > 0 {
            out.push_str("\nCaused by:\n");
        }
        out.push_str(&format!("{}: {}\n", text(exception, "type"), text(exception, "value")));
        for line in frames_of(exception) {
            out.push_str(&line);
            out.push('\n');
        }
    }

    let crumbs =
        entry("breadcrumbs").and_then(|data| data.get("values").and_then(Value::as_array).cloned()).unwrap_or_default();
    let told: Vec<String> = crumbs
        .iter()
        .rev()
        .take(MOST_CRUMBS)
        .rev()
        .filter_map(|crumb| {
            let said = match (text(crumb, "message"), crumb.get("data")) {
                (message, _) if !message.is_empty() => message,
                // A request the app made says itself in its data: the method, the address, how it ended.
                (_, Some(data)) if data.is_object() => {
                    let piece =
                        |name: &str| data.get(name).map(|value| value.to_string().trim_matches('"').to_string());
                    [piece("method"), piece("url"), piece("status_code")]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(" ")
                }
                _ => String::new(),
            };
            (!said.trim().is_empty()).then(|| format!("  {}: {}", text(crumb, "category"), line_of(&said)))
        })
        .collect();
    if !told.is_empty() {
        out.push_str("\nThe last things the app did before it, oldest first:\n");
        out.push_str(&told.join("\n"));
        out.push('\n');
    }
    out.push_str(CLOSING);
    out
}

const CLOSING: &str = "\nFind the cause in this repository and fix it. Say what the cause was, and what you could not tell from the code alone.";

impl Manager {
    fn sentry_place(&self) -> Option<(String, String, String)> {
        let kept = |key: &str| self.store().setting(key).ok().flatten().filter(|value| !value.is_empty());
        Some((kept(URL).unwrap_or_else(|| SENTRY_IO.into()), kept(ORG)?, kept(TOKEN)?))
    }

    /// Asks Sentry one thing and gives the JSON it answered. What it refuses is said in words.
    fn sentry_get(&self, base: &str, token: &str, path: &str) -> Result<Value> {
        let mut response = ureq::get(&format!("{base}/api/0/{path}"))
            .header("Authorization", &format!("Bearer {token}"))
            .config()
            .timeout_global(Some(PATIENCE))
            .http_status_as_error(false)
            .build()
            .call()
            .map_err(|error| anyhow!("Sentry could not be reached: {error}"))?;
        match response.status().as_u16() {
            200..=299 => response.body_mut().read_json().map_err(|_| anyhow!("Sentry's answer could not be read")),
            401 => bail!("Sentry did not accept the token. Make a new one under User settings, Auth tokens."),
            403 => bail!("The token may not read that. It needs event:read, project:read and org:read."),
            404 => bail!("Sentry knows nothing by that name. Check the organization and the project."),
            other => bail!("Sentry answered with an error ({other})"),
        }
    }

    /// What is connected: no token is ever said back, only that there is one.
    pub fn sentry_status(&self) -> Value {
        let kept = |key: &str| self.store().setting(key).ok().flatten().unwrap_or_default();
        let mut projects = serde_json::Map::new();
        for project in self.store().projects().unwrap_or_default() {
            if let Some(slug) = self.store().setting(&project_setting(&project.id)).ok().flatten() {
                projects.insert(project.id, json!(slug));
            }
        }
        json!({
            "connected": self.sentry_place().is_some(),
            "url": if kept(URL).is_empty() { SENTRY_IO.to_string() } else { kept(URL) },
            "org": kept(ORG),
            "projects": projects
        })
    }

    /// Connects Sentry: the token is tried on the organization before it is kept. An empty token disconnects.
    pub fn sentry_connect(&self, token: &str, org: &str, url: &str) -> Result<()> {
        let (token, org) = (token.trim(), org.trim());
        if token.is_empty() {
            for key in [TOKEN, ORG, URL] {
                self.store().set_setting(key, None)?;
            }
            self.broadcast("integrations.changed", json!({}));
            return Ok(());
        }
        if !is_slug(org) {
            bail!("Give the organization as it stands in Sentry's address: sentry.io/organizations/<this>/");
        }
        if token.contains(char::is_whitespace) || token.len() > 400 {
            bail!("That is not a token of Sentry");
        }
        let url = tidy_url(url)?;
        self.sentry_get(&url, token, &format!("organizations/{org}/"))?;
        self.store().set_setting(TOKEN, Some(token))?;
        self.store().set_setting(ORG, Some(org))?;
        self.store().set_setting(URL, (url != SENTRY_IO).then_some(url.as_str()))?;
        self.broadcast("integrations.changed", json!({}));
        Ok(())
    }

    /// The projects of the organization, for picking one: `[{ slug, name }]`.
    pub fn sentry_projects(&self) -> Result<Vec<Value>> {
        let (url, org, token) = self.sentry_place().ok_or_else(|| anyhow!("Sentry is not connected yet"))?;
        let found = self.sentry_get(&url, &token, &format!("organizations/{org}/projects/?per_page=100"))?;
        let list = found.as_array().cloned().unwrap_or_default();
        Ok(list
            .iter()
            .map(|one| json!({ "slug": text(one, "slug"), "name": text(one, "name") }))
            .filter(|one| one["slug"] != "")
            .collect())
    }

    /// Joins a project of the editor to a project of Sentry; an empty name parts them.
    pub fn sentry_join(&self, project_id: &str, slug: &str) -> Result<()> {
        self.store().project(project_id)?;
        let slug = slug.trim();
        if !slug.is_empty() && !is_slug(slug) {
            bail!("Give the project as it stands in Sentry's address: letters, digits and dashes");
        }
        self.store().set_setting(&project_setting(project_id), (!slug.is_empty()).then_some(slug))?;
        self.broadcast("integrations.changed", json!({}));
        Ok(())
    }

    /// The unresolved issues of the Sentry project a project is joined to, the most frequent first,
    /// each with the task that works on it.
    pub fn sentry_issues(&self, project_id: &str) -> Result<Value> {
        self.store().project(project_id)?;
        let Some(slug) = self.store().setting(&project_setting(project_id))? else {
            return Ok(json!({ "slug": Value::Null, "issues": [] }));
        };
        let (url, org, token) = self.sentry_place().ok_or_else(|| anyhow!("Sentry is not connected yet"))?;
        let path = format!("projects/{org}/{slug}/issues/?query=is%3Aunresolved&sort=freq&limit={MOST_ISSUES}");
        let found = self.sentry_get(&url, &token, &path)?;
        let tasks = self.store().tasks()?;
        let issues: Vec<Value> = found
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(listed)
            .map(|mut issue| {
                let mine = tasks.iter().find(|task| {
                    task.project_id == project_id
                        && task.issue.as_ref().is_some_and(|known| {
                            known.source == "sentry" && known.id == issue["id"].as_str().unwrap_or_default()
                        })
                });
                issue["taskId"] = json!(mine.map(|task| task.id.clone()));
                issue
            })
            .collect();
        Ok(json!({ "slug": slug, "issues": issues }))
    }

    /// Starts a task for an issue of Sentry. Its first message is the issue with its newest event.
    pub async fn sentry_start(self: &Arc<Self>, project_id: &str, issue_id: &str, agent_id: &str) -> Result<TaskInfo> {
        if issue_id.is_empty() || !issue_id.chars().all(|digit| digit.is_ascii_digit()) {
            bail!("That is not the number of an issue of Sentry");
        }
        let already = self.store().tasks()?.into_iter().any(|task| {
            task.project_id == project_id
                && task.issue.as_ref().is_some_and(|issue| issue.source == "sentry" && issue.id == issue_id)
        });
        if already {
            bail!("A task for that issue of Sentry is already open");
        }
        let (manager, id) = (self.clone(), issue_id.to_string());
        let (issue, event) = tokio::task::spawn_blocking(move || -> Result<(Value, Option<Value>)> {
            let (url, org, token) = manager.sentry_place().ok_or_else(|| anyhow!("Sentry is not connected yet"))?;
            let issue = manager.sentry_get(&url, &token, &format!("organizations/{org}/issues/{id}/"))?;
            // An issue without an event that can still be read is told without one.
            let event =
                manager.sentry_get(&url, &token, &format!("organizations/{org}/issues/{id}/events/latest/")).ok();
            Ok((issue, event))
        })
        .await??;
        let attached = TaskIssue {
            key: text(&issue, "shortId"),
            id: issue_id.to_string(),
            url: text(&issue, "permalink"),
            title: text(&issue, "title"),
            source: "sentry".into(),
        };
        self.create_for(project_id, agent_id, &first_message(&issue, event.as_ref()), Some(attached)).await
    }
}

/// The commands of the interface that are about Sentry.
///
/// - `sentry.status` gives `{ connected, url, org, projects: { <projectId>: slug } }`.
/// - `sentry.connect { token, org, url? }` tries the token and keeps it; an empty token disconnects.
/// - `sentry.projects` gives `{ projects: [{ slug, name }] }` of the organization.
/// - `sentry.join { projectId, slug }` joins a project to one of Sentry's; an empty slug parts them.
/// - `sentry.issues { projectId }` gives `{ slug, issues: [{ id, key, title, culprit, level, count,
///   users, firstSeen, lastSeen, url, taskId }] }`.
/// - `sentry.start { projectId, issueId, agentId }` starts the task and gives it.
pub async fn command(manager: &Arc<Manager>, method: &str, params: &Value) -> Result<Value> {
    let word = |name: &str| params.get(name).and_then(Value::as_str).unwrap_or_default().to_string();
    let needed = |name: &str| {
        params.get(name).and_then(Value::as_str).map(str::to_string).ok_or_else(|| anyhow!("Missing parameter: {name}"))
    };
    let asking = manager.clone();
    match method {
        "sentry.status" => {}
        "sentry.connect" => {
            let (token, org, url) = (word("token"), word("org"), word("url"));
            tokio::task::spawn_blocking(move || asking.sentry_connect(&token, &org, &url)).await??;
        }
        "sentry.projects" => {
            let projects = tokio::task::spawn_blocking(move || asking.sentry_projects()).await??;
            return Ok(json!({ "projects": projects }));
        }
        "sentry.join" => manager.sentry_join(&needed("projectId")?, &word("slug"))?,
        "sentry.issues" => {
            let project = needed("projectId")?;
            return tokio::task::spawn_blocking(move || asking.sentry_issues(&project)).await?;
        }
        "sentry.start" => {
            let task = manager.sentry_start(&needed("projectId")?, &needed("issueId")?, &needed("agentId")?).await?;
            return Ok(json!(task));
        }
        other => bail!("Unknown method: {other}"),
    }
    Ok(manager.sentry_status())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue() -> Value {
        json!({
            "id": "4001", "shortId": "SHOP-1A", "title": "TypeError: Cannot read properties of undefined (reading 'id')",
            "culprit": "submit(src/checkout/submit)", "level": "error", "count": "1204", "userCount": 87,
            "firstSeen": "2026-11-01T08:00:00Z", "lastSeen": "2026-11-13T17:40:12.5Z",
            "permalink": "https://acme.sentry.io/issues/4001/"
        })
    }

    #[test]
    fn an_issue_is_listed_with_its_numbers_whether_sentry_writes_them_as_text_or_not() {
        let one = listed(&issue()).unwrap();
        assert_eq!(
            (one["key"].as_str(), one["count"].as_u64(), one["users"].as_u64()),
            (Some("SHOP-1A"), Some(1204), Some(87))
        );
        assert_eq!(one["lastSeen"], 1_794_591_612_000_u64);
        assert_eq!(one["url"], "https://acme.sentry.io/issues/4001/");
        // What is no number is no issue, and what is no address of the web is no link.
        assert!(listed(&json!({ "id": "../x", "title": "t" })).is_none());
        assert_eq!(listed(&json!({ "id": "5", "permalink": "javascript:alert(1)" })).unwrap()["url"], "");
    }

    #[test]
    fn the_first_message_tells_the_numbers_then_the_event_from_where_it_broke() {
        let event = json!({
            "tags": [{ "key": "release", "value": "1.4.2" }, { "key": "environment", "value": "production" }, { "key": "handled", "value": "no" }],
            "entries": [
                { "type": "exception", "data": { "values": [
                    { "type": "NetworkError", "value": "fetch failed", "stacktrace": { "frames": [
                        { "filename": "node_modules/undici/index.js", "function": "fetch", "lineNo": 9, "inApp": false }
                    ] } },
                    { "type": "TypeError", "value": "Cannot read properties of undefined (reading 'id')", "stacktrace": { "frames": [
                        { "filename": "node_modules/express/lib/router.js", "function": "handle", "lineNo": 280, "colNo": 7, "inApp": false },
                        { "filename": "src/routes/checkout.ts", "function": "handler", "lineNo": 12, "inApp": true },
                        { "filename": "src/checkout/submit.ts", "function": "submit", "lineNo": 42, "colNo": 17, "inApp": true,
                          "context": [[39, "export async function submit() {"], [40, "  const cart = await load();"], [41, ""],
                                      [42, "  return cart.user.id;"], [43, "}"], [44, ""], [45, "// far below"]] }
                    ] } }
                ] } },
                { "type": "breadcrumbs", "data": { "values": [
                    { "category": "console", "message": "cart loaded" },
                    { "category": "xhr", "message": null, "data": { "method": "POST", "url": "/api/checkout", "status_code": 500 } },
                    { "category": "ui.click", "message": "" }
                ] } },
                { "type": "request", "data": { "method": "POST", "url": "https://shop.example.com/checkout" } }
            ]
        });
        let message = first_message(&issue(), Some(&event));
        let expected = "TypeError: Cannot read properties of undefined (reading 'id')

This is Sentry issue SHOP-1A (https://acme.sentry.io/issues/4001/). It happened 1204 times to 87 users, first on 2026-11-01 and last on 2026-11-13. Sentry names submit(src/checkout/submit) as where it comes from.
Release: 1.4.2. Environment: production.
Request: POST https://shop.example.com/checkout

The newest event:

TypeError: Cannot read properties of undefined (reading 'id')
  at submit (src/checkout/submit.ts:42:17)
           40 |   const cart = await load();
           41 |
      >    42 |   return cart.user.id;
           43 | }
           44 |
  at handler (src/routes/checkout.ts:12)
  (1 more frame of libraries and of the app left out)

Caused by:
NetworkError: fetch failed
  at fetch (node_modules/undici/index.js:9)

The last things the app did before it, oldest first:
  console: cart loaded
  xhr: POST /api/checkout 500

Find the cause in this repository and fix it. Say what the cause was, and what you could not tell from the code alone.";
        assert_eq!(message, expected);
    }

    #[test]
    fn an_issue_without_an_event_or_without_an_exception_says_so() {
        let alone = first_message(&issue(), None);
        assert!(alone.contains("Sentry gave no event for it, so there is no stack trace here."), "{alone}");
        assert!(alone.ends_with("what you could not tell from the code alone."));
        let said = first_message(
            &json!({ "title": "Slow page", "shortId": "SHOP-2", "count": 1 }),
            Some(&json!({ "message": "LCP was 9 s", "entries": [] })),
        );
        assert!(said.contains("It happened 1 time, first on"), "{said}");
        assert!(said.contains("LCP was 9 s\n(The event carries no exception, so there is no stack trace.)"), "{said}");
        // A minified line is cut, not quoted whole.
        let long = json!({ "entries": [{ "type": "exception", "data": { "values": [{ "type": "E", "value": "v", "stacktrace": { "frames": [
            { "filename": "app.min.js", "lineNo": 1, "inApp": true, "context": [[1, "x".repeat(5000)]] }
        ] } }] } }] });
        let cut = first_message(&issue(), Some(&long));
        assert!(cut.lines().all(|line| line.chars().count() < 400), "a line of code is cut");
    }

    #[test]
    fn a_name_and_an_address_are_told_from_what_is_none() {
        assert!(is_slug("acme-inc") && is_slug("shop_web.v2") && !is_slug("") && !is_slug("a/b") && !is_slug("a b"));
        assert_eq!(tidy_url("").unwrap(), "https://sentry.io");
        assert_eq!(tidy_url(" https://sentry.example.com/ ").unwrap(), "https://sentry.example.com");
        assert_eq!(tidy_url("http://127.0.0.1:9000").unwrap(), "http://127.0.0.1:9000");
        for wrong in ["sentry.example.com", "http://sentry.example.com", "https://", "https://a b"] {
            assert!(tidy_url(wrong).is_err(), "{wrong}");
        }
    }
}
