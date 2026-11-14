//! The API of a task's service: what it offers, a request to try, and what the task changed in it.
//!
//! A page can be looked at. A service that answers in JSON cannot, so its
//! preview is another thing: the list of what it can be asked, a request sent
//! to the task's own running service, and the answer with its status and the
//! time it took.
//!
//! The list comes from a description of the API, OpenAPI or Swagger (see
//! `spec`). It is looked for in two places:
//!
//! - **a file of the project**, such as `openapi.yaml`, for a project that
//!   writes the description first;
//! - **the running service**, at one of the paths frameworks serve it under,
//!   such as `/openapi.json`, for a project whose description is made of its code.
//!
//! The description of the base is found the same way: the same file as it was
//! where the task's branch started, or a service that runs the base branch,
//! whose address the user gives. The two are compared (see `contract`), and
//! the operations the task touched come first in the list.
//!
//! Requests go only to this computer, as with the page preview. The
//! description of the base may be read from anywhere: it is only read.

pub mod contract;
pub mod spec;

use crate::gitops;
use crate::preview::Target;
use crate::server::AppState;
use crate::tasks::Manager;
use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use std::io::Read;
use std::path::Path;
use std::time::{Duration, Instant};

/// The names a description goes by as a file. A file whose name starts as one of these does is taken too.
const FILE_NAMES: [&str; 6] =
    ["openapi.json", "openapi.yaml", "openapi.yml", "swagger.json", "swagger.yaml", "swagger.yml"];
/// The folders of a project a description is looked for in.
const FILE_FOLDERS: [&str; 10] = ["", "docs", "doc", "api", "spec", "specs", "openapi", "schema", "public", "static"];
/// Where frameworks serve the description of a running service.
const SERVICE_PATHS: [&str; 12] = [
    "/openapi.json",
    "/api/openapi.json",
    "/api/v1/openapi.json",
    "/openapi.yaml",
    "/swagger.json",
    "/v3/api-docs",
    "/api-docs",
    "/api-json",
    "/docs-json",
    "/api/schema/",
    "/swagger/v1/swagger.json",
    "/swagger/doc.json",
];
/// A description longer than this is not read.
const MAX_SPEC: u64 = 16 * 1024 * 1024;
/// This much of an answer is shown. The rest is said to be there.
const MAX_ANSWER: u64 = 2 * 1024 * 1024;
/// How long a service may take to give its description.
const SPEC_PATIENCE: Duration = Duration::from_secs(5);
/// How long a request that is tried may take.
const SEND_PATIENCE: Duration = Duration::from_secs(60);
/// How many headers are kept for a project, and how long one may be.
const MOST_HEADERS: usize = 20;
const MAX_HEADER: usize = 4096;
/// The methods a request can be sent with.
const SEND_METHODS: [&str; 7] = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

fn address_key(task_id: &str) -> String {
    format!("task.{task_id}.api.address")
}

fn project_key(project_id: &str, name: &str) -> String {
    format!("project.{project_id}.api.{name}")
}

/// Where a description was read from.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Source {
    /// A file of the task's folder, by its path in it.
    File(String),
    /// The running service, at this path.
    Service(String),
}

impl Source {
    fn shown(&self) -> Value {
        match self {
            Source::File(path) => json!({ "kind": "file", "where": path }),
            Source::Service(path) => json!({ "kind": "service", "where": path }),
        }
    }
}

/// What one address answered when asked for a description.
enum Asked {
    Found(Value),
    /// It answered, and not with a description.
    NotThere,
    /// Nothing answers there at all.
    Silent(String),
}

/// Asks one address for a description. `follow` lets the answer send the question on to another
/// address: never for the task's own service, which is asked with the headers kept for the project,
/// and those must not leave this computer.
fn ask(url: &str, headers: &[(String, String)], follow: bool) -> Asked {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(SPEC_PATIENCE))
        .http_status_as_error(false)
        .max_redirects(if follow { 3 } else { 0 })
        .max_redirects_will_error(false)
        .build()
        .into();
    let mut request = agent.get(url).header("Accept", "application/json, application/yaml, */*");
    for (name, value) in headers {
        request = request.header(name, value);
    }
    let mut response = match request.call() {
        Ok(response) => response,
        Err(error) => return Asked::Silent(error.to_string()),
    };
    if response.status().as_u16() != 200 {
        return Asked::NotThere;
    }
    let mut bytes = Vec::new();
    if response.body_mut().as_reader().take(MAX_SPEC).read_to_end(&mut bytes).is_err() {
        return Asked::NotThere;
    }
    match spec::read(&bytes) {
        Ok(found) => Asked::Found(found),
        Err(_) => Asked::NotThere,
    }
}

/// Looks for the description of a running service: at the one path given, or at the usual ones.
/// `Err` when nothing answers at the address.
fn from_service(
    origin: &str,
    path: Option<&str>,
    headers: &[(String, String)],
    follow: bool,
) -> Result<Option<(String, Value)>> {
    let paths: Vec<&str> = match path {
        Some(one) => vec![one],
        None => SERVICE_PATHS.to_vec(),
    };
    for path in paths {
        match ask(&format!("{origin}{path}"), headers, follow) {
            Asked::Found(found) => return Ok(Some((path.to_string(), found))),
            Asked::NotThere => {}
            Asked::Silent(why) => bail!("{why}"),
        }
    }
    Ok(None)
}

/// A path inside the task's folder, as the user may type it. Nothing that leads out of the folder.
fn inside(path: &str) -> Option<String> {
    let path = path.trim().trim_start_matches("./").replace('\\', "/");
    let fine = !path.is_empty()
        && !path.starts_with('/')
        && path.split('/').all(|part| !part.is_empty() && part != ".." && part != ".");
    fine.then_some(path)
}

/// Looks for the description among the files of a folder: the one named, or one under a usual name.
fn from_files(root: &Path, named: Option<&str>) -> Result<Option<(String, Value)>> {
    if let Some(path) = named {
        let bytes =
            std::fs::read(root.join(path)).map_err(|_| anyhow!("There is no file {path} in the task's folder"))?;
        let found = spec::read(&bytes).with_context(|| format!("{path} could not be read"))?;
        return Ok(Some((path.to_string(), found)));
    }
    for folder in FILE_FOLDERS {
        // The usual names first, then whatever else starts as they do: `openapi.snapshot.json`, `swagger-v2.yaml`.
        let mut names: Vec<String> = FILE_NAMES.iter().map(|name| name.to_string()).collect();
        let mut others: Vec<String> = std::fs::read_dir(root.join(folder))
            .into_iter()
            .flatten()
            .flatten()
            .map(|one| one.file_name().to_string_lossy().into_owned())
            .filter(|name| {
                let lower = name.to_lowercase();
                (lower.starts_with("openapi") || lower.starts_with("swagger"))
                    && [".json", ".yaml", ".yml"].iter().any(|end| lower.ends_with(end))
                    && !names.contains(name)
            })
            .collect();
        others.sort();
        names.extend(others.into_iter().take(8));
        for name in names {
            let path = if folder.is_empty() { name.to_string() } else { format!("{folder}/{name}") };
            let Ok(meta) = std::fs::metadata(root.join(&path)) else { continue };
            if !meta.is_file() || meta.len() > MAX_SPEC {
                continue;
            }
            if let Some(found) = std::fs::read(root.join(&path)).ok().and_then(|bytes| spec::read(&bytes).ok()) {
                return Ok(Some((path, found)));
            }
        }
    }
    Ok(None)
}

/// The headers kept for a project: sent with every request, such as the one that says who asks.
fn kept_headers(manager: &Manager, project_id: &str) -> Vec<(String, String)> {
    manager
        .store()
        .setting(&project_key(project_id, "headers"))
        .ok()
        .flatten()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// A header as it may be sent: a name of plain signs and a value on one line.
fn header(name: &str, value: &str) -> Result<(String, String)> {
    let (name, value) = (name.trim(), value.trim());
    let plain = !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b));
    if !plain {
        bail!("\"{name}\" is not a name a header can have");
    }
    if value.len() > MAX_HEADER || value.chars().any(char::is_control) {
        bail!("The value of {name} is too long or is not on one line");
    }
    Ok((name.to_string(), value.to_string()))
}

fn headers_param(params: &Value) -> Result<Vec<(String, String)>> {
    let mut headers = Vec::new();
    for pair in params.get("headers").and_then(Value::as_array).map_or(&[][..], Vec::as_slice) {
        let part = |at: usize| pair.get(at).and_then(Value::as_str).unwrap_or_default();
        if part(0).trim().is_empty() {
            continue;
        }
        headers.push(header(part(0), part(1))?);
    }
    if headers.len() > MOST_HEADERS {
        bail!("That is more headers than are kept: {MOST_HEADERS} at most");
    }
    Ok(headers)
}

/// An address of the base as the user gives it: where it is, and the path of the description when one is given.
fn base_address(text: &str) -> Result<(String, Option<String>)> {
    let text = text.trim();
    let with_scheme = if text.contains("://") { text.to_string() } else { format!("http://{text}") };
    let (scheme, rest) = with_scheme.split_once("://").unwrap_or_default();
    if !matches!(scheme, "http" | "https") || rest.is_empty() || rest.contains(char::is_whitespace) {
        bail!("\"{text}\" is not an address. Give one such as localhost:8000 or https://staging.example.com");
    }
    let (host, path) = match rest.find('/') {
        Some(at) => (&rest[..at], rest[at..].trim_end_matches('/')),
        None => (rest, ""),
    };
    if host.is_empty() {
        bail!("\"{text}\" is not an address. Give one such as localhost:8000 or https://staging.example.com");
    }
    Ok((format!("{scheme}://{host}"), (!path.is_empty()).then(|| path.to_string())))
}

/// Everything the API view of a task shows.
fn get(manager: &Manager, params: &Value) -> Result<Value> {
    let task_id = text(params, "taskId")?;
    let task = manager.store().task(task_id)?;
    let (root, project_id) = (Path::new(&task.cwd), task.project_id.as_str());
    let kept = |name: &str| manager.store().setting(&project_key(project_id, name)).ok().flatten().unwrap_or_default();
    let (set_address, set_spec, set_base) =
        (manager.store().setting(&address_key(task_id))?.unwrap_or_default(), kept("spec"), kept("base"));
    let headers = kept_headers(manager, project_id);

    // Where requests go: what the user gave, else the one of the running services that describes
    // itself, else the first of them.
    let guesses: Vec<String> = params
        .get("guess")
        .and_then(Value::as_array)
        .map(|all| all.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default();
    let candidates: Vec<String> = if set_address.is_empty() { guesses } else { vec![set_address.clone()] };

    let on_service = set_spec.starts_with('/').then_some(set_spec.as_str());
    let in_file = if set_spec.is_empty() || on_service.is_some() { None } else { inside(&set_spec) };
    if !set_spec.is_empty() && on_service.is_none() && in_file.is_none() {
        bail!("\"{set_spec}\" is neither a path on the service (those start with /) nor a file of the project");
    }

    let mut note: Option<String> = None;
    let mut found: Option<(Source, Value)> = None;
    let mut address: Option<String> = None;
    // A file is the description the project itself keeps: it is taken before what a service says.
    if on_service.is_none() {
        match from_files(root, in_file.as_deref()) {
            Ok(Some((path, spec))) => found = Some((Source::File(path), spec)),
            Ok(None) => {}
            Err(error) => note = Some(format!("{error:#}")),
        }
    }
    for candidate in &candidates {
        let Ok(target) = Target::parse(candidate) else {
            note = Some(format!(
                "\"{candidate}\" is not an address on this computer. Requests are only sent to this computer."
            ));
            continue;
        };
        if address.is_none() {
            address = Some(target.authority());
        }
        if found.is_some() || in_file.is_some() {
            // The description is a file. Of the services that run, the one that describes itself is
            // the API; a page of the same project may run beside it, and requests are not for that.
            let describes = || matches!(from_service(&target.origin(), None, &headers, false), Ok(Some(_)));
            if set_address.is_empty() && describes() {
                address = Some(target.authority());
                break;
            }
            continue;
        }
        match from_service(&target.origin(), on_service, &headers, false) {
            Ok(Some((path, spec))) => {
                address = Some(target.authority());
                found = Some((Source::Service(path), spec));
                break;
            }
            Ok(None) if note.is_none() => {
                let looked = on_service.map_or_else(|| SERVICE_PATHS[..5].join(", "), str::to_string);
                note = Some(format!(
                    "The service at {} gave no description of its API. It was asked at {looked}{}.",
                    target.authority(),
                    if on_service.is_some() { "" } else { " and other usual paths" }
                ));
            }
            Ok(None) => {}
            Err(_) if note.is_none() => {
                note = Some(format!(
                    "Nothing answers at {}. Start the service in Run, or give another address.",
                    target.authority()
                ));
            }
            Err(_) => {}
        }
    }
    if found.is_none() && note.is_none() {
        note = Some(match candidates.is_empty() {
            true => {
                "No description of the API was found among the files, and no service of this task is running to ask."
                    .into()
            }
            false => "No description of the API was found.".into(),
        });
    }

    let settings = json!({ "address": set_address, "spec": set_spec, "base": set_base, "headers": headers });
    let Some((source, branch)) = found else {
        return Ok(json!({
            "address": address, "source": null, "title": "", "version": "", "prefix": "", "operations": [],
            "note": note, "contract": null, "contractNote": null, "settings": settings
        }));
    };

    // The description of the base, to say what the task changed.
    let mut contract_note: Option<String> = None;
    let mut against: Option<(String, Option<Value>)> = None;
    if !set_base.is_empty() {
        match base_address(&set_base).and_then(|(origin, path)| {
            from_service(&origin, path.as_deref(), &[], true)
                .map_err(|why| anyhow!("Nothing answers at {set_base}, where the base should run ({why})"))?
                .ok_or_else(|| anyhow!("The service at {set_base} gave no description of its API"))
        }) {
            Ok((_, base)) => against = Some((set_base.clone(), Some(base))),
            Err(error) => contract_note = Some(format!("{error:#}")),
        }
    } else if let Source::File(path) = &source {
        let base = task.branch.as_ref().and_then(|_| gitops::resolve_base(root, task.base_branch.as_deref()));
        match base.as_deref().and_then(|base| gitops::fork_point(root, base).map(|start| (base, start))) {
            Some((base, start)) => match gitops::bytes_at(root, &start, path) {
                Ok(Some(bytes)) => match spec::read(&bytes) {
                    Ok(before) => against = Some((base.to_string(), Some(before))),
                    Err(_) => {
                        contract_note = Some(format!(
                            "{path} as it is in {base} could not be read, so there is nothing to compare with."
                        ))
                    }
                },
                // The file is the task's own: all of it is new.
                Ok(None) => against = Some((base.to_string(), None)),
                Err(error) => contract_note = Some(format!("{error:#}")),
            },
            None => contract_note = Some("This task has no base branch to compare the description with.".into()),
        }
    } else {
        contract_note = Some(
            "To see what this task changes in the contract, give the address of a service that runs the base branch: \
             the project's own folder on another port, or a staging server."
                .into(),
        );
    }

    let mut operations = spec::operations(&branch);
    let contract = against.map(|(name, base)| {
        let empty = json!({ "paths": {} });
        let changes = contract::compare(base.as_ref().unwrap_or(&empty), &branch);
        for operation in &mut operations {
            let mine: Vec<_> = changes.iter().filter(|change| change.operation == operation.id).collect();
            operation.change = match mine.as_slice() {
                [] => None,
                [only] if only.text == "It is new" => Some("new"),
                _ => Some("changed"),
            };
        }
        json!({ "against": name, "changes": changes })
    });
    // What the task touched comes first; the rest keeps its order.
    operations.sort_by_key(|operation| operation.change.is_none());
    let (title, version) = spec::title(&branch);
    Ok(json!({
        "address": address,
        "source": source.shown(),
        "title": title,
        "version": version,
        "prefix": spec::prefix(&branch),
        "operations": operations,
        "note": if address.is_none() { Some("No service of this task is running. Start one in Run, or give its address, to send requests.".to_string()) } else { None },
        "contract": contract,
        "contractNote": contract_note,
        "settings": settings
    }))
}

/// Keeps what the user chose: the address for the task, the rest for its project. An empty text forgets.
fn set(manager: &Manager, params: &Value) -> Result<Value> {
    let task_id = text(params, "taskId")?;
    let task = manager.store().task(task_id)?;
    let given = |name: &str| params.get(name).and_then(Value::as_str).map(str::trim);
    if let Some(address) = given("address") {
        if !address.is_empty() {
            Target::parse(address)?;
        }
        manager.store().set_setting(&address_key(task_id), (!address.is_empty()).then_some(address))?;
    }
    if let Some(place) = given("spec") {
        if !place.is_empty() && !place.starts_with('/') && inside(place).is_none() {
            bail!("\"{place}\" is neither a path on the service (those start with /) nor a file of the project");
        }
        manager.store().set_setting(&project_key(&task.project_id, "spec"), (!place.is_empty()).then_some(place))?;
    }
    if let Some(base) = given("base") {
        if !base.is_empty() {
            base_address(base)?;
        }
        manager.store().set_setting(&project_key(&task.project_id, "base"), (!base.is_empty()).then_some(base))?;
    }
    if params.get("headers").is_some() {
        let headers = headers_param(params)?;
        let text = serde_json::to_string(&headers)?;
        manager
            .store()
            .set_setting(&project_key(&task.project_id, "headers"), (!headers.is_empty()).then_some(text.as_str()))?;
    }
    Ok(json!({}))
}

/// Whether bytes are text a person can read, by what the service says they are and by what they are.
fn readable(kind: &str, bytes: &[u8]) -> bool {
    let kind = kind.to_ascii_lowercase();
    let texty = ["text/", "json", "xml", "yaml", "javascript", "x-www-form-urlencoded", "html", "csv"];
    let said_text = texty.iter().any(|word| kind.contains(word));
    (said_text || kind.is_empty() || !bytes.contains(&0)) && std::str::from_utf8(bytes).is_ok()
}

/// Sends one request to the task's service and gives the answer with the time it took.
fn send(manager: &Manager, params: &Value) -> Result<Value> {
    let task_id = text(params, "taskId")?;
    let task = manager.store().task(task_id)?;
    let address = text(params, "address")?;
    let target = Target::parse(address)?;
    let method = text(params, "method")?.to_ascii_uppercase();
    if !SEND_METHODS.contains(&method.as_str()) {
        bail!("{method} is not a method a request can be sent with here");
    }
    let path = text(params, "path")?;
    if !path.starts_with('/') || path.len() > 8192 || path.chars().any(|sign| sign.is_control() || sign == ' ') {
        bail!("The path of a request starts with / and has no spaces in it. Write a space as %20.");
    }
    let body = params.get("body").and_then(Value::as_str).unwrap_or_default();

    // The headers kept for the project first, so the ones of this request can say otherwise.
    let mut headers = kept_headers(manager, &task.project_id);
    for (name, value) in headers_param(params)? {
        headers.retain(|(known, _)| !known.eq_ignore_ascii_case(&name));
        headers.push((name, value));
    }
    let url = format!("{}{path}", target.origin());
    let mut request = ureq::http::Request::builder().method(method.as_str()).uri(&url);
    for (name, value) in &headers {
        request = request.header(name, value);
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(SEND_PATIENCE))
        .http_status_as_error(false)
        // A redirect is an answer to look at, not one to follow.
        .max_redirects(0)
        .max_redirects_will_error(false)
        .build()
        .into();
    let started = Instant::now();
    let sent = match body.is_empty() {
        true => agent.run(request.body(()).map_err(|error| anyhow!("The request could not be made: {error}"))?),
        false => {
            agent.run(request.body(body.as_bytes()).map_err(|error| anyhow!("The request could not be made: {error}"))?)
        }
    };
    let mut response = match sent {
        Ok(response) => response,
        Err(ureq::Error::Timeout(_)) => bail!("No answer came in {} seconds", SEND_PATIENCE.as_secs()),
        Err(ureq::Error::Io(_) | ureq::Error::ConnectionFailed) => {
            bail!("Nothing answers at {}. Start the service in Run, or give another address.", target.authority())
        }
        Err(error) => bail!("The request could not be sent: {error}"),
    };
    let mut bytes = Vec::new();
    let read = response.body_mut().as_reader().take(MAX_ANSWER + 1).read_to_end(&mut bytes);
    let took = started.elapsed().as_millis() as u64;
    if let Err(error) = read {
        bail!("The answer broke off while it was read: {error}");
    }
    let cut = bytes.len() as u64 > MAX_ANSWER;
    bytes.truncate(MAX_ANSWER as usize);
    let answered: Vec<(String, String)> = response
        .headers()
        .iter()
        .map(|(name, value)| (name.as_str().to_string(), String::from_utf8_lossy(value.as_bytes()).into_owned()))
        .collect();
    let kind =
        answered.iter().find(|(name, _)| name == "content-type").map(|(_, value)| value.clone()).unwrap_or_default();
    // A text cut in the middle of a sign is still text up to there.
    let whole = match std::str::from_utf8(&bytes) {
        Err(error) if cut => &bytes[..error.valid_up_to()],
        _ => &bytes[..],
    };
    let is_text = readable(&kind, whole);
    Ok(json!({
        "status": response.status().as_u16(),
        "reason": response.status().canonical_reason().unwrap_or_default(),
        "headers": answered,
        "body": if is_text { String::from_utf8_lossy(whole).into_owned() } else { String::new() },
        "kind": if !is_text { "binary" } else if kind.contains("json") { "json" } else { "text" },
        "size": bytes.len(),
        "cut": cut,
        "took": took,
        "sent": { "method": method, "url": url }
    }))
}

fn text<'a>(params: &'a Value, name: &str) -> Result<&'a str> {
    params.get(name).and_then(Value::as_str).ok_or_else(|| anyhow!("Missing parameter: {name}"))
}

/// The commands of the API view.
///
/// - `api.get { taskId, guess? }` gives the address, the operations and what the task changed.
///   `guess` is the addresses of the services that run, to ask when the user gave none.
/// - `api.set { taskId, address?, spec?, base?, headers? }` keeps a choice.
/// - `api.send { taskId, address, method, path, headers?, body? }` gives the answer of the service.
///
/// They wait for a service, so they run off the event loop.
pub async fn command(state: &AppState, method: &str, params: &Value) -> Result<Value> {
    let manager = state.manager.clone();
    let (method, params) = (method.to_string(), params.clone());
    tokio::task::spawn_blocking(move || match method.as_str() {
        "api.get" => get(&manager, &params),
        "api.set" => set(&manager, &params),
        "api.send" => send(&manager, &params),
        other => bail!("Unknown method: {other}"),
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_of_a_file_never_leads_out_of_the_folder() {
        assert_eq!(inside("docs/openapi.yaml").as_deref(), Some("docs/openapi.yaml"));
        assert_eq!(inside("./api/spec.json").as_deref(), Some("api/spec.json"));
        assert_eq!(inside("../secrets.json"), None);
        assert_eq!(inside("docs/../../x.json"), None);
        assert_eq!(inside("/etc/passwd"), None);
    }

    #[test]
    fn the_address_of_the_base_may_be_anywhere_and_may_name_the_description() {
        assert_eq!(base_address("localhost:8001").unwrap(), ("http://localhost:8001".into(), None));
        assert_eq!(
            base_address("https://staging.example.com/api/openapi.json").unwrap(),
            ("https://staging.example.com".into(), Some("/api/openapi.json".into()))
        );
        assert!(base_address("ftp://example.com").is_err());
        assert!(base_address("two words").is_err());
    }

    #[test]
    fn a_header_is_a_plain_name_and_one_line() {
        assert_eq!(header(" Authorization ", " Bearer abc ").unwrap(), ("Authorization".into(), "Bearer abc".into()));
        assert!(header("X Bad", "1").is_err());
        assert!(header("X-Fine", "two\nlines").is_err());
    }

    #[test]
    fn bytes_are_text_when_they_read_as_text() {
        assert!(readable("application/json", b"{\"a\":1}"));
        assert!(readable("", b"plain"));
        assert!(!readable("application/octet-stream", &[0, 159, 146, 150]));
        assert!(!readable("image/png", &[0x89, b'P', b'N', b'G', 0]));
    }
}
