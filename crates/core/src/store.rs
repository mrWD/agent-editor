//! What the core remembers between runs: projects, tasks and every event of every task.
//!
//! One SQLite file. All access goes through one connection behind a mutex;
//! the queries are small, so nothing waits long.

use anyhow::{Context, Result, anyhow, bail};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use serde_json::Value;
use std::path::Path;
use std::sync::Mutex;

/// A folder with code that tasks work on.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: String,
    /// The group this project is shown under, if any.
    pub folder: Option<String>,
    pub created_at: u64,
}

/// A task as stored. Whether its agent is running is not stored: that is known only while the core runs.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRow {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub agent_id: String,
    /// The folder the agent works in: a working copy of the project, or the project itself.
    pub cwd: String,
    /// The branch made for this task, when the project is a git repository.
    pub branch: Option<String>,
    /// The branch the task's branch was started from, such as `main`.
    pub base_branch: Option<String>,
    /// The issue this task was started from, if any.
    pub issue: Option<TaskIssue>,
    pub created_at: u64,
    /// The last event the user has seen, or -1.
    pub seen_seq: i64,
}

/// What the core knows about one changed file of a task. See `marks`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarksRow {
    /// The text the user last accepted. `None` when there was no file.
    pub base: Option<String>,
    /// The text the core last saw on disk. `None` when the file is gone.
    pub seen: Option<String>,
    /// One letter per line of `seen`.
    pub authors: String,
}

/// The issue a task works on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskIssue {
    /// How people call it: `#12` or `APP-214`.
    pub key: String,
    /// What the tracker's commands need: the number, or Linear's id.
    pub id: String,
    pub url: String,
    pub title: String,
    /// "github", "gitlab" or "linear".
    pub source: String,
}

pub struct Store {
    conn: Mutex<Connection>,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS projects (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL,
    path       TEXT NOT NULL UNIQUE,
    folder     TEXT,
    created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS tasks (
    id          TEXT PRIMARY KEY,
    project_id  TEXT NOT NULL REFERENCES projects(id),
    title       TEXT NOT NULL,
    agent_id    TEXT NOT NULL,
    cwd         TEXT NOT NULL,
    branch      TEXT,
    created_at  INTEGER NOT NULL,
    archived_at INTEGER,
    seen_seq    INTEGER NOT NULL DEFAULT -1
);
CREATE TABLE IF NOT EXISTS events (
    task_id TEXT NOT NULL REFERENCES tasks(id),
    seq     INTEGER NOT NULL,
    at      INTEGER NOT NULL,
    body    TEXT NOT NULL,
    PRIMARY KEY (task_id, seq)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS file_marks (
    task_id TEXT NOT NULL REFERENCES tasks(id),
    path    TEXT NOT NULL,
    base    TEXT,
    seen    TEXT,
    authors TEXT NOT NULL,
    PRIMARY KEY (task_id, path)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
) WITHOUT ROWID;
";

/// Brings a database made by an older version up to date.
fn upgrade(conn: &Connection) -> Result<()> {
    let has_column = |table: &str, column: &str| -> Result<bool> {
        let mut statement = conn.prepare(&format!("PRAGMA table_info({table})"))?;
        let names = statement.query_map([], |row| row.get::<_, String>(1))?;
        Ok(names.collect::<rusqlite::Result<Vec<_>>>()?.iter().any(|name| name == column))
    };
    if !has_column("tasks", "base_branch")? {
        conn.execute_batch("ALTER TABLE tasks ADD COLUMN base_branch TEXT")?;
    }
    if !has_column("tasks", "issue_key")? {
        conn.execute_batch(
            "ALTER TABLE tasks ADD COLUMN issue_key TEXT;
             ALTER TABLE tasks ADD COLUMN issue_id TEXT;
             ALTER TABLE tasks ADD COLUMN issue_url TEXT;
             ALTER TABLE tasks ADD COLUMN issue_title TEXT;
             ALTER TABLE tasks ADD COLUMN issue_source TEXT;",
        )?;
    }
    Ok(())
}

impl Store {
    /// Opens the database file, creating it and its tables when missing.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("Cannot create {}", dir.display()))?;
        }
        let conn = Connection::open(path).with_context(|| format!("Cannot open {}", path.display()))?;
        // Write-ahead logging keeps the many small event writes cheap and safe.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(SCHEMA)?;
        upgrade(&conn)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    /// A database that lives in memory. Used by tests.
    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(SCHEMA)?;
        upgrade(&conn)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    // ----- projects -----

    pub fn projects(&self) -> Result<Vec<Project>> {
        let conn = self.conn.lock().unwrap();
        let mut statement =
            conn.prepare("SELECT id, name, path, folder, created_at FROM projects ORDER BY name COLLATE NOCASE")?;
        let rows = statement.query_map([], project_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn project(&self, id: &str) -> Result<Project> {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT id, name, path, folder, created_at FROM projects WHERE id = ?1", [id], project_from_row)
            .optional()?
            .ok_or_else(|| anyhow!("No such project"))
    }

    pub fn project_by_path(&self, path: &str) -> Result<Option<Project>> {
        let conn = self.conn.lock().unwrap();
        Ok(conn
            .query_row(
                "SELECT id, name, path, folder, created_at FROM projects WHERE path = ?1",
                [path],
                project_from_row,
            )
            .optional()?)
    }

    pub fn insert_project(&self, project: &Project) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO projects (id, name, path, folder, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![project.id, project.name, project.path, project.folder, project.created_at as i64],
        )?;
        Ok(())
    }

    /// Removes a project. Refused while it still has tasks that are not archived.
    pub fn remove_project(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let open: i64 =
            conn.query_row("SELECT COUNT(*) FROM tasks WHERE project_id = ?1 AND archived_at IS NULL", [id], |row| {
                row.get(0)
            })?;
        if open > 0 {
            bail!("This project still has {open} open tasks. Archive them first.");
        }
        // Everything its archived tasks left behind goes with it, in one step or not at all.
        conn.execute_batch("BEGIN")?;
        let removed = (|| -> rusqlite::Result<()> {
            let of_project = "(SELECT id FROM tasks WHERE project_id = ?1)";
            conn.execute(&format!("DELETE FROM events WHERE task_id IN {of_project}"), [id])?;
            conn.execute(&format!("DELETE FROM file_marks WHERE task_id IN {of_project}"), [id])?;
            conn.execute("DELETE FROM tasks WHERE project_id = ?1", [id])?;
            conn.execute("DELETE FROM settings WHERE key LIKE ?1", [format!("project.{id}.%")])?;
            conn.execute("DELETE FROM projects WHERE id = ?1", [id])?;
            Ok(())
        })();
        match removed {
            Ok(()) => conn.execute_batch("COMMIT")?,
            Err(error) => {
                let _ = conn.execute_batch("ROLLBACK");
                return Err(error.into());
            }
        }
        Ok(())
    }

    // ----- tasks -----

    /// Every task that is not archived, oldest first.
    pub fn tasks(&self) -> Result<Vec<TaskRow>> {
        let conn = self.conn.lock().unwrap();
        let mut statement = conn.prepare(
            "SELECT id, project_id, title, agent_id, cwd, branch, created_at, seen_seq, base_branch, issue_key, issue_id, issue_url, issue_title, issue_source
             FROM tasks WHERE archived_at IS NULL ORDER BY created_at",
        )?;
        let rows = statement.query_map([], task_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn task(&self, id: &str) -> Result<TaskRow> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, project_id, title, agent_id, cwd, branch, created_at, seen_seq, base_branch, issue_key, issue_id, issue_url, issue_title, issue_source
             FROM tasks WHERE id = ?1 AND archived_at IS NULL",
            [id],
            task_from_row,
        )
        .optional()?
        .ok_or_else(|| anyhow!("No such task"))
    }

    pub fn insert_task(&self, task: &TaskRow) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO tasks (id, project_id, title, agent_id, cwd, branch, created_at, seen_seq, base_branch,
                                issue_key, issue_id, issue_url, issue_title, issue_source)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                task.id,
                task.project_id,
                task.title,
                task.agent_id,
                task.cwd,
                task.branch,
                task.created_at as i64,
                task.seen_seq,
                task.base_branch,
                task.issue.as_ref().map(|issue| &issue.key),
                task.issue.as_ref().map(|issue| &issue.id),
                task.issue.as_ref().map(|issue| &issue.url),
                task.issue.as_ref().map(|issue| &issue.title),
                task.issue.as_ref().map(|issue| &issue.source),
            ],
        )?;
        Ok(())
    }

    pub fn archive_task(&self, id: &str, at: u64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE tasks SET archived_at = ?2 WHERE id = ?1 AND archived_at IS NULL",
            params![id, at as i64],
        )?;
        if changed == 0 {
            bail!("No such task");
        }
        Ok(())
    }

    /// Remembers how far the user has read. It only moves forward.
    pub fn set_seen(&self, id: &str, seq: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("UPDATE tasks SET seen_seq = MAX(seen_seq, ?2) WHERE id = ?1", params![id, seq])?;
        Ok(())
    }

    // ----- events -----

    pub fn insert_event(&self, task_id: &str, seq: u64, at: u64, body: &Value) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO events (task_id, seq, at, body) VALUES (?1, ?2, ?3, ?4)",
            params![task_id, seq as i64, at as i64, body.to_string()],
        )?;
        Ok(())
    }

    /// Every event of a task, oldest first.
    pub fn events(&self, task_id: &str) -> Result<Vec<Value>> {
        let conn = self.conn.lock().unwrap();
        let mut statement = conn.prepare("SELECT body FROM events WHERE task_id = ?1 ORDER BY seq")?;
        let rows = statement.query_map([task_id], |row| row.get::<_, String>(0))?;
        let mut events = Vec::new();
        for body in rows {
            events.push(serde_json::from_str(&body?)?);
        }
        Ok(events)
    }

    /// The number the next event of a task gets.
    pub fn next_seq(&self, task_id: &str) -> Result<u64> {
        let conn = self.conn.lock().unwrap();
        let last: Option<i64> =
            conn.query_row("SELECT MAX(seq) FROM events WHERE task_id = ?1", [task_id], |row| row.get(0))?;
        Ok(last.map(|n| n as u64 + 1).unwrap_or(0))
    }
}

impl Store {
    // ----- settings -----

    /// A value the user or the core chose to keep, such as a project's Linear team.
    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| row.get(0)).optional()?)
    }

    /// Keeps a value; `None` forgets it.
    pub fn set_setting(&self, key: &str, value: Option<&str>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        match value {
            Some(value) => conn.execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT (key) DO UPDATE SET value = ?2",
                params![key, value],
            )?,
            None => conn.execute("DELETE FROM settings WHERE key = ?1", [key])?,
        };
        Ok(())
    }
}

impl Store {
    // ----- who wrote which line -----

    pub fn marks(&self, task_id: &str, path: &str) -> Result<Option<MarksRow>> {
        let conn = self.conn.lock().unwrap();
        Ok(conn
            .query_row(
                "SELECT base, seen, authors FROM file_marks WHERE task_id = ?1 AND path = ?2",
                params![task_id, path],
                |row| Ok(MarksRow { base: row.get(0)?, seen: row.get(1)?, authors: row.get(2)? }),
            )
            .optional()?)
    }

    pub fn put_marks(&self, task_id: &str, path: &str, row: &MarksRow) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO file_marks (task_id, path, base, seen, authors) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (task_id, path) DO UPDATE SET base = ?3, seen = ?4, authors = ?5",
            params![task_id, path, row.base, row.seen, row.authors],
        )?;
        Ok(())
    }

    pub fn remove_marks(&self, task_id: &str, path: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM file_marks WHERE task_id = ?1 AND path = ?2", params![task_id, path])?;
        Ok(())
    }

    /// Every tracked file of a task: its path, its authors, whether it existed
    /// at the last review and whether it exists now.
    pub fn marked_files(&self, task_id: &str) -> Result<Vec<(String, String, bool, bool)>> {
        let conn = self.conn.lock().unwrap();
        let mut statement = conn.prepare(
            "SELECT path, authors, base IS NOT NULL, seen IS NOT NULL FROM file_marks WHERE task_id = ?1 ORDER BY path",
        )?;
        let rows = statement.query_map([task_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

fn project_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Project> {
    Ok(Project {
        id: row.get(0)?,
        name: row.get(1)?,
        path: row.get(2)?,
        folder: row.get(3)?,
        created_at: row.get::<_, i64>(4)? as u64,
    })
}

fn task_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskRow> {
    Ok(TaskRow {
        id: row.get(0)?,
        project_id: row.get(1)?,
        title: row.get(2)?,
        agent_id: row.get(3)?,
        cwd: row.get(4)?,
        branch: row.get(5)?,
        base_branch: row.get(8)?,
        issue: match row.get::<_, Option<String>>(9)? {
            Some(key) => Some(TaskIssue {
                key,
                id: row.get::<_, Option<String>>(10)?.unwrap_or_default(),
                url: row.get::<_, Option<String>>(11)?.unwrap_or_default(),
                title: row.get::<_, Option<String>>(12)?.unwrap_or_default(),
                source: row.get::<_, Option<String>>(13)?.unwrap_or_default(),
            }),
            None => None,
        },
        created_at: row.get::<_, i64>(6)? as u64,
        seen_seq: row.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn project(id: &str, path: &str) -> Project {
        Project { id: id.into(), name: id.into(), path: path.into(), folder: None, created_at: 1 }
    }

    fn task(id: &str, project_id: &str) -> TaskRow {
        TaskRow {
            id: id.into(),
            project_id: project_id.into(),
            title: "A task".into(),
            agent_id: "mock".into(),
            cwd: "/tmp".into(),
            branch: None,
            base_branch: None,
            issue: None,
            created_at: 2,
            seen_seq: -1,
        }
    }

    #[test]
    fn projects_tasks_and_events_round_trip() {
        let store = Store::in_memory().unwrap();
        store.insert_project(&project("p1", "/code/app")).unwrap();
        store.insert_task(&task("t1", "p1")).unwrap();

        assert_eq!(store.next_seq("t1").unwrap(), 0);
        store.insert_event("t1", 0, 10, &json!({ "type": "user_message", "text": "hi" })).unwrap();
        store.insert_event("t1", 1, 11, &json!({ "type": "turn_end" })).unwrap();
        assert_eq!(store.next_seq("t1").unwrap(), 2);

        let events = store.events("t1").unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["text"], "hi");
        assert_eq!(store.projects().unwrap()[0].path, "/code/app");
        assert_eq!(store.tasks().unwrap()[0].title, "A task");
    }

    #[test]
    fn a_project_path_is_stored_once() {
        let store = Store::in_memory().unwrap();
        store.insert_project(&project("p1", "/code/app")).unwrap();
        assert!(store.insert_project(&project("p2", "/code/app")).is_err());
        assert_eq!(store.project_by_path("/code/app").unwrap().unwrap().id, "p1");
        assert!(store.project_by_path("/code/other").unwrap().is_none());
    }

    #[test]
    fn archived_tasks_leave_the_lists_and_free_the_project() {
        let store = Store::in_memory().unwrap();
        store.insert_project(&project("p1", "/code/app")).unwrap();
        store.insert_task(&task("t1", "p1")).unwrap();

        let refused = store.remove_project("p1").unwrap_err().to_string();
        assert!(refused.contains("1 open tasks"), "{refused}");

        // The task leaves things behind: an event, marks of a file, a setting of its project.
        store.insert_event("t1", 0, 1, &json!({ "type": "state" })).unwrap();
        store
            .put_marks("t1", "a.txt", &MarksRow { base: None, seen: Some("x\n".into()), authors: "y".into() })
            .unwrap();
        store.set_setting("project.p1.linearTeam", Some("APP")).unwrap();
        store.set_setting("linear.key", Some("kept")).unwrap();

        store.archive_task("t1", 99).unwrap();
        assert!(store.tasks().unwrap().is_empty());
        assert!(store.task("t1").is_err());
        store.remove_project("p1").unwrap();
        assert!(store.projects().unwrap().is_empty());
        assert_eq!(store.setting("project.p1.linearTeam").unwrap(), None);
        assert_eq!(store.setting("linear.key").unwrap().as_deref(), Some("kept"), "other settings stay");
    }

    #[test]
    fn a_database_from_an_older_version_is_brought_up_to_date() {
        let dir = std::env::temp_dir().join(format!("ae-store-old-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("old.db");
        {
            // The tables as the first version made them: no base branch, no issue, no settings.
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE projects (id TEXT PRIMARY KEY, name TEXT NOT NULL, path TEXT NOT NULL UNIQUE, folder TEXT, created_at INTEGER NOT NULL);
                 CREATE TABLE tasks (id TEXT PRIMARY KEY, project_id TEXT NOT NULL REFERENCES projects(id), title TEXT NOT NULL,
                    agent_id TEXT NOT NULL, cwd TEXT NOT NULL, branch TEXT, created_at INTEGER NOT NULL, archived_at INTEGER,
                    seen_seq INTEGER NOT NULL DEFAULT -1);
                 CREATE TABLE events (task_id TEXT NOT NULL REFERENCES tasks(id), seq INTEGER NOT NULL, at INTEGER NOT NULL,
                    body TEXT NOT NULL, PRIMARY KEY (task_id, seq)) WITHOUT ROWID;
                 INSERT INTO projects VALUES ('p1', 'shop', '/code/shop', NULL, 1);
                 INSERT INTO tasks VALUES ('t1', 'p1', 'An old task', 'mock', '/code/shop', 'task/old-t1', 2, NULL, 5);",
            )
            .unwrap();
        }
        let store = Store::open(&path).unwrap();
        let old = store.task("t1").unwrap();
        assert_eq!((old.title.as_str(), old.seen_seq, old.base_branch, old.issue), ("An old task", 5, None, None));
        // What newer versions keep can be kept for it too.
        store.set_setting("linear.key", Some("k")).unwrap();
        store
            .put_marks("t1", "a.txt", &MarksRow { base: None, seen: Some("x\n".into()), authors: "a".into() })
            .unwrap();
        // Opening it a second time changes nothing and does not fail.
        drop(store);
        assert_eq!(Store::open(&path).unwrap().task("t1").unwrap().title, "An old task");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn settings_and_a_tasks_issue_are_kept() {
        let store = Store::in_memory().unwrap();
        assert_eq!(store.setting("linear.key").unwrap(), None);
        store.set_setting("linear.key", Some("one")).unwrap();
        store.set_setting("linear.key", Some("two")).unwrap();
        assert_eq!(store.setting("linear.key").unwrap().as_deref(), Some("two"));
        store.set_setting("linear.key", None).unwrap();
        assert_eq!(store.setting("linear.key").unwrap(), None);

        store.insert_project(&project("p1", "/tmp/one")).unwrap();
        let mut with_issue = task("t1", "p1");
        let issue = TaskIssue {
            key: "APP-214".into(),
            id: "uuid".into(),
            url: "https://linear.app/x/issue/APP-214".into(),
            title: "Login allows unlimited attempts".into(),
            source: "linear".into(),
        };
        with_issue.issue = Some(issue.clone());
        store.insert_task(&with_issue).unwrap();
        store.insert_task(&task("t2", "p1")).unwrap();
        assert_eq!(store.task("t1").unwrap().issue, Some(issue));
        assert_eq!(store.task("t2").unwrap().issue, None);
    }

    #[test]
    fn marks_are_kept_per_task_and_file() {
        let store = Store::in_memory().unwrap();
        store.insert_project(&project("p1", "/tmp/one")).unwrap();
        store.insert_task(&task("t1", "p1")).unwrap();
        assert_eq!(store.marks("t1", "a.txt").unwrap(), None);

        let row = MarksRow { base: None, seen: Some("x\n".into()), authors: "a".into() };
        store.put_marks("t1", "a.txt", &row).unwrap();
        assert_eq!(store.marks("t1", "a.txt").unwrap(), Some(row.clone()));

        let gone = MarksRow { base: Some("x\n".into()), seen: None, authors: String::new() };
        store.put_marks("t1", "a.txt", &gone).unwrap();
        assert_eq!(store.marked_files("t1").unwrap(), vec![("a.txt".to_string(), String::new(), true, false)]);

        store.remove_marks("t1", "a.txt").unwrap();
        assert!(store.marked_files("t1").unwrap().is_empty());
    }

    #[test]
    fn seen_only_moves_forward() {
        let store = Store::in_memory().unwrap();
        store.insert_project(&project("p1", "/code/app")).unwrap();
        store.insert_task(&task("t1", "p1")).unwrap();
        store.set_seen("t1", 7).unwrap();
        store.set_seen("t1", 3).unwrap();
        assert_eq!(store.task("t1").unwrap().seen_seq, 7);
    }

    #[test]
    fn the_file_survives_a_reopen() {
        let path = std::env::temp_dir().join(format!("ae-store-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let store = Store::open(&path).unwrap();
            store.insert_project(&project("p1", "/code/app")).unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(store.projects().unwrap().len(), 1);
        drop(store);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }
}
