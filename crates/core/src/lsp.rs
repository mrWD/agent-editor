//! Language servers: what gives the editor errors and warnings in the code,
//! completions, hover text, "go to definition" and formatting. No model is
//! involved and nothing is spent.
//!
//! A language server is a child process that speaks the Language Server
//! Protocol on its stdin and stdout: JSON-RPC messages, each behind a
//! `Content-Length` header. The core starts one for a task and a language the
//! first time the interface opens a file of that language, with the task's
//! folder as its root, and from then on carries messages both ways: the
//! interface sends through the `lsp.send` command, and what the server says
//! comes back as `lsp.message` events.
//!
//! One server serves every window. So that two windows do not confuse it, the
//! core keeps count of who has which file open: a file is "opened" in the
//! server once and "closed" when the last window lets go of it, and a window
//! that goes away lets go of everything it held.
//!
//! Nothing is ever installed from here. A server that is not on this machine
//! is reported as missing, with the command that would install it.
//!
//! A server is stopped when its task is archived or cleaned up, when the core
//! stops, and after a while with no open file. It runs in a process group of
//! its own, so the helpers it starts are stopped with it.

use crate::acp::kill_family;
use crate::tasks::Manager;
use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{Mutex, mpsc, oneshot, watch};

/// How long a server may take to answer `initialize`.
const START_TIMEOUT: Duration = Duration::from_secs(20);
/// How long a server is given to end by itself after it was asked to.
const POLITE: Duration = Duration::from_millis(700);
/// How long a server with no open file is kept before it is stopped.
pub const IDLE: Duration = Duration::from_secs(180);
/// The largest message taken from a server. Anything longer is a broken header, not a message.
const MAX_MESSAGE: usize = 256 * 1024 * 1024;
/// How much of a stream may pass without the end of a header before it is given up as noise.
const MAX_HEADER: usize = 64 * 1024;
/// How many of a server's last stderr lines are kept, to say why it failed.
const STDERR_LINES: usize = 12;
/// How long a server's last words on stderr are waited for once its process has ended.
const LAST_WORDS: Duration = Duration::from_millis(300);
/// What the ids of the core's own requests start with, to tell their answers from the interface's.
const OWN_ID: &str = "ae-core-";

// ----- framing -----

/// Cuts a stream of bytes into the messages of the Language Server Protocol.
///
/// The stream arrives in pieces of any size: one message in many pieces, or
/// many messages in one piece. Each message is a few header lines, an empty
/// line, and then exactly as many bytes as the `Content-Length` header says.
#[derive(Default)]
pub struct Framer {
    buffer: Vec<u8>,
    /// Where the body of the message being read starts in `buffer` and how long it is, once its header is read.
    body: Option<(usize, usize)>,
}

impl Framer {
    /// Takes the next piece of the stream and returns every message it completes.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
        self.buffer.extend_from_slice(bytes);
        let mut messages = Vec::new();
        loop {
            let (start, length) = match self.body {
                Some(body) => body,
                None => {
                    let Some(end) = self.buffer.windows(4).position(|window| window == b"\r\n\r\n") else {
                        // Not a header and not about to become one: keep only what could still start one.
                        if self.buffer.len() > MAX_HEADER {
                            let keep = self.buffer.len() - 3;
                            self.buffer.drain(..keep);
                        }
                        break;
                    };
                    match content_length(&self.buffer[..end]) {
                        Some(length) if length <= MAX_MESSAGE => {
                            self.body = Some((end + 4, length));
                            (end + 4, length)
                        }
                        // A header without a usable length: skip it and look for the next one.
                        _ => {
                            self.buffer.drain(..end + 4);
                            continue;
                        }
                    }
                }
            };
            if self.buffer.len() < start + length {
                break;
            }
            messages.push(self.buffer[start..start + length].to_vec());
            self.buffer.drain(..start + length);
            self.body = None;
        }
        messages
    }
}

/// The length a header block gives for its message.
fn content_length(header: &[u8]) -> Option<usize> {
    String::from_utf8_lossy(header).lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim().eq_ignore_ascii_case("content-length").then(|| value.trim().parse().ok())?
    })
}

/// A message as it travels to a server: its header, then its JSON.
pub fn frame(message: &Value) -> Vec<u8> {
    let body = message.to_string();
    let mut bytes = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    bytes.extend_from_slice(body.as_bytes());
    bytes
}

/// A folder or file as a `file://` address, the way the protocol names places.
pub fn file_uri(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let mut uri = String::from("file://");
    // "C:/work" on Windows has no slash in front; the address needs one.
    if !text.starts_with('/') {
        uri.push('/');
    }
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => uri.push(byte as char),
            other => uri.push_str(&format!("%{other:02X}")),
        }
    }
    uri
}

// ----- the table of known servers -----

/// One way to start a server: a program and its arguments.
#[derive(Debug, Clone)]
pub struct Launch {
    /// A name to look for, or a full path.
    pub program: String,
    pub args: Vec<String>,
}

/// A language the core knows a server for.
#[derive(Debug, Clone)]
pub struct ServerDef {
    /// Stable id used in commands and events: "typescript", "rust".
    pub language: String,
    /// The name shown to the user: "TypeScript".
    pub name: String,
    /// The file endings of the language, each with the name the protocol has for that kind of file.
    pub endings: Vec<(String, String)>,
    /// The ways to start a server, the preferred one first. The first one found is used.
    pub launches: Vec<Launch>,
    /// The command that would install the preferred server. It is shown to the user and never run.
    pub install: String,
    /// What the server is given as `initializationOptions`.
    pub options: Value,
}

fn def(
    language: &str,
    name: &str,
    endings: &[(&str, &str)],
    launches: &[(&str, &[&str])],
    install: &str,
    options: Value,
) -> ServerDef {
    ServerDef {
        language: language.into(),
        name: name.into(),
        endings: endings.iter().map(|(ending, id)| (ending.to_string(), id.to_string())).collect(),
        launches: launches
            .iter()
            .map(|(program, args)| Launch {
                program: program.to_string(),
                args: args.iter().map(|arg| arg.to_string()).collect(),
            })
            .collect(),
        install: install.into(),
        options,
    }
}

/// The servers the core knows how to start, whether or not they are on this machine.
pub fn known_servers() -> Vec<ServerDef> {
    let extracted = "npm install -g vscode-langservers-extracted";
    // Without this the three servers below do not offer formatting.
    let formats = json!({ "provideFormatter": true });
    vec![
        def(
            "typescript",
            "TypeScript and JavaScript",
            &[
                ("ts", "typescript"),
                ("mts", "typescript"),
                ("cts", "typescript"),
                ("tsx", "typescriptreact"),
                ("js", "javascript"),
                ("mjs", "javascript"),
                ("cjs", "javascript"),
                ("jsx", "javascriptreact"),
            ],
            &[("typescript-language-server", &["--stdio"])],
            "npm install -g typescript-language-server typescript",
            json!({}),
        ),
        def(
            "rust",
            "Rust",
            &[("rs", "rust")],
            &[("rust-analyzer", &[])],
            "rustup component add rust-analyzer",
            json!({}),
        ),
        def("go", "Go", &[("go", "go")], &[("gopls", &[])], "go install golang.org/x/tools/gopls@latest", json!({})),
        def(
            "python",
            "Python",
            &[("py", "python"), ("pyi", "python")],
            &[("pyright-langserver", &["--stdio"]), ("pylsp", &[])],
            "npm install -g pyright",
            json!({}),
        ),
        def(
            "json",
            "JSON",
            &[("json", "json"), ("jsonc", "jsonc")],
            &[("vscode-json-language-server", &["--stdio"])],
            extracted,
            formats.clone(),
        ),
        def(
            "css",
            "CSS",
            &[("css", "css"), ("scss", "scss"), ("less", "less")],
            &[("vscode-css-language-server", &["--stdio"])],
            extracted,
            formats.clone(),
        ),
        def(
            "html",
            "HTML",
            &[("html", "html"), ("htm", "html")],
            &[("vscode-html-language-server", &["--stdio"])],
            extracted,
            formats,
        ),
    ]
}

/// The practice language server: the practice agent started with `--lsp`. It
/// serves files that end in `.practice`, which no real language uses.
pub fn practice_server(program: &Path) -> ServerDef {
    ServerDef {
        language: "practice".into(),
        name: "Practice".into(),
        endings: vec![("practice".into(), "practice".into())],
        launches: vec![Launch { program: program.to_string_lossy().into_owned(), args: vec!["--lsp".into()] }],
        install: "cargo build -p ae-mock-agent".into(),
        options: json!({}),
    }
}

/// The table a running core uses: the known servers, plus the practice server
/// when the practice agent is built. `AE_MOCK_LSP_TS=1` makes the practice
/// server stand in for the TypeScript one, so tests can try a real kind of
/// file without a real server.
fn detect() -> Vec<ServerDef> {
    let mut table = known_servers();
    if let Some(program) = crate::agents::mock_agent_path().filter(|path| path.is_file()) {
        let practice = practice_server(&program);
        if std::env::var("AE_MOCK_LSP_TS").is_ok_and(|value| value == "1") {
            for def in table.iter_mut().filter(|def| def.language == "typescript") {
                def.launches = practice.launches.clone();
            }
        }
        table.push(practice);
    }
    table
}

/// Where a program is: the task's own `node_modules/.bin`, then the project's, then PATH.
fn find_program(program: &str, cwd: &Path, project: Option<&Path>) -> Option<PathBuf> {
    let given = Path::new(program);
    if given.is_absolute() {
        return given.is_file().then(|| given.to_path_buf());
    }
    // A server installed for the project is the version the project wants.
    let mut places = vec![cwd.join("node_modules").join(".bin")];
    places.extend(project.map(|project| project.join("node_modules").join(".bin")));
    places.extend(std::env::var_os("PATH").iter().flat_map(std::env::split_paths));
    let joined = std::env::join_paths(places).ok()?;
    which::which_in(program, Some(joined), cwd).ok()
}

// ----- one running server -----

/// A file a server has open.
struct Doc {
    /// The windows that have it open. The server is told "closed" when the last one lets go.
    holders: HashSet<u64>,
    /// Counts the texts the server was given. Each window counts its own way, so the core does the counting.
    version: i64,
}

type Waiting = oneshot::Sender<Result<Value, String>>;

struct Server {
    manager: Arc<Manager>,
    task_id: String,
    language: String,
    /// The language's name as the user reads it.
    name: String,
    /// Changes with every start, so the interface can tell a server that was started again.
    run: u64,
    root_uri: String,
    /// The way to the server's stdin. Taken away when the server has ended.
    outgoing: StdMutex<Option<mpsc::UnboundedSender<Value>>>,
    /// The core's own requests that wait for an answer.
    pending: StdMutex<HashMap<String, Waiting>>,
    next_id: AtomicU64,
    alive: AtomicBool,
    /// Why the core stopped it, when it did. Ending without a reason is a crash.
    purpose: StdMutex<Option<&'static str>>,
    /// Pulls the plug: whoever holds the process stops it and everything it started.
    kill: StdMutex<Option<oneshot::Sender<()>>>,
    /// Turns true once the process is gone.
    gone: watch::Receiver<bool>,
    /// Set when the interface was told, or need not be told, that the server ended.
    told: AtomicBool,
    stderr: StdMutex<VecDeque<String>>,
    /// What the server said it can do, and what it calls itself.
    capabilities: StdMutex<Value>,
    info: StdMutex<Value>,
    docs: StdMutex<HashMap<String, Doc>>,
    /// Since when no file is open.
    empty_since: StdMutex<Instant>,
}

impl Server {
    /// Starts the program, says hello and returns once the server has answered.
    async fn start(
        manager: Arc<Manager>,
        task_id: &str,
        def: &ServerDef,
        program: &Path,
        args: &[String],
        cwd: &Path,
        run: u64,
    ) -> Result<Arc<Self>> {
        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // A language server starts helpers of its own (a compiler, a type checker).
        // The whole family gets one group, so stopping the server stops all of it.
        #[cfg(unix)]
        command.process_group(0);
        // What the core was configured with is the core's own business.
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("AE_") {
                command.env_remove(&name);
            }
        }

        let mut child = command.spawn().with_context(|| format!("Could not start {}", program.display()))?;
        let pid = child.id();
        let mut stdin = child.stdin.take().context("The language server has no stdin")?;
        let mut stdout = child.stdout.take().context("The language server has no stdout")?;
        let stderr = child.stderr.take().context("The language server has no stderr")?;

        let (outgoing, mut outgoing_rx) = mpsc::unbounded_channel::<Value>();
        let (kill, killed) = oneshot::channel::<()>();
        let (gone_tx, gone) = watch::channel(false);
        let server = Arc::new(Self {
            manager,
            task_id: task_id.to_string(),
            language: def.language.clone(),
            name: def.name.clone(),
            run,
            root_uri: file_uri(cwd),
            outgoing: StdMutex::new(Some(outgoing)),
            pending: StdMutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            alive: AtomicBool::new(true),
            purpose: StdMutex::new(None),
            kill: StdMutex::new(Some(kill)),
            gone,
            // Until it has said hello the interface does not know of it, so there is nobody to tell that it ended.
            told: AtomicBool::new(true),
            stderr: StdMutex::new(VecDeque::new()),
            capabilities: StdMutex::new(Value::Null),
            info: StdMutex::new(Value::Null),
            docs: StdMutex::new(HashMap::new()),
            empty_since: StdMutex::new(Instant::now()),
        });

        // Writer: the only place that touches the server's stdin.
        tokio::spawn(async move {
            while let Some(message) = outgoing_rx.recv().await {
                if stdin.write_all(&frame(&message)).await.is_err() || stdin.flush().await.is_err() {
                    break;
                }
            }
        });

        // Stderr: the last lines are kept, to say why a server failed.
        let noting = server.clone();
        let last_words = tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!("{} server of {}: {line}", noting.language, noting.task_id);
                let mut kept = noting.stderr.lock().unwrap();
                kept.push_back(line);
                while kept.len() > STDERR_LINES {
                    kept.pop_front();
                }
            }
        });

        // Reader: cuts the server's stdout into messages.
        let reading = server.clone();
        tokio::spawn(async move {
            let mut framer = Framer::default();
            let mut buffer = vec![0u8; 64 * 1024];
            loop {
                match stdout.read(&mut buffer).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        for body in framer.push(&buffer[..n]) {
                            match serde_json::from_slice::<Value>(&body) {
                                Ok(message) => reading.took(message),
                                Err(_) => tracing::debug!("{} server: a message that is not JSON", reading.language),
                            }
                        }
                    }
                }
            }
            // A server that closed its output is of no use, whether or not its process is still there.
            reading.pull_the_plug();
        });

        // Keeper: holds the process, and notices when it ends by itself.
        let keeping = server.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = child.wait() => {}
                _ = killed => {}
            }
            // Whatever the server started goes with it, and the server itself if it is still there.
            if let Some(pid) = pid {
                kill_family(pid);
            }
            let _ = child.kill().await;
            // What it wrote last says why it ended. The wait is short: something that got away may hold the pipe open.
            let _ = tokio::time::timeout(LAST_WORDS, last_words).await;
            keeping.ended();
            let _ = gone_tx.send(true);
        });

        let hello = server.request("initialize", server.hello(def, cwd));
        let answer = match tokio::time::timeout(START_TIMEOUT, hello).await {
            Ok(Ok(answer)) => answer,
            Ok(Err(error)) => {
                server.give_up().await;
                bail!("{error}{}", server.stderr_tail());
            }
            Err(_) => {
                server.give_up().await;
                bail!("It did not answer in time{}", server.stderr_tail());
            }
        };
        *server.capabilities.lock().unwrap() = answer.get("capabilities").cloned().unwrap_or(json!({}));
        *server.info.lock().unwrap() = answer.get("serverInfo").cloned().unwrap_or(Value::Null);
        server.notify("initialized", json!({}));
        // From here on the interface is told when the server ends.
        server.told.store(false, Ordering::SeqCst);
        if !server.alive() {
            bail!("It ended right after it started{}", server.stderr_tail());
        }
        server.expect_a_file();
        Ok(server)
    }

    /// What the server is told about its client when they first meet.
    fn hello(&self, def: &ServerDef, cwd: &Path) -> Value {
        let folder = cwd.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        json!({
            // A server watches this process and ends when it is gone, should the core die without a word.
            "processId": std::process::id(),
            "clientInfo": { "name": "agent-editor", "version": env!("CARGO_PKG_VERSION") },
            "rootUri": self.root_uri,
            "rootPath": cwd.to_string_lossy(),
            "workspaceFolders": [{ "uri": self.root_uri, "name": folder }],
            "initializationOptions": def.options,
            // What the interface's editor makes use of, and nothing it would have to ignore.
            "capabilities": {
                "general": { "positionEncodings": ["utf-16"] },
                "textDocument": {
                    "synchronization": { "dynamicRegistration": false, "didSave": true },
                    "publishDiagnostics": { "relatedInformation": false },
                    "completion": {
                        "contextSupport": true,
                        "completionItem": {
                            "snippetSupport": false,
                            "documentationFormat": ["markdown", "plaintext"],
                            "labelDetailsSupport": true
                        }
                    },
                    "hover": { "contentFormat": ["markdown", "plaintext"] },
                    "definition": { "linkSupport": true },
                    "formatting": { "dynamicRegistration": false }
                },
                "workspace": { "workspaceFolders": true, "configuration": true },
                "window": { "workDoneProgress": false }
            }
        })
    }

    fn alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    /// Puts a message on the way to the server. A server that has ended takes nothing.
    fn write(&self, message: Value) {
        if let Some(outgoing) = self.outgoing.lock().unwrap().as_ref() {
            let _ = outgoing.send(message);
        }
    }

    fn notify(&self, method: &str, params: Value) {
        self.write(json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    /// Sends a request of the core's own and waits for the server's answer.
    async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = format!("{OWN_ID}{}", self.next_id.fetch_add(1, Ordering::SeqCst));
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id.clone(), tx);
        // Looked at after the request is noted: a server that ended meanwhile has nobody left to answer it.
        if !self.alive() {
            self.pending.lock().unwrap().remove(&id);
            return Err("The language server has ended".into());
        }
        self.write(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        rx.await.unwrap_or_else(|_| Err("The language server ended before it answered".into()))
    }

    /// One message from the server: answered here, kept here, or passed on to the interface.
    fn took(&self, message: Value) {
        let method = message.get("method").and_then(Value::as_str).map(str::to_string);
        let id = message.get("id").filter(|id| !id.is_null()).cloned();
        match (method.as_deref(), id) {
            // The server asks its client something. Every window would answer, so the core does, once.
            (Some(method), Some(id)) => {
                let params = message.get("params").unwrap_or(&Value::Null);
                let answer = match self.answer_to(method, params) {
                    Some(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                    None => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": { "code": -32601, "message": format!("Not supported: {method}") }
                    }),
                };
                self.write(answer);
            }
            (Some(method), None) => {
                // Talk the interface has no place for; a busy server sends a great deal of it.
                if matches!(method, "$/progress" | "$/logTrace" | "window/logMessage" | "telemetry/event") {
                    tracing::trace!("{} server: {message}", self.language);
                    return;
                }
                self.pass_on(message);
            }
            (None, Some(id)) => {
                // An answer: to a request of the core's own, or to one a window made.
                let Some(own) = id.as_str().filter(|id| id.starts_with(OWN_ID)) else {
                    return self.pass_on(message);
                };
                let outcome = match message.get("error").filter(|error| !error.is_null()) {
                    Some(error) => {
                        Err(error.get("message").and_then(Value::as_str).unwrap_or("Unknown error").to_string())
                    }
                    None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
                };
                if let Some(waiting) = self.pending.lock().unwrap().remove(own) {
                    let _ = waiting.send(outcome);
                }
            }
            (None, None) => {}
        }
    }

    /// The core's answer to something a server asks of its client. `None`: not something this client does.
    fn answer_to(&self, method: &str, params: &Value) -> Option<Value> {
        match method {
            // No settings are kept for servers yet: each one asked for is "not set", and the server uses its own.
            "workspace/configuration" => {
                let asked = params.get("items").and_then(Value::as_array).map_or(0, Vec::len);
                Some(json!(vec![Value::Null; asked]))
            }
            "workspace/workspaceFolders" => Some(json!([{ "uri": self.root_uri, "name": self.name }])),
            "client/registerCapability"
            | "client/unregisterCapability"
            | "window/workDoneProgress/create"
            | "window/showMessageRequest" => Some(Value::Null),
            // The editor changes files only when the user saves them.
            "workspace/applyEdit" => Some(json!({ "applied": false })),
            "window/showDocument" => Some(json!({ "success": false })),
            _ => None,
        }
    }

    fn pass_on(&self, message: Value) {
        self.manager
            .broadcast("lsp.message", json!({ "taskId": self.task_id, "language": self.language, "message": message }));
    }

    /// A message from the interface on its way to the server. The messages that
    /// open, change and close a file are counted here; everything else goes as it is.
    fn window_says(&self, window: u64, mut message: Value) {
        let method = message.get("method").and_then(Value::as_str).unwrap_or_default().to_string();
        let uri = message.pointer("/params/textDocument/uri").and_then(Value::as_str).map(str::to_string);
        let (Some(uri), true) = (uri, method.starts_with("textDocument/did")) else {
            return self.write(message);
        };
        // The lock is held while the message is put on its way, so the count and the server agree on the order.
        let mut docs = self.docs.lock().unwrap();
        match method.as_str() {
            "textDocument/didOpen" => match docs.get_mut(&uri) {
                // Another window has the file open. A server must not be told "opened" twice; it is told the text.
                Some(doc) => {
                    doc.holders.insert(window);
                    doc.version += 1;
                    let text = message.pointer("/params/textDocument/text").cloned().unwrap_or(json!(""));
                    self.notify(
                        "textDocument/didChange",
                        json!({
                            "textDocument": { "uri": uri, "version": doc.version },
                            "contentChanges": [{ "text": text }]
                        }),
                    );
                }
                None => {
                    docs.insert(uri, Doc { holders: HashSet::from([window]), version: 1 });
                    message["params"]["textDocument"]["version"] = json!(1);
                    self.write(message);
                }
            },
            "textDocument/didChange" => {
                // A file the server does not have open cannot change: the window opens it first.
                if let Some(doc) = docs.get_mut(&uri) {
                    doc.version += 1;
                    message["params"]["textDocument"]["version"] = json!(doc.version);
                    self.write(message);
                }
            }
            "textDocument/didClose" => {
                let last = docs.get_mut(&uri).is_some_and(|doc| {
                    doc.holders.remove(&window);
                    doc.holders.is_empty()
                });
                if last {
                    docs.remove(&uri);
                    self.write(message);
                    if docs.is_empty() {
                        *self.empty_since.lock().unwrap() = Instant::now();
                    }
                }
            }
            // Saved, and whatever else is said about a file: only of a file the server has open.
            _ => {
                if docs.contains_key(&uri) {
                    self.write(message);
                }
            }
        }
    }

    /// A window went away: the files only it had open are closed.
    fn window_gone(&self, window: u64) {
        let mut docs = self.docs.lock().unwrap();
        let before = docs.len();
        docs.retain(|uri, doc| {
            doc.holders.remove(&window);
            if doc.holders.is_empty() {
                self.notify("textDocument/didClose", json!({ "textDocument": { "uri": uri } }));
            }
            !doc.holders.is_empty()
        });
        if docs.is_empty() && before > 0 {
            *self.empty_since.lock().unwrap() = Instant::now();
        }
    }

    /// How long no file has been open, or `None` while one is.
    fn empty_for(&self) -> Option<Duration> {
        let docs = self.docs.lock().unwrap();
        docs.is_empty().then(|| self.empty_since.lock().unwrap().elapsed())
    }

    /// A window is about to open a file: the wait for "no open file" starts over.
    fn expect_a_file(&self) {
        *self.empty_since.lock().unwrap() = Instant::now();
    }

    fn pull_the_plug(&self) {
        if let Some(kill) = self.kill.lock().unwrap().take() {
            let _ = kill.send(());
        }
    }

    /// The process is gone. Called once, by whoever held it.
    fn ended(&self) {
        self.alive.store(false, Ordering::SeqCst);
        // Nothing more can be sent, and nobody will answer what is still asked.
        self.outgoing.lock().unwrap().take();
        self.pending.lock().unwrap().clear();
        if self.told.swap(true, Ordering::SeqCst) {
            return;
        }
        let reason = *self.purpose.lock().unwrap();
        let mut data = json!({
            "taskId": self.task_id,
            "language": self.language,
            "name": self.name,
            "run": self.run,
            "reason": reason.unwrap_or("crashed")
        });
        if reason.is_none() {
            tracing::warn!(
                "The {} server of task {} ended by itself{}",
                self.language,
                self.task_id,
                self.stderr_tail()
            );
            data["message"] = json!(self.stderr_tail());
        }
        self.manager.broadcast("lsp.stopped", data);
    }

    /// Stops the server: asked nicely first, then for certain, together with everything it started.
    async fn stop(&self, reason: &'static str) {
        self.purpose.lock().unwrap().get_or_insert(reason);
        let mut gone = self.gone.clone();
        if self.alive() {
            // A server that is told to shut down saves what it has learned about the project.
            let polite = async {
                let _ = self.request("shutdown", Value::Null).await;
                self.notify("exit", Value::Null);
                let _ = gone.wait_for(|gone| *gone).await;
            };
            let _ = tokio::time::timeout(POLITE, polite).await;
        }
        self.pull_the_plug();
        let _ = gone.wait_for(|gone| *gone).await;
    }

    /// Ends a server that never got as far as saying hello.
    async fn give_up(&self) {
        self.pull_the_plug();
        let _ = self.gone.clone().wait_for(|gone| *gone).await;
    }

    /// The last lines the server wrote to stderr, as a readable suffix.
    fn stderr_tail(&self) -> String {
        let kept = self.stderr.lock().unwrap();
        if kept.is_empty() {
            return String::new();
        }
        format!(". It wrote: {}", kept.iter().cloned().collect::<Vec<_>>().join(" | "))
    }
}

// ----- every server -----

/// A task and a language: what one server is for.
type Key = (String, String);

/// Owns every running language server.
pub struct Lsp {
    manager: Arc<Manager>,
    table: Vec<ServerDef>,
    /// How long a server with no open file is kept.
    idle: Duration,
    /// One lock per task and language: two windows that open a file at the same moment start one server, not two.
    starting: StdMutex<HashMap<Key, Arc<Mutex<()>>>>,
    servers: StdMutex<HashMap<Key, Arc<Server>>>,
    next_window: AtomicU64,
    next_run: AtomicU64,
    /// Set when the core is stopping: nothing is started after that.
    closed: AtomicBool,
}

/// One connected interface, as the language servers know it. When it is
/// dropped, the files that window had open are closed.
pub struct Window {
    id: u64,
    lsp: Arc<Lsp>,
}

impl Window {
    pub fn id(&self) -> u64 {
        self.id
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        let servers: Vec<_> = self.lsp.servers.lock().unwrap().values().cloned().collect();
        for server in servers {
            server.window_gone(self.id);
        }
    }
}

/// Where it is kept that a project does not want its files formatted on save.
fn format_setting(project_id: &str) -> String {
    format!("project.{project_id}.formatOnSave")
}

impl Lsp {
    /// The servers found on this machine, each started when a file of its language is opened.
    pub fn new(manager: Arc<Manager>) -> Self {
        Self::with(manager, detect(), IDLE)
    }

    /// Like `new`, with the table of servers and the wait before an unused server is stopped given. Tests pass their own.
    pub fn with(manager: Arc<Manager>, table: Vec<ServerDef>, idle: Duration) -> Self {
        Self {
            manager,
            table,
            idle,
            starting: StdMutex::new(HashMap::new()),
            servers: StdMutex::new(HashMap::new()),
            // Zero is "no window": a command that came in some other way than from an interface.
            next_window: AtomicU64::new(1),
            next_run: AtomicU64::new(1),
            closed: AtomicBool::new(false),
        }
    }

    /// Registers a connected interface.
    pub fn window(self: &Arc<Self>) -> Window {
        Window { id: self.next_window.fetch_add(1, Ordering::SeqCst), lsp: self.clone() }
    }

    /// The server for a file, judged by the file's ending, and the protocol's name for that kind of file.
    fn def_for(&self, path: &str) -> Option<(&ServerDef, &str)> {
        let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
        let (_, ending) = name.rsplit_once('.')?;
        let ending = ending.to_ascii_lowercase();
        self.table.iter().find_map(|def| {
            let (_, id) = def.endings.iter().find(|(known, _)| *known == ending)?;
            Some((def, id.as_str()))
        })
    }

    /// Whether files of a project are formatted when they are saved. They are, unless the user said no.
    pub fn format_on_save(&self, project_id: &str) -> bool {
        self.manager.store().setting(&format_setting(project_id)).ok().flatten().as_deref() != Some("off")
    }

    /// Turns formatting on save on or off for a project, and tells every window.
    pub fn set_format_on_save(&self, project_id: &str, on: bool) -> Result<()> {
        self.manager.store().project(project_id)?;
        self.manager.store().set_setting(&format_setting(project_id), if on { None } else { Some("off") })?;
        self.manager.broadcast("lsp.settings", json!({ "projectId": project_id, "formatOnSave": on }));
        Ok(())
    }

    /// Makes sure the server for a file of a task runs, and says what it can do.
    ///
    /// The answer's `state` is "none" for a file of no known language, "missing"
    /// when the server is not installed (with the command that would install
    /// it), "failed" when it would not start, and "running" otherwise.
    pub async fn start(self: &Arc<Self>, task_id: &str, path: &str) -> Result<Value> {
        let Some((def, language_id)) = self.def_for(path) else { return Ok(json!({ "state": "none" })) };
        let task = self.manager.task(task_id)?;
        let cwd = PathBuf::from(&task.row.cwd);
        if !cwd.is_dir() {
            bail!("The task's folder is gone: {}", cwd.display());
        }
        if self.closed.load(Ordering::SeqCst) {
            bail!("The core is stopping");
        }
        let about = |state: &str| json!({ "state": state, "language": def.language, "name": def.name });

        let key = (task_id.to_string(), def.language.clone());
        let lock = self.starting.lock().unwrap().entry(key.clone()).or_default().clone();
        let _one_at_a_time = lock.lock().await;
        let running = self.servers.lock().unwrap().get(&key).filter(|server| server.alive()).cloned();
        let server = match running {
            Some(server) => {
                server.expect_a_file();
                server
            }
            None => {
                let project = self.manager.store().project(&task.row.project_id).ok();
                let project = project.as_ref().map(|project| Path::new(&project.path));
                let found = def.launches.iter().find_map(|launch| {
                    find_program(&launch.program, &cwd, project).map(|program| (program, &launch.args))
                });
                let Some((program, args)) = found else {
                    let mut answer = about("missing");
                    let wanted = def.launches.first().map(|launch| Path::new(&launch.program));
                    answer["command"] = json!(wanted.and_then(Path::file_name).map(|name| name.to_string_lossy()));
                    answer["install"] = json!(def.install);
                    return Ok(answer);
                };
                let run = self.next_run.fetch_add(1, Ordering::SeqCst);
                match Server::start(self.manager.clone(), task_id, def, &program, args, &cwd, run).await {
                    Ok(server) => {
                        // While it was starting, the task may have been archived or the core told to stop.
                        // Then nobody would ever stop this server, so it is not kept.
                        let still_wanted = !self.closed.load(Ordering::SeqCst)
                            && self.starting.lock().unwrap().get(&key).is_some_and(|held| Arc::ptr_eq(held, &lock));
                        if !still_wanted {
                            server.stop("task").await;
                            bail!("The task was closed while its language server was starting");
                        }
                        tracing::info!("Started the {} server of task {task_id}: {}", def.language, program.display());
                        self.servers.lock().unwrap().insert(key, server.clone());
                        self.stop_when_idle(server.clone());
                        server
                    }
                    Err(error) => {
                        let mut answer = about("failed");
                        answer["message"] = json!(format!("{error:#}"));
                        return Ok(answer);
                    }
                }
            }
        };

        let mut answer = about("running");
        answer["languageId"] = json!(language_id);
        answer["run"] = json!(server.run);
        answer["root"] = json!(server.root_uri);
        answer["server"] = server.info.lock().unwrap().clone();
        answer["capabilities"] = server.capabilities.lock().unwrap().clone();
        answer["formatOnSave"] = json!(self.format_on_save(&task.row.project_id));
        Ok(answer)
    }

    /// Stops a server once it has had no open file for a while.
    fn stop_when_idle(&self, server: Arc<Server>) {
        let idle = self.idle;
        let step = (idle / 4).clamp(Duration::from_millis(10), Duration::from_secs(15));
        tokio::spawn(async move {
            while server.alive() {
                tokio::time::sleep(step).await;
                if server.empty_for().is_some_and(|empty| empty >= idle) {
                    server.stop("idle").await;
                }
            }
        });
    }

    /// Carries one JSON-RPC message from a window to the server of a task and language.
    pub fn send(&self, task_id: &str, language: &str, window: u64, mut message: Value) -> Result<()> {
        if !message.is_object() {
            bail!("A message to a language server is a JSON object");
        }
        let key = (task_id.to_string(), language.to_string());
        let server = self.servers.lock().unwrap().get(&key).filter(|server| server.alive()).cloned();
        let server = server.ok_or_else(|| anyhow!("The language server is not running"))?;
        message["jsonrpc"] = json!("2.0");
        server.window_says(window, message);
        Ok(())
    }

    /// The commands of the interface that are about language servers.
    pub async fn command(self: &Arc<Self>, method: &str, params: &Value) -> Result<Value> {
        let text =
            |name: &str| params.get(name).and_then(Value::as_str).ok_or_else(|| anyhow!("Missing parameter: {name}"));
        match method {
            "lsp.start" => self.start(text("taskId")?, text("path")?).await,
            "lsp.send" => {
                // Which window is asking is filled in by the connection, not by the interface.
                let window = params.get("window").and_then(Value::as_u64).unwrap_or(0);
                let message = params.get("message").cloned().ok_or_else(|| anyhow!("Missing parameter: message"))?;
                self.send(text("taskId")?, text("language")?, window, message)?;
                Ok(json!({}))
            }
            "lsp.setFormatOnSave" => {
                let on = params.get("on").and_then(Value::as_bool).ok_or_else(|| anyhow!("Missing parameter: on"))?;
                self.set_format_on_save(text("projectId")?, on)?;
                Ok(json!({ "on": on }))
            }
            other => bail!("Unknown method: {other}"),
        }
    }

    /// How many servers are running, for all tasks or for one.
    pub fn running(&self, task_id: Option<&str>) -> usize {
        let servers = self.servers.lock().unwrap();
        servers.iter().filter(|((task, _), server)| server.alive() && task_id.is_none_or(|id| id == task)).count()
    }

    /// Stops the servers of a task. For a task that is archived or cleaned up.
    pub async fn stop_task(&self, task_id: &str) {
        let of_task: Vec<_> = {
            let mut servers = self.servers.lock().unwrap();
            let keys: Vec<Key> = servers.keys().filter(|(task, _)| task == task_id).cloned().collect();
            keys.iter().filter_map(|key| servers.remove(key)).collect()
        };
        self.starting.lock().unwrap().retain(|(task, _), _| task != task_id);
        futures_util::future::join_all(of_task.iter().map(|server| server.stop("task"))).await;
    }

    /// Stops every server. Called when the core shuts down; nothing is started after it.
    pub async fn stop_all(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let all: Vec<_> = self.servers.lock().unwrap().drain().map(|(_, server)| server).collect();
        futures_util::future::join_all(all.iter().map(|server| server.stop("core"))).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(messages: Vec<Vec<u8>>) -> Vec<String> {
        messages.into_iter().map(|body| String::from_utf8(body).unwrap()).collect()
    }

    #[test]
    fn a_message_split_into_pieces_is_put_together() {
        let whole = frame(&json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }));
        // One byte at a time is the hardest way a stream can arrive.
        let mut framer = Framer::default();
        let mut found = Vec::new();
        for byte in &whole {
            found.extend(framer.push(std::slice::from_ref(byte)));
        }
        assert_eq!(text(found), [r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#]);

        // Cut in the middle of the header, and in the middle of the body.
        for cut in [5, 17, whole.len() - 9] {
            let mut framer = Framer::default();
            assert!(framer.push(&whole[..cut]).is_empty(), "nothing is complete after {cut} bytes");
            assert_eq!(framer.push(&whole[cut..]).len(), 1);
        }
    }

    #[test]
    fn messages_that_arrive_joined_are_told_apart() {
        let mut stream = Vec::new();
        for n in 0..3 {
            stream.extend(frame(&json!({ "id": n })));
        }
        // The first piece ends inside the third message; the rest of it comes later.
        let cut = stream.len() - 4;
        let mut framer = Framer::default();
        assert_eq!(text(framer.push(&stream[..cut])), [r#"{"id":0}"#, r#"{"id":1}"#]);
        assert_eq!(text(framer.push(&stream[cut..])), [r#"{"id":2}"#]);
        assert!(framer.push(b"").is_empty());
    }

    #[test]
    fn a_large_message_arrives_whole() {
        // A completion list of a real server is easily this long. The length counts bytes, not letters.
        let long = "ж".repeat(3 * 1024 * 1024);
        let whole = frame(&json!({ "result": long }));
        let mut framer = Framer::default();
        let mut found = Vec::new();
        for piece in whole.chunks(8192) {
            found.extend(framer.push(piece));
        }
        assert_eq!(found.len(), 1);
        let message: Value = serde_json::from_slice(&found[0]).unwrap();
        assert_eq!(message["result"].as_str().unwrap().len(), long.len());
    }

    #[test]
    fn headers_are_read_whatever_their_case_and_company() {
        let mut framer = Framer::default();
        let found =
            framer.push(b"Content-Type: application/vscode-jsonrpc; charset=utf-8\r\ncontent-length: 2\r\n\r\n{}");
        assert_eq!(text(found), ["{}"]);
        // A header without a length is passed over, and the message after it is still read.
        let found = framer.push(b"Something: else\r\n\r\nContent-Length: 4\r\n\r\nnull");
        assert_eq!(text(found), ["null"]);
        // Noise that never becomes a header does not pile up.
        let mut noisy = Framer::default();
        assert!(noisy.push(&vec![b'x'; MAX_HEADER * 2]).is_empty());
        assert!(noisy.buffer.len() < 8);
        assert_eq!(text(noisy.push(b"\r\nContent-Length: 2\r\n\r\n[]")), ["[]"]);
    }

    #[test]
    fn a_folder_becomes_a_file_address() {
        assert_eq!(file_uri(Path::new("/work/my shop/src")), "file:///work/my%20shop/src");
        assert_eq!(file_uri(Path::new("/work/файл.ts")), "file:///work/%D1%84%D0%B0%D0%B9%D0%BB.ts");
        assert_eq!(file_uri(Path::new("C:\\work\\shop")), "file:///C:/work/shop");
    }

    #[test]
    fn the_known_servers_cover_the_languages_people_write() {
        let table = known_servers();
        let command = |language: &str| {
            let def = table.iter().find(|def| def.language == language).unwrap();
            def.launches
                .iter()
                .map(|l| format!("{} {}", l.program, l.args.join(" ")).trim().to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(command("typescript"), ["typescript-language-server --stdio"]);
        assert_eq!(command("rust"), ["rust-analyzer"]);
        assert_eq!(command("go"), ["gopls"]);
        assert_eq!(command("python"), ["pyright-langserver --stdio", "pylsp"]);
        assert_eq!(command("json"), ["vscode-json-language-server --stdio"]);
        assert_eq!(command("css"), ["vscode-css-language-server --stdio"]);
        assert_eq!(command("html"), ["vscode-html-language-server --stdio"]);
        // Every language says how its server would be installed, and no ending belongs to two languages.
        assert!(table.iter().all(|def| !def.install.is_empty()));
        let mut endings: Vec<&str> = table.iter().flat_map(|def| def.endings.iter().map(|(e, _)| e.as_str())).collect();
        let count = endings.len();
        endings.sort_unstable();
        endings.dedup();
        assert_eq!(endings.len(), count);
    }
}
