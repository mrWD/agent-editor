//! The database of a task: what its migrations do, a trial of them, and a look at the rows.
//!
//! A change to a service is often a change to its tables. What that change
//! is cannot be seen on a page, so Preview shows it another way:
//!
//! - **the migrations the task added or changed**, each read from its text
//!   and said step by step, with the steps that can lose data marked
//!   (see `migrations`);
//! - **a trial**: the migrations are run on the task's own database, taken
//!   back, and run again, with the tables looked at before and after each
//!   step. It says what the migrations did to the tables for real, and
//!   whether taking them back leaves the database as it was;
//! - **the tables and a query**, to look at what is there.
//!
//! A database is reached in one of three ways. One that is a service of the
//! task's compose project is asked through the client inside its own
//! container, so nothing has to be installed and no password is handled here.
//! One named by an address in the task's environment (`DATABASE_URL`) is
//! asked with the client on this computer. A file of SQLite is opened here.
//!
//! A query is run for reading only unless the user says it may change things.

pub mod migrations;

use crate::gitops;
use crate::runner::{self, Place, Runner};
use crate::server::AppState;
use crate::tasks::Manager;
use anyhow::{Context, Result, anyhow, bail};
use migrations::{Step, Tool};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The names an address of a database goes by in a project's environment.
const URL_NAMES: [&str; 7] = [
    "DATABASE_URL",
    "DB_URL",
    "POSTGRES_URL",
    "MYSQL_URL",
    "SQLALCHEMY_DATABASE_URI",
    "DATABASE_URI",
    "DB_CONNECTION_STRING",
];
/// Where a file of SQLite is looked for when nothing names one.
const SQLITE_FOLDERS: [&str; 7] = ["", "data", "db", "instance", "prisma", "var", "storage"];
const SQLITE_ENDS: [&str; 3] = [".sqlite3", ".sqlite", ".db"];
/// How many rows of an answer are given. More are said to be there.
const MOST_ROWS: usize = 500;
/// How much of what a client printed is read.
const MOST_OUTPUT: usize = 8 * 1024 * 1024;
/// How long one query may take, and how long one run of the migrations.
const QUERY_PATIENCE: Duration = Duration::from_secs(40);
const MIGRATE_PATIENCE: Duration = Duration::from_secs(600);
/// How much of what a migration command printed is shown: its end.
const MOST_SAID: usize = 6000;
/// What stands for "nothing" in a cell, so it is not taken for an empty text.
pub const NOTHING: &str = "∅";

const READ_ONLY_HINT: &str = "The query was run for reading only. Allow changes to run one that writes.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Engine {
    Postgres,
    Mysql,
    Sqlite,
    Other,
}

impl Engine {
    fn word(self) -> &'static str {
        match self {
            Engine::Postgres => "postgres",
            Engine::Mysql => "mysql",
            Engine::Sqlite => "sqlite",
            Engine::Other => "other",
        }
    }

    fn of_image(image: &str) -> Engine {
        match runner::image_name(image) {
            "postgres" | "postgresql" | "postgis" | "pgvector" | "timescaledb" => Engine::Postgres,
            "mysql" | "mysql-server" | "mariadb" | "percona" => Engine::Mysql,
            _ => Engine::Other,
        }
    }
}

/// How a database is asked.
#[derive(Debug, Clone)]
enum Way {
    /// A service of the task's compose project, through the client in its container.
    Compose { service: String },
    /// A file of SQLite.
    File(PathBuf),
    /// An address, through a client on this computer.
    Client { program: PathBuf, address: Address },
}

/// The parts of an address of a database.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Address {
    user: String,
    password: String,
    host: String,
    port: String,
    name: String,
}

/// One database of a task.
#[derive(Debug, Clone)]
struct Db {
    /// By what it is reached: `docker:db`, `env:DATABASE_URL`, `file:db.sqlite3`.
    id: String,
    name: String,
    engine: Engine,
    way: Way,
    /// Whether it can be asked right now.
    ready: bool,
    note: Option<String>,
    /// Whether other tasks may keep their rows in it too.
    shared: bool,
}

impl Db {
    fn shown(&self) -> Value {
        let via = match self.way {
            Way::Compose { .. } => "docker",
            Way::File(_) => "file",
            Way::Client { .. } => "address",
        };
        json!({ "id": self.id, "name": self.name, "engine": self.engine.word(), "via": via,
                "ready": self.ready, "note": self.note, "shared": self.shared })
    }
}

/// Reads an address as projects write it: `postgresql+asyncpg://user:pw@db:5432/shop`, `sqlite:///./app.db`.
fn parse_url(url: &str) -> Option<(Engine, Address, String)> {
    let (scheme, rest) = url.trim().split_once(':')?;
    let scheme = scheme.split('+').next().unwrap_or_default().to_ascii_lowercase();
    let engine = match scheme.as_str() {
        "postgres" | "postgresql" => Engine::Postgres,
        "mysql" | "mariadb" => Engine::Mysql,
        "sqlite" | "sqlite3" | "file" => Engine::Sqlite,
        _ => return None,
    };
    if engine == Engine::Sqlite {
        // `sqlite:///app.db` is a path beside the project, `sqlite:////var/app.db` one from the root.
        let path = rest.strip_prefix("///").or_else(|| rest.strip_prefix("//")).unwrap_or(rest);
        let path = path.split('?').next().unwrap_or_default();
        return (!path.is_empty() && path != ":memory:").then(|| (engine, Address::default(), path.to_string()));
    }
    let rest = rest.strip_prefix("//")?;
    let (rest, _) = rest.split_once('?').unwrap_or((rest, ""));
    let (who, at) = rest.rsplit_once('@').unwrap_or(("", rest));
    let (user, password) = who.split_once(':').unwrap_or((who, ""));
    let (place, name) = at.split_once('/').unwrap_or((at, ""));
    let (host, port) = place.rsplit_once(':').unwrap_or((place, ""));
    let address = Address {
        user: unescape(user),
        password: unescape(password),
        host: host.to_string(),
        port: port.to_string(),
        name: name.to_string(),
    };
    Some((engine, address, String::new()))
}

/// Takes the percent signs out of a piece of an address: a password with `@` in it is written `%40`.
fn unescape(text: &str) -> String {
    let bytes = text.as_bytes();
    let (mut out, mut at) = (Vec::with_capacity(bytes.len()), 0);
    while at < bytes.len() {
        let written_out = (bytes[at] == b'%')
            .then(|| bytes.get(at + 1..at + 3))
            .flatten()
            .and_then(|hex| std::str::from_utf8(hex).ok())
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match written_out {
            Some(byte) => {
                out.push(byte);
                at += 3;
            }
            None => {
                out.push(bytes[at]);
                at += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Every database of a task, the ones of its compose project first.
fn databases_of(runner: &Runner, place: &Place) -> Vec<Db> {
    let mut found: Vec<Db> = Vec::new();
    let root = place.root.as_path();
    let mut services: HashSet<String> = HashSet::new();

    if let Some(file) = runner::compose_file(root)
        && runner.docker_running().0
        && let Ok(config) = runner.resolved(place, &file)
    {
        let project = place.compose_project();
        let containers = runner.containers(&project, root);
        services.extend(
            config.get("services").and_then(Value::as_object).into_iter().flatten().map(|(name, _)| name.clone()),
        );
        for database in runner::databases(&config, &project) {
            let engine = Engine::of_image(&database.image);
            let running = containers.iter().any(|container| {
                container.get("Service").and_then(Value::as_str) == Some(database.name.as_str())
                    && container.get("State").and_then(Value::as_str) == Some("running")
            });
            let note = match (engine, running) {
                (Engine::Other, _) => {
                    Some(format!("What {} holds cannot be read here.", runner::image_name(&database.image)))
                }
                (_, false) => Some("It is not running. Start it in Run.".to_string()),
                _ => None,
            };
            found.push(Db {
                id: format!("docker:{}", database.name),
                name: format!("{} ({})", database.name, runner::image_name(&database.image)),
                engine,
                way: Way::Compose { service: database.name.clone() },
                ready: running && engine != Engine::Other,
                note,
                shared: database.volumes.iter().any(|volume| !volume.own),
            });
        }
    }

    let mut files: Vec<PathBuf> = Vec::new();
    for (name, url) in place.env.iter().filter(|(name, value)| URL_NAMES.contains(&name.as_str()) && !value.is_empty())
    {
        let Some((engine, address, path)) = parse_url(url) else { continue };
        if engine == Engine::Sqlite {
            let file = root.join(&path);
            if !files.contains(&file) {
                files.push(file);
            }
            continue;
        }
        // An address that points at a service of the compose project is that service, listed already.
        let local = matches!(address.host.as_str(), "localhost" | "127.0.0.1" | "::1" | "[::1]" | "");
        let listed = found.iter().any(|db| db.engine == engine);
        if services.contains(&address.host) || (local && listed) {
            continue;
        }
        let programs: &[&str] = if engine == Engine::Postgres { &["psql"] } else { &["mysql", "mariadb"] };
        let program = programs.iter().find_map(|program| which::which(program).ok());
        found.push(Db {
            id: format!("env:{name}"),
            name: format!(
                "{} at {} ({name})",
                address.name,
                if address.host.is_empty() { "this computer" } else { &address.host }
            ),
            engine,
            ready: program.is_some(),
            note: program.is_none().then(|| format!("To read it, install {} on this computer.", programs[0])),
            way: Way::Client { program: program.unwrap_or_default(), address },
            // Whose rows these are cannot be told from an address.
            shared: !local,
        });
    }
    for folder in SQLITE_FOLDERS {
        let Ok(listed) = std::fs::read_dir(root.join(folder)) else { continue };
        let mut names: Vec<PathBuf> = listed
            .flatten()
            .map(|one| one.path())
            .filter(|path| path.is_file() && SQLITE_ENDS.iter().any(|end| path.to_string_lossy().ends_with(end)))
            .collect();
        names.sort();
        files.extend(names.into_iter().filter(|path| !files.contains(path)).collect::<Vec<_>>());
    }
    files.dedup();
    for file in files.into_iter().take(6) {
        let name = file.strip_prefix(root).unwrap_or(&file).to_string_lossy().into_owned();
        let there = file.is_file();
        found.push(Db {
            id: format!("file:{name}"),
            name: name.clone(),
            engine: Engine::Sqlite,
            ready: there,
            note: (!there).then(|| "The file is not there yet. The migrations make it.".to_string()),
            way: Way::File(file),
            shared: false,
        });
    }
    found
}

// ----- asking a database -----

/// What a query gave.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct Rows {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    /// Whether there were more rows than are given.
    pub more: bool,
    /// What there is to say when no rows came back.
    pub said: Option<String>,
}

/// What a program printed, and how it ended.
struct Ran {
    ok: bool,
    out: Vec<u8>,
    err: String,
}

/// Runs a program to its end, reading at most `most` of what it prints and waiting at most `patience`.
fn run(command: &mut Command, patience: Duration, most: usize) -> Result<Ran> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // A group of its own, so what the program started goes with it when it is stopped.
        command.process_group(0);
    }
    let mut child = command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let read = |mut pipe: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut kept = Vec::new();
            let mut buffer = [0u8; 16 * 1024];
            while let Ok(count) = pipe.read(&mut buffer) {
                if count == 0 {
                    break;
                }
                // Past the limit the rest is read and dropped, so the program is not left waiting.
                let room = most.saturating_sub(kept.len());
                kept.extend_from_slice(&buffer[..count.min(room)]);
            }
            kept
        })
    };
    let out = read(Box::new(child.stdout.take().context("no output")?));
    let err = read(Box::new(child.stderr.take().context("no output")?));
    let deadline = Instant::now() + patience;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if Instant::now() >= deadline {
            #[cfg(unix)]
            // SAFETY: the group was made above for this child alone.
            unsafe {
                libc::killpg(child.id() as i32, libc::SIGKILL);
            }
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(15));
    };
    let (out, err) =
        (out.join().unwrap_or_default(), String::from_utf8_lossy(&err.join().unwrap_or_default()).into_owned());
    match status {
        Some(status) => Ok(Ran { ok: status.success(), out, err }),
        None => bail!("It did not end in {} seconds and was stopped", patience.as_secs()),
    }
}

/// Reads what `psql --csv` printed: a line of names, then the rows.
pub fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let (mut row, mut cell, mut quoted, mut any) = (Vec::new(), String::new(), false, false);
    let mut signs = text.chars().peekable();
    while let Some(sign) = signs.next() {
        match (quoted, sign) {
            (true, '"') if signs.peek() == Some(&'"') => {
                signs.next();
                cell.push('"');
            }
            (true, '"') => quoted = false,
            (true, other) => cell.push(other),
            (false, '"') => {
                quoted = true;
                any = true;
            }
            (false, ',') => {
                row.push(std::mem::take(&mut cell));
                any = true;
            }
            (false, '\r') => {}
            (false, '\n') => {
                if any || !cell.is_empty() {
                    row.push(std::mem::take(&mut cell));
                    rows.push(std::mem::take(&mut row));
                }
                any = false;
            }
            (false, other) => cell.push(other),
        }
    }
    if any || !cell.is_empty() {
        row.push(cell);
        rows.push(row);
    }
    rows
}

/// Reads what `mysql --batch` printed: cells between tabs, with a tab or a new line inside a cell written out.
pub fn parse_tsv(text: &str) -> Vec<Vec<String>> {
    text.lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            line.split('\t')
                .map(|cell| {
                    if cell == "NULL" {
                        return NOTHING.to_string();
                    }
                    let (mut out, mut signs) = (String::with_capacity(cell.len()), cell.chars());
                    while let Some(sign) = signs.next() {
                        match (sign, sign == '\\') {
                            (_, true) => match signs.next() {
                                Some('t') => out.push('\t'),
                                Some('n') => out.push('\n'),
                                Some('0') => out.push('\0'),
                                Some(other) => out.push(other),
                                None => out.push('\\'),
                            },
                            (other, false) => out.push(other),
                        }
                    }
                    out
                })
                .collect()
        })
        .collect()
}

fn rows_of(mut table: Vec<Vec<String>>) -> Rows {
    if table.is_empty() {
        return Rows { said: Some("Done. It gave no rows.".into()), ..Rows::default() };
    }
    let columns = table.remove(0);
    let more = table.len() > MOST_ROWS;
    table.truncate(MOST_ROWS);
    Rows { columns, rows: table, more, said: None }
}

/// What a client complained of, in a line or two, without what it says every time.
fn complaint(text: &str) -> String {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.to_lowercase().contains("using a password on the command line"))
        .collect();
    let said = lines.iter().take(4).copied().collect::<Vec<_>>().join(" ");
    let said = said.trim_start_matches("psql:").trim().trim_start_matches("ERROR:").trim().to_string();
    let lower = said.to_lowercase();
    if lower.contains("read-only transaction")
        || lower.contains("readonly database")
        || lower.contains("read only transaction")
    {
        return format!("{said}. {READ_ONLY_HINT}");
    }
    if said.is_empty() { "The database refused the query and did not say why".into() } else { said }
}

const PG_SCRIPT: &str = r#"exec psql -X -q -U "${POSTGRES_USER:-postgres}" -d "${POSTGRES_DB:-${POSTGRES_USER:-postgres}}" -v ON_ERROR_STOP=1 -P "null=∅" --csv -c "$1""#;
const MYSQL_SCRIPT: &str = r#"client=$(command -v mariadb || command -v mysql); exec "$client" -uroot -p"${MYSQL_ROOT_PASSWORD:-$MARIADB_ROOT_PASSWORD}" --batch ${AE_INIT:+--init-command="$AE_INIT"} -e "$1" "${MYSQL_DATABASE:-$MARIADB_DATABASE}""#;
const MYSQL_READ_ONLY: &str = "SET SESSION TRANSACTION READ ONLY";

fn pg_options(write: bool) -> String {
    let patience = format!("-c statement_timeout={}", (QUERY_PATIENCE.as_secs() - 10) * 1000);
    if write { patience } else { format!("{patience} -c default_transaction_read_only=on") }
}

fn sqlite_query(file: &Path, sql: &str, write: bool) -> Result<Rows> {
    use rusqlite::types::ValueRef;
    use rusqlite::{Connection, OpenFlags};
    let flags = if write { OpenFlags::SQLITE_OPEN_READ_WRITE } else { OpenFlags::SQLITE_OPEN_READ_ONLY };
    let conn = Connection::open_with_flags(file, flags | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .map_err(|error| anyhow!("{} could not be opened: {error}", file.display()))?;
    conn.busy_timeout(Duration::from_secs(5))?;
    let said = |error: rusqlite::Error| match error {
        rusqlite::Error::MultipleStatement => anyhow!("One statement at a time, please"),
        other => anyhow!("{}", complaint(&other.to_string())),
    };
    let mut statement = conn.prepare(sql).map_err(said)?;
    let columns: Vec<String> = statement.column_names().map(str::to_string).collect();
    if columns.is_empty() {
        let changed = statement.execute([]).map_err(said)?;
        return Ok(Rows {
            said: Some(format!("Done. {changed} {} changed.", if changed == 1 { "row" } else { "rows" })),
            ..Rows::default()
        });
    }
    let mut rows = Vec::new();
    let mut found = statement.query([]).map_err(said)?;
    let mut more = false;
    while let Some(row) = found.next().map_err(said)? {
        if rows.len() == MOST_ROWS {
            more = true;
            break;
        }
        let mut cells = Vec::with_capacity(columns.len());
        for at in 0..columns.len() {
            cells.push(match row.get_ref(at)? {
                ValueRef::Null => NOTHING.to_string(),
                ValueRef::Integer(number) => number.to_string(),
                ValueRef::Real(number) => number.to_string(),
                ValueRef::Text(text) => String::from_utf8_lossy(text).into_owned(),
                ValueRef::Blob(bytes) => format!("({} bytes)", bytes.len()),
            });
        }
        rows.push(cells);
    }
    Ok(Rows { columns, rows, more, said: None })
}

/// Asks a database one thing.
fn query(runner: &Runner, place: &Place, db: &Db, sql: &str, write: bool) -> Result<Rows> {
    if !db.ready {
        bail!("{}", db.note.clone().unwrap_or_else(|| format!("{} cannot be asked right now", db.name)));
    }
    let sql = sql.trim();
    if sql.is_empty() {
        bail!("Write a query first");
    }
    let (mut command, csv) = match (&db.way, db.engine) {
        (Way::File(file), _) => return sqlite_query(file, sql.trim_end_matches(';'), write),
        (Way::Compose { service }, engine) => {
            let mut command = Command::new(&runner.docker);
            command.args(runner.compose_args(&place.compose_project(), &place.task_id)).args(["exec", "-T"]);
            let script = match engine {
                Engine::Postgres => {
                    command.args(["-e", &format!("PGOPTIONS={}", pg_options(write))]);
                    PG_SCRIPT
                }
                _ => {
                    if !write {
                        command.args(["-e", &format!("AE_INIT={MYSQL_READ_ONLY}")]);
                    }
                    MYSQL_SCRIPT
                }
            };
            command.args([service.as_str(), "sh", "-c", script, "sh", sql]).current_dir(&place.root);
            (command, engine == Engine::Postgres)
        }
        (Way::Client { program, address }, Engine::Postgres) => {
            let mut command = Command::new(program);
            command.args(["-X", "-q", "-v", "ON_ERROR_STOP=1", "-P", &format!("null={NOTHING}"), "--csv", "-c", sql]);
            command.env("PGOPTIONS", pg_options(write)).env("PGCONNECT_TIMEOUT", "5");
            let pairs = [
                ("PGHOST", &address.host),
                ("PGPORT", &address.port),
                ("PGUSER", &address.user),
                ("PGPASSWORD", &address.password),
                ("PGDATABASE", &address.name),
            ];
            command.envs(pairs.into_iter().filter(|(_, value)| !value.is_empty()));
            (command, true)
        }
        (Way::Client { program, address }, _) => {
            let mut command = Command::new(program);
            command.args(["--batch", "--connect-timeout=5"]);
            for (flag, value) in [("-h", &address.host), ("-P", &address.port), ("-u", &address.user)] {
                if !value.is_empty() {
                    command.args([flag, value]);
                }
            }
            if !write {
                command.arg(format!("--init-command={MYSQL_READ_ONLY}"));
            }
            command.args(["-e", sql, &address.name]).env("MYSQL_PWD", &address.password);
            (command, false)
        }
    };
    let ran = run(&mut command, QUERY_PATIENCE, MOST_OUTPUT)
        .map_err(|error| anyhow!("The query could not be run: {error}"))?;
    if !ran.ok {
        bail!("{}", runner::env::scrub(&complaint(&ran.err), &place.secrets));
    }
    let text = String::from_utf8_lossy(&ran.out);
    Ok(rows_of(if csv { parse_csv(&text) } else { parse_tsv(&text) }))
}

// ----- the tables -----

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Column {
    pub name: String,
    pub kind: String,
    /// Whether it may hold nothing.
    pub nullable: bool,
    pub default: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Index {
    pub name: String,
    /// The columns it is on, or how the database itself writes it.
    pub on: String,
    pub unique: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Table {
    pub name: String,
    pub columns: Vec<Column>,
    pub indexes: Vec<Index>,
}

const PG_COLUMNS: &str = "select table_name, column_name, case when character_maximum_length is not null then data_type || '(' || character_maximum_length || ')' else data_type end, is_nullable, coalesce(column_default, '') from information_schema.columns where table_schema = any(current_schemas(false)) order by table_name, ordinal_position";
const PG_INDEXES: &str = "select tablename, indexname, indexdef, '' from pg_indexes where schemaname = any(current_schemas(false)) order by tablename, indexname";
const MYSQL_COLUMNS: &str = "select table_name, column_name, column_type, is_nullable, coalesce(column_default, '') from information_schema.columns where table_schema = database() order by table_name, ordinal_position";
const MYSQL_INDEXES: &str = "select table_name, index_name, group_concat(column_name order by seq_in_index), non_unique from information_schema.statistics where table_schema = database() group by table_name, index_name, non_unique order by table_name, index_name";
const SQLITE_COLUMNS: &str = "select m.name, p.name, p.type, case when p.\"notnull\" = 0 then 'YES' else 'NO' end, coalesce(p.dflt_value, '') from sqlite_master m join pragma_table_info(m.name) p where m.type = 'table' and m.name not like 'sqlite_%' order by m.name, p.cid";
const SQLITE_INDEXES: &str = "select m.name, l.name, (select group_concat(name) from pragma_index_info(l.name)), case when l.\"unique\" = 1 then '0' else '1' end from sqlite_master m join pragma_index_list(m.name) l where m.type = 'table' and m.name not like 'sqlite_%' order by m.name, l.name";

/// Every table of a database with its columns and indexes.
fn schema(runner: &Runner, place: &Place, db: &Db) -> Result<Vec<Table>> {
    let (columns, indexes) = match db.engine {
        Engine::Postgres => (PG_COLUMNS, PG_INDEXES),
        Engine::Mysql => (MYSQL_COLUMNS, MYSQL_INDEXES),
        Engine::Sqlite => (SQLITE_COLUMNS, SQLITE_INDEXES),
        Engine::Other => bail!("{}", db.note.clone().unwrap_or_default()),
    };
    let mut tables: BTreeMap<String, Table> = BTreeMap::new();
    for row in query(runner, place, db, columns, false)?.rows {
        let [table, name, kind, nullable, default] = &row[..] else { continue };
        let table = tables.entry(table.clone()).or_insert_with(|| Table {
            name: table.clone(),
            columns: vec![],
            indexes: vec![],
        });
        table.columns.push(Column {
            name: name.clone(),
            kind: kind.to_lowercase(),
            nullable: nullable.eq_ignore_ascii_case("yes"),
            default: if default == NOTHING { String::new() } else { default.clone() },
        });
    }
    for row in query(runner, place, db, indexes, false)?.rows {
        let [table, name, on, not_unique] = &row[..] else { continue };
        let Some(table) = tables.get_mut(table) else { continue };
        // Postgres writes the whole index out; what is worth showing of it is what comes after "USING".
        let unique = if db.engine == Engine::Postgres { on.starts_with("CREATE UNIQUE") } else { not_unique == "0" };
        let on = on.split_once(" USING ").map_or(on.as_str(), |(_, rest)| rest).to_string();
        table.indexes.push(Index { name: name.clone(), on, unique });
    }
    Ok(tables.into_values().collect())
}

/// What differs between the tables at two moments, a sentence each.
pub fn schema_changes(before: &[Table], after: &[Table]) -> Vec<String> {
    let mut said = Vec::new();
    let find = |tables: &'_ [Table], name: &str| tables.iter().position(|table| table.name == name);
    for table in after {
        let Some(was) = find(before, &table.name).map(|at| &before[at]) else {
            let columns = table.columns.len();
            said.push(format!(
                "Table `{}` is new, with {columns} {}",
                table.name,
                if columns == 1 { "column" } else { "columns" }
            ));
            continue;
        };
        for column in &table.columns {
            let place = format!("`{}.{}`", table.name, column.name);
            match was.columns.iter().find(|one| one.name == column.name) {
                None => said.push(format!(
                    "{place} is new: {}{}{}",
                    column.kind,
                    if column.nullable { "" } else { ", never empty" },
                    if column.default.is_empty() { String::new() } else { format!(", {} by default", column.default) }
                )),
                Some(old) => {
                    if old.kind != column.kind {
                        said.push(format!("{place} is now {} (it was {})", column.kind, old.kind));
                    }
                    if old.nullable != column.nullable {
                        said.push(format!(
                            "{place} {}",
                            if column.nullable { "may be empty now" } else { "may no longer be empty" }
                        ));
                    }
                    if old.default != column.default {
                        let now = if column.default.is_empty() { "nothing" } else { &column.default };
                        said.push(format!("{place} is now {now} by default"));
                    }
                }
            }
        }
        for column in was.columns.iter().filter(|old| !table.columns.iter().any(|one| one.name == old.name)) {
            said.push(format!("`{}.{}` is gone", table.name, column.name));
        }
        for index in table.indexes.iter().filter(|index| !was.indexes.iter().any(|old| old.name == index.name)) {
            said.push(format!("Index `{}` on `{}` is new", index.name, table.name));
        }
        for index in was.indexes.iter().filter(|old| !table.indexes.iter().any(|index| index.name == old.name)) {
            said.push(format!("Index `{}` on `{}` is gone", index.name, table.name));
        }
    }
    for table in before.iter().filter(|table| find(after, &table.name).is_none()) {
        said.push(format!("Table `{}` is gone", table.name));
    }
    said
}

// ----- the migrations of a task -----

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Migration {
    path: String,
    /// "added", "modified" or "deleted", against where the task's branch started.
    change: &'static str,
    steps: Vec<Step>,
    /// Whether it says how to take itself back. `None` when its text does not tell.
    way_back: Option<bool>,
}

fn up_key(project_id: &str) -> String {
    format!("project.{project_id}.db.up")
}

fn down_key(project_id: &str) -> String {
    format!("project.{project_id}.db.down")
}

/// The migrations of a task and the commands that run them.
struct Plan {
    tool: Option<Tool>,
    up: String,
    down: String,
    migrations: Vec<Migration>,
    note: Option<String>,
}

fn plan(manager: &Manager, place: &Place, project_id: &str) -> Result<Plan> {
    let root = place.root.as_path();
    let task = manager.store().task(&place.task_id)?;
    let kept = |key: String| manager.store().setting(&key).ok().flatten().unwrap_or_default();
    let (set_up, set_down) = (kept(up_key(project_id)), kept(down_key(project_id)));
    let tool = migrations::detect(root);
    let mut found: Vec<Migration> = Vec::new();
    let mut note = None;
    let start = task
        .branch
        .as_ref()
        .and_then(|_| gitops::resolve_base(root, task.base_branch.as_deref()))
        .and_then(|base| gitops::fork_point(root, &base));
    match (&tool, &start) {
        (None, _) => {}
        (Some(_), None) => {
            note = Some("This task has no base branch, so its own migrations cannot be told from the rest.".into());
        }
        (Some(tool), Some(start)) => {
            for (path, change) in gitops::changed_since(root, start)?.into_iter().filter(|(path, _)| tool.owns(path)) {
                let text = std::fs::read_to_string(root.join(&path)).unwrap_or_default();
                found.push(Migration {
                    steps: if change == "deleted" { Vec::new() } else { migrations::read(&path, &text) },
                    way_back: if change == "deleted" { None } else { migrations::has_way_back(&path, &text) },
                    path,
                    change,
                });
            }
        }
    }
    let added = found.iter().filter(|one| one.change == "added").count();
    let mut down = tool.as_ref().map(|tool| tool.down.replace("{n}", &added.max(1).to_string())).unwrap_or_default();
    // Django takes an app back to a migration by name: the last one each app had where the branch started.
    if let (Some(tool), Some(start)) = (tool.as_ref().filter(|tool| tool.kind == "django"), &start) {
        let mut apps: BTreeMap<String, String> = BTreeMap::new();
        for one in found.iter().filter(|one| one.change == "added") {
            let Some((folder, _)) = one.path.rsplit_once('/') else { continue };
            let Some(app) = folder.strip_suffix("/migrations").and_then(|app| app.rsplit('/').next()) else { continue };
            let last = gitops::names_at(root, start, folder)
                .iter()
                .filter_map(|name| migrations::django_number(name).map(str::to_string))
                .max();
            apps.insert(app.to_string(), last.unwrap_or_else(|| "zero".into()));
        }
        let manage = tool.up.trim_end_matches("migrate").trim_end();
        down = apps.iter().map(|(app, to)| format!("{manage} migrate {app} {to}")).collect::<Vec<_>>().join(" && ");
    }
    Ok(Plan {
        up: if set_up.is_empty() { tool.as_ref().map(|tool| tool.up.clone()).unwrap_or_default() } else { set_up },
        down: if set_down.is_empty() { down } else { set_down },
        tool,
        migrations: found,
        note,
    })
}

// ----- the trial -----

/// One run of a migration command.
#[derive(Debug, Serialize)]
struct TrialStep {
    /// "up", "down" or "up again".
    name: &'static str,
    command: String,
    ok: bool,
    /// The end of what it printed.
    output: String,
    took: u64,
}

/// The tasks whose migrations are being tried right now: one trial at a time on one database.
fn trying() -> &'static Mutex<HashSet<String>> {
    static TRYING: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    TRYING.get_or_init(Mutex::default)
}

struct Trying(String);

impl Drop for Trying {
    fn drop(&mut self) {
        trying().lock().unwrap().remove(&self.0);
    }
}

fn migrate(place: &Place, folder: &Path, name: &'static str, line: &str) -> TrialStep {
    let started = Instant::now();
    let mut command = Command::new("sh");
    command
        .args(["-c", &format!("( {line} ) 2>&1")])
        .current_dir(folder)
        .envs(place.env.iter().map(|(name, value)| (name, value)));
    let (ok, output) = match run(&mut command, MIGRATE_PATIENCE, MOST_OUTPUT) {
        Ok(ran) => (ran.ok, String::from_utf8_lossy(&ran.out).into_owned()),
        Err(error) => (false, format!("{error:#}")),
    };
    let output = crate::terminals::strip_codes(&runner::env::scrub(output.trim(), &place.secrets));
    let cut = output.len().saturating_sub(MOST_SAID);
    let from = (cut..=output.len()).find(|at| output.is_char_boundary(*at)).unwrap_or(output.len());
    TrialStep {
        name,
        command: line.to_string(),
        ok,
        output: output[from..].to_string(),
        took: started.elapsed().as_millis() as u64,
    }
}

/// Runs the task's migrations on its own database, takes them back and runs them again.
fn try_migrations(
    manager: &Manager,
    runner: &Runner,
    place: &Place,
    project_id: &str,
    database: Option<&str>,
) -> Result<Value> {
    let plan = plan(manager, place, project_id)?;
    if plan.up.is_empty() {
        bail!("No command that runs the migrations is known for this project. Give one under Set up.");
    }
    let found = databases_of(runner, place);
    let db = match database {
        Some(id) => Some(found.iter().find(|db| db.id == id).ok_or_else(|| anyhow!("This task has no database {id}"))?),
        None => found.iter().find(|db| db.ready),
    };
    if let Some(shared) = db.filter(|db| db.shared) {
        bail!(
            "{} may hold the rows of other tasks too, and a trial takes migrations back, which loses what they added. \
             It is not tried from here.",
            shared.name
        );
    }
    if !trying().lock().unwrap().insert(place.task_id.clone()) {
        bail!("The migrations of this task are being tried already");
    }
    let _guard = Trying(place.task_id.clone());
    let folder = place.root.join(plan.tool.as_ref().map(|tool| tool.folder.as_str()).unwrap_or_default());
    // The tables as they stand at a moment; `None` when there is no database to look at.
    let look = || {
        db.filter(|db| db.ready || matches!(db.way, Way::File(_))).and_then(|db| {
            // A file of SQLite that the migrations have not made yet is a database with no tables.
            if let Way::File(file) = &db.way
                && !file.is_file()
            {
                return Some(Vec::new());
            }
            let ready = Db { ready: true, ..db.clone() };
            schema(runner, place, &ready).ok()
        })
    };

    let mut steps = Vec::new();
    let at_start = look();
    steps.push(migrate(place, &folder, "up", &plan.up));
    let mut result =
        json!({ "database": db.map(|db| db.name.clone()), "does": null, "back": null, "left": [], "again": null });
    if !steps[0].ok {
        result["steps"] = json!(steps);
        return Ok(result);
    }
    let at_head = look();
    let mut base = at_start.clone();
    if !plan.down.is_empty() {
        steps.push(migrate(place, &folder, "down", &plan.down));
        if steps[1].ok {
            let taken_back = look();
            // Whether taking them back undid them can be told only when they were not applied before the trial.
            if let (Some(start), Some(head), Some(back)) = (&at_start, &at_head, &taken_back) {
                let left = schema_changes(start, back);
                result["back"] = json!(if start == head {
                    "unknown"
                } else if left.is_empty() {
                    "same"
                } else {
                    "differs"
                });
                result["left"] = json!(if start == head { Vec::new() } else { left });
            }
            base = taken_back;
            steps.push(migrate(place, &folder, "up again", &plan.up));
            if steps[2].ok
                && let (Some(head), Some(again)) = (&at_head, look())
            {
                result["again"] = json!(*head == again);
            }
        }
    }
    if let (Some(base), Some(head)) = (&base, &at_head) {
        result["does"] = json!(schema_changes(base, head));
    }
    result["steps"] = json!(steps);
    Ok(result)
}

// ----- the commands -----

fn text<'a>(params: &'a Value, name: &str) -> Result<&'a str> {
    params.get(name).and_then(Value::as_str).ok_or_else(|| anyhow!("Missing parameter: {name}"))
}

fn handle(manager: &Manager, runner: &Runner, method: &str, params: &Value) -> Result<Value> {
    let task_id = text(params, "taskId")?;
    let (place, project_id) = runner::place_of(manager, task_id)?;
    let one = |id: &str| -> Result<Db> {
        databases_of(runner, &place)
            .into_iter()
            .find(|db| db.id == id)
            .ok_or_else(|| anyhow!("This task has no database {id}"))
    };
    match method {
        "db.get" => {
            let plan = plan(manager, &place, &project_id)?;
            let kept = |key: String| manager.store().setting(&key).ok().flatten().unwrap_or_default();
            Ok(json!({
                "tool": plan.tool,
                "up": plan.up,
                "down": plan.down,
                "migrations": plan.migrations,
                "note": plan.note,
                "databases": databases_of(runner, &place).iter().map(Db::shown).collect::<Vec<_>>(),
                "settings": { "up": kept(up_key(&project_id)), "down": kept(down_key(&project_id)) }
            }))
        }
        "db.set" => {
            for (name, key) in [("up", up_key(&project_id)), ("down", down_key(&project_id))] {
                if let Some(line) = params.get(name).and_then(Value::as_str).map(str::trim) {
                    if line.len() > 2000 || line.contains('\n') {
                        bail!("A command is one line");
                    }
                    manager.store().set_setting(&key, (!line.is_empty()).then_some(line))?;
                }
            }
            Ok(json!({}))
        }
        "db.schema" => Ok(json!({ "tables": schema(runner, &place, &one(text(params, "database")?)?)? })),
        "db.query" => {
            let write = params.get("write").and_then(Value::as_bool).unwrap_or(false);
            let started = Instant::now();
            let rows = query(runner, &place, &one(text(params, "database")?)?, text(params, "sql")?, write)?;
            let mut answer = json!(rows);
            answer["took"] = json!(started.elapsed().as_millis() as u64);
            Ok(answer)
        }
        "db.try" => {
            try_migrations(manager, runner, &place, &project_id, params.get("database").and_then(Value::as_str))
        }
        other => bail!("Unknown method: {other}"),
    }
}

/// The commands of the Database view.
///
/// - `db.get { taskId }` gives the migration tool, the task's migrations read step by step, and its databases.
/// - `db.set { taskId, up?, down? }` keeps the project's own commands for running the migrations.
/// - `db.schema { taskId, database }` gives the tables.
/// - `db.query { taskId, database, sql, write? }` gives rows.
/// - `db.try { taskId, database? }` runs the migrations, takes them back and runs them again.
///
/// They wait for programs, so they run off the event loop.
pub async fn command(state: &AppState, method: &str, params: &Value) -> Result<Value> {
    let (manager, runner) = (state.manager.clone(), state.runner.clone());
    let (method, params) = (method.to_string(), params.clone());
    tokio::task::spawn_blocking(move || handle(&manager, &runner, &method, &params)).await?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_is_read_the_ways_projects_write_it() {
        let (engine, address, _) =
            parse_url("postgresql+asyncpg://shop:p%40ss@db:5432/shop_dev?sslmode=disable").unwrap();
        assert_eq!(engine, Engine::Postgres);
        assert_eq!(
            address,
            Address {
                user: "shop".into(),
                password: "p@ss".into(),
                host: "db".into(),
                port: "5432".into(),
                name: "shop_dev".into()
            }
        );
        let (engine, address, _) = parse_url("mysql://root@localhost/shop").unwrap();
        assert_eq!(
            (engine, address.user.as_str(), address.host.as_str(), address.port.as_str()),
            (Engine::Mysql, "root", "localhost", "")
        );
        assert_eq!(parse_url("sqlite:///./app.db").unwrap().2, "./app.db");
        assert_eq!(parse_url("sqlite:////var/data/app.db").unwrap().2, "/var/data/app.db");
        assert_eq!(parse_url("file:./dev.db?mode=rwc").unwrap().2, "./dev.db");
        assert!(parse_url("sqlite://:memory:").is_none());
        assert!(parse_url("redis://localhost:6379/0").is_none());
    }

    #[test]
    fn what_the_clients_print_is_read_cell_by_cell() {
        let csv = "id,note,at\n1,\"a, \"\"quoted\"\"\nline\",∅\n2,,2026-11-14\n";
        assert_eq!(
            parse_csv(csv),
            [vec!["id", "note", "at"], vec!["1", "a, \"quoted\"\nline", "∅"], vec!["2", "", "2026-11-14"]]
        );
        assert_eq!(
            parse_tsv("id\tnote\n1\ta\\tb\\nc\n2\tNULL\n"),
            [vec!["id", "note"], vec!["1", "a\tb\nc"], vec!["2", "∅"]]
        );
        let rows = rows_of(parse_csv("n\n1\n2\n"));
        assert_eq!((rows.columns, rows.rows.len(), rows.more), (vec!["n".to_string()], 2, false));
        assert_eq!(rows_of(Vec::new()).said.as_deref(), Some("Done. It gave no rows."));
    }

    #[test]
    fn a_refusal_to_write_says_how_to_allow_it() {
        let said = complaint("psql:<stdin>:1: ERROR:  cannot execute UPDATE in a read-only transaction\n");
        assert!(
            said.starts_with("<stdin>:1: ERROR:  cannot execute UPDATE") || said.starts_with("cannot execute UPDATE"),
            "{said}"
        );
        assert!(said.ends_with(READ_ONLY_HINT));
        assert_eq!(
            complaint(
                "mysql: [Warning] Using a password on the command line interface can be insecure.\nERROR 1146 (42S02): Table 'shop.x' doesn't exist"
            ),
            "ERROR 1146 (42S02): Table 'shop.x' doesn't exist"
        );
    }

    fn table(name: &str, columns: &[(&str, &str, bool)], indexes: &[&str]) -> Table {
        Table {
            name: name.into(),
            columns: columns
                .iter()
                .map(|(name, kind, nullable)| Column {
                    name: (*name).into(),
                    kind: (*kind).into(),
                    nullable: *nullable,
                    default: String::new(),
                })
                .collect(),
            indexes: indexes
                .iter()
                .map(|name| Index { name: (*name).into(), on: String::new(), unique: false })
                .collect(),
        }
    }

    #[test]
    fn the_tables_at_two_moments_are_compared_in_sentences() {
        let before = [
            table(
                "orders",
                &[("id", "integer", false), ("total", "numeric", true), ("state", "text", true)],
                &["ix_state"],
            ),
            table("carts", &[("id", "integer", false)], &[]),
        ];
        let after = [
            table("gifts", &[("id", "integer", false)], &[]),
            table(
                "orders",
                &[("id", "bigint", false), ("state", "text", false), ("count", "integer", false)],
                &["ix_count"],
            ),
        ];
        assert_eq!(
            schema_changes(&before, &after),
            [
                "Table `gifts` is new, with 1 column",
                "`orders.id` is now bigint (it was integer)",
                "`orders.state` may no longer be empty",
                "`orders.count` is new: integer, never empty",
                "`orders.total` is gone",
                "Index `ix_count` on `orders` is new",
                "Index `ix_state` on `orders` is gone",
                "Table `carts` is gone",
            ]
        );
        assert!(schema_changes(&before, &before).is_empty());
    }
}
