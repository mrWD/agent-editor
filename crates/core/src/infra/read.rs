//! What a change to one infrastructure file does, read from its text before and after.
//!
//! Nothing is run and no tool is asked: a Terraform file is read block by
//! block, a manifest object by object, a Dockerfile instruction by instruction.
//! Every change is said in a sentence. The fields a deploy turns on (image,
//! replicas, resources, env, ports) are named, and what deserves a second look
//! is marked with the reason: something removed, a volume or a database
//! touched, a privileged container, a secret written in plain text.
//!
//! The reading is by eye, not by the tools' own parsers: what it does not
//! recognise it leaves out. The value of anything that may be a secret is
//! never repeated, only its name.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// One thing a change to an infrastructure file does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Step {
    pub text: String,
    /// The field that matters this is about: "image", "replicas", "resources", "env" or "ports".
    /// The text reads on from that word.
    pub field: Option<&'static str>,
    /// Why it deserves a second look. `None` for a step that only adds or adjusts.
    pub danger: Option<String>,
}

fn plain(text: String) -> Step {
    Step { text, field: None, danger: None }
}

fn about(field: &'static str, text: String) -> Step {
    Step { text, field: Some(field), danger: None }
}

fn risky(text: String, why: &str) -> Step {
    Step { text, field: None, danger: Some(why.to_string()) }
}

const DESTROYS: &str = "Terraform destroys it on the next apply, with whatever it holds.";
const DESTROYS_ALL: &str = "Everything the module manages is destroyed on the next apply.";
const HOLDS_DATA: &str = "It holds data: a change that replaces it loses what it holds.";
const STOPS: &str = "What ran from it is taken down when this is applied.";
const DATA_GOES: &str = "What it holds may be deleted with it.";
const VOLUME: &str = "A volume holds data: what was in the old one does not follow by itself.";
const NO_VOLUME: &str = "What the volume held is no longer mounted anywhere.";
const DATABASE: &str = "It looks like a database: a restart or another image is felt by everything that reads it.";
const PRIVILEGED: &str = "A privileged container can do on its host whatever root can.";
const AS_ROOT: &str = "Whatever gets hold of the process is root in the container.";
const NO_USER: &str = "It runs as whoever its base image runs as, and that is often root.";
const HOST: &str = "It is no longer kept apart from the node it runs on.";
const HOST_FILES: &str = "It reads and writes files of the node itself.";
const PLAIN_SECRET: &str = "A secret written in a file stays in the history of the repository for whoever reads it.";
const NO_JOB: &str = "What it checked or deployed no longer happens.";
const NO_FILE: &str = "Whatever was built or deployed from this file no longer is.";
const BROKEN: &str = "Whatever applies this file fails on it.";

/// How many steps of one file are said, and how many names one step lists.
const MOST_STEPS: usize = 60;
const MOST_NAMES: usize = 6;

fn mark(name: &str) -> String {
    format!("`{name}`")
}

/// Names in backquotes, the first few of them: `a`, `b` and 3 more.
fn listed<'a>(names: impl IntoIterator<Item = &'a String>) -> String {
    let names: Vec<&String> = names.into_iter().collect();
    let mut said: Vec<String> = names.iter().take(MOST_NAMES).map(|name| mark(name)).collect();
    if names.len() > MOST_NAMES {
        said.push(format!("and {} more", names.len() - MOST_NAMES));
    }
    said.join(", ")
}

/// What a change to a file does. `tool` is "terraform", "kubernetes", "helm", "docker", "compose" or "ci";
/// `before` is `None` for a new file and `after` for one that was removed.
pub fn read(tool: &str, path: &str, before: Option<&str>, after: Option<&str>) -> Vec<Step> {
    let (old, new) = (before.unwrap_or_default(), after.unwrap_or_default());
    let name = path.rsplit('/').next().unwrap_or(path);
    // A file that stopped being YAML has not lost its objects: it cannot be read, and that is what is said.
    let yaml = matches!(tool, "kubernetes" | "compose" | "ci") || (tool == "helm" && !is_template(path));
    if yaml && let Some(problem) = after.and_then(yaml_problem) {
        return vec![risky(format!("It cannot be read as YAML: {problem}"), BROKEN)];
    }
    let mut steps = match tool {
        "terraform" if name.ends_with(".tfvars") => values_file(old, new),
        "terraform" => terraform(old, new),
        "kubernetes" => kubernetes(old, new),
        "helm" => helm(path, before, after),
        "docker" => dockerfile(before, after),
        "compose" => compose(old, new),
        "ci" if path.contains(".github/") => github(old, new),
        "ci" => gitlab(old, new),
        _ => Vec::new(),
    };
    for name in plain_secrets(old, new) {
        steps.push(risky(format!("{} is written in plain text", mark(&name)), PLAIN_SECRET));
    }
    if steps.len() > MOST_STEPS {
        let more = steps.len() - MOST_STEPS;
        // What deserves a second look is never the part that is left out.
        steps.sort_by_key(|step| step.danger.is_none());
        steps.truncate(MOST_STEPS);
        steps.push(plain(format!("and {more} more changes")));
    }
    steps
}

/// What a file holds as it stands, in the words of [`read`]. It is read as a file that is new, and
/// what such a reading "adds" is simply there: a resource, an object with its image and its
/// replicas, a service, a job. What deserves a second look in a file as it stands is marked the same:
/// a privileged container, a secret written in plain text, an image that runs as root.
pub fn whole(tool: &str, path: &str, text: &str) -> Vec<Step> {
    let mut steps = read(tool, path, None, Some(text));
    for step in &mut steps {
        let said = &step.text;
        if let Some(rest) = said.strip_prefix("Adds ") {
            // "Adds resource `a.b`" is "Resource `a.b`" of a file that is not new.
            let mut letters = rest.chars();
            step.text = letters.next().map_or(String::new(), |first| first.to_uppercase().chain(letters).collect());
        } else if let Some(rest) = said.strip_prefix("Changes ") {
            // Whatever else a compose file says: its networks, its name.
            step.text = format!("Also sets {rest}");
        } else if let Some(count) = said.strip_prefix("and ").and_then(|rest| rest.strip_suffix(" more changes")) {
            step.text = format!("and {count} more");
        } else if let Some(name) = said.strip_suffix(" is set anew") {
            step.text = format!("{name} is set");
        } else if let Some(names) = said.strip_prefix("Variables of the pipeline: adds ") {
            step.text = format!("Variables of the pipeline: {names}");
        }
    }
    // Who the image runs as is the last word of its last stage.
    if tool == "docker"
        && let Some((_, Some(user))) = built(text).users.last()
    {
        let text = format!("Runs as {}", mark(user));
        steps.push(if is_root(user) { risky(text, AS_ROOT) } else { plain(text) });
    }
    steps
}

// ----- secrets in plain text -----

/// Whether a name says that what it holds is a secret.
fn is_secret_name(name: &str) -> bool {
    let name = name.to_lowercase().replace('-', "_");
    let secret =
        ["password", "passwd", "secret", "token", "api_key", "apikey", "access_key", "private_key", "credential"]
            .iter()
            .any(|word| name.contains(word));
    // A name that says where a secret is, not what it is.
    let points = [
        "name", "ref", "file", "path", "arn", "url", "uri", "endpoint", "version", "length", "policy", "ttl", "expir",
        "enabled", "type", "mount", "provider", "store", "rotation",
    ]
    .iter()
    .any(|word| name.contains(word));
    secret && !points
}

/// The names given a secret as a literal on a line that is new: `password = "…"`, `API_TOKEN: …`.
fn plain_secrets(old: &str, new: &str) -> Vec<String> {
    let before: BTreeSet<&str> = old.lines().map(str::trim).collect();
    let mut found: Vec<String> = Vec::new();
    // A sealed secret and a file encrypted by SOPS are made to be kept in a repository.
    if new.lines().any(|line| tidy(line) == "kind: SealedSecret" || line.starts_with("sops:")) {
        return found;
    }
    for line in new.lines().map(str::trim).filter(|line| !before.contains(line)) {
        if line.starts_with('#') || line.starts_with("//") {
            continue;
        }
        // An item of a list, as compose files give variables: `- NAME=value`.
        let (line, item) = line.strip_prefix("- ").map_or((line, false), |rest| (rest.trim_start(), true));
        let (line, instruction) = ["ENV ", "ARG ", "env ", "arg ", "export "]
            .iter()
            .find_map(|start| line.strip_prefix(start))
            .map_or((line, false), |rest| (rest.trim_start(), true));
        let split = match line.find(['=', ':']) {
            Some(at) => Some((&line[..at], &line[at + 1..], line.as_bytes()[at] == b'=')),
            // `ENV NAME value`, as Dockerfiles once had it.
            None if instruction => line.split_once(' ').map(|(name, value)| (name, value, false)),
            None => None,
        };
        let Some((name, value, assigned)) = split else { continue };
        let name = name.trim().trim_matches(['"', '\'']);
        let value = value.trim();
        if name.is_empty() || name.contains(' ') || !is_secret_name(name) {
            continue;
        }
        // In Terraform a literal is in quotes; anything else points at a value kept elsewhere.
        let quoted = value.starts_with(['"', '\'']);
        if assigned && !instruction && !item && !quoted {
            continue;
        }
        let inside = value.trim_end_matches(',').trim_matches(['"', '\'']);
        let points = inside.len() < 3
            || inside.starts_with(['$', '<', '*', '&', '!', '|', '>', '{', '[', '%'])
            || ["${", "{{", "$(", "%{"].iter().any(|sign| inside.contains(sign))
            // Encrypted where it stands, or the address of a secret kept elsewhere.
            || ["ENC[", "vault:", "ref+", "sm://", "arn:"].iter().any(|start| inside.starts_with(start))
            || matches!(inside, "null" | "true" | "false" | "none" | "None");
        if !points && !found.iter().any(|one| one == name) {
            found.push(name.to_string());
        }
    }
    found
}

// ----- Terraform -----

/// One statement of a Terraform file: an attribute with its value, or a block with what it holds.
struct Statement {
    name: String,
    labels: Vec<String>,
    /// The value, or what is between the brackets of the block, as written.
    text: String,
    block: bool,
}

fn line_end(text: &[u8], at: usize) -> usize {
    text[at..].iter().position(|sign| *sign == b'\n').map_or(text.len(), |found| at + found)
}

/// The index past the quote that closes the string opened at `at`.
fn string_end(text: &[u8], at: usize) -> usize {
    let mut i = at + 1;
    while i < text.len() {
        match text[i] {
            b'\\' => i += 2,
            b'"' => return i + 1,
            // What is worked out inside a string may hold strings of its own.
            b'$' | b'%' if text.get(i + 1) == Some(&b'{') => i = brackets_end(text, i + 1),
            _ => i += 1,
        }
    }
    text.len()
}

/// The index past the end of a text given as `<<NAME` … `NAME`. Past the two signs when it is not one.
fn heredoc_end(text: &[u8], at: usize) -> usize {
    let mut i = at + 2;
    if text.get(i) == Some(&b'-') {
        i += 1;
    }
    let start = i;
    while i < text.len() && (text[i].is_ascii_alphanumeric() || text[i] == b'_') {
        i += 1;
    }
    if i == start {
        return at + 2;
    }
    let name = &text[start..i];
    let mut line = line_end(text, i);
    while line < text.len() {
        let next = line_end(text, line + 1);
        if text[line + 1..next].trim_ascii() == name {
            return next;
        }
        line = next;
    }
    text.len()
}

/// The index past whatever is not code at `at`: a comment or a string. `None` when `at` is code.
fn skipped(text: &[u8], at: usize) -> Option<usize> {
    match (text[at], text.get(at + 1)) {
        (b'"', _) => Some(string_end(text, at)),
        (b'#', _) | (b'/', Some(b'/')) => Some(line_end(text, at)),
        (b'/', Some(b'*')) => {
            let close = text[at..].windows(2).position(|pair| pair == b"*/");
            Some(close.map_or(text.len(), |found| at + found + 2))
        }
        (b'<', Some(b'<')) => Some(heredoc_end(text, at)),
        _ => None,
    }
}

/// The index past the bracket that closes the one opened at `at`.
fn brackets_end(text: &[u8], at: usize) -> usize {
    let mut depth = 0usize;
    let mut i = at;
    while i < text.len() {
        if let Some(past) = skipped(text, i) {
            i = past;
            continue;
        }
        match text[i] {
            b'{' | b'[' | b'(' => depth += 1,
            b'}' | b']' | b')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return i + 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    text.len()
}

/// The index where the value that starts at `at` ends: the end of its line, brackets and strings aside.
fn value_end(text: &[u8], at: usize) -> usize {
    let mut i = at;
    while i < text.len() {
        match (text[i], text.get(i + 1)) {
            (b'\n', _) | (b'#', _) | (b'/', Some(b'/')) => return i,
            (b'{' | b'[' | b'(', _) => i = brackets_end(text, i),
            _ => i = skipped(text, i).unwrap_or(i + 1),
        }
    }
    text.len()
}

fn tidy(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The statements of a Terraform file, or of what one of its blocks holds, at their own level.
fn statements(source: &str) -> Vec<Statement> {
    let text = source.as_bytes();
    let word = |sign: u8| sign.is_ascii_alphanumeric() || sign == b'_' || sign == b'-' || sign == b'.';
    let mut found = Vec::new();
    let mut i = 0;
    while i < text.len() {
        if text[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if text[i] != b'"'
            && let Some(past) = skipped(text, i)
        {
            i = past;
            continue;
        }
        // A name, bare or in quotes, then either `=` and a value or labels and a block.
        let mut names: Vec<String> = Vec::new();
        loop {
            while i < text.len() && (text[i] == b' ' || text[i] == b'\t') {
                i += 1;
            }
            match text.get(i) {
                Some(b'"') => {
                    let end = string_end(text, i);
                    names.push(source.get(i + 1..end.saturating_sub(1).max(i + 1)).unwrap_or_default().to_string());
                    i = end;
                }
                Some(sign) if word(*sign) => {
                    let start = i;
                    while i < text.len() && word(text[i]) {
                        i += 1;
                    }
                    names.push(source[start..i].to_string());
                }
                _ => break,
            }
        }
        let Some(name) = names.first().cloned() else {
            // Nothing a statement starts with: the rest of the line is passed over.
            i = line_end(text, i) + 1;
            continue;
        };
        match text.get(i) {
            Some(b'=' | b':') if names.len() == 1 => {
                let end = value_end(text, i + 1);
                found.push(Statement { name, labels: vec![], text: tidy(&source[i + 1..end]), block: false });
                i = end;
            }
            Some(b'{') => {
                let end = brackets_end(text, i);
                let inside = source.get(i + 1..end.saturating_sub(1).max(i + 1)).unwrap_or_default();
                found.push(Statement { name, labels: names[1..].to_vec(), text: inside.to_string(), block: true });
                i = end;
            }
            _ => i = line_end(text, i) + 1,
        }
    }
    found
}

/// What a block says at its own level, each attribute and each block inside it by name.
fn said_in(body: &str) -> BTreeMap<String, String> {
    let mut said: BTreeMap<String, String> = BTreeMap::new();
    for one in statements(body) {
        let mut name = std::iter::once(one.name).chain(one.labels).collect::<Vec<_>>().join(" ");
        // The same block may be written several times: `ingress { … }` and again.
        let base = name.clone();
        let mut n = 1;
        while said.contains_key(&name) {
            n += 1;
            name = format!("{base} #{n}");
        }
        said.insert(name, tidy(&one.text));
    }
    said
}

/// The names that differ between two blocks: set in one only, or set to something else.
fn differing(old: &str, new: &str) -> Vec<String> {
    let (old, new) = (said_in(old), said_in(new));
    let names: BTreeSet<&String> = old.keys().chain(new.keys()).collect();
    names.into_iter().filter(|name| old.get(*name) != new.get(*name)).cloned().collect()
}

/// The words of a Terraform resource type that say it holds data.
const STORES: [&str; 30] = [
    "db",
    "database",
    "rds",
    "sql",
    "mysql",
    "postgresql",
    "postgres",
    "dynamodb",
    "redis",
    "elasticache",
    "memcached",
    "mongodb",
    "cosmosdb",
    "spanner",
    "bigtable",
    "docdb",
    "neptune",
    "redshift",
    "opensearch",
    "elasticsearch",
    "volume",
    "disk",
    "ebs",
    "efs",
    "bucket",
    "storage",
    "filestore",
    "snapshot",
    "backup",
    "keyspaces",
];

fn terraform(old: &str, new: &str) -> Vec<Step> {
    // The blocks of a file by what they are and what they are called. One written twice is read as one.
    let blocks = |text: &str| {
        let mut found: BTreeMap<(u8, String, String), String> = BTreeMap::new();
        for one in statements(text).into_iter().filter(|one| one.block) {
            let rank = match one.name.as_str() {
                "resource" => 0,
                "module" => 1,
                "data" => 2,
                _ => 3,
            };
            let entry = found.entry((rank, one.name, one.labels.join("."))).or_default();
            entry.push_str(&one.text);
            entry.push('\n');
        }
        found
    };
    let (old, new) = (blocks(old), blocks(new));
    let keys: BTreeSet<&(u8, String, String)> = old.keys().chain(new.keys()).collect();
    let mut steps = Vec::new();
    for key in keys {
        let (_, kind, name) = key;
        let what = match (kind.as_str(), name.is_empty()) {
            ("data", _) => format!("data source {}", mark(name)),
            ("terraform", _) => "the settings of Terraform".to_string(),
            (_, true) => kind.clone(),
            _ => format!("{kind} {}", mark(name)),
        };
        let holds = kind == "resource"
            && name.split('.').next().is_some_and(|kind| kind.split('_').any(|word| STORES.contains(&word)));
        // A variable that is a secret by its name, with what it is by default written out.
        let default = |body: Option<&String>| {
            said_in(body?).remove("default").filter(|value| value.starts_with('"') && value.len() > 4)
        };
        let written = default(new.get(key));
        if kind == "variable" && is_secret_name(name) && written.is_some() && written != default(old.get(key)) {
            steps.push(risky(format!("The default of variable {} is written in plain text", mark(name)), PLAIN_SECRET));
        }
        match (old.get(key), new.get(key)) {
            (None, Some(_)) => steps.push(plain(format!("Adds {what}"))),
            (Some(_), None) => steps.push(match kind.as_str() {
                "resource" => risky(format!("Removes {what}"), DESTROYS),
                "module" => risky(format!("Removes {what}"), DESTROYS_ALL),
                _ => plain(format!("Removes {what}")),
            }),
            (Some(before), Some(after)) => {
                let names = differing(before, after);
                if names.is_empty() {
                    continue;
                }
                let text = format!("Changes {what}: {}", listed(&names));
                steps.push(if holds { risky(text, HOLDS_DATA) } else { plain(text) });
            }
            (None, None) => {}
        }
    }
    steps
}

/// Terraform reads the files of a folder as one: a block taken out of one file and put into another
/// of the same folder is moved, not destroyed. Given the steps of two files of one folder and the
/// names of the files, says so in both, in place of "Removes" in the first and "Adds" in the second.
pub fn settle_moves(from: &mut [Step], from_name: &str, to: &mut [Step], to_name: &str) {
    for removed in from.iter_mut() {
        let Some(what) = removed.text.strip_prefix("Removes ").map(str::to_string) else { continue };
        let Some(added) = to.iter_mut().find(|step| step.text.strip_prefix("Adds ") == Some(what.as_str())) else {
            continue;
        };
        *removed = plain(format!("Moves {what} to {}", mark(to_name)));
        *added = plain(format!("Takes {what} from {}", mark(from_name)));
    }
}

/// What is said of a file that was moved with nothing in it changed: at the place it left, and at the one it came to.
pub fn moved_whole(from: &str, to: &str) -> (Vec<Step>, Vec<Step>) {
    (
        vec![plain(format!("Moved to {}, with nothing in it changed", mark(to)))],
        vec![plain(format!("Moved from {}, with nothing in it changed", mark(from)))],
    )
}

/// What is said of a file that is too long to be read.
pub fn too_long() -> Vec<Step> {
    vec![plain("It is too long to be read here".to_string())]
}

/// A file of values for Terraform's variables: which are set, set anew or no longer set.
fn values_file(old: &str, new: &str) -> Vec<Step> {
    let (old, new) = (said_in(old), said_in(new));
    let added: Vec<&String> = new.keys().filter(|name| !old.contains_key(*name)).collect();
    let changed: Vec<&String> =
        new.keys().filter(|name| old.get(*name).is_some_and(|was| was != &new[*name])).collect();
    let removed: Vec<&String> = old.keys().filter(|name| !new.contains_key(*name)).collect();
    [("Sets", added), ("Changes", changed), ("No longer sets", removed)]
        .into_iter()
        .filter(|(_, names)| !names.is_empty())
        .map(|(verb, names)| plain(format!("{verb} {}", listed(names))))
        .collect()
}

// ----- YAML -----

fn to_json(yaml: serde_yaml_ng::Value) -> Value {
    use serde_yaml_ng::Value as Yaml;
    match yaml {
        Yaml::Null => Value::Null,
        Yaml::Bool(yes) => Value::Bool(yes),
        Yaml::Number(number) => serde_json::to_value(number).unwrap_or(Value::Null),
        Yaml::String(text) => Value::String(text),
        Yaml::Sequence(items) => Value::Array(items.into_iter().map(to_json).collect()),
        Yaml::Mapping(map) => Value::Object(
            map.into_iter()
                .map(|(key, value)| {
                    let key = match to_json(key) {
                        Value::String(text) => text,
                        other => other.to_string(),
                    };
                    (key, to_json(value))
                })
                .collect(),
        ),
        Yaml::Tagged(tagged) => to_json(tagged.value),
    }
}

/// Why a text is not YAML, in the words of the parser. `None` when it is.
fn yaml_problem(text: &str) -> Option<String> {
    serde_yaml_ng::Deserializer::from_str(text)
        .take(400)
        .find_map(|document| serde_yaml_ng::Value::deserialize(document).err())
        .map(|error| error.to_string())
}

/// Whether a file of a chart is one of its templates, which are not YAML until Helm has filled them in.
fn is_template(path: &str) -> bool {
    path.contains("/templates/") || path.starts_with("templates/")
}

/// The documents of a YAML file that are maps. Reading stops at one that is not YAML.
fn documents(text: &str) -> Vec<Value> {
    serde_yaml_ng::Deserializer::from_str(text)
        .take(400)
        .map_while(|document| serde_yaml_ng::Value::deserialize(document).ok())
        .map(|mut document| {
            // `<<: *default` is what GitLab and compose files are full of.
            let _ = document.apply_merge();
            to_json(document)
        })
        .filter(Value::is_object)
        .collect()
}

fn first_document(text: &str) -> Value {
    documents(text).into_iter().next().unwrap_or(Value::Null)
}

/// A value as a person would write it. Lists and maps are given short.
fn shown(value: &Value) -> String {
    let text = match value {
        Value::String(text) => text.clone(),
        Value::Null => "nothing".to_string(),
        other => other.to_string(),
    };
    if text.chars().count() > 80 { format!("{}…", text.chars().take(80).collect::<String>()) } else { text }
}

fn keys_of(value: Option<&Value>) -> BTreeSet<String> {
    value.and_then(Value::as_object).map(|map| map.keys().cloned().collect()).unwrap_or_default()
}

/// The places where two values differ, as `spec.selector.app`. Lists are compared whole.
fn differing_paths(old: &Value, new: &Value, at: &str, depth: usize, out: &mut Vec<String>) {
    if old == new {
        return;
    }
    match (old.as_object(), new.as_object()) {
        (Some(before), Some(after)) if depth < 4 => {
            let names: BTreeSet<&String> = before.keys().chain(after.keys()).collect();
            for name in names {
                let path = if at.is_empty() { name.clone() } else { format!("{at}.{name}") };
                let nothing = Value::Null;
                differing_paths(
                    before.get(name).unwrap_or(&nothing),
                    after.get(name).unwrap_or(&nothing),
                    &path,
                    depth + 1,
                    out,
                );
            }
        }
        _ => out.push(at.to_string()),
    }
}

/// Something as it is now and as it was, in words. `was` is `None` when there is nothing to compare
/// with: the thing is new. Nothing is said of what is the same, or not there at all.
fn now_and_was(lead: &str, now: Option<String>, was: Option<Option<String>>) -> Option<String> {
    Some(match (now, was) {
        (Some(now), None) => format!("{lead}{now}"),
        (now, Some(was)) if now == was => return None,
        (Some(now), Some(Some(was))) => format!("{lead}{now}, was {was}"),
        (Some(now), Some(None)) => format!("{lead}{now}, was not set"),
        (None, Some(Some(was))) => format!("{lead}no longer set, was {was}"),
        (None, _) => return None,
    })
}

/// One of the fields that matter, as it is now and as it was.
fn field(name: &'static str, lead: &str, now: Option<String>, was: Option<Option<String>>) -> Option<Step> {
    now_and_was(lead, now, was).map(|text| about(name, text))
}

/// The words in a name or an image that say a database runs there.
const DATABASES: [&str; 20] = [
    "postgres",
    "postgis",
    "mysql",
    "mariadb",
    "mongo",
    "redis",
    "valkey",
    "memcached",
    "elasticsearch",
    "opensearch",
    "cassandra",
    "clickhouse",
    "cockroach",
    "mssql",
    "influx",
    "timescale",
    "etcd",
    "couch",
    "neo4j",
    "percona",
];

fn looks_like_database<'a>(names: impl IntoIterator<Item = &'a str>) -> bool {
    names.into_iter().any(|name| {
        let name = name.to_lowercase();
        DATABASES.iter().any(|word| name.contains(word))
            || name.split(|sign: char| !sign.is_alphanumeric()).any(|word| matches!(word, "db" | "database"))
    })
}

/// The variables given to a container, each by name with what it is set to. A value is kept only to
/// tell whether it changed: it is never said.
fn variables(given: Option<&Value>) -> BTreeMap<String, String> {
    let mut found = BTreeMap::new();
    match given {
        // Kubernetes: a list of `{ name, value }`; compose: a list of `NAME=value`.
        Some(Value::Array(items)) => {
            for item in items {
                match item {
                    Value::String(text) => {
                        let (name, value) = text.split_once('=').unwrap_or((text, ""));
                        found.insert(name.to_string(), value.to_string());
                    }
                    other => {
                        if let Some(name) = other.get("name").and_then(Value::as_str) {
                            found.insert(name.to_string(), other.to_string());
                        }
                    }
                }
            }
        }
        Some(Value::Object(map)) => {
            found.extend(map.iter().map(|(name, value)| (name.clone(), value.to_string())));
        }
        _ => {}
    }
    found
}

/// How the variables of a container changed, by name: "adds `A`; changes `B`; removes `C`".
fn variables_step(lead: &str, now: &BTreeMap<String, String>, was: Option<&BTreeMap<String, String>>) -> Option<Step> {
    let Some(was) = was else {
        return (!now.is_empty()).then(|| about("env", format!("{lead}{}", listed(now.keys()))));
    };
    let added: Vec<&String> = now.keys().filter(|name| !was.contains_key(*name)).collect();
    let changed: Vec<&String> =
        now.keys().filter(|name| was.get(*name).is_some_and(|old| old != &now[*name])).collect();
    let removed: Vec<&String> = was.keys().filter(|name| !now.contains_key(*name)).collect();
    let said: Vec<String> = [("adds", added), ("changes", changed), ("removes", removed)]
        .into_iter()
        .filter(|(_, names)| !names.is_empty())
        .map(|(verb, names)| format!("{verb} {}", listed(names)))
        .collect();
    (!said.is_empty()).then(|| about("env", format!("{lead}{}", said.join("; "))))
}

// ----- Kubernetes -----

/// One object of a manifest.
struct Object {
    kind: String,
    name: String,
    namespace: String,
    whole: Value,
}

impl Object {
    fn label(&self) -> String {
        format!("{} {}", self.kind, mark(&self.name))
    }
}

fn objects(text: &str) -> Vec<Object> {
    let mut found = Vec::new();
    let mut waiting = documents(text);
    waiting.reverse();
    while let Some(mut whole) = waiting.pop() {
        let Some(kind) = whole.get("kind").and_then(Value::as_str).map(str::to_string) else { continue };
        // A list of objects is its objects.
        if kind.ends_with("List")
            && let Some(Value::Array(items)) = whole.get_mut("items").map(Value::take)
        {
            waiting.extend(items.into_iter().rev());
            continue;
        }
        let meta = |name: &str| whole.pointer(&format!("/metadata/{name}")).and_then(Value::as_str);
        let name = meta("name").or(meta("generateName")).unwrap_or("without a name").to_string();
        let namespace = meta("namespace").unwrap_or_default().to_string();
        found.push(Object { kind, name, namespace, whole });
    }
    found
}

/// What the pods of an object are made of, for the kinds that run pods.
fn pod_of(object: &Object) -> Option<&Value> {
    match object.kind.as_str() {
        "Pod" => object.whole.pointer("/spec"),
        "CronJob" => object.whole.pointer("/spec/jobTemplate/spec/template/spec"),
        _ => object.whole.pointer("/spec/template/spec"),
    }
    .filter(|pod| pod.get("containers").is_some())
}

fn containers(pod: Option<&Value>) -> BTreeMap<String, &Value> {
    let mut found = BTreeMap::new();
    for list in ["initContainers", "containers"] {
        for one in pod.and_then(|pod| pod.get(list)).and_then(Value::as_array).into_iter().flatten() {
            let name = one.get("name").and_then(Value::as_str).unwrap_or("container").to_string();
            found.insert(name, one);
        }
    }
    found
}

/// "requests cpu 100m, memory 128Mi; limits memory 256Mi".
fn resources_of(container: &Value) -> Option<String> {
    let parts: Vec<String> = ["requests", "limits"]
        .iter()
        .filter_map(|side| {
            let set = container.pointer(&format!("/resources/{side}"))?.as_object()?;
            let said: Vec<String> = set.iter().map(|(name, value)| format!("{name} {}", shown(value))).collect();
            (!said.is_empty()).then(|| format!("{side} {}", said.join(", ")))
        })
        .collect();
    (!parts.is_empty()).then(|| parts.join("; "))
}

fn container_ports(container: &Value) -> Option<String> {
    let ports: Vec<String> = container
        .get("ports")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|port| port.get("containerPort").map(shown))
        .collect();
    (!ports.is_empty()).then(|| ports.join(", "))
}

fn is_true(value: Option<&Value>) -> bool {
    value.and_then(Value::as_bool) == Some(true)
}

/// The paths of the node that the pods mount.
fn host_paths(pod: Option<&Value>) -> BTreeSet<String> {
    pod.and_then(|pod| pod.get("volumes"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|volume| volume.pointer("/hostPath/path").and_then(Value::as_str).map(str::to_string))
        .collect()
}

/// What an object says in the fields that matter, against what it said before. `old` is `None` for a new one.
fn pod_facts(new: &Object, old: Option<&Object>, steps: &mut Vec<Step>) {
    let replicas = |object: &Object| object.whole.pointer("/spec/replicas").map(shown);
    steps.extend(field("replicas", "", replicas(new), old.map(replicas)));
    let (pod, pod_before) = (pod_of(new), old.and_then(pod_of));
    let (now, before) = (containers(pod), containers(pod_before));
    for (name, container) in &now {
        let was = before.get(name);
        // A container that is new in an object that was there is said to be: nothing of it is compared.
        if old.is_some() && was.is_none() {
            steps.push(plain(format!("Adds container {}", mark(name))));
        }
        let compared = |read: &dyn Fn(&Value) -> Option<String>| was.map(|was| read(was));
        let of = format!("of {}: ", mark(name));
        let image = |one: &Value| one.get("image").and_then(Value::as_str).map(mark);
        steps.extend(field("image", &of, image(container), compared(&image)));
        steps.extend(field("resources", &of, resources_of(container), compared(&resources_of)));
        steps.extend(field("ports", &of, container_ports(container), compared(&container_ports)));
        let env = |one: &Value| {
            let mut all = variables(one.get("env"));
            for (at, source) in one.get("envFrom").and_then(Value::as_array).into_iter().flatten().enumerate() {
                all.insert(format!("everything from source {}", at + 1), source.to_string());
            }
            all
        };
        steps.extend(variables_step(&of, &env(container), was.map(|was| env(was)).as_ref()));
        // A variable that is a secret by its name, given as a literal and not from a Secret.
        let literal = |one: &Value, variable: &str| {
            one.get("env")?
                .as_array()?
                .iter()
                .find(|item| item.get("name").and_then(Value::as_str) == Some(variable))?
                .get("value")
                .filter(|value| value.as_str().is_none_or(|text| !text.is_empty()) && !value.is_null())
                .cloned()
        };
        for variable in variables(container.get("env")).keys().filter(|variable| is_secret_name(variable)) {
            let now = literal(container, variable);
            if now.is_some() && now != was.and_then(|was| literal(was, variable)) {
                steps.push(risky(
                    format!("{} of {} is written in plain text", mark(variable), mark(name)),
                    PLAIN_SECRET,
                ));
            }
        }
        let privileged = |one: &Value| is_true(one.pointer("/securityContext/privileged"));
        if privileged(container) && !was.is_some_and(|was| privileged(was)) {
            steps.push(risky(format!("{} runs privileged", mark(name)), PRIVILEGED));
        }
    }
    for name in before.keys().filter(|name| !now.contains_key(*name)) {
        steps.push(plain(format!("Removes container {}", mark(name))));
    }
    for (key, words) in [("hostNetwork", "network"), ("hostPID", "processes"), ("hostIPC", "memory")] {
        if is_true(pod.and_then(|pod| pod.get(key))) && !is_true(pod_before.and_then(|pod| pod.get(key))) {
            steps.push(risky(format!("Shares the {words} of the node"), HOST));
        }
    }
    let mounted_before = host_paths(pod_before);
    for path in host_paths(pod).difference(&mounted_before) {
        steps.push(risky(format!("Mounts {} of the node", mark(path)), HOST_FILES));
    }
    // The volumes of an object that was there: where its data is kept is another place now.
    let volumes = |object: &Object, pod: Option<&Value>| {
        (pod.and_then(|pod| pod.get("volumes")).cloned(), object.whole.pointer("/spec/volumeClaimTemplates").cloned())
    };
    if let Some(old) = old
        && volumes(new, pod) != volumes(old, pod_before)
    {
        steps.push(risky("Changes its volumes".to_string(), VOLUME));
    }
}

/// What a Service, an Ingress and the like say in the fields that matter.
fn other_facts(new: &Object, old: Option<&Object>, steps: &mut Vec<Step>) {
    match new.kind.as_str() {
        "Service" => {
            let ports = |object: &Object| {
                let said: Vec<String> = object
                    .whole
                    .pointer("/spec/ports")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|port| {
                        let from = shown(port.get("port")?);
                        Some(match port.get("targetPort") {
                            Some(to) if shown(to) != from => format!("{from} to {}", shown(to)),
                            _ => from,
                        })
                    })
                    .collect();
                (!said.is_empty()).then(|| said.join(", "))
            };
            steps.extend(field("ports", "", ports(new), old.map(ports)));
            let kind = |object: &Object| object.whole.pointer("/spec/type").and_then(Value::as_str).map(mark);
            steps.extend(now_and_was("Its type is ", kind(new), old.map(kind)).map(plain));
        }
        "Ingress" => {
            let hosts = |object: &Object| {
                let said: Vec<String> = object
                    .whole
                    .pointer("/spec/rules")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|rule| rule.get("host").and_then(Value::as_str).map(mark))
                    .collect();
                (!said.is_empty()).then(|| said.join(", "))
            };
            steps.extend(now_and_was("Answers for ", hosts(new), old.map(hosts)).map(plain));
        }
        "Secret" => {
            let holds = |object: &Object| {
                ["data", "stringData"]
                    .iter()
                    .any(|key| object.whole.get(*key).and_then(Value::as_object).is_some_and(|map| !map.is_empty()))
            };
            let same = old.is_some_and(|old| {
                (old.whole.get("data"), old.whole.get("stringData"))
                    == (new.whole.get("data"), new.whole.get("stringData"))
            });
            if holds(new) && !same {
                steps.push(risky("Keeps what the Secret holds in the repository".to_string(), PLAIN_SECRET));
            }
        }
        _ => {}
    }
}

/// The kinds that are data, or the place it is kept.
const DATA_KINDS: [&str; 4] = ["PersistentVolumeClaim", "PersistentVolume", "StorageClass", "VolumeSnapshot"];

fn kubernetes(old: &str, new: &str) -> Vec<Step> {
    let (old, new) = (objects(old), objects(new));
    let same = |a: &Object, b: &Object| (&a.kind, &a.name, &a.namespace) == (&b.kind, &b.name, &b.namespace);
    let mut steps = Vec::new();
    for object in &new {
        let before = old.iter().find(|one| same(one, object));
        if before.is_some_and(|before| before.whole == object.whole) {
            continue;
        }
        let mut facts = Vec::new();
        pod_facts(object, before, &mut facts);
        other_facts(object, before, &mut facts);
        let Some(before) = before else {
            steps.push(plain(format!("Adds {}", object.label())));
            steps.extend(facts);
            continue;
        };
        let mut text = format!("Changes {}", object.label());
        // Nothing of what matters changed: where it did change is said instead.
        if facts.is_empty() {
            let mut paths = Vec::new();
            differing_paths(&before.whole, &object.whole, "", 0, &mut paths);
            text = format!("{text}: {}", listed(&paths));
        }
        let images = containers(pod_of(object)).into_values().filter_map(|one| one.get("image")?.as_str());
        steps.push(if DATA_KINDS.contains(&object.kind.as_str()) {
            risky(text, HOLDS_DATA)
        } else if pod_of(object).is_some() && looks_like_database(images.chain([object.name.as_str()])) {
            risky(text, DATABASE)
        } else {
            plain(text)
        });
        steps.extend(facts);
    }
    for object in old.iter().filter(|one| !new.iter().any(|other| same(one, other))) {
        let holds =
            DATA_KINDS.contains(&object.kind.as_str()) || object.whole.pointer("/spec/volumeClaimTemplates").is_some();
        steps.push(risky(format!("Removes {}", object.label()), if holds { DATA_GOES } else { STOPS }));
    }
    steps
}

// ----- Helm -----

/// Every value of a file of values, by where it is: `image.tag`.
fn flat(value: &Value, at: &str, out: &mut BTreeMap<String, String>) {
    match value.as_object() {
        Some(map) if !map.is_empty() => {
            for (name, inside) in map {
                flat(inside, &if at.is_empty() { name.clone() } else { format!("{at}.{name}") }, out);
            }
        }
        _ => {
            out.insert(at.to_string(), shown(value));
        }
    }
}

/// The field that matters a value of a chart is about, judged by where it is.
fn field_of(path: &str) -> Option<&'static str> {
    // `containerPort` and `ports` are ports; `support` is not.
    let port = path
        .split('.')
        .any(|part| matches!(part, "port" | "ports") || part.ends_with("Port") || part.ends_with("Ports"));
    let path = path.to_lowercase();
    let has = |word: &str| path.split('.').any(|part| part.contains(word));
    if has("replica") {
        Some("replicas")
    } else if has("resources") {
        Some("resources")
    } else if has("image") {
        Some("image")
    } else if port {
        Some("ports")
    } else if path.split('.').any(|part| matches!(part, "env" | "envvars" | "extraenv" | "environment")) {
        Some("env")
    } else {
        None
    }
}

fn helm(path: &str, before: Option<&str>, after: Option<&str>) -> Vec<Step> {
    let (old, new) = (before.unwrap_or_default(), after.unwrap_or_default());
    let name = path.rsplit('/').next().unwrap_or(path);
    let mut steps = Vec::new();
    if name == "Chart.yaml" {
        let (was, now) = (first_document(old), first_document(new));
        let text =
            |chart: &Value, key: &str| chart.get(key).filter(|value| !value.is_null()).map(|value| mark(&shown(value)));
        let chart = text(&now, "name").or(text(&was, "name")).unwrap_or_else(|| "without a name".into());
        if after.is_none() {
            return vec![risky(format!("Removes the chart {chart}"), NO_FILE)];
        }
        let compared = |key: &str| before.map(|_| text(&was, key));
        if before.is_none() {
            steps.push(plain(format!("Adds the chart {chart}")));
        }
        for (key, lead) in [("version", "The version of the chart is "), ("appVersion", "The version of the app is ")] {
            steps.extend(now_and_was(lead, text(&now, key), compared(key)).map(plain));
        }
        let needs = |chart: &Value| -> BTreeMap<String, String> {
            chart
                .get("dependencies")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|one| {
                    Some((one.get("name")?.as_str()?.to_string(), one.get("version").map(shown).unwrap_or_default()))
                })
                .collect()
        };
        let (needed, needs) = (needs(&was), needs(&now));
        for (name, version) in &needs {
            match needed.get(name) {
                None => steps.push(plain(format!("Needs the chart {} {}", mark(name), mark(version)))),
                Some(old) if old != version => {
                    steps.push(plain(format!("Needs the chart {} {}, was {}", mark(name), mark(version), mark(old))));
                }
                _ => {}
            }
        }
        for name in needed.keys().filter(|name| !needs.contains_key(*name)) {
            steps.push(plain(format!("No longer needs the chart {}", mark(name))));
        }
        return steps;
    }
    if !is_template(path) {
        // A file of values.
        let (mut was, mut now) = (BTreeMap::new(), BTreeMap::new());
        flat(&first_document(old), "", &mut was);
        flat(&first_document(new), "", &mut now);
        was.remove("");
        now.remove("");
        let names: BTreeSet<&String> = was.keys().chain(now.keys()).collect();
        for name in names {
            let (old, new) = (was.get(name), now.get(name));
            if old == new {
                continue;
            }
            // What may be a secret is named and never repeated.
            let text = if name.rsplit('.').next().is_some_and(is_secret_name) {
                format!("{} is {}", mark(name), if new.is_none() { "no longer set" } else { "set anew" })
            } else {
                match (new, old) {
                    (Some(new), Some(old)) => format!("{}: {}, was {}", mark(name), mark(new), mark(old)),
                    (Some(new), None) => format!("{}: {}", mark(name), mark(new)),
                    (None, Some(old)) => format!("{} is no longer set, was {}", mark(name), mark(old)),
                    (None, None) => continue,
                }
            };
            steps.push(Step { text, field: field_of(name), danger: None });
        }
        return steps;
    }
    // A template is not YAML until Helm has filled it in: it is read line by line.
    let kinds = |text: &str| -> Vec<String> {
        let mut found: Vec<String> =
            text.lines().filter_map(|line| line.strip_prefix("kind:")).map(|kind| kind.trim().to_string()).collect();
        found.retain(|kind| !kind.is_empty() && kind.chars().all(char::is_alphanumeric));
        found.dedup();
        found
    };
    let of = |text: &str| match kinds(text) {
        found if found.is_empty() => "a template".to_string(),
        found => format!("the template of {}", found.join(", ")),
    };
    match (before, after) {
        (_, None) => return vec![risky(format!("Removes {}", of(old)), STOPS)],
        (None, Some(_)) => steps.push(plain(format!("Adds {}", of(new)))),
        (Some(_), Some(_)) => steps.push(plain(format!("Changes {}", of(new)))),
    }
    let had: BTreeSet<&str> = old.lines().map(str::trim).collect();
    for line in new.lines().map(str::trim).filter(|line| !had.contains(line)) {
        let line = line.strip_prefix("- ").unwrap_or(line);
        if let Some(image) = line.strip_prefix("image:") {
            steps.push(about("image", mark(image.trim().trim_matches('"'))));
        } else if let Some(replicas) = line.strip_prefix("replicas:") {
            steps.push(about("replicas", mark(replicas.trim())));
        } else if tidy(line) == "privileged: true" {
            steps.push(risky("A container runs privileged".to_string(), PRIVILEGED));
        }
    }
    steps
}

// ----- Dockerfile -----

/// The instructions of a Dockerfile, each with what follows it. A line that goes on is joined.
fn instructions(text: &str) -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = Vec::new();
    let mut whole = String::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') || (line.is_empty() && whole.is_empty()) {
            continue;
        }
        match line.strip_suffix('\\') {
            Some(start) => {
                whole.push_str(start);
                whole.push(' ');
                continue;
            }
            None => whole.push_str(line),
        }
        let (name, rest) = whole.split_once(char::is_whitespace).unwrap_or((whole.as_str(), ""));
        found.push((name.to_uppercase(), rest.trim().to_string()));
        whole.clear();
    }
    found
}

/// What a Dockerfile says of the image it builds: what it starts from, the ports it says it
/// listens on, and who each stage runs as when it ends.
struct Built {
    bases: Option<String>,
    ports: Option<String>,
    /// By the name of the stage, or by its number for one without a name. `None` for a stage
    /// that names no user: it runs as whoever its base runs as.
    users: Vec<(String, Option<String>)>,
}

/// A value as a Dockerfile writes it, with the `$NAME`, `${NAME}` and `${NAME:-other}` in it
/// filled in from the values the file itself gives. What the file does not give stays as written:
/// it comes from whoever builds the image.
fn filled(text: &str, given: &BTreeMap<String, String>) -> String {
    let is_name = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find('$') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let (name, fallback, taken) = match after.strip_prefix('{').and_then(|inner| inner.split_once('}')) {
            Some((inner, _)) => {
                let (name, fallback) = match inner.split_once(":-") {
                    Some((name, fallback)) => (name, Some(fallback)),
                    None => (inner, None),
                };
                (name, fallback, inner.len() + 2)
            }
            None => {
                let length = after.find(|c: char| !is_name(c)).unwrap_or(after.len());
                (&after[..length], None, length)
            }
        };
        let known = given.get(name).filter(|value| !value.is_empty() || fallback.is_none());
        match (known, fallback) {
            _ if name.is_empty() || !name.chars().all(is_name) => out.push_str(&rest[at..at + 1 + taken]),
            (Some(value), _) => out.push_str(value),
            (None, Some(fallback)) => out.push_str(&filled(fallback, given)),
            (None, None) => out.push_str(&rest[at..at + 1 + taken]),
        }
        rest = &after[taken..];
    }
    out.push_str(rest);
    out
}

/// True for a user that is root: by name or by number, with or without a group.
fn is_root(user: &str) -> bool {
    matches!(user.split(':').next().unwrap_or_default(), "root" | "0")
}

fn built(text: &str) -> Built {
    let mut stages: Vec<String> = Vec::new();
    let mut bases: Vec<String> = Vec::new();
    let mut ports: Vec<String> = Vec::new();
    let mut users: Vec<(String, Option<String>)> = Vec::new();
    // The values the file gives its own arguments: an image is often named through one.
    let mut given: BTreeMap<String, String> = BTreeMap::new();
    for (name, rest) in instructions(text) {
        let mut words = rest.split_whitespace().filter(|word| !word.starts_with("--"));
        match name.as_str() {
            "ARG" => {
                for word in rest.split_whitespace() {
                    if let Some((name, value)) = word.split_once('=') {
                        let value = filled(value.trim_matches(|c| c == '"' || c == '\''), &given);
                        given.insert(name.to_string(), value);
                    }
                }
            }
            "FROM" => {
                let Some(image) = words.next() else { continue };
                let image = filled(image, &given);
                // A stage built on an earlier one starts from no image of its own, and runs as that one does.
                let earlier = users.iter().find(|(stage, _)| *stage == image).map(|(_, user)| user.clone());
                if !stages.contains(&image) && !bases.contains(&image) {
                    bases.push(image);
                }
                let stage = match (words.next(), words.next()) {
                    (Some(word), Some(stage)) if word.eq_ignore_ascii_case("as") => {
                        stages.push(stage.to_string());
                        stage.to_string()
                    }
                    _ => format!("#{}", users.len() + 1),
                };
                users.push((stage, earlier.flatten()));
            }
            "USER" => {
                if let (Some(user), Some(stage)) = (words.next(), users.last_mut()) {
                    stage.1 = Some(filled(user, &given));
                }
            }
            "EXPOSE" => {
                for port in words {
                    if !ports.iter().any(|one| one == port) {
                        ports.push(port.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    let said = |names: Vec<String>, marked: bool| {
        (!names.is_empty())
            .then(|| names.iter().map(|name| if marked { mark(name) } else { name.clone() }).collect::<Vec<_>>())
            .map(|names| names.join(", "))
    };
    Built { bases: said(bases, true), ports: said(ports, false), users }
}

/// The stages that ran as somebody of their own and now run as root, or as whoever the base runs as.
fn now_root(new: &Built, old: &Built) -> Vec<Step> {
    let only = new.users.len() == 1;
    new.users
        .iter()
        .filter_map(|(stage, user)| {
            let (_, was) = old.users.iter().find(|(name, _)| name == stage)?;
            let was = was.as_deref().filter(|was| !is_root(was))?;
            let who = if only { "The image".to_string() } else { format!("Stage {}", mark(stage)) };
            match user.as_deref() {
                Some(user) if is_root(user) => {
                    Some(risky(format!("{who} runs as {}, was {}", mark(user), mark(was)), AS_ROOT))
                }
                None => Some(risky(format!("{who} no longer says who it runs as, was {}", mark(was)), NO_USER)),
                Some(_) => None,
            }
        })
        .collect()
}

fn dockerfile(before: Option<&str>, after: Option<&str>) -> Vec<Step> {
    let Some(new) = after else {
        return vec![risky("Removes the Dockerfile".to_string(), NO_FILE)];
    };
    let now = built(new);
    let was = before.map(built);
    let mut steps = Vec::new();
    steps.extend(field("image", "base ", now.bases.clone(), was.as_ref().map(|was| was.bases.clone())));
    steps.extend(field("ports", "exposes ", now.ports.clone(), was.as_ref().map(|was| was.ports.clone())));
    if let Some(was) = &was {
        steps.extend(now_root(&now, was));
    }
    if steps.is_empty() && before.is_some() {
        steps.push(plain("Changes how the image is built, on the same base and with the same ports".to_string()));
    }
    steps
}

// ----- compose -----

/// A value of a compose file that is a list of short things, said in one line.
fn short_list(value: Option<&Value>) -> Option<String> {
    let items: Vec<String> = match value? {
        Value::Array(items) => items
            .iter()
            .map(|item| match item {
                // The long form of a port or a volume: what it is and where it goes.
                Value::Object(map) => ["published", "source", "target"]
                    .iter()
                    .filter_map(|key| map.get(*key).map(shown))
                    .collect::<Vec<_>>()
                    .join(":"),
                other => shown(other),
            })
            .collect(),
        other => vec![shown(other)],
    };
    (!items.is_empty()).then(|| items.join(", "))
}

fn compose(old: &str, new: &str) -> Vec<Step> {
    let (was, now) = (first_document(old), first_document(new));
    let services = |file: &Value| file.get("services").and_then(Value::as_object).cloned().unwrap_or_default();
    let (before, after) = (services(&was), services(&now));
    let mut steps = Vec::new();
    for (name, service) in &after {
        let old = before.get(name);
        if old == Some(service) {
            continue;
        }
        let image = |one: &Value| {
            let built = one.get("build").map(|build| match build {
                Value::String(folder) => format!("built from {}", mark(folder)),
                other => format!("built from {}", mark(&other.get("context").map(shown).unwrap_or_else(|| ".".into()))),
            });
            one.get("image").and_then(Value::as_str).map(mark).or(built)
        };
        let compared = |read: &dyn Fn(&Value) -> Option<String>| old.map(read);
        let of = format!("of {}: ", mark(name));
        let mut facts: Vec<Step> = Vec::new();
        facts.extend(field("image", &of, image(service), compared(&image)));
        let ports = |one: &Value| short_list(one.get("ports"));
        facts.extend(field("ports", &of, ports(service), compared(&ports)));
        let replicas = |one: &Value| one.pointer("/deploy/replicas").map(shown);
        facts.extend(field("replicas", &of, replicas(service), compared(&replicas)));
        let limits = |one: &Value| {
            let set = one.pointer("/deploy/resources")?;
            let mut said = BTreeMap::new();
            flat(set, "", &mut said);
            Some(said.iter().map(|(name, value)| format!("{name} {value}")).collect::<Vec<_>>().join(", "))
        };
        facts.extend(field("resources", &of, limits(service), compared(&limits)));
        let env = |one: &Value| variables(one.get("environment"));
        facts.extend(variables_step(&of, &env(service), old.map(env).as_ref()));
        let privileged = |one: &Value| is_true(one.get("privileged"));
        if privileged(service) && !old.is_some_and(privileged) {
            facts.push(risky(format!("{} runs privileged", mark(name)), PRIVILEGED));
        }
        let volumes = |one: &Value| short_list(one.get("volumes"));
        if let Some(old) = old
            && volumes(old) != volumes(service)
        {
            let said = |list: Option<String>| list.unwrap_or_else(|| "none".into());
            facts.push(risky(
                format!("The volumes of {} are {}, were {}", mark(name), said(volumes(service)), said(volumes(old))),
                VOLUME,
            ));
        }
        let Some(old) = old else {
            steps.push(plain(format!("Adds service {}", mark(name))));
            steps.extend(facts);
            continue;
        };
        let mut text = format!("Changes service {}", mark(name));
        if facts.is_empty() {
            let mut paths = Vec::new();
            differing_paths(old, service, "", 0, &mut paths);
            text = format!("{text}: {}", listed(&paths));
        }
        let images = [service, old].map(|one| one.get("image").and_then(Value::as_str).unwrap_or_default());
        steps.push(if looks_like_database(images.into_iter().chain([name.as_str()])) {
            risky(text, DATABASE)
        } else {
            plain(text)
        });
        steps.extend(facts);
    }
    for name in before.keys().filter(|name| !after.contains_key(*name)) {
        steps.push(risky(format!("Removes service {}", mark(name)), STOPS));
    }
    let (had, has) = (keys_of(was.get("volumes")), keys_of(now.get("volumes")));
    for name in has.difference(&had) {
        steps.push(plain(format!("Adds volume {}", mark(name))));
    }
    for name in had.difference(&has) {
        steps.push(risky(format!("Removes volume {}", mark(name)), NO_VOLUME));
    }
    // Whatever else the file says: networks, configs, its name. Of a file that is gone, nothing more is said.
    if new.trim().is_empty() {
        return steps;
    }
    let mut rest = Vec::new();
    for key in
        keys_of(Some(&was)).union(&keys_of(Some(&now))).filter(|key| !matches!(key.as_str(), "services" | "volumes"))
    {
        if was.get(key) != now.get(key) {
            rest.push(key.clone());
        }
    }
    if !rest.is_empty() {
        steps.push(plain(format!("Changes {}", listed(&rest))));
    }
    steps
}

// ----- CI -----

/// The jobs of a pipeline against the ones it had: added, changed (with the keys that differ), removed.
fn jobs_steps(before: &BTreeMap<String, Value>, after: &BTreeMap<String, Value>, steps: &mut Vec<Step>) {
    for (name, job) in after {
        match before.get(name) {
            None => steps.push(plain(format!("Adds job {}", mark(name)))),
            Some(old) if old != job => {
                let keys = keys_of(Some(old));
                let differ: Vec<String> =
                    keys.union(&keys_of(Some(job))).filter(|key| old.get(*key) != job.get(*key)).cloned().collect();
                steps.push(plain(format!("Changes job {}: {}", mark(name), listed(&differ))));
            }
            _ => {}
        }
    }
    for name in before.keys().filter(|name| !after.contains_key(*name)) {
        steps.push(risky(format!("Removes job {}", mark(name)), NO_JOB));
    }
}

/// The top-level words of a GitLab pipeline that are not jobs.
const GITLAB_WORDS: [&str; 13] = [
    "stages",
    "variables",
    "include",
    "default",
    "workflow",
    "image",
    "services",
    "before_script",
    "after_script",
    "cache",
    "spec",
    "types",
    "pages:deploy",
];

fn gitlab(old: &str, new: &str) -> Vec<Step> {
    let (was, now) = (first_document(old), first_document(new));
    // A name that starts with a dot is a template for jobs, not a job.
    let jobs = |file: &Value| -> BTreeMap<String, Value> {
        file.as_object()
            .into_iter()
            .flatten()
            .filter(|(name, job)| !GITLAB_WORDS.contains(&name.as_str()) && !name.starts_with('.') && job.is_object())
            .map(|(name, job)| (name.clone(), job.clone()))
            .collect()
    };
    let mut steps = Vec::new();
    jobs_steps(&jobs(&was), &jobs(&now), &mut steps);
    // Of a pipeline that is gone, its jobs are all there is to say.
    if new.trim().is_empty() {
        return steps;
    }
    let stages = |file: &Value| short_list(file.get("stages"));
    let had = (!old.is_empty()).then(|| stages(&was));
    steps.extend(now_and_was("The stages are ", stages(&now), had).map(plain));
    let (had, has) = (variables(was.get("variables")), variables(now.get("variables")));
    if let Some(step) = variables_step("Variables of the pipeline: ", &has, Some(&had)) {
        steps.push(plain(step.text));
    }
    let rest: Vec<String> = keys_of(Some(&was))
        .union(&keys_of(Some(&now)))
        .filter(|key| {
            let word = GITLAB_WORDS.contains(&key.as_str()) && !matches!(key.as_str(), "stages" | "variables");
            (word || key.starts_with('.')) && was.get(*key) != now.get(*key)
        })
        .cloned()
        .collect();
    if !rest.is_empty() && !old.is_empty() {
        steps.push(plain(format!("Changes {}", listed(&rest))));
    }
    steps
}

fn github(old: &str, new: &str) -> Vec<Step> {
    let (was, now) = (first_document(old), first_document(new));
    let jobs = |file: &Value| -> BTreeMap<String, Value> {
        file.get("jobs")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .map(|(name, job)| (name.clone(), job.clone()))
            .collect()
    };
    let mut steps = Vec::new();
    jobs_steps(&jobs(&was), &jobs(&now), &mut steps);
    if new.trim().is_empty() {
        return steps;
    }
    // What starts the workflow: a word, a list of words, or a map by word.
    let starts = |file: &Value| match file.get("on") {
        Some(Value::Object(map)) => Some(map.keys().cloned().collect::<Vec<_>>().join(", ")),
        other => short_list(other),
    };
    let started = (!old.is_empty()).then(|| starts(&was));
    steps.extend(now_and_was("Runs on ", starts(&now), started).map(plain));
    let rest: Vec<String> = keys_of(Some(&was))
        .union(&keys_of(Some(&now)))
        .filter(|key| !matches!(key.as_str(), "jobs" | "on") && was.get(*key) != now.get(*key))
        .cloned()
        .collect();
    if !rest.is_empty() && !old.is_empty() {
        steps.push(plain(format!("Changes {}", listed(&rest))));
    }
    steps
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each step as its field, its text and whether it is marked.
    fn said(steps: &[Step]) -> Vec<(&str, &str, bool)> {
        steps.iter().map(|step| (step.field.unwrap_or(""), step.text.as_str(), step.danger.is_some())).collect()
    }

    #[test]
    fn terraform_resources_are_read_by_their_blocks() {
        let old = r#"
terraform {
  required_version = ">= 1.6"
}

# The database of the shop.
resource "aws_db_instance" "main" {
  instance_class    = "db.t3.micro"
  allocated_storage = 20
  tags = {
    Name = "shop-${var.env}"
  }
}

resource "aws_s3_bucket" "old_assets" {
  bucket = "shop-old-assets"
}

resource "aws_security_group" "web" {
  name = "web"
  ingress {
    from_port = 80
  }
}

module "network" {
  source = "./modules/network"
}

variable "env" {}
"#;
        let new = r#"
terraform {
  required_version = ">= 1.6"
}

# The database of the shop, a size up.
resource "aws_db_instance" "main" {
  instance_class    = "db.t3.small"
  allocated_storage = 20
  tags = {
    Name = "shop-${lookup(var.names, "env")}"
  }
}

resource "aws_security_group" "web" {
  name = "web"
  ingress {
    from_port = 80
  }
  ingress {
    from_port = 443
  }
}

resource "aws_sqs_queue" "mail" {
  name = "mail"
}

variable "env" {}

output "queue" {
  value = aws_sqs_queue.mail.arn
}
"#;
        assert_eq!(
            said(&terraform(old, new)),
            [
                ("", "Changes resource `aws_db_instance.main`: `instance_class`, `tags`", true),
                ("", "Removes resource `aws_s3_bucket.old_assets`", true),
                ("", "Changes resource `aws_security_group.web`: `ingress #2`", false),
                ("", "Adds resource `aws_sqs_queue.mail`", false),
                ("", "Removes module `network`", true),
                ("", "Adds output `queue`", false),
            ]
        );
        assert!(terraform(new, new).is_empty(), "the same text is no change");
        // A secret that a variable is by default is marked; one that is only declared is not.
        let secret = "variable \"db_password\" {\n  default = \"hunter2hunter2\"\n}\nvariable \"api_token\" {\n  sensitive = true\n}\n";
        let steps = terraform("", secret);
        assert_eq!(
            said(&steps),
            [
                ("", "Adds variable `api_token`", false),
                ("", "The default of variable `db_password` is written in plain text", true),
                ("", "Adds variable `db_password`", false),
            ]
        );
        assert!(!format!("{steps:?}").contains("hunter2"));

        // A resource taken out of one file and put into another of the same folder is moved, not destroyed.
        let bucket = "resource \"aws_s3_bucket\" \"assets\" {}\n";
        let mut left = terraform(&format!("{bucket}resource \"aws_sqs_queue\" \"old\" {{}}\n"), "");
        let mut came = terraform("", bucket);
        settle_moves(&mut left, "main.tf", &mut came, "storage.tf");
        assert_eq!(
            said(&left),
            [
                ("", "Moves resource `aws_s3_bucket.assets` to `storage.tf`", false),
                ("", "Removes resource `aws_sqs_queue.old`", true),
            ]
        );
        assert_eq!(said(&came), [("", "Takes resource `aws_s3_bucket.assets` from `main.tf`", false)]);
        // A text between marks may hold brackets of its own, and a comment a bracket that closes nothing.
        let odd = "resource \"a_b\" \"c\" {\n  policy = <<EOF\n{ \"x\": \"}\" }\nEOF\n  # }\n  n = 1\n}\nresource \"a_b\" \"d\" {}\n";
        assert_eq!(
            said(&terraform("", odd)),
            [("", "Adds resource `a_b.c`", false), ("", "Adds resource `a_b.d`", false)]
        );
    }

    #[test]
    fn values_for_terraform_are_named_and_a_secret_among_them_is_marked() {
        let steps = read(
            "terraform",
            "prod.tfvars",
            Some("region = \"eu-west-1\"\nsize = 2\nold = true\n"),
            Some("region = \"eu-west-1\"\nsize = 3\ndb_password = \"hunter2hunter2\"\n"),
        );
        assert_eq!(
            said(&steps),
            [
                ("", "Sets `db_password`", false),
                ("", "Changes `size`", false),
                ("", "No longer sets `old`", false),
                ("", "`db_password` is written in plain text", true),
            ]
        );
        assert!(!format!("{steps:?}").contains("hunter2"), "the secret itself is never repeated");
    }

    #[test]
    fn only_a_literal_on_a_new_line_counts_as_a_secret_in_plain_text() {
        let old = "password = \"was-here-before\"\n";
        let new = r#"password = "was-here-before"
api_token = var.api_token
secret_key = "${var.secret}"
password_file = "/run/secrets/db"
secretName: web-tls
API_TOKEN: {{ .Values.token }}
client_secret: abc123def
- STRIPE_SECRET_KEY=sk_live_123
ENV NPM_TOKEN abcdef
# password: commented-out
token: ""
"#;
        assert_eq!(plain_secrets(old, new), ["client_secret", "STRIPE_SECRET_KEY", "NPM_TOKEN"]);
        // What is encrypted where it stands is made to be kept in a repository.
        assert!(
            plain_secrets("", "password: ENC[AES256_GCM,data:abc]\napi_token: vault:secret/data/shop#token\n")
                .is_empty()
        );
        let sealed = "kind: SealedSecret\nspec:\n  encryptedData:\n    password: AgBy3i4OJSWK\n";
        assert!(plain_secrets("", sealed).is_empty());
        assert!(plain_secrets("", "db_password: AgBy3i4OJSWK\nsops:\n  version: 3.8.1\n").is_empty());
    }

    const WEB: &str = r#"apiVersion: apps/v1
kind: Deployment
metadata:
  name: web
spec:
  replicas: 2
  template:
    spec:
      containers:
        - name: web
          image: shop/web:1.4
          ports:
            - containerPort: 8080
          env:
            - name: LOG_LEVEL
              value: info
            - name: OLD_FLAG
              value: "1"
          resources:
            limits:
              memory: 256Mi
---
apiVersion: v1
kind: Service
metadata:
  name: web
spec:
  ports:
    - port: 80
      targetPort: 8080
---
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: uploads
spec:
  resources:
    requests:
      storage: 5Gi
---
apiVersion: v1
kind: ConfigMap
metadata:
  name: settings
data:
  greeting: hello
"#;

    #[test]
    fn kubernetes_objects_are_read_by_kind_and_name_with_the_fields_that_matter() {
        let new = r#"apiVersion: apps/v1
kind: Deployment
metadata:
  name: web
spec:
  replicas: 5
  template:
    spec:
      hostNetwork: true
      containers:
        - name: web
          image: shop/web:1.5
          securityContext:
            privileged: true
          ports:
            - containerPort: 8080
            - containerPort: 9090
          env:
            - name: LOG_LEVEL
              value: debug
            - name: FEATURE_X
              valueFrom:
                secretKeyRef: { name: flags, key: x }
          resources:
            limits:
              memory: 512Mi
        - name: proxy
          image: envoy:1.30
---
apiVersion: v1
kind: Service
metadata:
  name: web
spec:
  type: LoadBalancer
  ports:
    - port: 80
      targetPort: 8080
    - port: 9090
---
apiVersion: v1
kind: ConfigMap
metadata:
  name: settings
data:
  greeting: good day
  farewell: bye
---
apiVersion: batch/v1
kind: CronJob
metadata:
  name: digest
spec:
  jobTemplate:
    spec:
      template:
        spec:
          containers:
            - name: digest
              image: shop/jobs:2
"#;
        assert_eq!(
            said(&kubernetes(WEB, new)),
            [
                ("", "Changes Deployment `web`", false),
                ("replicas", "5, was 2", false),
                ("", "Adds container `proxy`", false),
                ("image", "of `proxy`: `envoy:1.30`", false),
                ("image", "of `web`: `shop/web:1.5`, was `shop/web:1.4`", false),
                ("resources", "of `web`: limits memory 512Mi, was limits memory 256Mi", false),
                ("ports", "of `web`: 8080, 9090, was 8080", false),
                ("env", "of `web`: adds `FEATURE_X`; changes `LOG_LEVEL`; removes `OLD_FLAG`", false),
                ("", "`web` runs privileged", true),
                ("", "Shares the network of the node", true),
                ("", "Changes Service `web`", false),
                ("ports", "80 to 8080, 9090, was 80 to 8080", false),
                ("", "Its type is `LoadBalancer`, was not set", false),
                ("", "Changes ConfigMap `settings`: `data.farewell`, `data.greeting`", false),
                ("", "Adds CronJob `digest`", false),
                ("image", "of `digest`: `shop/jobs:2`", false),
                ("", "Removes PersistentVolumeClaim `uploads`", true),
            ]
        );
    }

    #[test]
    fn a_volume_a_database_and_a_secret_in_a_manifest_are_marked() {
        let db = |image: &str, size: &str| {
            format!(
                "apiVersion: apps/v1\nkind: StatefulSet\nmetadata:\n  name: orders-db\nspec:\n  template:\n    spec:\n      containers:\n        - name: main\n          image: {image}\n  volumeClaimTemplates:\n    - metadata: {{ name: data }}\n      spec: {{ resources: {{ requests: {{ storage: {size} }} }} }}\n"
            )
        };
        assert_eq!(
            said(&kubernetes(&db("postgres:15", "10Gi"), &db("postgres:16", "20Gi"))),
            [
                ("", "Changes StatefulSet `orders-db`", true),
                ("image", "of `main`: `postgres:16`, was `postgres:15`", false),
                ("", "Changes its volumes", true),
            ]
        );
        let secret = "apiVersion: v1\nkind: Secret\nmetadata:\n  name: keys\nstringData:\n  stripe: sk_live_123456\n";
        let steps = read("kubernetes", "k8s/keys.yaml", None, Some(secret));
        assert_eq!(
            said(&steps),
            [("", "Adds Secret `keys`", false), ("", "Keeps what the Secret holds in the repository", true)]
        );
        assert!(!format!("{steps:?}").contains("sk_live"));
        // A file that stopped being YAML has not lost its objects: it is said to be unreadable, and marked.
        let broken = read("kubernetes", "k8s/web.yaml", Some(WEB), Some("kind: [\nmetadata: {\n"));
        assert_eq!(broken.len(), 1, "{broken:?}");
        assert!(broken[0].text.starts_with("It cannot be read as YAML: "), "{broken:?}");
        assert!(broken[0].danger.is_some());
        // A template of a chart is not YAML before Helm fills it in, and is not held to it.
        let template = read("helm", "c/templates/x.yaml", None, Some("{{- if .Values.on }}\nkind: Job\n{{- end }}\n"));
        assert_eq!(said(&template), [("", "Adds the template of Job", false)]);
    }

    #[test]
    fn a_dockerfile_is_read_for_its_base_image_and_its_ports() {
        let old = "FROM node:18-alpine AS build\nRUN npm ci\nFROM build AS test\nFROM nginx:1.25\nEXPOSE 80\n";
        let new = "# syntax=docker/dockerfile:1\nFROM --platform=linux/amd64 node:20-alpine AS build\nRUN npm ci \\\n  && npm run build\nFROM build AS test\nFROM nginx:1.25\nEXPOSE 80 443\nARG NPM_TOKEN=abcdef123\n";
        assert_eq!(
            said(&read("docker", "Dockerfile", Some(old), Some(new))),
            [
                ("image", "base `node:20-alpine`, `nginx:1.25`, was `node:18-alpine`, `nginx:1.25`", false),
                ("ports", "exposes 80, 443, was 80", false),
                ("", "`NPM_TOKEN` is written in plain text", true),
            ]
        );
        assert_eq!(
            said(&dockerfile(None, Some("FROM python:3.12-slim\nEXPOSE 8000\n"))),
            [("image", "base `python:3.12-slim`", false), ("ports", "exposes 8000", false)]
        );
        assert_eq!(
            said(&dockerfile(Some("FROM a\nRUN x\n"), Some("FROM a\nRUN y\n"))),
            [("", "Changes how the image is built, on the same base and with the same ports", false)]
        );
        assert_eq!(said(&dockerfile(Some("FROM a\n"), None)), [("", "Removes the Dockerfile", true)]);
    }

    #[test]
    fn an_image_named_through_an_argument_is_read_by_what_the_argument_holds() {
        let file = |node: &str| {
            format!(
                "ARG IMAGE_PREFIX=\nARG NODE_BASE_IMAGE=library/node:{node}-bookworm\nARG NGINX=nginxinc/nginx-unprivileged:slim\n\
                 FROM ${{IMAGE_PREFIX}}${{NODE_BASE_IMAGE}} AS setup\nRUN npm ci\nFROM setup AS build\n\
                 FROM ${{IMAGE_PREFIX}}$NGINX AS production\nFROM scratch AS export\n"
            )
        };
        // The line that starts the stage is the same; the image it stands for is not.
        assert_eq!(
            said(&dockerfile(Some(&file("24.14.1")), Some(&file("25.0.0")))),
            [(
                "image",
                "base `library/node:25.0.0-bookworm`, `nginxinc/nginx-unprivileged:slim`, `scratch`, was `library/node:24.14.1-bookworm`, `nginxinc/nginx-unprivileged:slim`, `scratch`",
                false
            )]
        );
        // What the file does not give stays as it is written; a value to fall back on is used.
        let given = BTreeMap::from([("TAG".to_string(), "1.2".to_string()), ("EMPTY".to_string(), String::new())]);
        assert_eq!(filled("app:${TAG}-$TAG.${OTHER}", &given), "app:1.2-1.2.${OTHER}");
        assert_eq!(filled("${REGISTRY:-docker.io}/app:${EMPTY:-latest}$", &given), "docker.io/app:latest$");
        assert_eq!(filled("${EMPTY}app", &given), "app");
    }

    #[test]
    fn a_file_as_it_stands_is_said_in_the_words_of_a_change_without_adds() {
        // Terraform: every block by what it is; a secret written out is marked as in a change.
        let tf = "resource \"aws_db_instance\" \"orders\" {\n  password = \"hunter22\"\n}\nmodule \"net\" {\n  source = \"./net\"\n}\n";
        assert_eq!(
            said(&whole("terraform", "infra/main.tf", tf)),
            [
                ("", "Resource `aws_db_instance.orders`", false),
                ("", "Module `net`", false),
                ("", "`password` is written in plain text", true),
            ]
        );
        assert_eq!(said(&whole("terraform", "prod.tfvars", "region = \"eu\"\n")), [("", "Sets `region`", false)]);
        // A manifest: the object, then what it says in the fields that matter.
        let web = "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: web\nspec:\n  replicas: 2\n  template:\n    spec:\n      containers:\n        - name: web\n          image: shop/web:1.4\n          securityContext:\n            privileged: true\n";
        assert_eq!(
            said(&whole("kubernetes", "k8s/web.yaml", web)),
            [
                ("", "Deployment `web`", false),
                ("replicas", "2", false),
                ("image", "of `web`: `shop/web:1.4`", false),
                ("", "`web` runs privileged", true),
            ]
        );
        // A compose file: its services and volumes, and whatever else it sets.
        let compose =
            "name: shop\nservices:\n  db:\n    image: postgres:16\n    ports: ['5432:5432']\nvolumes:\n  data: {}\n";
        assert_eq!(
            said(&whole("compose", "compose.yaml", compose)),
            [
                ("", "Service `db`", false),
                ("image", "of `db`: `postgres:16`", false),
                ("ports", "of `db`: 5432:5432", false),
                ("", "Volume `data`", false),
                ("", "Also sets `name`", false),
            ]
        );
        // A chart and its values; what may be a secret is named and never repeated.
        assert_eq!(
            said(&whole("helm", "deploy/shop/Chart.yaml", "name: shop\nversion: 1.0.0\n")),
            [("", "The chart `shop`", false), ("", "The version of the chart is `1.0.0`", false)]
        );
        assert_eq!(
            said(&whole("helm", "deploy/shop/values.yaml", "image:\n  tag: '1.4'\ndb:\n  password: ${DB}\n")),
            [("", "`db.password` is set", false), ("image", "`image.tag`: `1.4`", false)]
        );
        // The pipelines: their jobs, and what starts them.
        assert_eq!(
            said(&whole("ci", ".gitlab-ci.yml", "stages: [test]\nvariables:\n  NODE: '20'\nunit:\n  script: [x]\n")),
            [
                ("", "Job `unit`", false),
                ("", "The stages are test", false),
                ("", "Variables of the pipeline: `NODE`", false),
            ]
        );
        assert_eq!(
            said(&whole("ci", ".github/workflows/ci.yml", "on: push\njobs:\n  test:\n    runs-on: ubuntu-latest\n")),
            [("", "Job `test`", false), ("", "Runs on push", false)]
        );
        // A Dockerfile: what it starts from, its ports, and who the image runs as when its last stage ends.
        let image = "ARG NODE=node:20\nFROM ${NODE} AS build\nUSER root\nFROM nginx:1.27 AS production\nUSER nginx\nEXPOSE 8080\n";
        assert_eq!(
            said(&whole("docker", "Dockerfile", image)),
            [
                ("image", "base `node:20`, `nginx:1.27`", false),
                ("ports", "exposes 8080", false),
                ("", "Runs as `nginx`", false),
            ]
        );
        assert_eq!(
            said(&whole("docker", "Dockerfile", "FROM a\nUSER 0\n")),
            [("image", "base `a`", false), ("", "Runs as `0`", true)]
        );
        // One that names no user says nothing of it.
        assert_eq!(said(&whole("docker", "Dockerfile", "FROM a\n")), [("image", "base `a`", false)]);
        // A file that is not YAML says so.
        assert!(whole("compose", "compose.yaml", "services: [\n")[0].text.starts_with("It cannot be read as YAML"));
    }

    #[test]
    fn a_stage_that_starts_to_run_as_root_is_marked() {
        let old = "FROM node:20 AS setup\nUSER app\nFROM setup AS build\nRUN npm ci\nFROM nginx AS production\nUSER root\nRUN chmod 755 /x\nUSER nginx\n";
        // The last word of the stage is root now.
        let new = old.replace("USER nginx", "USER 0:0");
        assert_eq!(
            said(&dockerfile(Some(old), Some(&new))),
            [("", "Stage `production` runs as `0:0`, was `nginx`", true)]
        );
        // The stage it is built on stops naming a user: the one built on it goes with it.
        let new = old.replace("USER app\n", "");
        assert_eq!(
            said(&dockerfile(Some(old), Some(&new))),
            [
                ("", "Stage `setup` no longer says who it runs as, was `app`", true),
                ("", "Stage `build` no longer says who it runs as, was `app`", true),
            ]
        );
        // One stage without a name is the image. Another user that is not root is no danger.
        assert_eq!(
            said(&dockerfile(Some("FROM a\nUSER app\n"), Some("FROM a\nUSER root\n"))),
            [("", "The image runs as `root`, was `app`", true)]
        );
        assert_eq!(
            said(&dockerfile(Some("FROM a\nUSER app\n"), Some("FROM a\nUSER web\n"))),
            [("", "Changes how the image is built, on the same base and with the same ports", false)]
        );
        // An image that was root all along, or is new, is not news.
        assert!(
            dockerfile(Some("FROM a\nUSER root\n"), Some("FROM a\nUSER root\nRUN x\n"))
                .iter()
                .all(|step| step.danger.is_none())
        );
        assert!(dockerfile(None, Some("FROM a\nUSER root\n")).iter().all(|step| step.danger.is_none()));
    }

    #[test]
    fn a_compose_file_is_read_service_by_service() {
        let old = "services:\n  web:\n    build: .\n    ports: ['3000:3000']\n  db:\n    image: postgres:15\n    volumes: ['pgdata:/var/lib/postgresql/data']\n  mail:\n    image: mailhog\nvolumes:\n  pgdata: {}\n  uploads: {}\n";
        let new = "services:\n  web:\n    build: .\n    ports: ['3000:3000', '9229:9229']\n    environment:\n      NODE_ENV: development\n  db:\n    image: postgres:16\n    volumes: ['./data:/var/lib/postgresql/data']\n  dind:\n    image: docker:27-dind\n    privileged: true\nvolumes:\n  pgdata: {}\n";
        assert_eq!(
            said(&compose(old, new)),
            [
                ("", "Changes service `db`", true),
                ("image", "of `db`: `postgres:16`, was `postgres:15`", false),
                (
                    "",
                    "The volumes of `db` are ./data:/var/lib/postgresql/data, were pgdata:/var/lib/postgresql/data",
                    true
                ),
                ("", "Adds service `dind`", false),
                ("image", "of `dind`: `docker:27-dind`", false),
                ("", "`dind` runs privileged", true),
                ("", "Changes service `web`", false),
                ("ports", "of `web`: 3000:3000, 9229:9229, was 3000:3000", false),
                ("env", "of `web`: adds `NODE_ENV`", false),
                ("", "Removes service `mail`", true),
                ("", "Removes volume `uploads`", true),
            ]
        );
    }

    #[test]
    fn the_jobs_of_a_pipeline_are_read_as_added_changed_and_removed() {
        let old = "stages: [test]\n.base: &base\n  image: node:20\nlint:\n  <<: *base\n  script: npm run lint\ntest:\n  <<: *base\n  script: npm test\n";
        let new = "stages: [test, deploy]\n.base: &base\n  image: node:20\ntest:\n  <<: *base\n  script: npm test -- --ci\ndeploy:\n  stage: deploy\n  script: ./deploy.sh\n";
        assert_eq!(
            said(&gitlab(old, new)),
            [
                ("", "Adds job `deploy`", false),
                ("", "Changes job `test`: `script`", false),
                ("", "Removes job `lint`", true),
                ("", "The stages are test, deploy, was test", false),
            ]
        );
        let workflow = |jobs: &str, on: &str| format!("name: CI\non: {on}\njobs:\n{jobs}");
        let before = workflow("  test:\n    runs-on: ubuntu-latest\n  lint:\n    runs-on: ubuntu-latest\n", "push");
        let after = workflow(
            "  test:\n    runs-on: ubuntu-24.04\n  release:\n    runs-on: ubuntu-latest\n",
            "[push, workflow_dispatch]",
        );
        assert_eq!(
            said(&github(&before, &after)),
            [
                ("", "Adds job `release`", false),
                ("", "Changes job `test`: `runs-on`", false),
                ("", "Removes job `lint`", true),
                ("", "Runs on push, workflow_dispatch, was push", false),
            ]
        );
        assert_eq!(
            said(&github("", &before)),
            [("", "Adds job `lint`", false), ("", "Adds job `test`", false), ("", "Runs on push", false)]
        );
    }

    #[test]
    fn a_chart_is_read_for_its_versions_its_values_and_its_templates() {
        let chart = |version: &str, needs: &str| {
            format!(
                "apiVersion: v2\nname: shop\nversion: {version}\nappVersion: \"2.0\"\ndependencies:\n  - name: postgresql\n    version: {needs}\n"
            )
        };
        assert_eq!(
            said(&helm("deploy/shop/Chart.yaml", Some(&chart("1.2.0", "12.1.0")), Some(&chart("1.3.0", "13.0.0")))),
            [
                ("", "The version of the chart is `1.3.0`, was `1.2.0`", false),
                ("", "Needs the chart `postgresql` `13.0.0`, was `12.1.0`", false),
            ]
        );
        let values = helm(
            "deploy/shop/values.yaml",
            Some("image:\n  tag: '1.4'\nreplicaCount: 2\nadmin:\n  password: old-one\nservice:\n  port: 80\n"),
            Some("image:\n  tag: '1.5'\nreplicaCount: 3\nadmin:\n  password: new-one\ningress:\n  enabled: true\n"),
        );
        assert_eq!(
            said(&values),
            [
                ("", "`admin.password` is set anew", false),
                ("image", "`image.tag`: `1.5`, was `1.4`", false),
                ("", "`ingress.enabled`: `true`", false),
                ("replicas", "`replicaCount`: `3`, was `2`", false),
                ("ports", "`service.port` is no longer set, was `80`", false),
            ]
        );
        let template = "apiVersion: apps/v1\nkind: Deployment\nspec:\n  replicas: {{ .Values.replicaCount }}\n  template:\n    spec:\n      containers:\n        - image: \"{{ .Values.image.repository }}:{{ .Values.image.tag }}\"\n          securityContext:\n            privileged: true\n";
        assert_eq!(
            said(&helm("deploy/shop/templates/deployment.yaml", None, Some(template))),
            [
                ("", "Adds the template of Deployment", false),
                ("replicas", "`{{ .Values.replicaCount }}`", false),
                ("image", "`{{ .Values.image.repository }}:{{ .Values.image.tag }}`", false),
                ("", "A container runs privileged", true),
            ]
        );
    }
}
