//! What SonarQube found in a project, on the Health screen.
//!
//! A team that runs SonarQube (its own server, or SonarCloud) already has a
//! list of what is wrong with each project: bugs, vulnerabilities, smells,
//! places to review for security, and a quality gate that passes or fails.
//! Health shows that list next to what the editor sees itself, and "Fix"
//! starts a task with the findings spelled out: the rule, the file, the line.
//!
//! The server is asked when the user presses "Check now", together with the
//! package managers' audits, and what it said is kept until the next time.
//! Nothing is analysed from here and nothing is changed there: a finding is
//! closed by SonarQube's own next analysis of the fixed code.
//!
//! Which project of SonarQube a project is, is read from its
//! `sonar-project.properties` (`sonar.projectKey`), or said by the user. The
//! token is kept with the other settings on this computer.

use crate::health::{Finding, finding};
use crate::store::Project;
use crate::tasks::{Manager, now_ms};
use anyhow::{Result, anyhow, bail};
use base64::Engine;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

const URL: &str = "sonar.url";
const TOKEN: &str = "sonar.token";
/// What the server said the last time it was asked.
const FOUND: &str = "health.sonar";
const PATIENCE: Duration = Duration::from_secs(20);
/// How many findings of one project are read: the worst ones, which is what a task is about.
const MOST_FINDINGS: usize = 20;
/// How many of them are named in the line under the finding.
const NAMED: usize = 3;

fn key_setting(project_id: &str) -> String {
    format!("project.{project_id}.sonar")
}

fn text(item: &Value, name: &str) -> String {
    item.get(name).and_then(Value::as_str).unwrap_or_default().to_string()
}

/// Whether text can be the key of a project in SonarQube: letters, digits, and the four signs it allows.
fn is_key(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 400
        && text.chars().all(|sign| sign.is_ascii_alphanumeric() || "-_.:".contains(sign))
}

/// The address of a SonarQube, tidied: https, or http on this computer for one that runs here.
fn tidy_url(text: &str) -> Result<String> {
    let url = text.trim().trim_end_matches('/');
    let local = ["http://127.0.0.1", "http://localhost"].iter().any(|start| url.starts_with(start));
    let rest = url.strip_prefix("https://").unwrap_or_default();
    if !(local || !rest.is_empty() && !rest.starts_with('/')) || url.contains(char::is_whitespace) {
        bail!("Give the address of your SonarQube, beginning with https:// (https://sonarcloud.io for SonarCloud)");
    }
    Ok(url.to_string())
}

/// The key a project names for itself in its `sonar-project.properties`.
pub fn key_in_files(root: &Path) -> Option<String> {
    let properties = std::fs::read_to_string(root.join("sonar-project.properties")).ok()?;
    properties
        .lines()
        .filter_map(|line| line.split_once('='))
        .find(|(name, _)| name.trim() == "sonar.projectKey")
        .map(|(_, value)| value.trim().to_string())
        .filter(|key| is_key(key))
}

/// How bad a finding is, as SonarQube says it one of two ways: the older "severity", or the newer "impacts".
fn severity_of(issue: &Value) -> String {
    let old = text(issue, "severity");
    let new = issue.pointer("/impacts/0/severity").and_then(Value::as_str).unwrap_or_default();
    (if old.is_empty() { new } else { &old }).to_lowercase()
}

/// A finding of SonarQube as it is kept: what a task needs to know of it.
pub fn read_issue(issue: &Value, project_key: &str) -> Value {
    // "shop:src/limit.ts": the file inside the project.
    let component = text(issue, "component");
    let file = component.strip_prefix(&format!("{project_key}:")).unwrap_or(&component).to_string();
    json!({
        "severity": severity_of(issue),
        "kind": text(issue, "type").to_lowercase().replace('_', " "),
        "rule": text(issue, "rule"),
        "message": text(issue, "message"),
        "file": file,
        "line": issue.get("line").and_then(Value::as_u64)
    })
}

/// One kept finding on one line: "src/limit.ts:42: Remove this unused import. (typescript:S1128, major)".
fn line_of(issue: &Value) -> String {
    let place = match issue.get("line").and_then(Value::as_u64) {
        Some(line) => format!("{}:{line}", text(issue, "file")),
        None => text(issue, "file"),
    };
    let how =
        [text(issue, "rule"), text(issue, "severity")].into_iter().filter(|word| !word.is_empty()).collect::<Vec<_>>();
    format!("{place}: {} ({})", text(issue, "message"), how.join(", "))
}

/// A condition of the quality gate that failed, in words: "new_coverage is 61.2, and must not be below 80".
fn condition_of(condition: &Value) -> Option<String> {
    if text(condition, "status") != "ERROR" {
        return None;
    }
    let bound = match text(condition, "comparator").as_str() {
        "LT" => "must not be below",
        "GT" => "must not be above",
        _ => "is held against",
    };
    Some(format!(
        "{} is {}, and {bound} {}",
        text(condition, "metricKey"),
        text(condition, "actualValue"),
        text(condition, "errorThreshold")
    ))
}

/// What Health shows for one project, from what its SonarQube said the last time.
pub(crate) fn findings_of(kept: &Value) -> Vec<Finding> {
    let mut found = Vec::new();
    let key = text(kept, "key");
    if let Some(problem) = kept.get("problem").and_then(Value::as_str) {
        found.push(finding(
            "sonar-unread",
            "Quality",
            "SonarQube could not be read for this project",
            problem,
            &format!("SonarQube could not be read for its project {key}: {problem} Find out why from the project's files (sonar-project.properties, the pipeline) and say what has to be put right. Change no code."),
        ));
        return found;
    }
    let failed: Vec<String> =
        kept.get("conditions").and_then(Value::as_array).into_iter().flatten().filter_map(condition_of).collect();
    if text(kept, "gate") == "ERROR" {
        let why = if failed.is_empty() {
            "SonarQube does not say which condition.".to_string()
        } else {
            format!("{}.", failed.join("; "))
        };
        found.push(finding(
            "sonar-gate",
            "Quality",
            "The quality gate of SonarQube fails",
            &capitalized(&why),
            &format!("The quality gate of SonarQube fails for its project {key}: {why} Put right what makes it fail, without lowering the gate or excluding files from the analysis. Say what you changed and what the next analysis should show."),
        ));
    }
    let issues: Vec<Value> = kept.get("issues").and_then(Value::as_array).cloned().unwrap_or_default();
    let total = kept.get("total").and_then(Value::as_u64).unwrap_or(issues.len() as u64);
    if total > 0 {
        let named: Vec<String> = issues.iter().take(NAMED).map(line_of).collect();
        let more = total.saturating_sub(named.len() as u64);
        let detail = match more {
            0 => named.join(" "),
            more => format!("{} And {more} more.", named.join(" ")),
        };
        let listed: Vec<String> = issues.iter().map(|issue| format!("- {}", line_of(issue))).collect();
        let rest = match total.saturating_sub(issues.len() as u64) {
            0 => String::new(),
            rest => format!("\n\n{rest} more are open in SonarQube; these are the worst."),
        };
        found.push(finding(
            "sonar-findings",
            "Quality",
            &format!("SonarQube has {total} open {}", if total == 1 { "finding" } else { "findings" }),
            &detail,
            &format!(
                "SonarQube reports these open findings for its project {key}, the worst first:\n\n{}{rest}\n\nFix them in that order, as far as each is a real fault: where one is not, say why instead of silencing it. Run the project's checks. SonarQube closes a finding with its next analysis.",
                listed.join("\n")
            ),
        ));
    }
    let hotspots = kept.get("hotspots").and_then(Value::as_u64).unwrap_or(0);
    if hotspots > 0 {
        found.push(finding(
            "sonar-hotspots",
            "Security",
            &format!("{hotspots} {} for a security review in SonarQube", if hotspots == 1 { "place waits" } else { "places wait" }),
            "SonarQube marks code that is not wrong by itself and can be: a person decides for each place.",
            &format!("SonarQube lists {hotspots} security hotspots to review for its project {key}. Open the project's code where such places usually are (input that reaches a query, a command or a path; secrets; random numbers; cookies and headers), say for each place you find whether it is safe and why, and fix the ones that are not."),
        ));
    }
    found
}

fn capitalized(text: &str) -> String {
    let mut letters = text.chars();
    match letters.next() {
        Some(first) => first.to_uppercase().chain(letters).collect(),
        None => String::new(),
    }
}

impl Manager {
    fn sonar_place(&self) -> Option<(String, String)> {
        let kept = |key: &str| self.store().setting(key).ok().flatten().filter(|value| !value.is_empty());
        Some((kept(URL)?, kept(TOKEN)?))
    }

    /// Asks SonarQube one thing and gives the JSON it answered. What it refuses is said in words.
    fn sonar_get(&self, url: &str, token: &str, path: &str) -> Result<Value> {
        // A token is the name of a user with no password: the one way every version of SonarQube takes.
        let login = base64::engine::general_purpose::STANDARD.encode(format!("{token}:"));
        let mut response = ureq::get(&format!("{url}/{path}"))
            .header("Authorization", &format!("Basic {login}"))
            .config()
            .timeout_global(Some(PATIENCE))
            .http_status_as_error(false)
            .build()
            .call()
            .map_err(|error| anyhow!("SonarQube could not be reached: {error}"))?;
        match response.status().as_u16() {
            200..=299 => response.body_mut().read_json().map_err(|_| anyhow!("SonarQube's answer could not be read")),
            401 => bail!("SonarQube did not accept the token."),
            403 => bail!("The token may not read that project in SonarQube."),
            404 => bail!("SonarQube knows no project by that key."),
            other => bail!("SonarQube answered with an error ({other})."),
        }
    }

    /// The key of a project in SonarQube, and where it is from: its own files, or the user.
    fn sonar_key(&self, project: &Project) -> Option<(String, &'static str)> {
        let said = self.store().setting(&key_setting(&project.id)).ok().flatten().filter(|key| !key.is_empty());
        said.map(|key| (key, "set")).or_else(|| key_in_files(Path::new(&project.path)).map(|key| (key, "file")))
    }

    /// What is connected, and which project of SonarQube each project is. No token is ever said back.
    pub fn sonar_status(&self) -> Value {
        let projects: Vec<Value> = self
            .store()
            .projects()
            .unwrap_or_default()
            .iter()
            .map(|project| {
                let key = self.sonar_key(project);
                json!({ "projectId": project.id, "key": key.as_ref().map(|(key, _)| key), "from": key.map(|(_, from)| from) })
            })
            .collect();
        let url = self.store().setting(URL).ok().flatten().unwrap_or_default();
        let at = self.sonar_found().get("at").cloned().unwrap_or(Value::Null);
        json!({ "connected": self.sonar_place().is_some(), "url": url, "projects": projects, "checkedAt": at })
    }

    /// Connects SonarQube: the token is tried before it is kept. An empty token disconnects and forgets what was found.
    pub fn sonar_connect(&self, url: &str, token: &str) -> Result<()> {
        let token = token.trim();
        if token.is_empty() {
            for key in [URL, TOKEN, FOUND] {
                self.store().set_setting(key, None)?;
            }
            self.broadcast("health.changed", json!({}));
            return Ok(());
        }
        if token.contains(char::is_whitespace) || token.len() > 400 {
            bail!("That is not a token of SonarQube");
        }
        let url = tidy_url(url)?;
        let said = self.sonar_get(&url, token, "api/authentication/validate")?;
        if said.get("valid").and_then(Value::as_bool) != Some(true) {
            bail!("SonarQube did not accept the token.");
        }
        self.store().set_setting(URL, Some(&url))?;
        self.store().set_setting(TOKEN, Some(token))?;
        self.broadcast("health.changed", json!({}));
        Ok(())
    }

    /// Says which project of SonarQube a project is; an empty key goes back to what its files say.
    pub fn sonar_set_key(&self, project_id: &str, key: &str) -> Result<()> {
        self.store().project(project_id)?;
        let key = key.trim();
        if !key.is_empty() && !is_key(key) {
            bail!("A project key of SonarQube is made of letters, digits, dashes, underscores, dots and colons");
        }
        self.store().set_setting(&key_setting(project_id), (!key.is_empty()).then_some(key))?;
        self.broadcast("health.changed", json!({}));
        Ok(())
    }

    fn sonar_found(&self) -> Value {
        let kept = self.store().setting(FOUND).ok().flatten();
        kept.and_then(|text| serde_json::from_str(&text).ok()).unwrap_or(Value::Null)
    }

    /// Asks SonarQube about every project that has a key, and keeps what it said. This reaches the
    /// network; it runs when the user presses "Check now". With SonarQube not connected it does nothing.
    pub fn sonar_check(&self) -> Result<()> {
        let Some((url, token)) = self.sonar_place() else { return Ok(()) };
        let mut projects = serde_json::Map::new();
        for project in self.store().projects()? {
            let Some((key, _)) = self.sonar_key(&project) else { continue };
            let read = || -> Result<Value> {
                let gate =
                    self.sonar_get(&url, &token, &format!("api/qualitygates/project_status?projectKey={key}"))?;
                let search = format!(
                    "api/issues/search?components={key}&issueStatuses=OPEN,CONFIRMED&s=SEVERITY&asc=false&ps={MOST_FINDINGS}"
                );
                let issues = self.sonar_get(&url, &token, &search)?;
                // Places to review are a list of their own. A server without them (an old one) has none to count.
                let hotspots = self
                    .sonar_get(&url, &token, &format!("api/hotspots/search?project={key}&status=TO_REVIEW&ps=1"))
                    .ok()
                    .and_then(|said| said.pointer("/paging/total").and_then(Value::as_u64))
                    .unwrap_or(0);
                let listed: Vec<Value> = issues
                    .get("issues")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .map(|issue| read_issue(issue, &key))
                    .collect();
                let total = issues.pointer("/paging/total").or_else(|| issues.get("total")).and_then(Value::as_u64);
                Ok(json!({
                    "key": key,
                    "gate": gate.pointer("/projectStatus/status").cloned().unwrap_or(Value::Null),
                    "conditions": gate.pointer("/projectStatus/conditions").cloned().unwrap_or(json!([])),
                    "total": total.unwrap_or(listed.len() as u64),
                    "issues": listed,
                    "hotspots": hotspots
                }))
            };
            // A project that could not be read says so on Health; the others are still read.
            let said = read().unwrap_or_else(|error| json!({ "key": key, "problem": format!("{error:#}") }));
            projects.insert(project.id, said);
        }
        self.store().set_setting(FOUND, Some(&json!({ "at": now_ms(), "projects": projects }).to_string()))?;
        Ok(())
    }

    /// What SonarQube said of one project the last time, as findings for Health.
    pub(crate) fn sonar_findings(&self, project_id: &str) -> Vec<Finding> {
        if self.sonar_place().is_none() {
            return Vec::new();
        }
        let found = self.sonar_found();
        found.pointer(&format!("/projects/{project_id}")).map(findings_of).unwrap_or_default()
    }
}

/// The commands of the interface that are about SonarQube.
///
/// - `sonar.status` gives `{ connected, url, checkedAt, projects: [{ projectId, key, from }] }`; `from`
///   is "file" for a key read from the project's own files and "set" for one the user gave.
/// - `sonar.connect { url, token }` tries the token and keeps it; an empty token disconnects.
/// - `sonar.key { projectId, key }` says which project of SonarQube a project is.
///
/// What SonarQube found is part of `health.get`, and is read anew by `health.audit`.
pub async fn command(manager: &Arc<Manager>, method: &str, params: &Value) -> Result<Value> {
    let word = |name: &str| params.get(name).and_then(Value::as_str).unwrap_or_default().to_string();
    match method {
        "sonar.status" => {}
        "sonar.connect" => {
            let (asking, url, token) = (manager.clone(), word("url"), word("token"));
            tokio::task::spawn_blocking(move || asking.sonar_connect(&url, &token)).await??;
        }
        "sonar.key" => manager.sonar_set_key(&word("projectId"), &word("key"))?,
        other => bail!("Unknown method: {other}"),
    }
    Ok(manager.sonar_status())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_project_names_its_own_key_in_its_files() {
        let dir = std::env::temp_dir().join(format!("ae-sonar-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(key_in_files(&dir), None);
        std::fs::write(
            dir.join("sonar-project.properties"),
            "# ours\nsonar.sources = src\n sonar.projectKey = acme:shop-web \n",
        )
        .unwrap();
        assert_eq!(key_in_files(&dir).as_deref(), Some("acme:shop-web"));
        std::fs::write(dir.join("sonar-project.properties"), "sonar.projectKey=a b&c=d\n").unwrap();
        assert_eq!(key_in_files(&dir), None, "what could not stand in an address is no key");
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(tidy_url("https://sonarcloud.io/").is_ok() && tidy_url("http://localhost:9000").is_ok());
        assert!(
            tidy_url("").is_err()
                && tidy_url("sonar.example.com").is_err()
                && tidy_url("http://sonar.example.com").is_err()
        );
    }

    #[test]
    fn a_finding_is_read_in_both_ways_sonarqube_says_how_bad_it_is() {
        let old = json!({ "severity": "CRITICAL", "type": "CODE_SMELL", "rule": "typescript:S3776", "component": "shop:src/limit.ts", "line": 42, "message": "Refactor this function." });
        assert_eq!(
            read_issue(&old, "shop"),
            json!({ "severity": "critical", "kind": "code smell", "rule": "typescript:S3776", "message": "Refactor this function.", "file": "src/limit.ts", "line": 42 })
        );
        let new = json!({ "impacts": [{ "softwareQuality": "RELIABILITY", "severity": "HIGH" }], "rule": "rust:S1", "component": "shop:src/a.rs", "message": "m" });
        let read = read_issue(&new, "shop");
        assert_eq!((read["severity"].as_str(), read["line"].is_null()), (Some("high"), true));
        assert_eq!(
            line_of(&read_issue(&old, "shop")),
            "src/limit.ts:42: Refactor this function. (typescript:S3776, critical)"
        );
        assert_eq!(line_of(&read), "src/a.rs: m (rust:S1, high)");
    }

    #[test]
    fn what_sonarqube_said_becomes_findings_a_task_can_be_started_from() {
        let issue = |n: u64| json!({ "severity": "major", "rule": "ts:S1", "message": format!("Finding {n}."), "file": "src/a.ts", "line": n });
        let kept = json!({
            "key": "shop", "gate": "ERROR", "total": 41, "hotspots": 2,
            "conditions": [
                { "status": "ERROR", "metricKey": "new_coverage", "comparator": "LT", "actualValue": "61.2", "errorThreshold": "80" },
                { "status": "OK", "metricKey": "new_bugs", "comparator": "GT", "actualValue": "0", "errorThreshold": "0" }
            ],
            "issues": [issue(1), issue(2), issue(3), issue(4)]
        });
        let found = findings_of(&kept);
        let told: Vec<(&str, &str, &str)> =
            found.iter().map(|one| (one.kind, one.category, one.title.as_str())).collect();
        assert_eq!(
            told,
            [
                ("sonar-gate", "Quality", "The quality gate of SonarQube fails"),
                ("sonar-findings", "Quality", "SonarQube has 41 open findings"),
                ("sonar-hotspots", "Security", "2 places wait for a security review in SonarQube"),
            ]
        );
        assert_eq!(found[0].detail, "New_coverage is 61.2, and must not be below 80.");
        assert_eq!(
            found[1].detail,
            "src/a.ts:1: Finding 1. (ts:S1, major) src/a.ts:2: Finding 2. (ts:S1, major) src/a.ts:3: Finding 3. (ts:S1, major) And 38 more."
        );
        assert!(
            found[1].fix.contains(
                "- src/a.ts:4: Finding 4. (ts:S1, major)\n\n37 more are open in SonarQube; these are the worst."
            ),
            "{}",
            found[1].fix
        );

        // A clean project has nothing to show; one that could not be read says why.
        assert!(
            findings_of(&json!({ "key": "shop", "gate": "OK", "total": 0, "issues": [], "hotspots": 0 })).is_empty()
        );
        let unread = findings_of(&json!({ "key": "shop", "problem": "SonarQube knows no project by that key." }));
        assert_eq!(
            (unread.len(), unread[0].kind, unread[0].detail.as_str()),
            (1, "sonar-unread", "SonarQube knows no project by that key.")
        );
    }
}
