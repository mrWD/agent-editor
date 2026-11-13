//! The environment of a task: the values its programs are started with.
//!
//! The values come from the project's own `.env`, and from `.env.example` for
//! the names the `.env` lacks, with the task's own values on top. The task's
//! own values are kept by the core, outside the task's folder: an agent reads
//! the files of its folder, and a secret must not reach it that way.
//!
//! A value that reads as a secret never leaves the core whole. The interface
//! is given a masked form, and what is said about a failure is cleaned of it.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The file a project keeps its values in.
pub const ENV_FILE: &str = ".env";
/// The file a project shows the names of its values in, with values safe to share.
pub const EXAMPLE_FILE: &str = ".env.example";

/// How long one value may be. A certificate fits; a file put there by mistake does not.
const MAX_VALUE: usize = 32 * 1024;
/// How many values a task may have of its own.
const MAX_OWN: usize = 200;

/// Words in a name that say the value is a secret wherever they stand.
const SECRET_INSIDE: [&str; 7] = ["SECRET", "TOKEN", "PASSWORD", "PASSWD", "APIKEY", "CREDENTIAL", "PRIVATE"];
/// Short words that say so only as a whole part of the name: `API_KEY`, not `KEYBOARD`.
const SECRET_PARTS: [&str; 8] = ["KEY", "KEYS", "PASS", "PWD", "AUTH", "SALT", "DSN", "SIGNATURE"];

/// What stands in for the hidden part of a secret.
const HIDDEN: &str = "••••••••";

/// One writer at a time: a value is set by reading the file, changing it and writing it back.
static WRITING: Mutex<()> = Mutex::new(());

/// True for a name a shell would take: letters, digits and underscores, not starting with a digit.
pub fn is_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Reads the text of a `.env` file: `NAME=value` lines, with `#` comments,
/// an optional `export`, and values in single or double quotes. A quoted
/// value may run over several lines. A name given twice keeps its last value.
pub fn parse(text: &str) -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let line_end = rest.find('\n').unwrap_or(rest.len());
        let line = &rest[..line_end];
        let mut next = &rest[(line_end + 1).min(rest.len())..];

        let entry = line.trim_start();
        let entry = entry.strip_prefix("export ").map_or(entry, str::trim_start);
        let named = entry.split_once('=').map(|(name, value)| (name.trim(), value.trim_start()));
        if let Some((name, value)) = named.filter(|(name, _)| !entry.starts_with('#') && is_name(name)) {
            // The value is the tail of the line, so where it starts in `rest` follows from its length.
            let starts = line_end - value.len();
            let quoted = value.chars().next().filter(|c| *c == '"' || *c == '\'');
            let read = match quoted.and_then(|quote| read_quoted(&rest[starts + 1..], quote)) {
                Some((read, used)) => {
                    // Whatever follows the closing quote on its line is a comment at best.
                    let after = &rest[starts + 1 + used..];
                    next = after.find('\n').map_or("", |at| &after[at + 1..]);
                    read
                }
                None => bare(value),
            };
            match found.iter_mut().find(|(known, _)| known == name) {
                Some(known) => known.1 = read,
                None => found.push((name.to_string(), read)),
            }
        }
        rest = next;
    }
    found
}

/// A value without quotes: up to a comment that starts after a space, without the spaces around it.
fn bare(value: &str) -> String {
    let value = value.strip_prefix('#').map_or(value, |_| "");
    let end = value.find(" #").or_else(|| value.find("\t#")).unwrap_or(value.len());
    value[..end].trim_end().to_string()
}

/// Reads up to the closing quote. Returns the value and how many bytes it took,
/// the closing quote included; `None` when the quote is never closed.
fn read_quoted(text: &str, quote: char) -> Option<(String, usize)> {
    let mut read = String::new();
    let mut chars = text.char_indices();
    while let Some((at, c)) = chars.next() {
        if c == quote {
            return Some((read, at + c.len_utf8()));
        }
        // Inside double quotes a backslash names a character; inside single quotes nothing is special.
        if c == '\\' && quote == '"' {
            match chars.next() {
                Some((_, 'n')) => read.push('\n'),
                Some((_, 'r')) => read.push('\r'),
                Some((_, 't')) => read.push('\t'),
                Some((_, other)) if other == '"' || other == '\\' || other == '$' => read.push(other),
                Some((_, other)) => {
                    read.push('\\');
                    read.push(other);
                }
                None => read.push('\\'),
            }
            continue;
        }
        read.push(c);
    }
    None
}

/// Where the password stands in an address such as `postgres://app:hunter2@db/app`.
fn password_in_url(value: &str) -> Option<(usize, usize)> {
    let authority_from = value.find("://")? + 3;
    let authority = &value[authority_from..];
    let authority = &authority[..authority.find(['/', '?', '#']).unwrap_or(authority.len())];
    let at = authority.rfind('@')?;
    let colon = authority[..at].find(':')?;
    (colon + 1 < at).then_some((authority_from + colon + 1, authority_from + at))
}

/// Whether a value must not be shown whole: its name says so, or it is an address with a password in it.
pub fn is_secret(name: &str, value: &str) -> bool {
    if value.is_empty() {
        return false;
    }
    let upper = name.to_ascii_uppercase();
    SECRET_INSIDE.iter().any(|word| upper.contains(word))
        || upper.split('_').any(|part| SECRET_PARTS.contains(&part))
        || password_in_url(value).is_some()
}

/// A secret as it may be shown: an address keeps everything but its password;
/// anything else keeps its last four characters when it is long enough for
/// those to give nothing away.
pub fn masked(value: &str) -> String {
    if let Some((from, to)) = password_in_url(value) {
        return format!("{}{HIDDEN}{}", &value[..from], &value[to..]);
    }
    let count = value.chars().count();
    if count < 16 {
        return HIDDEN.to_string();
    }
    let tail: String = value.chars().skip(count - 4).collect();
    format!("{HIDDEN}{tail}")
}

/// Takes every secret out of a text that is about to be shown or handed to an agent.
pub fn scrub(text: &str, secrets: &[String]) -> String {
    let mut clean = text.to_string();
    for secret in secrets {
        clean = clean.replace(secret.as_str(), HIDDEN);
    }
    clean
}

/// Where the task's own values are kept: in the core's data folder, next to
/// what else the core keeps for running the task. It goes when the task does.
fn own_file(data_dir: &Path, task_id: &str) -> PathBuf {
    data_dir.join("run").join(task_id).join("env.json")
}

/// The values set for this task only.
pub fn own(data_dir: &Path, task_id: &str) -> BTreeMap<String, String> {
    std::fs::read_to_string(own_file(data_dir, task_id))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Sets a value for this task only; `None` takes the task's own value away.
pub fn set_own(data_dir: &Path, task_id: &str, name: &str, value: Option<&str>) -> Result<()> {
    let name = name.trim();
    if !is_name(name) {
        bail!("A name is letters, digits and underscores, and does not start with a digit");
    }
    if value.is_some_and(|value| value.len() > MAX_VALUE || value.contains('\0')) {
        bail!("That value is too long to keep");
    }
    let _one_at_a_time = WRITING.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut values = own(data_dir, task_id);
    match value {
        Some(value) => {
            if !values.contains_key(name) && values.len() >= MAX_OWN {
                bail!("A task keeps at most {MAX_OWN} values of its own");
            }
            values.insert(name.to_string(), value.to_string());
        }
        None => {
            values.remove(name);
        }
    }
    let file = own_file(data_dir, task_id);
    if values.is_empty() {
        let _ = std::fs::remove_file(file);
        return Ok(());
    }
    std::fs::create_dir_all(file.parent().context("The data folder has no place for this task")?)?;
    write_private(&file, &serde_json::to_string_pretty(&values)?)
}

/// Writes a file only this user can read: it may hold secrets.
pub(crate) fn write_private(file: &Path, text: &str) -> Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut out = options.open(file).with_context(|| format!("Could not write {}", file.display()))?;
    out.write_all(text.as_bytes())?;
    Ok(())
}

/// A value as the project's files give it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Given {
    name: String,
    value: String,
    /// The file it was read from.
    from: &'static str,
}

/// Everything a task's programs are given, and where each value comes from.
#[derive(Debug, Clone, Default)]
pub struct TaskEnv {
    /// The project's files that were read, `.env` first.
    files: Vec<&'static str>,
    given: Vec<Given>,
    own: BTreeMap<String, String>,
}

impl TaskEnv {
    /// Reads the environment of a task. A file is looked for in the task's
    /// folder first and in the project's folder second: a `.env` is rarely in
    /// git, so the working copy of a task does not have one.
    pub fn read(data_dir: &Path, task_id: &str, cwd: &Path, project_dir: &Path) -> Self {
        let text_of = |name: &str| {
            [cwd, project_dir].iter().find_map(|folder| {
                let file = folder.join(name);
                // A file put there by mistake, or a folder of that name, is not read.
                let small = file.metadata().is_ok_and(|meta| meta.is_file() && meta.len() <= 1024 * 1024);
                if !small {
                    return None;
                }
                std::fs::read_to_string(file).ok()
            })
        };
        let mut env = Self { own: own(data_dir, task_id), ..Self::default() };
        for from in [ENV_FILE, EXAMPLE_FILE] {
            let Some(text) = text_of(from) else { continue };
            env.files.push(from);
            for (name, value) in parse(&text) {
                // The `.env` is read first and wins; the example only adds the names it lacks.
                if !env.given.iter().any(|given| given.name == name) {
                    env.given.push(Given { name, value, from });
                }
            }
        }
        env
    }

    /// The values as programs get them: the project's, with the task's own on top.
    pub fn pairs(&self) -> Vec<(String, String)> {
        let mut pairs: Vec<(String, String)> = self
            .given
            .iter()
            .filter(|given| !self.own.contains_key(&given.name))
            .map(|given| (given.name.clone(), given.value.clone()))
            .collect();
        pairs.extend(self.own.iter().map(|(name, value)| (name.clone(), value.clone())));
        pairs
    }

    /// Every text that must not be shown: the secrets, and the password inside an address.
    /// Longest first, so a secret that holds another is taken out whole.
    pub fn secrets(&self) -> Vec<String> {
        let all = self.given.iter().map(|given| (&given.name, &given.value)).chain(self.own.iter());
        let mut secrets: Vec<String> = Vec::new();
        for (_, value) in all.filter(|(name, value)| is_secret(name, value)) {
            secrets.push(value.clone());
            if let Some((from, to)) = password_in_url(value) {
                secrets.push(value[from..to].to_string());
            }
        }
        // A very short one would take ordinary words apart ("dev" inside "devices").
        secrets.retain(|secret| secret.chars().count() >= 4);
        secrets.sort_by(|a, b| b.len().cmp(&a.len()).then(a.cmp(b)));
        secrets.dedup();
        secrets
    }

    /// The environment as the interface may see it: every secret masked.
    pub fn shown(&self) -> Value {
        let show = |name: &str, value: &str| {
            let secret = is_secret(name, value);
            (if secret { masked(value) } else { value.to_string() }, secret)
        };
        let mut values: Vec<Value> = self
            .given
            .iter()
            .map(|given| {
                let (project_value, project_secret) = show(&given.name, &given.value);
                let own = self.own.get(&given.name);
                let (value, secret) = own.map_or((project_value.clone(), project_secret), |own| show(&given.name, own));
                json!({
                    "name": given.name, "value": value, "secret": secret || project_secret, "own": own.is_some(),
                    "project": { "value": project_value, "from": given.from }
                })
            })
            .collect();
        for (name, own) in self.own.iter().filter(|(name, _)| !self.given.iter().any(|given| &given.name == *name)) {
            let (value, secret) = show(name, own);
            values.push(json!({ "name": name, "value": value, "secret": secret, "own": true, "project": null }));
        }
        json!({ "files": self.files, "values": values, "own": self.own.len() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter().map(|(name, value)| (name.to_string(), value.to_string())).collect()
    }

    #[test]
    fn a_dotenv_file_is_read_the_way_its_tools_read_it() {
        let text = "# The shop\nPORT=3000\nexport API_URL = http://localhost:4000/api  # the local one\n\n\
                    EMPTY=\nQUOTED=\"two words # not a comment\"  # a comment\nSINGLE='a $literal \\n'\n\
                    not a line\n9LIVES=no\nHASH=#nothing\r\nWINDOWS=yes\r\n";
        assert_eq!(
            parse(text),
            pairs(&[
                ("PORT", "3000"),
                ("API_URL", "http://localhost:4000/api"),
                ("EMPTY", ""),
                ("QUOTED", "two words # not a comment"),
                ("SINGLE", "a $literal \\n"),
                ("HASH", ""),
                ("WINDOWS", "yes"),
            ])
        );
    }

    #[test]
    fn a_quoted_value_may_run_over_lines_and_a_name_given_twice_keeps_the_last() {
        let text = "KEY=\"-----BEGIN-----\nabc\\ndef\n-----END-----\"\nAFTER=1\nOPEN=\"never closed\nAFTER=2\n";
        assert_eq!(
            parse(text),
            pairs(&[("KEY", "-----BEGIN-----\nabc\ndef\n-----END-----"), ("AFTER", "2"), ("OPEN", "\"never closed")])
        );
        assert_eq!(parse("A=\"say \\\"hi\\\" \\\\ \\x\""), pairs(&[("A", "say \"hi\" \\ \\x")]));
        assert_eq!(parse(""), vec![]);
    }

    #[test]
    fn a_secret_is_known_by_its_name_or_by_a_password_in_an_address() {
        for name in
            ["API_KEY", "STRIPE_SECRET", "authToken", "DB_PASSWORD", "SENTRY_DSN", "JWT_PRIVATE_KEY", "PGPASSWORD"]
        {
            assert!(is_secret(name, "value"), "{name}");
        }
        for name in ["PORT", "KEYBOARD", "COMPASS_URL", "AUTHOR", "PASSPORT_COUNTRY", "NODE_ENV"] {
            assert!(!is_secret(name, "value"), "{name}");
        }
        assert!(is_secret("DATABASE_URL", "postgres://app:hunter2@localhost:5432/app"));
        assert!(!is_secret("DATABASE_URL", "postgres://localhost:5432/app"));
        assert!(!is_secret("SITE", "https://user@example.com/"), "a name alone is no password");
        assert!(!is_secret("API_KEY", ""), "nothing to hide in an empty value");
    }

    #[test]
    fn a_secret_is_never_shown_whole() {
        assert_eq!(masked("hunter2"), "••••••••");
        assert_eq!(masked("sk_live_0123456789abcdef"), "••••••••cdef");
        assert_eq!(masked("fifteen-letters"), "••••••••", "too short to show any of it");
        assert_eq!(masked("postgres://app:hunter2@localhost:5432/app"), "postgres://app:••••••••@localhost:5432/app");
        assert_eq!(
            scrub(
                "could not connect to postgres://app:hunter2@db/app: password hunter2 refused",
                &["postgres://app:hunter2@db/app".to_string(), "hunter2".to_string()]
            ),
            "could not connect to ••••••••: password •••••••• refused"
        );
    }

    #[test]
    fn the_tasks_own_values_lie_over_the_projects_and_secrets_are_masked() {
        let dir = std::env::temp_dir().join(format!("ae-env-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (data, cwd, project) = (dir.join("data"), dir.join("copy"), dir.join("project"));
        for folder in [&data, &cwd, &project] {
            std::fs::create_dir_all(folder).unwrap();
        }
        // The `.env` is in the project only, as it is when git does not carry it; the example is in the working copy.
        std::fs::write(project.join(".env"), "PORT=3000\nAPI_KEY=sk_live_0123456789abcdef\n").unwrap();
        std::fs::write(cwd.join(".env.example"), "PORT=8080\nAPI_KEY=\nMAIL_FROM=shop@example.com\n").unwrap();

        set_own(&data, "t1", "PORT", Some("3012")).unwrap();
        set_own(&data, "t1", "DB_PASSWORD", Some("hunter2")).unwrap();
        assert!(set_own(&data, "t1", "not a name", Some("x")).is_err());
        let env = TaskEnv::read(&data, "t1", &cwd, &project);
        assert_eq!(
            env.pairs(),
            pairs(&[
                ("API_KEY", "sk_live_0123456789abcdef"),
                ("MAIL_FROM", "shop@example.com"),
                ("DB_PASSWORD", "hunter2"),
                ("PORT", "3012"),
            ])
        );
        assert_eq!(env.secrets(), vec!["sk_live_0123456789abcdef".to_string(), "hunter2".to_string()]);
        let shown = env.shown();
        assert_eq!(shown["files"], json!([".env", ".env.example"]));
        assert_eq!(shown["own"], 2);
        assert_eq!(
            shown["values"],
            json!([
                { "name": "PORT", "value": "3012", "secret": false, "own": true,
                  "project": { "value": "3000", "from": ".env" } },
                { "name": "API_KEY", "value": "••••••••cdef", "secret": true, "own": false,
                  "project": { "value": "••••••••cdef", "from": ".env" } },
                { "name": "MAIL_FROM", "value": "shop@example.com", "secret": false, "own": false,
                  "project": { "value": "shop@example.com", "from": ".env.example" } },
                { "name": "DB_PASSWORD", "value": "••••••••", "secret": true, "own": true, "project": null },
            ])
        );
        assert!(!shown.to_string().contains("hunter2") && !shown.to_string().contains("sk_live"));

        // Taking the task's own value away gives the project's back; the file goes with the last one.
        set_own(&data, "t1", "PORT", None).unwrap();
        set_own(&data, "t1", "DB_PASSWORD", None).unwrap();
        assert!(!own_file(&data, "t1").exists());
        let env = TaskEnv::read(&data, "t1", &cwd, &project);
        assert_eq!(env.pairs()[0], ("PORT".to_string(), "3000".to_string()));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
