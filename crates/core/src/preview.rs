//! Preview: the running app of a task, served through a relay of our own.
//!
//! The interface shows the app in frames. A page from another address cannot
//! be touched from the interface, so the app is not shown straight from its
//! dev server. Each task that is previewed gets a relay: a listener on a free
//! port of this computer that passes every request on to the app, HTTP and
//! WebSocket alike, and adds one script of ours to every HTML page. That
//! script is what sets the theme, picks elements and draws on the page; it
//! talks to the interface in messages between windows.
//!
//! The relay speaks plain HTTP/1.1 over the sockets itself: one request per
//! connection, the head read and rewritten, the rest passed on as bytes. A
//! WebSocket is therefore just bytes going both ways after its first answer,
//! and a file of any size never sits in memory. Only an HTML page is read
//! whole, because the script tag has to go inside it.
//!
//! A screenshot is taken through the relay too (see `shots`): for the moment
//! of one screenshot the relay serves a page of its own that holds the app's
//! page in a frame of exactly the asked width, the way the interface shows it.

use crate::devices::Devices;
use crate::tasks::Manager;
use crate::tools::Tools;
use anyhow::{Result, anyhow, bail};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinHandle;

/// Everything under this path is answered by the relay itself and never reaches the app.
const OWN_PREFIX: &str = "/__ae_preview/";
/// Where the relay serves the script it adds to pages.
pub const SCRIPT_PATH: &str = "/__ae_preview/page.js";
/// The script itself. It is kept as a file of its own so it can be read and linted as JavaScript.
const PAGE_SCRIPT: &str = include_str!("preview_page.js");
/// A request or answer whose first lines are longer than this is refused.
const MAX_HEAD: usize = 64 * 1024;
/// A page that says it is longer than this is passed on as it is, without the script.
const MAX_PAGE: u64 = 16 * 1024 * 1024;
/// A page that does not say its length is read up to this much; more is an error.
const MAX_PAGE_UNKNOWN: usize = 64 * 1024 * 1024;
/// How long the app may take to accept a connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// A header the relay adds to what it passes on, to notice a request that came back to it.
const HOP_HEADER: &str = "X-Ae-Preview";
/// How long the choices of the Preview view may be as stored text.
const MAX_SETTINGS: usize = 16 * 1024;
/// Under this path the relay serves the pages screenshots are taken of, each under a word only the core knows.
const SHOT_PREFIX: &str = "/__ae_preview/shot/";
/// How long after the app's page has loaded its picture is taken: what the page draws
/// a moment later, from an answer it waited for, should be in the picture.
const SETTLE: Duration = Duration::from_millis(1500);
/// The smallest picture there is: what the relay answers with when the wait above is over.
const TINY_GIF: &[u8] = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\x00\x00\x00\xff\xff\xff!\xf9\x04\x01\x00\x00\x00\x00,\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x01D\x00;";
/// A page of the app is at most this long as an address.
const MAX_PATH: usize = 2048;

/// The address of the app a relay passes requests to. Always on this computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// "localhost", "127.0.0.1" or "::1".
    pub host: String,
    pub port: u16,
}

impl Target {
    /// Reads an address as people write it: `localhost:3000`, `http://127.0.0.1:5173/login`, `[::1]:8080`.
    /// A path after the address is ignored: the relay passes every path on.
    pub fn parse(text: &str) -> Result<Target> {
        let text = text.trim();
        let lower = text.to_ascii_lowercase();
        if lower.starts_with("https://") {
            bail!("Only http addresses can be shown. https is not supported yet.");
        }
        let rest = if lower.starts_with("http://") { &text[7..] } else { text };
        if rest.contains("://") {
            bail!("Only http addresses can be shown.");
        }
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        if authority.is_empty() {
            bail!("Type the address of the running app, such as localhost:3000.");
        }
        if authority.contains('@') {
            bail!("An address with a user name in it cannot be shown.");
        }
        let (host, port) = if let Some(inner) = authority.strip_prefix('[') {
            let (host, after) = inner.split_once(']').ok_or_else(|| anyhow!("The address is missing a \"]\"."))?;
            (host, after.strip_prefix(':'))
        } else {
            match authority.rsplit_once(':') {
                Some((host, port)) => (host, Some(port)),
                None => (authority, None),
            }
        };
        let port = match port {
            None | Some("") => 80,
            Some(digits) => digits
                .parse::<u16>()
                .ok()
                .filter(|port| *port != 0)
                .ok_or_else(|| anyhow!("\"{digits}\" is not a port number."))?,
        };
        let host = host.to_ascii_lowercase();
        if !matches!(host.as_str(), "localhost" | "127.0.0.1" | "::1") {
            bail!(
                "Only an app on this computer can be shown: localhost, 127.0.0.1 or [::1]. \"{host}\" is somewhere else."
            );
        }
        Ok(Target { host, port })
    }

    /// The host and port as they stand in an address and in the `Host` header.
    pub fn authority(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    pub fn origin(&self) -> String {
        format!("http://{}", self.authority())
    }
}

/// Reads an address with the page it names: `localhost:3000/login?next=1` is the app
/// `localhost:3000` and its page `/login?next=1`. The page of a bare address is `/`.
pub fn split_address(text: &str) -> Result<(Target, String)> {
    let target = Target::parse(text)?;
    let text = text.trim();
    let rest = if text.to_ascii_lowercase().starts_with("http://") { &text[7..] } else { text };
    let after = rest.find(['/', '?', '#']).map_or("", |at| &rest[at..]);
    let path = if after.starts_with('/') { after.to_string() } else { format!("/{after}") };
    Ok((target, page_path(&path)?))
}

/// Checks the page of an app as an address inside it: it starts with one slash and is plain text.
/// Anything else could name another site, or carry more than an address into a page or a command.
pub fn page_path(path: &str) -> Result<String> {
    let plain = path.chars().all(|c| !c.is_control() && !c.is_whitespace());
    let inside = path.starts_with('/') && !path[1..].starts_with(['/', '\\']);
    if !plain || !inside || path.len() > MAX_PATH || path.starts_with(OWN_PREFIX) {
        bail!("\"{}\" is not a page of the app.", path.chars().take(80).collect::<String>());
    }
    Ok(path.to_string())
}

/// A page the relay serves for one screenshot: the app's page in a frame of one size.
#[derive(Debug, Clone)]
struct ShotPage {
    width: u32,
    height: u32,
    path: String,
    /// How high the browser's window was inside, as the page said once the app's page had loaded.
    /// `None` until then: a picture taken before that shows a page that was still loading.
    window_height: Option<u32>,
}

impl ShotPage {
    /// The page itself. The frame is as wide as the screenshot, so the app's page is laid out at
    /// that width whatever the browser's own window does (a window of Chrome is never narrower
    /// than 500 pixels, so a phone's width cannot be asked of the window). When the app's page
    /// has loaded, one more picture is asked of the relay, which answers after `SETTLE`: the
    /// browser takes its screenshot when everything has loaded, so this is what makes it wait.
    /// The asking also says how high the window is inside, so the core knows the frame fitted.
    fn html(&self, word: &str) -> String {
        let src = self.path.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;").replace('>', "&gt;");
        format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"><title>Screenshot</title>\
             <style>html,body{{margin:0;overflow:hidden;background:#fff}}\
             iframe{{position:absolute;left:0;top:0;width:{}px;height:{}px;border:0}}</style></head>\
             <body><iframe id=\"page\" src=\"{src}\" sandbox=\"allow-scripts allow-same-origin allow-forms\"></iframe>\
             <script>document.getElementById('page').addEventListener('load',function(){{\
             var wait=new Image();wait.src='{SHOT_PREFIX}{word}/settle?h='+window.innerHeight;}});</script></body></html>",
            self.width, self.height
        )
    }
}

/// The address of a page made for one screenshot. The relay stops serving it when this is dropped.
pub struct ShotTicket {
    relay: Arc<Relay>,
    word: String,
    /// Where a browser finds the page.
    pub url: String,
}

impl ShotTicket {
    /// How high the browser's window was inside when the app's page had loaded, in the page's pixels.
    /// `None` when the app's page never finished loading.
    pub fn window_height(&self) -> Option<u32> {
        self.relay.shots.lock().unwrap().get(&self.word).and_then(|page| page.window_height)
    }
}

impl Drop for ShotTicket {
    fn drop(&mut self) {
        self.relay.shots.lock().unwrap().remove(&self.word);
    }
}

/// What the interface is told about a relay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Opened {
    /// The port on 127.0.0.1 the relay listens on.
    pub port: u16,
    /// The app it passes requests to, such as `localhost:3000`.
    pub target: String,
}

/// One task's relay.
struct Relay {
    port: u16,
    /// Read anew for every request, so the app's address can change while frames stay on the same port.
    target: Mutex<Target>,
    /// The addresses of the interfaces that asked for this relay. The page's script talks to these and to nobody else.
    parents: Mutex<Vec<String>>,
    /// The pages being served for screenshots right now, by the word in their address.
    shots: Mutex<HashMap<String, ShotPage>>,
    stop: watch::Sender<bool>,
    accepting: Mutex<Option<JoinHandle<()>>>,
}

impl Relay {
    fn opened(&self) -> Opened {
        Opened { port: self.port, target: self.target.lock().unwrap().authority() }
    }

    /// The script as the relay serves it: our file, told which windows it may talk to.
    fn script(&self) -> String {
        let config = json!({ "parents": *self.parents.lock().unwrap() });
        format!("(function () {{\nvar AE_PREVIEW_CONFIG = {config};\n{PAGE_SCRIPT}\n}})();\n")
    }
}

/// Everything the Preview of every task runs: the relays, by task, and the devices the device tabs show.
pub struct Previews {
    relays: Mutex<HashMap<String, Arc<Relay>>>,
    /// Where the browser that takes screenshots is, and the tools of the device tabs.
    tools: Tools,
    /// The simulators and emulators that are shown.
    pub devices: Arc<Devices>,
}

impl Default for Previews {
    fn default() -> Self {
        Self::new()
    }
}

impl Previews {
    pub fn new() -> Self {
        Self::with_tools(Tools::from_env())
    }

    /// Previews that run the given programs and no others. Tests come in here with stand-ins.
    pub fn with_tools(tools: Tools) -> Self {
        Self { relays: Mutex::default(), devices: Arc::new(Devices::new(tools.clone())), tools }
    }

    pub fn tools(&self) -> &Tools {
        &self.tools
    }

    /// Makes the relay of a task serve a page for one screenshot: the app's page at `path`, in a
    /// frame `width` by `height`. The task's app must be on show: there is no relay otherwise.
    pub fn shot_page(&self, task_id: &str, width: u32, height: u32, path: &str) -> Result<ShotTicket> {
        let path = page_path(path)?;
        let relay = self.relays.lock().unwrap().get(task_id).cloned();
        let relay = relay
            .ok_or_else(|| anyhow!("Show the app in the Preview first: there is no page to take a picture of."))?;
        // Nobody but the core knows the word, so no other page can make the relay serve a frame of its choosing.
        let word = uuid::Uuid::new_v4().simple().to_string();
        relay.shots.lock().unwrap().insert(word.clone(), ShotPage { width, height, path, window_height: None });
        let url = format!("http://127.0.0.1:{}{SHOT_PREFIX}{word}", relay.port);
        Ok(ShotTicket { relay, word, url })
    }

    /// Starts the relay of a task, or points the one it has at another address.
    /// `parent` is the address of the interface that asks, as its own window knows it.
    /// `prefer` is the port to take when it is free: the one the task's relay had last time.
    /// Must be called inside the core's runtime: the relay runs as a task of it.
    pub fn open(&self, task_id: &str, target: &str, parent: Option<&str>, prefer: Option<u16>) -> Result<Opened> {
        let target = Target::parse(target)?;
        let mut relays = self.relays.lock().unwrap();
        let relay = match relays.get(task_id) {
            Some(relay) => relay.clone(),
            None => {
                // A page keeps what it stores under its address, port and all: the same port as
                // last time keeps the app signed in. Port 0 asks the system for any free port.
                // Only this computer can connect.
                let listener = match prefer.and_then(|port| std::net::TcpListener::bind(("127.0.0.1", port)).ok()) {
                    Some(listener) => listener,
                    None => std::net::TcpListener::bind(("127.0.0.1", 0))?,
                };
                listener.set_nonblocking(true)?;
                let listener = TcpListener::from_std(listener)?;
                let (stop, stopped) = watch::channel(false);
                let relay = Arc::new(Relay {
                    port: listener.local_addr()?.port(),
                    target: Mutex::new(target.clone()),
                    parents: Mutex::new(Vec::new()),
                    shots: Mutex::default(),
                    stop,
                    accepting: Mutex::new(None),
                });
                let accepting = tokio::spawn(accept(relay.clone(), listener, stopped));
                *relay.accepting.lock().unwrap() = Some(accepting);
                relays.insert(task_id.to_string(), relay.clone());
                relay
            }
        };
        if target.port == relay.port && target.host != "::1" {
            bail!("That is the address of the preview itself. Type the address of the running app.");
        }
        *relay.target.lock().unwrap() = target;
        if let Some(parent) = parent.filter(|parent| is_origin(parent)) {
            let mut parents = relay.parents.lock().unwrap();
            if !parents.iter().any(|known| known == parent) {
                parents.push(parent.to_string());
            }
        }
        Ok(relay.opened())
    }

    /// The relay of a task, when it has one.
    pub fn state(&self, task_id: &str) -> Option<Opened> {
        self.relays.lock().unwrap().get(task_id).map(|relay| relay.opened())
    }

    /// Whether one of the relays listens on `port`. The desktop app asks, to let the pages
    /// of a preview load inside its window, where every other address goes to the browser.
    pub fn serves(&self, port: u16) -> bool {
        self.relays.lock().unwrap().values().any(|relay| relay.port == port)
    }

    /// Stops the relay of a task and lets go of the device it shows. When this returns, nothing listens on its port.
    pub async fn close(&self, task_id: &str) {
        let relay = self.relays.lock().unwrap().remove(task_id);
        if let Some(relay) = relay {
            stop(relay).await;
        }
        self.devices.close(task_id).await;
    }

    /// Stops every relay and lets go of every device. Called when the core shuts down.
    pub async fn close_all(&self) {
        let all: Vec<Arc<Relay>> = self.relays.lock().unwrap().drain().map(|(_, relay)| relay).collect();
        for relay in all {
            stop(relay).await;
        }
        self.devices.close_all().await;
    }
}

async fn stop(relay: Arc<Relay>) {
    // Every connection of the relay watches this and ends with it.
    let _ = relay.stop.send(true);
    let accepting = relay.accepting.lock().unwrap().take();
    if let Some(accepting) = accepting {
        // The listener is dropped when the accepting task ends: only then is the port free.
        let _ = accepting.await;
    }
}

/// Whether a text reads as the address of a window: a scheme, a host, perhaps a port, and nothing else.
fn is_origin(text: &str) -> bool {
    let Some((scheme, rest)) = text.split_once("://") else { return false };
    text.len() <= 200
        && !scheme.is_empty()
        && scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
        && !rest.is_empty()
        && rest.chars().all(|c| c.is_ascii_alphanumeric() || ".-:[]".contains(c))
}

/// The key the choices of a task's Preview view are stored under.
fn settings_key(task_id: &str) -> String {
    format!("task.{task_id}.preview")
}

/// The key the port of a task's relay is stored under, to ask for the same one next time.
fn port_key(task_id: &str) -> String {
    format!("task.{task_id}.previewPort")
}

/// The commands of the Preview view.
///
/// - `preview.state { taskId }` gives `{ relay: { port, target } | null, settings: {...} | null }`.
/// - `preview.open { taskId, target, origin? }` gives `{ port, target }`.
/// - `preview.close { taskId }` and `preview.remember { taskId, settings }` give `{}`.
pub async fn command(previews: &Previews, manager: &Manager, method: &str, params: &Value) -> Result<Value> {
    let task_id = params.get("taskId").and_then(Value::as_str).ok_or_else(|| anyhow!("Missing parameter: taskId"))?;
    // A task that is not there has nothing to preview.
    manager.task(task_id)?;
    match method {
        "preview.state" => {
            let settings = manager
                .store()
                .setting(&settings_key(task_id))?
                .and_then(|text| serde_json::from_str::<Value>(&text).ok());
            Ok(json!({ "relay": previews.state(task_id), "settings": settings }))
        }
        "preview.open" => {
            let target =
                params.get("target").and_then(Value::as_str).ok_or_else(|| anyhow!("Missing parameter: target"))?;
            let origin = params.get("origin").and_then(Value::as_str);
            let key = port_key(task_id);
            let last = manager.store().setting(&key)?.and_then(|port| port.parse::<u16>().ok());
            let opened = previews.open(task_id, target, origin, last)?;
            if last != Some(opened.port) {
                manager.store().set_setting(&key, Some(&opened.port.to_string()))?;
            }
            Ok(json!(opened))
        }
        "preview.close" => {
            previews.close(task_id).await;
            Ok(json!({}))
        }
        "preview.remember" => {
            let settings = params.get("settings").filter(|value| value.is_object());
            let text = settings.ok_or_else(|| anyhow!("Missing parameter: settings"))?.to_string();
            if text.len() > MAX_SETTINGS {
                bail!("That is too much to remember for one preview");
            }
            manager.store().set_setting(&settings_key(task_id), Some(&text))?;
            Ok(json!({}))
        }
        other => bail!("Unknown method: {other}"),
    }
}

/// Takes connections until the relay is stopped.
async fn accept(relay: Arc<Relay>, listener: TcpListener, mut stopped: watch::Receiver<bool>) {
    loop {
        tokio::select! {
            _ = stopped.changed() => break,
            accepted = listener.accept() => {
                let Ok((client, _)) = accepted else {
                    // Out of sockets, most likely. A short rest, so the loop does not spin.
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                };
                let relay = relay.clone();
                let mut stopped = stopped.clone();
                tokio::spawn(async move {
                    tokio::select! {
                        _ = stopped.changed() => {}
                        // A connection that breaks half way is the browser's or the app's business, not an error of ours.
                        _ = serve(&relay, client) => {}
                    }
                });
            }
        }
    }
}

/// The first lines of a request or of an answer: the line that says what it is, then the headers.
/// Header values are kept as bytes, so nothing a browser or an app sends is changed by accident.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Head {
    first: String,
    headers: Vec<(String, Vec<u8>)>,
}

impl Head {
    fn parse(bytes: &[u8]) -> Option<Head> {
        let mut lines = bytes.split(|byte| *byte == b'\n').map(|line| line.strip_suffix(b"\r").unwrap_or(line));
        let first = String::from_utf8(lines.next()?.to_vec()).ok()?;
        if first.trim().is_empty() {
            return None;
        }
        let mut headers = Vec::new();
        for line in lines {
            if line.is_empty() {
                break;
            }
            let colon = line.iter().position(|byte| *byte == b':')?;
            let name = String::from_utf8(line[..colon].to_vec()).ok()?.trim().to_string();
            headers.push((name, line[colon + 1..].trim_ascii().to_vec()));
        }
        Some(Head { first, headers })
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(known, _)| known.eq_ignore_ascii_case(name))
            .and_then(|(_, value)| std::str::from_utf8(value).ok())
    }

    fn has(&self, name: &str) -> bool {
        self.headers.iter().any(|(known, _)| known.eq_ignore_ascii_case(name))
    }

    fn remove(&mut self, name: &str) {
        self.headers.retain(|(known, _)| !known.eq_ignore_ascii_case(name));
    }

    /// Replaces a header, or adds it when it is not there.
    fn set(&mut self, name: &str, value: &str) {
        match self.headers.iter().position(|(known, _)| known.eq_ignore_ascii_case(name)) {
            Some(at) => {
                self.headers[at].1 = value.as_bytes().to_vec();
                let mut seen = false;
                // A header given twice would be half replaced otherwise.
                self.headers
                    .retain(|(known, _)| !known.eq_ignore_ascii_case(name) || !std::mem::replace(&mut seen, true));
            }
            None => self.headers.push((name.to_string(), value.as_bytes().to_vec())),
        }
    }

    /// Changes every value of a header that is text.
    fn change(&mut self, name: &str, change: impl Fn(&str) -> Option<String>) {
        for (known, value) in &mut self.headers {
            if known.eq_ignore_ascii_case(name)
                && let Some(next) = std::str::from_utf8(value).ok().and_then(&change)
            {
                *value = next.into_bytes();
            }
        }
        self.headers.retain(|(known, value)| !known.eq_ignore_ascii_case(name) || !value.is_empty());
    }

    fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(512);
        bytes.extend_from_slice(self.first.as_bytes());
        bytes.extend_from_slice(b"\r\n");
        for (name, value) in &self.headers {
            bytes.extend_from_slice(name.as_bytes());
            bytes.extend_from_slice(b": ");
            bytes.extend_from_slice(value);
            bytes.extend_from_slice(b"\r\n");
        }
        bytes.extend_from_slice(b"\r\n");
        bytes
    }

    /// The status of an answer, from its first line: "HTTP/1.1 200 OK".
    fn status(&self) -> u16 {
        self.first.split_whitespace().nth(1).and_then(|code| code.parse().ok()).unwrap_or(0)
    }
}

/// Reads up to and including the empty line that ends a head. `None` when the other side closed before sending one.
async fn read_head<R: AsyncRead + Unpin>(reader: &mut BufReader<R>) -> io::Result<Option<Vec<u8>>> {
    let mut head = Vec::new();
    loop {
        let before = head.len();
        if reader.read_until(b'\n', &mut head).await? == 0 {
            return if head.is_empty() { Ok(None) } else { Err(io::ErrorKind::UnexpectedEof.into()) };
        }
        let line = &head[before..];
        if line == b"\r\n" || line == b"\n" {
            return Ok(Some(head));
        }
        if head.len() > MAX_HEAD {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "the head is too long"));
        }
    }
}

/// Ends a task when it goes out of scope, so a connection that is dropped half way leaves nothing running.
struct EndsWith(JoinHandle<()>);

impl Drop for EndsWith {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Answers a request by the relay itself with a short page.
async fn answer(client: &mut (impl AsyncWriteExt + Unpin), status: &str, kind: &str, body: &[u8]) -> io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    client.write_all(head.as_bytes()).await?;
    client.write_all(body).await?;
    client.shutdown().await
}

/// A page of the relay's own that says what is wrong. It carries our script too,
/// so the interface learns that the frame shows a problem and not the app.
fn problem_page(kind: &str, title: &str, text: &str) -> Vec<u8> {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"ae-preview-problem\" content=\"{kind}\">\
         <script src=\"{SCRIPT_PATH}\"></script><title>{title}</title></head>\
         <body style=\"margin: 0; padding: 24px; font: 15px/1.5 system-ui, sans-serif; background: #fff; color: #1f2328\">\
         <h1 style=\"margin: 0 0 8px; font-size: 18px\">{title}</h1><p style=\"margin: 0\">{text}</p></body></html>"
    )
    .into_bytes()
}

/// Serves one connection: one request, passed on to the app, and its answer.
async fn serve(relay: &Relay, client: TcpStream) -> io::Result<()> {
    let _ = client.set_nodelay(true);
    let (client_read, mut client_write) = client.into_split();
    let mut client_read = BufReader::new(client_read);
    let Some(bytes) = read_head(&mut client_read).await? else { return Ok(()) };
    let Some(mut request) = Head::parse(&bytes) else {
        return answer(&mut client_write, "400 Bad Request", "text/plain", b"The request could not be read.").await;
    };
    let mut words = request.first.split_whitespace();
    let method = words.next().unwrap_or_default().to_string();
    let path = words.next().unwrap_or_default().to_string();

    // A page of some other site must not reach the app through the relay under another name.
    let own_hosts = [format!("127.0.0.1:{}", relay.port), format!("localhost:{}", relay.port)];
    if !request.get("Host").is_some_and(|host| own_hosts.iter().any(|own| own.eq_ignore_ascii_case(host))) {
        return answer(
            &mut client_write,
            "403 Forbidden",
            "text/plain",
            b"The preview answers only on its own address.",
        )
        .await;
    }
    if path.starts_with(OWN_PREFIX) {
        let (path_only, query) = path.split_once('?').unwrap_or((&path, ""));
        if path_only == SCRIPT_PATH {
            let script = relay.script();
            return answer(&mut client_write, "200 OK", "text/javascript; charset=utf-8", script.as_bytes()).await;
        }
        // A page made for one screenshot, and the wait it asks for. Only a word the core handed out finds either.
        if let Some(rest) = path_only.strip_prefix(SHOT_PREFIX) {
            let (word, part) = rest.split_once('/').unwrap_or((rest, ""));
            let page = relay.shots.lock().unwrap().get(word).cloned();
            match (page, part) {
                (Some(page), "") => {
                    let html = page.html(word);
                    return answer(&mut client_write, "200 OK", "text/html; charset=utf-8", html.as_bytes()).await;
                }
                (Some(_), "settle") => {
                    let height = query.strip_prefix("h=").and_then(|digits| digits.parse::<u32>().ok());
                    if let Some(page) = relay.shots.lock().unwrap().get_mut(word) {
                        page.window_height = height.or(Some(0));
                    }
                    tokio::time::sleep(SETTLE).await;
                    return answer(&mut client_write, "200 OK", "image/gif", TINY_GIF).await;
                }
                _ => {}
            }
        }
        return answer(&mut client_write, "404 Not Found", "text/plain", b"The preview has no such file.").await;
    }
    if request.has(HOP_HEADER) {
        let page = problem_page("loop", "The preview points at itself", "Type the address of the running app.");
        return answer(&mut client_write, "508 Loop Detected", "text/html; charset=utf-8", &page).await;
    }

    let target = relay.target.lock().unwrap().clone();
    let connecting = TcpStream::connect((target.host.as_str(), target.port));
    let upstream = match tokio::time::timeout(CONNECT_TIMEOUT, connecting).await {
        Ok(Ok(upstream)) => upstream,
        _ => {
            let text = format!("Nothing answers at {}. Start the app, then reload.", target.authority());
            let page = problem_page("unreachable", "The app is not running", &text);
            return answer(&mut client_write, "502 Bad Gateway", "text/html; charset=utf-8", &page).await;
        }
    };
    let _ = upstream.set_nodelay(true);
    let (upstream_read, mut upstream_write) = upstream.into_split();
    let mut upstream_read = BufReader::new(upstream_read);

    let upgrade = request.has("Upgrade");
    prepare_request(&mut request, &target, relay.port, upgrade);
    upstream_write.write_all(&request.to_bytes()).await?;
    // Whatever else the browser sends, the body of a form or the frames of a WebSocket, goes on as it comes.
    let _upward = EndsWith(tokio::spawn(async move {
        let _ = tokio::io::copy(&mut client_read, &mut upstream_write).await;
        let _ = upstream_write.shutdown().await;
    }));

    loop {
        let Some(bytes) = read_head(&mut upstream_read).await? else {
            let page =
                problem_page("unreachable", "The app did not answer", "It closed the connection without a word.");
            return answer(&mut client_write, "502 Bad Gateway", "text/html; charset=utf-8", &page).await;
        };
        let Some(mut response) = Head::parse(&bytes) else {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "the answer could not be read"));
        };
        let status = response.status();
        if status == 101 {
            // A WebSocket from here on: bytes both ways until one side is done.
            client_write.write_all(&bytes).await?;
            tokio::io::copy(&mut upstream_read, &mut client_write).await?;
            return client_write.shutdown().await;
        }
        if (100..200).contains(&status) {
            // "Go on", then the real answer follows on the same connection.
            client_write.write_all(&bytes).await?;
            continue;
        }

        let framing = Framing::of(&response, &method, status);
        prepare_response(&mut response, &target);
        if is_page(&response) && framing != Framing::Nothing {
            let known = match framing {
                Framing::Length(length) => Some(length),
                _ => None,
            };
            if known.is_none_or(|length| length <= MAX_PAGE) {
                let page = read_body(&mut upstream_read, framing).await?;
                let page = inject(&page, &script_tag(&response));
                response.remove("Transfer-Encoding");
                response.set("Content-Length", &page.len().to_string());
                client_write.write_all(&response.to_bytes()).await?;
                client_write.write_all(&page).await?;
                return client_write.shutdown().await;
            }
        }
        client_write.write_all(&response.to_bytes()).await?;
        match framing {
            Framing::Nothing => {}
            Framing::Length(length) => {
                tokio::io::copy(&mut (&mut upstream_read).take(length), &mut client_write).await?;
            }
            // The app was asked to close when it is done, so the end of the connection is the end of the answer.
            Framing::Chunked | Framing::UntilClose => {
                tokio::io::copy(&mut upstream_read, &mut client_write).await?;
            }
        }
        return client_write.shutdown().await;
    }
}

/// Makes a browser's request to the relay into a request the app takes as its own.
fn prepare_request(request: &mut Head, target: &Target, relay_port: u16, upgrade: bool) {
    // Dev servers refuse a `Host` they do not know; the app's own address is the one they expect.
    request.set("Host", &target.authority());
    // The page lives at the relay's address. To the app, its requests must come from the app's own address.
    let own = [format!("http://127.0.0.1:{relay_port}"), format!("http://localhost:{relay_port}")];
    let as_app = |value: &str| {
        own.iter().find_map(|origin| {
            let rest = value.strip_prefix(origin.as_str())?;
            (rest.is_empty() || rest.starts_with('/')).then(|| format!("{}{rest}", target.origin()))
        })
    };
    request.change("Origin", as_app);
    request.change("Referer", as_app);
    request.set(HOP_HEADER, "1");
    request.remove("Proxy-Connection");
    if !upgrade {
        // One request per connection keeps the relay simple: the end of the connection ends the answer.
        request.remove("Keep-Alive");
        request.set("Connection", "close");
        // The script tag goes into the page as text, so the page must not arrive compressed.
        request.set("Accept-Encoding", "identity");
    }
}

/// Changes the head of the app's answer so the page works in a frame at the relay's address.
fn prepare_response(response: &mut Head, target: &Target) {
    response.remove("Keep-Alive");
    response.set("Connection", "close");
    // The whole point is to show the page in a frame of the interface.
    response.remove("X-Frame-Options");
    for name in ["Content-Security-Policy", "Content-Security-Policy-Report-Only"] {
        response.change(name, |policy| Some(without_frame_ancestors(policy)));
    }
    // A redirect to the app's own address would take the frame away from the relay.
    let origin = target.origin();
    response.change("Location", |to| {
        let rest = to.strip_prefix(origin.as_str())?;
        (rest.is_empty() || rest.starts_with(['/', '?', '#']))
            .then(|| if rest.starts_with('/') { rest.to_string() } else { format!("/{rest}") })
    });
    // A cookie pinned to the app's host name would be thrown away at the relay's address.
    let host = target.host.clone();
    response.change("Set-Cookie", |cookie| {
        let kept: Vec<&str> = cookie
            .split(';')
            .filter(|part| {
                let (name, value) = part.trim().split_once('=').unwrap_or((part.trim(), ""));
                !(name.eq_ignore_ascii_case("domain") && value.trim_start_matches('.').eq_ignore_ascii_case(&host))
            })
            .collect();
        Some(kept.join(";"))
    });
}

/// A content policy without the part that says who may show the page in a frame.
fn without_frame_ancestors(policy: &str) -> String {
    policy
        .split(';')
        .filter(|directive| {
            !directive.split_whitespace().next().is_some_and(|name| name.eq_ignore_ascii_case("frame-ancestors"))
        })
        .collect::<Vec<_>>()
        .join(";")
        .trim()
        .to_string()
}

/// Whether an answer is an HTML page that our script can be added to as text.
fn is_page(response: &Head) -> bool {
    let html = response
        .get("Content-Type")
        .is_some_and(|kind| kind.split(';').next().unwrap_or_default().trim().eq_ignore_ascii_case("text/html"));
    let plain = response.get("Content-Encoding").is_none_or(|coding| coding.trim().eq_ignore_ascii_case("identity"));
    html && plain
}

/// The tag that loads our script. A page that allows scripts by a one-time word gets that word on the tag.
fn script_tag(response: &Head) -> String {
    let nonce = response.get("Content-Security-Policy").and_then(|policy| {
        let start = policy.find("'nonce-")? + "'nonce-".len();
        let word = &policy[start..start + policy[start..].find('\'')?];
        word.chars().all(|c| c.is_ascii_alphanumeric() || "+/=_-".contains(c)).then(|| word.to_string())
    });
    match nonce {
        Some(nonce) => format!("<script nonce=\"{nonce}\" src=\"{SCRIPT_PATH}\"></script>"),
        None => format!("<script src=\"{SCRIPT_PATH}\"></script>"),
    }
}

/// How the end of an answer's body is told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Framing {
    /// The answer has no body at all.
    Nothing,
    Length(u64),
    Chunked,
    UntilClose,
}

impl Framing {
    fn of(response: &Head, method: &str, status: u16) -> Framing {
        if method.eq_ignore_ascii_case("HEAD") || status == 204 || status == 304 {
            return Framing::Nothing;
        }
        if response.get("Transfer-Encoding").is_some_and(|coding| coding.to_ascii_lowercase().contains("chunked")) {
            return Framing::Chunked;
        }
        match response.get("Content-Length").and_then(|length| length.trim().parse::<u64>().ok()) {
            Some(length) => Framing::Length(length),
            None => Framing::UntilClose,
        }
    }
}

/// Reads the whole body of an answer, undoing the chunks when it came in chunks.
async fn read_body<R: AsyncRead + Unpin>(reader: &mut BufReader<R>, framing: Framing) -> io::Result<Vec<u8>> {
    let too_long = || io::Error::new(io::ErrorKind::InvalidData, "the page is too long to add the script to");
    let mut body = Vec::new();
    match framing {
        Framing::Nothing => {}
        Framing::Length(length) => {
            body.resize(length as usize, 0);
            reader.read_exact(&mut body).await?;
        }
        Framing::UntilClose => {
            reader.take(MAX_PAGE_UNKNOWN as u64 + 1).read_to_end(&mut body).await?;
            if body.len() > MAX_PAGE_UNKNOWN {
                return Err(too_long());
            }
        }
        Framing::Chunked => loop {
            // Each chunk: its length in hex on a line, the bytes, an empty line. A chunk of no bytes ends the body.
            let mut line = Vec::new();
            if reader.read_until(b'\n', &mut line).await? == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            let digits = String::from_utf8_lossy(&line);
            let digits = digits.split(';').next().unwrap_or_default().trim();
            let length = usize::from_str_radix(digits, 16)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "a chunk has no length"))?;
            if length == 0 {
                break;
            }
            if body.len() + length > MAX_PAGE_UNKNOWN {
                return Err(too_long());
            }
            let start = body.len();
            body.resize(start + length, 0);
            reader.read_exact(&mut body[start..]).await?;
            let mut end = [0u8; 2];
            reader.read_exact(&mut end).await?;
        },
    }
    Ok(body)
}

/// Finds `needle` in `haystack` from `from` on, ignoring the case of ASCII letters.
fn find_from(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    haystack
        .get(from..)?
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle))
        .map(|at| at + from)
}

/// Where the tag that opens at `open` ends: one past its `>`. A `>` inside a quoted value does not end it.
fn tag_end(page: &[u8], open: usize) -> usize {
    let mut quote = None;
    for (at, byte) in page.iter().enumerate().skip(open + 1) {
        match (quote, *byte) {
            (None, b'"' | b'\'') => quote = Some(*byte),
            (Some(opened), closed) if opened == closed => quote = None,
            (None, b'>') => return at + 1,
            _ => {}
        }
    }
    page.len()
}

/// Puts `tag` into an HTML page, as early as it can go without changing how the page is read:
/// right after `<head>`; in a page without one, after `<html>` or the doctype; else at the very start.
/// Our script has to run before the page's own, and a tag before the doctype would switch the page to old rules.
pub fn inject(page: &[u8], tag: &str) -> Vec<u8> {
    // A page in two-byte text would be broken by a tag in one-byte text; leave it alone.
    if page.starts_with(&[0xFF, 0xFE]) || page.starts_with(&[0xFE, 0xFF]) {
        return page.to_vec();
    }
    let mut at = 0;
    let mut place = 0;
    while let Some(open) = find_from(page, b"<", at) {
        let rest = &page[open..];
        if rest.starts_with(b"<!--") {
            // Whatever a comment says is not the page.
            match find_from(page, b"-->", open + 4) {
                Some(end) => at = end + 3,
                None => break,
            }
            continue;
        }
        let name_end = rest[1..]
            .iter()
            .position(|byte| byte.is_ascii_whitespace() || *byte == b'>' || *byte == b'/')
            .map_or(rest.len(), |n| n + 1);
        let name = rest[1..name_end].to_ascii_lowercase();
        match name.as_slice() {
            b"head" => {
                place = tag_end(page, open);
                break;
            }
            b"!doctype" | b"html" => {
                place = tag_end(page, open);
                at = place;
            }
            // "<?xml ...?>" before the doctype of an older page.
            [b'?', ..] => at = tag_end(page, open),
            // The first tag of the page proper: there is no head to find after it.
            [first, ..] if first.is_ascii_alphabetic() => break,
            // Text that happens to hold a "<".
            _ => at = open + 1,
        }
    }
    let mut out = Vec::with_capacity(page.len() + tag.len());
    out.extend_from_slice(&page[..place]);
    out.extend_from_slice(tag.as_bytes());
    out.extend_from_slice(&page[place..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const TAG: &str = "<script src=\"/__ae_preview/page.js\"></script>";

    fn injected(page: &str) -> String {
        String::from_utf8(inject(page.as_bytes(), TAG)).unwrap()
    }

    #[test]
    fn an_address_is_read_as_people_write_it() {
        let target =
            |text: &str| Target::parse(text).map(|target| target.authority()).map_err(|error| error.to_string());
        assert_eq!(target("localhost:3000").unwrap(), "localhost:3000");
        assert_eq!(target("  http://localhost:5173/login?next=1#top ").unwrap(), "localhost:5173");
        assert_eq!(target("HTTP://LOCALHOST:8080").unwrap(), "localhost:8080");
        assert_eq!(target("127.0.0.1:8000/").unwrap(), "127.0.0.1:8000");
        assert_eq!(target("[::1]:4000").unwrap(), "[::1]:4000");
        assert_eq!(target("http://localhost").unwrap(), "localhost:80", "no port means the usual one");
        assert_eq!(Target::parse("[::1]:4000").unwrap().origin(), "http://[::1]:4000");
    }

    #[test]
    fn an_address_that_is_not_on_this_computer_is_refused() {
        for elsewhere in [
            "example.com",
            "http://example.com:3000/",
            "192.168.1.20:3000",
            "localhost.example.com:3000",
            "http://localhost@example.com/",
            "http://example.com#localhost:3000",
            "0.0.0.0:3000",
            "ftp://localhost:21",
        ] {
            assert!(Target::parse(elsewhere).is_err(), "{elsewhere} must be refused");
        }
        let refusal = Target::parse("example.com:3000").unwrap_err().to_string();
        assert!(refusal.contains("Only an app on this computer can be shown"), "{refusal}");
        assert!(Target::parse("https://localhost:3000").unwrap_err().to_string().contains("https is not supported"));
        assert!(Target::parse("localhost:99999").is_err(), "not a port");
        assert!(Target::parse("localhost:0").is_err(), "port 0 is nobody's");
        assert!(Target::parse("   ").is_err());
    }

    #[test]
    fn the_script_goes_right_after_the_head_tag() {
        assert_eq!(
            injected("<!doctype html><html lang=\"en\"><head><title>x</title></head><body></body></html>"),
            format!("<!doctype html><html lang=\"en\"><head>{TAG}<title>x</title></head><body></body></html>")
        );
        assert_eq!(injected("<HTML><HEAD data-x='>'><p>"), format!("<HTML><HEAD data-x='>'>{TAG}<p>"));
        // A header is not a head, and a head in a comment is not one either.
        assert_eq!(
            injected("<!DOCTYPE html>\n<!-- <head> -->\n<html>\n<header>hi</header>"),
            format!("<!DOCTYPE html>\n<!-- <head> -->\n<html>{TAG}\n<header>hi</header>")
        );
    }

    #[test]
    fn a_page_without_a_head_gets_the_script_at_its_start_and_never_before_the_doctype() {
        assert_eq!(injected("<!doctype html><p>Hello</p>"), format!("<!doctype html>{TAG}<p>Hello</p>"));
        assert_eq!(
            injected("<?xml version=\"1.0\"?>\n<!DOCTYPE html>\n<html><body>"),
            format!("<?xml version=\"1.0\"?>\n<!DOCTYPE html>\n<html>{TAG}<body>")
        );
        assert_eq!(
            injected("<p>Hello</p><head>"),
            format!("{TAG}<p>Hello</p><head>"),
            "a head after the page began is not one"
        );
        assert_eq!(injected("a < b <p>x</p>"), format!("{TAG}a < b <p>x</p>"));
        assert_eq!(injected("just words"), format!("{TAG}just words"));
        assert_eq!(injected(""), TAG);
    }

    #[test]
    fn a_page_in_two_byte_text_is_left_alone() {
        let page = [0xFF, 0xFE, b'<', 0, b'p', 0, b'>', 0];
        assert_eq!(inject(&page, TAG), page);
    }

    fn head(text: &str) -> Head {
        Head::parse(text.as_bytes()).unwrap()
    }

    #[test]
    fn a_request_is_made_into_one_the_app_takes_as_its_own() {
        let target = Target::parse("localhost:3000").unwrap();
        let mut request = head(
            "POST /api/login HTTP/1.1\r\nHost: 127.0.0.1:5999\r\nConnection: keep-alive\r\nOrigin: http://127.0.0.1:5999\r\n\
             Referer: http://127.0.0.1:5999/login?a=1\r\nAccept-Encoding: gzip, br\r\nCookie: sid=1\r\n\r\n",
        );
        prepare_request(&mut request, &target, 5999, false);
        assert_eq!(request.first, "POST /api/login HTTP/1.1");
        assert_eq!(request.get("Host"), Some("localhost:3000"));
        assert_eq!(request.get("Origin"), Some("http://localhost:3000"));
        assert_eq!(request.get("Referer"), Some("http://localhost:3000/login?a=1"));
        assert_eq!(request.get("Connection"), Some("close"));
        assert_eq!(request.get("Accept-Encoding"), Some("identity"));
        assert_eq!(request.get("Cookie"), Some("sid=1"), "everything else goes on as it came");

        // A request from some other site keeps saying where it came from: the app decides about it.
        let mut foreign = head("GET / HTTP/1.1\r\nHost: localhost:5999\r\nOrigin: http://evil.example\r\n\r\n");
        prepare_request(&mut foreign, &target, 5999, false);
        assert_eq!(foreign.get("Origin"), Some("http://evil.example"));
        let mut lookalike = head("GET / HTTP/1.1\r\nHost: localhost:5999\r\nOrigin: http://127.0.0.1:59990\r\n\r\n");
        prepare_request(&mut lookalike, &target, 5999, false);
        assert_eq!(lookalike.get("Origin"), Some("http://127.0.0.1:59990"));

        // A WebSocket keeps the headers that ask for it.
        let mut socket = head(
            "GET /ws HTTP/1.1\r\nHost: localhost:5999\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nOrigin: http://localhost:5999\r\n\r\n",
        );
        prepare_request(&mut socket, &target, 5999, true);
        assert_eq!(socket.get("Connection"), Some("Upgrade"));
        assert_eq!(socket.get("Upgrade"), Some("websocket"));
        assert_eq!(socket.get("Origin"), Some("http://localhost:3000"));
        assert_eq!(socket.get("Accept-Encoding"), None);
    }

    #[test]
    fn an_answer_is_made_to_work_in_a_frame_at_the_relays_address() {
        let target = Target::parse("localhost:3000").unwrap();
        let mut response = head(
            "HTTP/1.1 302 Found\r\nLocation: http://localhost:3000/login\r\nX-Frame-Options: DENY\r\nConnection: keep-alive\r\n\
             Content-Security-Policy: default-src 'self'; frame-ancestors 'none'; script-src 'self' 'nonce-abc123'\r\n\
             Set-Cookie: sid=1; Path=/; Domain=localhost; HttpOnly\r\nSet-Cookie: theme=dark\r\n\r\n",
        );
        let tag = script_tag(&response);
        prepare_response(&mut response, &target);
        assert_eq!(response.get("Location"), Some("/login"));
        assert!(!response.has("X-Frame-Options"));
        assert_eq!(response.get("Connection"), Some("close"));
        assert_eq!(
            response.get("Content-Security-Policy"),
            Some("default-src 'self'; script-src 'self' 'nonce-abc123'")
        );
        let cookies: Vec<&[u8]> = response
            .headers
            .iter()
            .filter(|(name, _)| name == "Set-Cookie")
            .map(|(_, value)| value.as_slice())
            .collect();
        assert_eq!(cookies, vec![b"sid=1; Path=/; HttpOnly".as_slice(), b"theme=dark".as_slice()]);
        assert_eq!(tag, "<script nonce=\"abc123\" src=\"/__ae_preview/page.js\"></script>");

        // A redirect to somewhere else is none of the relay's business.
        let mut away = head("HTTP/1.1 302 Found\r\nLocation: http://localhost:30001/x\r\n\r\n");
        prepare_response(&mut away, &target);
        assert_eq!(away.get("Location"), Some("http://localhost:30001/x"));
        // A policy that was only about frames goes away whole.
        let mut only = head("HTTP/1.1 200 OK\r\nContent-Security-Policy: frame-ancestors 'self'\r\n\r\n");
        prepare_response(&mut only, &target);
        assert!(!only.has("Content-Security-Policy"));
    }

    #[test]
    fn only_an_uncompressed_html_page_gets_the_script() {
        assert!(is_page(&head("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\r\n")));
        assert!(is_page(&head("HTTP/1.1 200 OK\r\ncontent-type: TEXT/HTML\r\n\r\n")));
        assert!(!is_page(&head("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Encoding: gzip\r\n\r\n")));
        assert!(!is_page(&head("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n")));
        assert!(!is_page(&head("HTTP/1.1 200 OK\r\n\r\n")));
    }

    #[test]
    fn how_an_answer_ends_is_read_from_its_head() {
        let of = |text: &str, method: &str| {
            let response = head(text);
            Framing::of(&response, method, response.status())
        };
        assert_eq!(of("HTTP/1.1 200 OK\r\nContent-Length: 12\r\n\r\n", "GET"), Framing::Length(12));
        assert_eq!(of("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n", "GET"), Framing::Chunked);
        assert_eq!(of("HTTP/1.1 200 OK\r\n\r\n", "GET"), Framing::UntilClose);
        assert_eq!(of("HTTP/1.1 200 OK\r\nContent-Length: 12\r\n\r\n", "HEAD"), Framing::Nothing);
        assert_eq!(of("HTTP/1.1 304 Not Modified\r\n\r\n", "GET"), Framing::Nothing);
    }

    #[tokio::test]
    async fn a_body_in_chunks_is_put_back_together() {
        let wire = b"5\r\nHello\r\n7;note=1\r\n, world\r\n0\r\n\r\n";
        let mut reader = BufReader::new(&wire[..]);
        assert_eq!(read_body(&mut reader, Framing::Chunked).await.unwrap(), b"Hello, world");
        let mut cut = BufReader::new(&b"5\r\nHel"[..]);
        assert!(read_body(&mut cut, Framing::Chunked).await.is_err(), "a body that stops half way is an error");
    }

    #[test]
    fn an_address_is_split_into_the_app_and_its_page() {
        let split = |text: &str| split_address(text).map(|(target, path)| (target.authority(), path));
        assert_eq!(split("localhost:3000").unwrap(), ("localhost:3000".into(), "/".into()));
        assert_eq!(
            split(" http://127.0.0.1:5173/login?next=/cart&x=1#top ").unwrap(),
            ("127.0.0.1:5173".into(), "/login?next=/cart&x=1#top".into())
        );
        assert_eq!(split("localhost:3000?tab=2").unwrap(), ("localhost:3000".into(), "/?tab=2".into()));
        assert!(split("example.com/login").is_err(), "the app must be on this computer");
        assert!(split("localhost:3000/a b").is_err(), "an address has no spaces");
    }

    #[test]
    fn only_a_page_of_the_app_is_a_page() {
        assert_eq!(page_path("/login?a=1&b=2#x").unwrap(), "/login?a=1&b=2#x");
        for other in
            ["", "login", "//example.com/", "/\\example.com", "javascript:alert(1)", "/a\nb", "/__ae_preview/page.js"]
        {
            assert!(page_path(other).is_err(), "{other:?} must be refused");
        }
        assert!(page_path(&format!("/{}", "x".repeat(MAX_PATH))).is_err());
    }

    #[test]
    fn the_page_for_a_screenshot_frames_the_apps_page_at_the_asked_size() {
        let page = ShotPage { width: 320, height: 700, path: "/login?a=1&b=\"2\"<i>".into(), window_height: None };
        let html = page.html("w0rd");
        assert!(html.contains("width:320px;height:700px"), "{html}");
        // What the address holds stays inside the address.
        assert!(html.contains("src=\"/login?a=1&amp;b=&quot;2&quot;&lt;i&gt;\""), "{html}");
        assert!(html.contains("wait.src='/__ae_preview/shot/w0rd/settle?h='+window.innerHeight"), "{html}");
        assert!(!html.contains("allow-top-navigation") && !html.contains("allow-modals"), "{html}");
    }

    #[test]
    fn only_a_plain_window_address_is_taken_as_one() {
        assert!(is_origin("http://127.0.0.1:4477"));
        assert!(is_origin("http://localhost:5173"));
        assert!(is_origin("tauri://localhost"));
        assert!(is_origin("http://[::1]:4477"));
        assert!(!is_origin("http://127.0.0.1:4477/path"));
        assert!(!is_origin("null"));
        assert!(!is_origin("http://a\"b"));
        assert!(!is_origin(""));
    }
}
