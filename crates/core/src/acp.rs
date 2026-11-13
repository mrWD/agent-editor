//! A small client for the Agent Client Protocol.
//!
//! The agent is a child process. Messages are JSON-RPC 2.0, one per line, on
//! the child's stdin and stdout. This module only moves messages: it matches
//! answers to requests and hands everything the agent sends on its own
//! (notifications, its requests to us, stderr lines) to the caller.
//!
//! Message bodies stay as `serde_json::Value` on purpose. The protocol keeps
//! growing, and fields this code does not know about must reach the interface
//! untouched.

use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, mpsc, oneshot};

/// A JSON-RPC error, either sent by the agent or made here when the agent is gone.
#[derive(Debug, Clone)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl RpcError {
    /// The agent process ended before it answered.
    pub fn closed() -> Self {
        Self { code: -32000, message: "The agent stopped before it answered".into(), data: None }
    }

    fn from_value(value: &Value) -> Self {
        Self {
            code: value.get("code").and_then(Value::as_i64).unwrap_or(-32603),
            message: value.get("message").and_then(Value::as_str).unwrap_or("Unknown error").to_string(),
            data: value.get("data").cloned(),
        }
    }
}

impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (code {})", self.message, self.code)
    }
}

impl std::error::Error for RpcError {}

/// Something the agent sent without being asked.
#[derive(Debug)]
pub enum Incoming {
    /// A notification, for example `session/update`.
    Notification { method: String, params: Value },
    /// A request the agent expects an answer to, for example `session/request_permission`.
    Request { id: Value, method: String, params: Value },
    /// The agent's answer to one of our requests, handed over in the order it
    /// arrived. Pass it to `Connection::settle`. Only sent once `answers_in_order`
    /// was called; before that, answers go straight to whoever asked.
    Answer { id: u64, outcome: Result<Value, RpcError> },
    /// One line the agent wrote to stderr.
    Stderr(String),
    /// The agent's stdout closed: the process ended.
    Closed,
}

type Pending = Arc<StdMutex<HashMap<u64, oneshot::Sender<Result<Value, RpcError>>>>>;

/// A live connection to one agent process.
pub struct Connection {
    outgoing: mpsc::UnboundedSender<String>,
    pending: Pending,
    /// When set, answers travel through the same queue as notifications. See `answers_in_order`.
    in_order: Arc<AtomicBool>,
    next_id: AtomicU64,
    child: Mutex<Option<Child>>,
    /// A second way into the stream of what the agent sends, to say "closed"
    /// when the agent was stopped but something it started keeps its output open.
    closed: mpsc::UnboundedSender<Incoming>,
}

impl Connection {
    /// Starts the agent and returns the connection plus the stream of what it sends.
    pub fn spawn(
        program: &str,
        args: &[String],
        cwd: &Path,
        env: &[(String, String)],
    ) -> Result<(Arc<Self>, mpsc::UnboundedReceiver<Incoming>)> {
        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // Agents are often started through a launcher (`npx` is one), which starts
        // the real agent as its own child. The whole family gets one group, so
        // stopping the agent stops all of it.
        #[cfg(unix)]
        {
            command.process_group(0);
            // SAFETY: the function only calls `fcntl`, which is safe between `fork` and `exec`.
            unsafe { command.pre_exec(leave_others_streams) };
        }
        for (key, value) in env {
            command.env(key, value);
        }

        let mut child = command.spawn().with_context(|| format!("Could not start the agent: {program}"))?;
        let mut stdin = child.stdin.take().context("The agent has no stdin")?;
        let stdout = child.stdout.take().context("The agent has no stdout")?;
        let stderr = child.stderr.take().context("The agent has no stderr")?;

        let (outgoing, mut outgoing_rx) = mpsc::unbounded_channel::<String>();
        let (incoming_tx, incoming_rx) = mpsc::unbounded_channel::<Incoming>();
        let pending: Pending = Arc::new(StdMutex::new(HashMap::new()));

        // Writer: the only place that touches the agent's stdin.
        tokio::spawn(async move {
            while let Some(line) = outgoing_rx.recv().await {
                if stdin.write_all(line.as_bytes()).await.is_err()
                    || stdin.write_all(b"\n").await.is_err()
                    || stdin.flush().await.is_err()
                {
                    break;
                }
            }
        });

        // Stderr: passed on line by line, never parsed.
        let stderr_tx = incoming_tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if stderr_tx.send(Incoming::Stderr(line)).is_err() {
                    break;
                }
            }
        });

        // Reader: one JSON message per line. A line can be very long (a whole
        // diff), so no length limit is applied.
        let closed = incoming_tx.clone();
        let reader_pending = pending.clone();
        let in_order = Arc::new(AtomicBool::new(false));
        let reader_in_order = in_order.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if line.trim().is_empty() {
                    continue;
                }
                let Ok(message) = serde_json::from_str::<Value>(&line) else {
                    let _ = incoming_tx.send(Incoming::Stderr(format!("Not JSON on stdout: {line}")));
                    continue;
                };
                Self::dispatch(message, &reader_pending, &incoming_tx, reader_in_order.load(Ordering::SeqCst));
            }
            // The agent is gone: nobody will answer what is still pending.
            let waiting: Vec<_> = reader_pending.lock().unwrap().drain().collect();
            for (_, tx) in waiting {
                let _ = tx.send(Err(RpcError::closed()));
            }
            let _ = incoming_tx.send(Incoming::Closed);
        });

        let connection = Arc::new(Self {
            outgoing,
            pending,
            in_order,
            next_id: AtomicU64::new(1),
            child: Mutex::new(Some(child)),
            closed,
        });
        Ok((connection, incoming_rx))
    }

    fn dispatch(message: Value, pending: &Pending, incoming: &mpsc::UnboundedSender<Incoming>, in_order: bool) {
        if let Some(method) = message.get("method").and_then(Value::as_str) {
            let params = message.get("params").cloned().unwrap_or(Value::Null);
            let event = match message.get("id") {
                Some(id) if !id.is_null() => Incoming::Request { id: id.clone(), method: method.to_string(), params },
                _ => Incoming::Notification { method: method.to_string(), params },
            };
            let _ = incoming.send(event);
            return;
        }
        // No method: this is an answer to one of our requests.
        let Some(id) = message.get("id").and_then(Value::as_u64) else { return };
        let outcome = match message.get("error") {
            Some(error) if !error.is_null() => Err(RpcError::from_value(error)),
            _ => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
        };
        if in_order {
            // Behind everything the agent sent before it. If nobody reads the queue any more, answer directly.
            if let Err(mpsc::error::SendError(Incoming::Answer { id, outcome })) =
                incoming.send(Incoming::Answer { id, outcome })
            {
                Self::settle_in(pending, id, outcome);
            }
        } else {
            Self::settle_in(pending, id, outcome);
        }
    }

    fn settle_in(pending: &Pending, id: u64, outcome: Result<Value, RpcError>) {
        if let Some(tx) = pending.lock().unwrap().remove(&id) {
            let _ = tx.send(outcome);
        }
    }

    /// From now on answers are handed over through the queue of incoming
    /// messages, as `Incoming::Answer`, instead of going straight to whoever
    /// asked. An agent ends a turn by answering the prompt, right after its last
    /// words; taken out of order, "the turn is over" could be acted on before
    /// those words were read. Whoever reads the queue must call `settle`.
    pub fn answers_in_order(&self) {
        self.in_order.store(true, Ordering::SeqCst);
    }

    /// Hands an answer taken from the queue to the request that waits for it.
    pub fn settle(&self, id: u64, outcome: Result<Value, RpcError>) {
        Self::settle_in(&self.pending, id, outcome);
    }

    /// Sends a request and waits for the agent's answer.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        let line = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }).to_string();
        if self.outgoing.send(line).is_err() {
            self.pending.lock().unwrap().remove(&id);
            return Err(RpcError::closed());
        }
        rx.await.unwrap_or_else(|_| Err(RpcError::closed()))
    }

    /// Sends a notification: no answer is expected.
    pub fn notify(&self, method: &str, params: Value) {
        let line = json!({ "jsonrpc": "2.0", "method": method, "params": params }).to_string();
        let _ = self.outgoing.send(line);
    }

    /// Answers a request the agent made.
    pub fn respond(&self, id: &Value, result: Value) {
        let line = json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string();
        let _ = self.outgoing.send(line);
    }

    /// Refuses a request the agent made.
    pub fn respond_error(&self, id: &Value, code: i64, message: &str) {
        let line = json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": code, "message": message }
        })
        .to_string();
        let _ = self.outgoing.send(line);
    }

    /// Stops the agent process and everything it started. Safe to call more than once.
    pub async fn kill(&self) {
        let Some(mut child) = self.child.lock().await.take() else { return };
        if let Some(pid) = child.id() {
            kill_family(pid);
        }
        let _ = child.kill().await;
        // Nobody will answer what is still pending, and nothing more will come.
        let waiting: Vec<_> = self.pending.lock().unwrap().drain().collect();
        for (_, tx) in waiting {
            let _ = tx.send(Err(RpcError::closed()));
        }
        let _ = self.closed.send(Incoming::Closed);
    }
}

/// Run in a child between `fork` and `exec`: everything the child inherited
/// besides its own three streams is marked to be closed when its program starts.
///
/// On macOS a pipe is made first and marked "close when a program starts"
/// second, so a child started by another thread in between inherits it. A
/// short-lived child gives it back at once. An agent, a language server or a
/// check would hold it for as long as it lives, and whoever waits for that pipe
/// to end (a `git` call whose output is being read) would wait as long.
#[cfg(unix)]
pub(crate) fn leave_others_streams() -> std::io::Result<()> {
    for fd in 3..1024 {
        // SAFETY: `fcntl` may be called between `fork` and `exec`; a number that is no open file is refused, which is fine.
        unsafe {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
    }
    Ok(())
}

/// Stops a process together with the processes it started.
#[cfg(unix)]
pub(crate) fn kill_family(pid: u32) {
    // The agent was started as the head of its own group: the group's number is its own.
    // SAFETY: `killpg` only sends a signal; a group that is gone already is not an error to act on.
    unsafe {
        libc::killpg(pid as libc::pid_t, libc::SIGKILL);
    }
}

/// Stops a process together with the processes it started.
#[cfg(windows)]
pub(crate) fn kill_family(pid: u32) {
    let _ = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}
