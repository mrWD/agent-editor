//! The database of a task: its migrations read and tried, its tables and its rows.
//!
//! One test works on a real file of SQLite, with "migrations" that are two
//! shell lines of the test's own. The other reaches Postgres the way the core
//! does, through `docker compose exec`, where `docker` is a stand-in that
//! answers as `psql` inside a container would.
#![cfg(unix)]

mod common;

use ae_core::hosts::Hosts;
use ae_core::server::{AppState, dispatch};
use common::*;
use rusqlite::Connection;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

async fn asked(state: &AppState, method: &str, params: Value) -> anyhow::Result<Value> {
    dispatch(state, method, &params).await
}

async fn idle_task(state: &AppState, project_id: &str) -> String {
    let mut events = state.manager.subscribe();
    let task = state.manager.create(project_id, "mock", "What is here?").await.unwrap();
    until(&mut events, &task.row.id, |e| state_is(e, "idle")).await;
    task.row.id
}

/// A file of SQLite with the shop's tables: as they are on the base, or as the task's migration leaves them.
fn shop_db(file: &Path, with_gifts: bool) {
    let _ = std::fs::remove_file(file);
    let conn = Connection::open(file).unwrap();
    conn.execute_batch(
        "create table orders (id integer primary key, sku text not null, total numeric);
         create index ix_orders_sku on orders (sku);
         insert into orders (sku, total) values ('tea-1', 4.5), ('cup-2', null);",
    )
    .unwrap();
    if with_gifts {
        conn.execute_batch(
            "create table gifts (id integer primary key, name text);
             alter table orders add column count integer not null default 1;",
        )
        .unwrap();
    }
}

#[tokio::test]
async fn the_migrations_of_a_task_are_read_tried_and_taken_back_on_its_own_database() {
    let sandbox = Sandbox::new("database-sqlite");
    let project = git_project(&sandbox);
    std::fs::create_dir_all(project.join("migrations")).unwrap();
    std::fs::write(project.join("migrations/0001_start.sql"), "create table orders (id integer primary key);\n")
        .unwrap();
    std::fs::write(project.join(".env"), "DATABASE_URL=sqlite:///app.db\n").unwrap();
    std::fs::write(project.join(".gitignore"), "app.db\n").unwrap();
    git(&project, &["add", "."]);
    git(&project, &["commit", "-q", "-m", "Keep migrations"]);
    let state = AppState::new(sandbox.manager());
    let added = state.manager.add_project(&project.to_string_lossy(), None).unwrap();
    let task_id = idle_task(&state, &added.id).await;
    let folder = state.manager.cwd(&task_id).unwrap();

    // Untouched, the task has no migrations of its own. Plain files of SQL have no command to run them.
    let before = asked(&state, "db.get", json!({ "taskId": task_id })).await.unwrap();
    assert_eq!(before["tool"]["kind"], "sql");
    assert_eq!((before["up"].as_str(), before["down"].as_str()), (Some(""), Some("")));
    assert_eq!(before["migrations"], json!([]));
    assert_eq!(
        before["databases"],
        json!([{ "id": "file:app.db", "name": "app.db", "engine": "sqlite", "via": "file", "ready": false,
                 "note": "The file is not there yet. The migrations make it.", "shared": false }])
    );
    let nothing = asked(&state, "db.try", json!({ "taskId": task_id })).await.unwrap_err().to_string();
    assert!(nothing.starts_with("No command that runs the migrations is known"), "{nothing}");

    // The task writes a migration and changes an old one: both are read, the new one step by step.
    std::fs::write(
        folder.join("migrations/0002_gifts.sql"),
        "create table gifts (id integer primary key, name text);\nalter table orders add column count integer not null default 1;\nalter table orders drop column note;\n",
    )
    .unwrap();
    std::fs::write(
        folder.join("migrations/0001_start.sql"),
        "create table orders (id integer primary key, sku text);\n",
    )
    .unwrap();
    std::fs::write(folder.join("migrations/README.md"), "not a migration\n").unwrap();
    let read = asked(&state, "db.get", json!({ "taskId": task_id })).await.unwrap();
    let paths: Vec<_> = read["migrations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|one| (one["path"].as_str().unwrap(), one["change"].as_str().unwrap()))
        .collect();
    assert_eq!(paths, [("migrations/0001_start.sql", "modified"), ("migrations/0002_gifts.sql", "added")]);
    assert_eq!(
        read["migrations"][1]["steps"],
        json!([
            { "text": "Creates table `gifts`", "danger": null },
            { "text": "Adds `orders.count` (integer not null default 1)", "danger": null },
            { "text": "Drops `orders.note`", "danger": "What it held is gone, and code that still reads it breaks." }
        ])
    );

    // The project's own commands are kept, and the trial runs them: up, down, up again.
    let (base, head) = (sandbox.0.join("base.db"), sandbox.0.join("head.db"));
    shop_db(&base, false);
    shop_db(&head, true);
    std::fs::copy(&base, folder.join("app.db")).unwrap();
    let up = format!("echo migrating with $SHOP_KEY && cp '{}' app.db", head.display());
    let down = format!("cp '{}' app.db", base.display());
    std::fs::write(folder.join(".env"), "DATABASE_URL=sqlite:///app.db\nSHOP_KEY=sk-secret-value-123\n").unwrap();
    asked(&state, "db.set", json!({ "taskId": task_id, "up": up, "down": down })).await.unwrap();
    assert!(asked(&state, "db.set", json!({ "taskId": task_id, "up": "two\nlines" })).await.is_err());
    let tried = asked(&state, "db.try", json!({ "taskId": task_id, "database": "file:app.db" })).await.unwrap();
    let steps: Vec<_> = tried["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|step| (step["name"].as_str().unwrap(), step["ok"].as_bool().unwrap()))
        .collect();
    assert_eq!(steps, [("up", true), ("down", true), ("up again", true)]);
    assert_eq!(tried["steps"][0]["command"], up);
    let printed = tried["steps"][0]["output"].as_str().unwrap();
    assert!(
        printed.starts_with("migrating with ") && !printed.contains("sk-secret-value-123"),
        "a secret of the task is never shown: {printed}"
    );
    assert_eq!(
        tried["does"],
        json!(["Table `gifts` is new, with 2 columns", "`orders.count` is new: integer, never empty, 1 by default"])
    );
    assert_eq!(
        (tried["back"].as_str(), tried["left"].clone(), tried["again"].as_bool()),
        (Some("same"), json!([]), Some(true))
    );

    // Tried once more, the migrations are in already: what they do is still told, by taking them back.
    let again = asked(&state, "db.try", json!({ "taskId": task_id })).await.unwrap();
    assert_eq!(again["does"].as_array().unwrap().len(), 2);
    assert_eq!(again["back"], "unknown");

    // A way back that leaves something behind is said to; a command that fails stops the trial there.
    let left = sandbox.0.join("left.db");
    shop_db(&left, false);
    Connection::open(&left).unwrap().execute_batch("create table gifts (id integer primary key, name text);").unwrap();
    std::fs::copy(&base, folder.join("app.db")).unwrap();
    asked(&state, "db.set", json!({ "taskId": task_id, "down": format!("cp '{}' app.db", left.display()) }))
        .await
        .unwrap();
    let sloppy = asked(&state, "db.try", json!({ "taskId": task_id })).await.unwrap();
    assert_eq!(
        (sloppy["back"].as_str(), sloppy["left"].clone()),
        (Some("differs"), json!(["Table `gifts` is new, with 2 columns"]))
    );
    asked(
        &state,
        "db.set",
        json!({ "taskId": task_id, "up": "echo 'relation \"orders\" already exists' >&2; exit 3" }),
    )
    .await
    .unwrap();
    let failed = asked(&state, "db.try", json!({ "taskId": task_id })).await.unwrap();
    assert_eq!(failed["steps"].as_array().unwrap().len(), 1);
    assert_eq!(
        (failed["steps"][0]["ok"].as_bool(), failed["steps"][0]["output"].as_str()),
        (Some(false), Some("relation \"orders\" already exists"))
    );
    assert_eq!(failed["does"], Value::Null);
}

#[tokio::test]
async fn the_tables_and_the_rows_of_a_file_of_sqlite_are_read_and_changed_only_when_allowed() {
    let sandbox = Sandbox::new("database-rows");
    let project = sandbox.project();
    shop_db(&project.join("db.sqlite3"), true);
    let state = AppState::new(sandbox.manager());
    let added = state.manager.add_project(&project.to_string_lossy(), None).unwrap();
    let task_id = idle_task(&state, &added.id).await;

    // No address names it: a file with a usual name in the folder is found by itself.
    let found = asked(&state, "db.get", json!({ "taskId": task_id })).await.unwrap();
    assert_eq!(found["databases"][0]["id"], "file:db.sqlite3");
    assert_eq!(found["tool"], Value::Null);
    let ask = |sql: &str, write: bool| json!({ "taskId": task_id, "database": "file:db.sqlite3", "sql": sql, "write": write });

    let tables = asked(&state, "db.schema", json!({ "taskId": task_id, "database": "file:db.sqlite3" })).await.unwrap();
    assert_eq!(
        tables["tables"].as_array().unwrap().iter().map(|table| table["name"].as_str().unwrap()).collect::<Vec<_>>(),
        ["gifts", "orders"]
    );
    assert_eq!(
        tables["tables"][1]["columns"],
        json!([
            { "name": "id", "kind": "integer", "nullable": true, "default": "" },
            { "name": "sku", "kind": "text", "nullable": false, "default": "" },
            { "name": "total", "kind": "numeric", "nullable": true, "default": "" },
            { "name": "count", "kind": "integer", "nullable": false, "default": "1" }
        ])
    );
    assert_eq!(tables["tables"][1]["indexes"], json!([{ "name": "ix_orders_sku", "on": "sku", "unique": false }]));

    let rows = asked(&state, "db.query", ask("select id, sku, total from orders order by id;", false)).await.unwrap();
    assert_eq!(rows["columns"], json!(["id", "sku", "total"]));
    assert_eq!(rows["rows"], json!([["1", "tea-1", "4.5"], ["2", "cup-2", "∅"]]), "nothing is not an empty text");
    assert!(rows["took"].is_u64() && rows["more"] == false);

    // A query is for reading unless it is said to be more.
    let refused = asked(&state, "db.query", ask("update orders set total = 1", false)).await.unwrap_err().to_string();
    assert!(refused.ends_with("Allow changes to run one that writes."), "{refused}");
    let changed = asked(&state, "db.query", ask("update orders set total = 1", true)).await.unwrap();
    assert_eq!(changed["said"], "Done. 2 rows changed.");
    let wrong = asked(&state, "db.query", ask("select * from nowhere", false)).await.unwrap_err().to_string();
    assert!(wrong.contains("no such table: nowhere"), "{wrong}");
    let two = asked(&state, "db.query", ask("select 1; select 2", false)).await.unwrap_err().to_string();
    assert_eq!(two, "One statement at a time, please");
    assert!(asked(&state, "db.query", ask("  ", false)).await.is_err());
    assert!(
        asked(&state, "db.query", json!({ "taskId": task_id, "database": "file:other.db", "sql": "select 1" }))
            .await
            .is_err()
    );

    // More rows than are shown are said to be there.
    let many = "with recursive n(i) as (select 1 union all select i + 1 from n where i < 700) select i from n";
    let long = asked(&state, "db.query", ask(many, false)).await.unwrap();
    assert_eq!((long["rows"].as_array().unwrap().len(), long["more"].as_bool()), (500, Some(true)));
}

/// A stand-in for `docker` with one Postgres service. What is asked inside the container is written
/// down, and answered as `psql --csv` would.
fn fake_docker(dir: &Path, volume_name: Option<&str>) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let volume = match volume_name {
        Some(name) => json!({ "name": name }),
        None => json!({ "name": "__PROJECT___pgdata" }),
    };
    let config = json!({ "name": "shop", "services": {
        "db": { "image": "postgres:16", "environment": { "POSTGRES_PASSWORD": "pw" },
                "volumes": [{ "type": "volume", "source": "pgdata", "target": "/var/lib/postgresql/data" }] },
        "cache": { "image": "redis:7" }
    }, "volumes": { "pgdata": volume } });
    std::fs::write(dir.join("config.json"), config.to_string()).unwrap();
    fake_tool(
        dir,
        "docker",
        r#"here="$(dirname "$0")"
if [ "$1" = "info" ]; then echo "29.0.0"; exit 0; fi
project=""; prev=""; options=""; sql=""
for word in "$@"; do
  [ "$prev" = "-p" ] && project="$word"
  case "$word" in PGOPTIONS=*) options="$word" ;; esac
  prev="$word"; sql="$word"
done
case "$*" in
  *" config --format json") sed "s/__PROJECT__/$project/g" "$here/config.json" ;;
  *" ps --all --format json") [ -f "$here/running" ] && echo '{"Service":"db","State":"running"}'; exit 0 ;;
  *" exec -T "*)
    printf '%s | %s\n' "$options" "$sql" >> "$here/asked.log"
    case "$options $sql" in
      *read_only=on*update*) echo 'ERROR:  cannot execute UPDATE in a read-only transaction' >&2; exit 1 ;;
      *information_schema.columns*) printf 'orders,id,integer,NO,"nextval(%s)"\norders,note,character varying(80),YES,\n' "'orders_id_seq'::regclass" | sed '1s/^/table_name,column_name,case,is_nullable,coalesce\n/' ;;
      *pg_indexes*) printf 'tablename,indexname,indexdef,?column?\norders,orders_pkey,CREATE UNIQUE INDEX orders_pkey ON public.orders USING btree (id),\n' ;;
      *update*) ;;
      *) printf 'id,note\n1,"a, b"\n2,∅\n' ;;
    esac ;;
  *) echo "unexpected call: $*" >&2; exit 2 ;;
esac
"#,
    )
}

async fn state_with_docker(sandbox: &Sandbox, docker: PathBuf) -> (AppState, String) {
    let missing = PathBuf::from("/nonexistent/ae-test/tool");
    let hosts = Hosts::with(missing.clone(), missing, docker, "http://127.0.0.1:9/graphql".into());
    let project = sandbox.project();
    std::fs::write(project.join("compose.yaml"), "services:\n  db:\n    image: postgres:16\n").unwrap();
    std::fs::write(project.join(".env"), "DATABASE_URL=postgresql+asyncpg://shop:pw@db:5432/shop\n").unwrap();
    let state = AppState::new(sandbox.manager_with(hosts));
    let added = state.manager.add_project(&project.to_string_lossy(), None).unwrap();
    let task_id = idle_task(&state, &added.id).await;
    (state, task_id)
}

#[tokio::test]
async fn a_database_of_the_compose_project_is_asked_through_the_client_in_its_container() {
    let sandbox = Sandbox::new("database-docker");
    let tools = sandbox.0.join("tools");
    let (state, task_id) = state_with_docker(&sandbox, fake_docker(&tools, None)).await;
    let ask =
        |sql: &str, write: bool| json!({ "taskId": task_id, "database": "docker:db", "sql": sql, "write": write });

    // Stopped, it is listed and cannot be asked. The address in the environment points at the same
    // service, so it is not listed a second time; a cache is not a database to read.
    let stopped = asked(&state, "db.get", json!({ "taskId": task_id })).await.unwrap();
    assert_eq!(
        stopped["databases"],
        json!([{ "id": "docker:db", "name": "db (postgres)", "engine": "postgres", "via": "docker", "ready": false,
                 "note": "It is not running. Start it in Run.", "shared": false }])
    );
    let not_yet = asked(&state, "db.query", ask("select 1", false)).await.unwrap_err().to_string();
    assert_eq!(not_yet, "It is not running. Start it in Run.");

    std::fs::write(tools.join("running"), "").unwrap();
    let rows = asked(&state, "db.query", ask("select id, note from orders", false)).await.unwrap();
    assert_eq!(rows["columns"], json!(["id", "note"]));
    assert_eq!(rows["rows"], json!([["1", "a, b"], ["2", "∅"]]));
    let log = std::fs::read_to_string(tools.join("asked.log")).unwrap();
    assert!(log.contains("default_transaction_read_only=on | select id, note from orders"), "{log}");

    // Reading only until changes are allowed; then the same statement goes through.
    let refused = asked(&state, "db.query", ask("update orders set note = 'x'", false)).await.unwrap_err().to_string();
    assert_eq!(
        refused,
        "cannot execute UPDATE in a read-only transaction. The query was run for reading only. Allow changes to run one that writes."
    );
    let done = asked(&state, "db.query", ask("update orders set note = 'x'", true)).await.unwrap();
    assert_eq!(done["said"], "Done. It gave no rows.");
    let log = std::fs::read_to_string(tools.join("asked.log")).unwrap();
    assert!(log.lines().last().unwrap().starts_with("PGOPTIONS=-c statement_timeout=30000 | update"), "{log}");

    let tables = asked(&state, "db.schema", json!({ "taskId": task_id, "database": "docker:db" })).await.unwrap();
    assert_eq!(
        tables["tables"],
        json!([{ "name": "orders",
            "columns": [
                { "name": "id", "kind": "integer", "nullable": false, "default": "nextval('orders_id_seq'::regclass)" },
                { "name": "note", "kind": "character varying(80)", "nullable": true, "default": "" }
            ],
            "indexes": [{ "name": "orders_pkey", "on": "btree (id)", "unique": true }] }])
    );
}

#[tokio::test]
async fn migrations_are_not_tried_on_a_database_other_tasks_may_use() {
    let sandbox = Sandbox::new("database-shared");
    let tools = sandbox.0.join("tools");
    // The compose file names the volume itself, so every task of the project keeps its rows there.
    let (state, task_id) = state_with_docker(&sandbox, fake_docker(&tools, Some("shop_pgdata"))).await;
    std::fs::write(tools.join("running"), "").unwrap();
    let found = asked(&state, "db.get", json!({ "taskId": task_id })).await.unwrap();
    assert_eq!(found["databases"][0]["shared"], true);
    asked(&state, "db.set", json!({ "taskId": task_id, "up": "true", "down": "true" })).await.unwrap();
    let refused = asked(&state, "db.try", json!({ "taskId": task_id })).await.unwrap_err().to_string();
    assert!(refused.starts_with("db (postgres) may hold the rows of other tasks too"), "{refused}");
    // Looking at it is still fine.
    assert!(
        asked(&state, "db.query", json!({ "taskId": task_id, "database": "docker:db", "sql": "select 1" }))
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn a_trial_takes_back_only_what_the_task_added_and_never_runs_against_another_computer() {
    let sandbox = Sandbox::new("database-guards");
    let project = git_project(&sandbox);
    std::fs::create_dir_all(project.join("migrations")).unwrap();
    std::fs::write(project.join("migrations/0001_start.sql"), "create table orders (id integer primary key);\n")
        .unwrap();
    std::fs::write(project.join(".gitignore"), "app.db\nran.log\n.env\n").unwrap();
    git(&project, &["add", "."]);
    git(&project, &["commit", "-q", "-m", "Keep migrations"]);
    let state = AppState::new(sandbox.manager());
    let added = state.manager.add_project(&project.to_string_lossy(), None).unwrap();
    let task_id = idle_task(&state, &added.id).await;
    let folder = state.manager.cwd(&task_id).unwrap();
    shop_db(&folder.join("app.db"), false);
    asked(&state, "db.set", json!({ "taskId": task_id, "up": "echo up >> ran.log", "down": "echo down >> ran.log" }))
        .await
        .unwrap();

    // The task added no migration: "the last one" would be somebody else's, so nothing is taken back.
    let plain = asked(&state, "db.get", json!({ "taskId": task_id })).await.unwrap();
    assert_eq!(
        (plain["down"].as_str(), plain["note"].as_str()),
        (Some(""), Some("This task added no migrations of its own, so a trial takes nothing back."))
    );
    let tried = asked(&state, "db.try", json!({ "taskId": task_id })).await.unwrap();
    assert_eq!(tried["steps"].as_array().unwrap().len(), 1);
    assert_eq!(std::fs::read_to_string(folder.join("ran.log")).unwrap(), "up\n");

    // With one of its own, the way back is the project's.
    std::fs::write(folder.join("migrations/0002_gifts.sql"), "create table gifts (id integer primary key);\n").unwrap();
    assert_eq!(asked(&state, "db.get", json!({ "taskId": task_id })).await.unwrap()["down"], "echo down >> ran.log");

    // The commands go where the task's environment points. An address there that is on another
    // computer is refused, whichever database is on show.
    std::fs::write(folder.join(".env"), "DATABASE_URL=postgresql://shop:pw@db.example.com:5432/shop\n").unwrap();
    let listed = asked(&state, "db.get", json!({ "taskId": task_id })).await.unwrap();
    let remote = listed["databases"].as_array().unwrap().iter().find(|db| db["id"] == "env:DATABASE_URL").unwrap();
    assert_eq!(remote["shared"], true);
    let refused = asked(&state, "db.try", json!({ "taskId": task_id, "database": "file:app.db" })).await.unwrap_err();
    assert!(
        refused.to_string().starts_with("shop at db.example.com (DATABASE_URL) may hold the rows of other tasks too"),
        "{refused}"
    );
    assert_eq!(std::fs::read_to_string(folder.join("ran.log")).unwrap(), "up\n", "nothing was run");
}
