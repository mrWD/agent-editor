//! Tool servers of the user's own, given to every agent.
//!
//! An agent reaches the world outside the code through tool servers that
//! speak the Model Context Protocol: one for Sentry, one for Figma, one for a
//! database. The editor calls none of them itself. It keeps the list the user
//! wrote in Settings and names each server to an agent when a session opens,
//! next to its own tools (see `mcp`), the way the protocol between editor and
//! agent provides for:
//!
//! - a program the agent starts and talks to over its input and output. Every
//!   agent takes these;
//! - a server at an address, over HTTP or over the older SSE. An agent takes
//!   these only when it says it can.
//!
//! A server an agent cannot take is left out for that agent, and the start of
//! the session says so. A program in a terminal is not handed any: it keeps
//! its own list, in its own settings.
//!
//! What a server is handed (a token in its environment or in a header) is
//! kept with the other settings on this computer and goes to nobody but the
//! agent that is started.

use crate::settings::Command;
use crate::tasks::Manager;
use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

/// Where the list is kept.
const KEY: &str = "tools.servers";
/// The name of the editor's own server: nobody else's may be called that.
const OWN: &str = "agent-editor";
/// How many servers the list holds. Each one is started or reached anew for every session.
const MOST: usize = 30;

/// How a server is reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A program the agent starts.
    Stdio,
    Http,
    Sse,
}

/// One tool server as the user wrote it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolServer {
    pub id: String,
    /// What the agent sees it as.
    pub name: String,
    pub kind: Kind,
    /// For a program: what starts it, its arguments and what it finds in its environment.
    #[serde(default)]
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Vec<(String, String)>,
    /// For a server at an address: the address, and what is sent with every request.
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    /// Off keeps it in the list and hands it to nobody.
    pub on: bool,
}

/// What an agent says it can reach.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Reach {
    pub http: bool,
    pub sse: bool,
}

/// Whether text can be the name of a header: letters, digits and the few signs HTTP allows there.
fn is_header_name(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|letter| letter.is_ascii_alphanumeric() || "-_.".contains(letter))
}

impl ToolServer {
    /// Reads a server the interface sent, and says in words what is wrong with it.
    fn read(params: &Value) -> Result<Self> {
        let text = |name: &str| params.get(name).and_then(Value::as_str).map(str::trim).unwrap_or_default();
        let name = text("name");
        if name.is_empty() {
            bail!("Give the server a name: agents see it by that name");
        }
        if name.chars().count() > 40
            || !name.chars().all(|letter| letter.is_ascii_alphanumeric() || " _-.".contains(letter))
        {
            bail!("A server's name is up to 40 letters, digits, spaces, dots, dashes and underscores");
        }
        if name.eq_ignore_ascii_case(OWN) {
            bail!("\"{OWN}\" is the name of the editor's own tools. Give this one another name.");
        }
        let kind = match text("kind") {
            "stdio" => Kind::Stdio,
            "http" => Kind::Http,
            "sse" => Kind::Sse,
            _ => bail!("Say how the server is reached: a program, or an address over HTTP or SSE"),
        };
        let on = params.get("on").and_then(Value::as_bool).unwrap_or(true);
        let id = text("id").to_string();
        let mut server = Self {
            id,
            name: name.to_string(),
            kind,
            program: String::new(),
            args: vec![],
            env: vec![],
            url: String::new(),
            headers: vec![],
            on,
        };
        if kind == Kind::Stdio {
            let command = Command::read(params).map_err(|error| {
                // The reader speaks of agents; here the program is a server's.
                anyhow!("{}", error.to_string().replace("that starts the agent", "that starts the server"))
            })?;
            (server.program, server.args, server.env) = (command.program, command.args, command.env);
            return Ok(server);
        }
        let url = text("url");
        let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"));
        if !rest.is_some_and(|rest| !rest.is_empty() && !rest.starts_with('/')) || url.contains(char::is_whitespace) {
            bail!("Give the server's address, beginning with https:// or http://");
        }
        server.url = url.to_string();
        for pair in params.get("headers").and_then(Value::as_array).into_iter().flatten() {
            let (Some(header), Some(value)) = (
                pair.get(0).and_then(Value::as_str).map(str::trim),
                pair.get(1).and_then(Value::as_str).map(str::trim),
            ) else {
                bail!("Every header is a name and a value");
            };
            if !is_header_name(header) || value.contains(['\r', '\n', '\0']) {
                bail!("\"{header}\" cannot be a header: a name of letters, digits and dashes, and a value on one line");
            }
            if server.headers.iter().any(|(known, _)| known.eq_ignore_ascii_case(header)) {
                bail!("The header {header} is there twice");
            }
            server.headers.push((header.to_string(), value.to_string()));
        }
        Ok(server)
    }

    /// The server as the protocol between editor and agent names it at the start of a session.
    fn named(&self) -> Value {
        let pairs = |list: &[(String, String)]| -> Vec<Value> {
            list.iter().map(|(name, value)| json!({ "name": name, "value": value })).collect()
        };
        match self.kind {
            Kind::Stdio => {
                // The protocol asks for the whole path; a bare name is looked up as a shell would.
                let found = which::which(&self.program).map(|path| path.to_string_lossy().into_owned());
                json!({
                    "name": self.name,
                    "command": found.unwrap_or_else(|_| self.program.clone()),
                    "args": self.args,
                    "env": pairs(&self.env)
                })
            }
            Kind::Http | Kind::Sse => json!({
                "type": if self.kind == Kind::Http { "http" } else { "sse" },
                "name": self.name,
                "url": self.url,
                "headers": pairs(&self.headers)
            }),
        }
    }
}

impl Manager {
    /// The tool servers of the user's own, in the order they were added.
    pub fn tool_servers(&self) -> Vec<ToolServer> {
        let kept = self.store().setting(KEY).ok().flatten();
        kept.and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
    }

    fn keep_tool_servers(&self, servers: &[ToolServer]) -> Result<()> {
        self.store().set_setting(KEY, Some(&serde_json::to_string(servers)?))?;
        self.broadcast("tools.changed", json!({}));
        Ok(())
    }

    /// Adds a server, or changes the one with the `id` that was sent. Gives its id.
    pub fn tool_server_save(&self, params: &Value) -> Result<String> {
        let mut server = ToolServer::read(params)?;
        let mut servers = self.tool_servers();
        let taken = servers.iter().any(|other| other.id != server.id && other.name.eq_ignore_ascii_case(&server.name));
        if taken {
            bail!("There is a server called {} already", server.name);
        }
        match servers.iter_mut().find(|known| !server.id.is_empty() && known.id == server.id) {
            Some(known) => *known = server.clone(),
            None if !server.id.is_empty() => bail!("No such server: {}", server.id),
            None => {
                if servers.len() >= MOST {
                    bail!("The list holds {MOST} servers at most. Remove one first.");
                }
                server.id = uuid::Uuid::new_v4().simple().to_string();
                servers.push(server.clone());
            }
        }
        self.keep_tool_servers(&servers)?;
        Ok(server.id)
    }

    pub fn tool_server_remove(&self, id: &str) -> Result<()> {
        let mut servers = self.tool_servers();
        let before = servers.len();
        servers.retain(|server| server.id != id);
        if servers.len() == before {
            bail!("No such server: {id}");
        }
        self.keep_tool_servers(&servers)
    }

    /// The servers to name to an agent when its session opens, and the names of the ones that are
    /// left out because this agent cannot reach them, each with the reason.
    pub(crate) fn tool_servers_for(&self, reach: Reach) -> (Vec<Value>, Vec<(String, &'static str)>) {
        let (mut named, mut left_out) = (Vec::new(), Vec::new());
        for server in self.tool_servers().into_iter().filter(|server| server.on) {
            let why = match server.kind {
                Kind::Http if !reach.http => Some("this agent takes no tool servers over HTTP"),
                Kind::Sse if !reach.sse => Some("this agent takes no tool servers over SSE"),
                _ => None,
            };
            match why {
                Some(why) => left_out.push((server.name, why)),
                None => named.push(server.named()),
            }
        }
        (named, left_out)
    }
}

/// The commands of the interface that are about tool servers.
///
/// - `tools.list` gives `{ servers: [{ id, name, kind, program, args, env, url, headers, on }] }`.
/// - `tools.save { id?, name, kind, … }` adds or changes one and gives the list, with `saved`: its id.
/// - `tools.remove { id }` takes one out and gives the list.
///
/// A change holds for sessions that open from then on; an agent that is running goes on with what it was given.
pub async fn command(manager: &Arc<Manager>, method: &str, params: &Value) -> Result<Value> {
    let mut saved = None;
    match method {
        "tools.list" => {}
        "tools.save" => saved = Some(manager.tool_server_save(params)?),
        "tools.remove" => {
            let id = params.get("id").and_then(Value::as_str).ok_or_else(|| anyhow!("Missing parameter: id"))?;
            manager.tool_server_remove(id)?;
        }
        other => bail!("Unknown method: {other}"),
    }
    Ok(json!({ "servers": manager.tool_servers(), "saved": saved }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_server_is_read_and_what_is_wrong_with_it_is_said() {
        let program = ToolServer::read(&json!({
            "name": " Sentry ", "kind": "stdio", "program": "npx", "args": ["-y", "@sentry/mcp-server"],
            "env": [["SENTRY_ACCESS_TOKEN", "secret"]]
        }))
        .unwrap();
        assert_eq!((program.name.as_str(), program.kind, program.on), ("Sentry", Kind::Stdio, true));
        assert_eq!(program.args, vec!["-y", "@sentry/mcp-server"]);
        assert_eq!(program.env, vec![("SENTRY_ACCESS_TOKEN".to_string(), "secret".to_string())]);

        let remote = ToolServer::read(&json!({
            "name": "Notion", "kind": "http", "url": " https://mcp.notion.com/mcp ",
            "headers": [["Authorization", "Bearer abc"]], "on": false
        }))
        .unwrap();
        assert_eq!((remote.url.as_str(), remote.on), ("https://mcp.notion.com/mcp", false));
        assert_eq!(remote.headers, vec![("Authorization".to_string(), "Bearer abc".to_string())]);

        let wrong = |params: Value| ToolServer::read(&params).unwrap_err().to_string();
        assert!(wrong(json!({ "kind": "http", "url": "https://x.dev" })).contains("Give the server a name"));
        assert!(wrong(json!({ "name": "a/b", "kind": "http", "url": "https://x.dev" })).contains("up to 40 letters"));
        assert!(
            wrong(json!({ "name": "Agent-Editor", "kind": "http", "url": "https://x.dev" }))
                .contains("editor's own tools")
        );
        assert!(wrong(json!({ "name": "A", "kind": "pipe" })).contains("how the server is reached"));
        assert!(wrong(json!({ "name": "A", "kind": "stdio", "program": " " })).contains("that starts the server"));
        for url in ["", "mcp.notion.com/mcp", "ftp://x", "https://", "https:///mcp", "https://a b"] {
            assert!(
                wrong(json!({ "name": "A", "kind": "http", "url": url })).contains("beginning with https://"),
                "{url}"
            );
        }
        let header = |name: &str, value: &str| json!({ "name": "A", "kind": "sse", "url": "http://localhost:1/sse", "headers": [[name, value]] });
        assert!(wrong(header("X Y", "1")).contains("cannot be a header"));
        assert!(wrong(header("X", "1\r\nEvil: 2")).contains("cannot be a header"));
        let twice =
            json!({ "name": "A", "kind": "http", "url": "https://x.dev", "headers": [["Key", "1"], ["key", "2"]] });
        assert!(wrong(twice).contains("there twice"));
    }

    #[test]
    fn a_server_is_named_to_an_agent_the_way_the_protocol_says() {
        let remote = ToolServer::read(&json!({
            "name": "Notion", "kind": "http", "url": "https://mcp.notion.com/mcp", "headers": [["Authorization", "Bearer abc"]]
        }))
        .unwrap();
        assert_eq!(
            remote.named(),
            json!({ "type": "http", "name": "Notion", "url": "https://mcp.notion.com/mcp",
                    "headers": [{ "name": "Authorization", "value": "Bearer abc" }] })
        );
        // A program has no "type", and one that is not on this computer keeps the name it was given.
        let program = ToolServer::read(&json!({
            "name": "Db", "kind": "stdio", "program": "ae-no-such-program", "args": ["--ro"], "env": [["URL", "x"]]
        }))
        .unwrap();
        assert_eq!(
            program.named(),
            json!({ "name": "Db", "command": "ae-no-such-program", "args": ["--ro"], "env": [{ "name": "URL", "value": "x" }] })
        );
    }
}
