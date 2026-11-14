//! The migrations of a project: which tool runs them, and what each one does.
//!
//! The tool is told from the files a project keeps: `alembic.ini`,
//! `manage.py`, a Prisma schema and so on. Each tool has a folder its
//! migrations live in, a command that brings a database up to date and, for
//! most, one that takes the last steps back.
//!
//! What a migration does is read from its text, without running it. Every
//! step is said in a sentence, and the steps that can lose data or break the
//! code that is still running during a deploy are marked: a dropped column,
//! a new column that cannot be empty and has no default, a rename.
//! The reading is by eye, not by a parser of each language: what it does not
//! recognise it leaves out, and SQL written by hand inside a migration is
//! read as SQL where it can be.

use crate::runner::python_prefix;
use serde::Serialize;
use std::path::Path;

/// One thing a migration does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Step {
    pub text: String,
    /// Why it deserves a second look. `None` for a step that only adds.
    pub danger: Option<String>,
}

fn plain(text: String) -> Step {
    Step { text, danger: None }
}

fn risky(text: String, why: &str) -> Step {
    Step { text, danger: Some(why.to_string()) }
}

const LOSES: &str = "What it held is gone, and code that still reads it breaks.";
const RENAMES: &str = "Code that still uses the old name breaks while both versions run.";
const NEEDS_VALUE: &str = "It cannot be empty and has no default: this fails on a table that has rows.";
const NOT_EMPTY: &str = "This fails if a row has nothing there.";
const RETYPES: &str = "The table may be rewritten, and values that do not fit the new type make it fail.";
const BY_HAND: &str = "It is written by hand, so what it does is not read here.";

/// A tool that runs migrations, as found in one folder of a project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tool {
    /// "alembic", "django", "prisma", "rails", "laravel", "knex", "diesel", "ecto", "migrate", "drizzle" or "sql".
    pub kind: &'static str,
    /// What people call it.
    pub name: &'static str,
    /// The folder its commands run in, within the task's folder. Empty for the task's folder itself.
    pub folder: String,
    /// Brings a database up to date. Empty when the tool has no command of its own.
    pub up: String,
    /// Takes the last steps back. `{n}` stands for how many. Empty when the tool cannot.
    pub down: String,
}

impl Tool {
    /// Whether a file of the project is one of this tool's migrations. The path is within the task's folder.
    pub fn owns(&self, path: &str) -> bool {
        let inside = match self.folder.is_empty() {
            true => path,
            false => match path.strip_prefix(&format!("{}/", self.folder)) {
                Some(inside) => inside,
                None => return false,
            },
        };
        let under = |folder: &str, end: &str| inside.starts_with(folder) && inside.ends_with(end);
        let name = inside.rsplit('/').next().unwrap_or_default();
        match self.kind {
            "alembic" => inside.contains("/versions/") && name.ends_with(".py"),
            "django" => inside.contains("migrations/") && name.ends_with(".py") && name != "__init__.py",
            "prisma" => under("prisma/migrations/", ".sql"),
            "rails" => under("db/migrate/", ".rb"),
            "laravel" => under("database/migrations/", ".php"),
            "knex" => {
                inside.starts_with("migrations/")
                    && [".js", ".ts", ".cjs", ".mjs"].iter().any(|end| name.ends_with(end))
            }
            "diesel" => inside.starts_with("migrations/") && name == "up.sql",
            "ecto" => under("priv/repo/migrations/", ".exs"),
            "migrate" => inside.starts_with("migrations/") && name.ends_with(".up.sql"),
            "drizzle" => under("drizzle/", ".sql"),
            _ => {
                ["migrations/", "db/migrations/", "sql/migrations/"].iter().any(|folder| under(folder, ".sql"))
                    && !name.ends_with(".down.sql")
            }
        }
    }
}

fn has_sql(folder: &Path, end: &str) -> bool {
    std::fs::read_dir(folder)
        .map(|all| all.flatten().any(|one| one.file_name().to_string_lossy().ends_with(end)))
        .unwrap_or(false)
}

/// The tool of one folder, if the folder has the marks of one.
fn tool_in(folder: &Path, within: &str) -> Option<Tool> {
    let has = |name: &str| folder.join(name).is_file();
    let dir = |name: &str| folder.join(name).is_dir();
    let tool = |kind, name, up: String, down: &str| {
        Some(Tool { kind, name, folder: within.to_string(), up, down: down.to_string() })
    };
    if has("alembic.ini") {
        let alembic = python_prefix(folder, "alembic");
        return tool("alembic", "Alembic", format!("{alembic} upgrade head"), &format!("{alembic} downgrade -{{n}}"));
    }
    if has("manage.py") {
        // The way back depends on which apps the task touched; it is worked out from its migrations.
        return tool("django", "Django", format!("{} manage.py migrate", python_prefix(folder, "python")), "");
    }
    if has("prisma/schema.prisma") {
        return tool("prisma", "Prisma", "npx prisma migrate deploy".into(), "");
    }
    if has("bin/rails") && dir("db/migrate") {
        return tool("rails", "Rails", "bin/rails db:migrate".into(), "bin/rails db:rollback STEP={n}");
    }
    if has("artisan") && dir("database/migrations") {
        return tool("laravel", "Laravel", "php artisan migrate".into(), "php artisan migrate:rollback --step={n}");
    }
    if ["knexfile.js", "knexfile.ts", "knexfile.cjs", "knexfile.mjs"].iter().any(|name| has(name)) {
        return tool("knex", "Knex", "npx knex migrate:latest".into(), "npx knex migrate:rollback");
    }
    if has("diesel.toml") {
        return tool("diesel", "Diesel", "diesel migration run".into(), "diesel migration revert -n {n}");
    }
    if has("mix.exs") && dir("priv/repo/migrations") {
        return tool("ecto", "Ecto", "mix ecto.migrate".into(), "mix ecto.rollback -n {n}");
    }
    if ["drizzle.config.ts", "drizzle.config.js", "drizzle.config.mjs"].iter().any(|name| has(name)) {
        return tool("drizzle", "Drizzle", "npx drizzle-kit migrate".into(), "");
    }
    if has_sql(&folder.join("migrations"), ".up.sql") {
        let migrate = "migrate -path migrations -database \"$DATABASE_URL\"";
        return tool("migrate", "golang-migrate", format!("{migrate} up"), &format!("{migrate} down {{n}}"));
    }
    if ["migrations", "db/migrations", "sql/migrations"].iter().any(|name| has_sql(&folder.join(name), ".sql")) {
        return tool("sql", "SQL files", String::new(), "");
    }
    None
}

/// The folders of a project that are worth a look beside its top: not hidden, not what tools fill.
const NOT_OURS: [&str; 8] = ["node_modules", "target", "vendor", "dist", "build", "venv", "__pycache__", "tmp"];
const MOST_FOLDERS: usize = 60;

/// The migration tool of a project: the one of its top folder, else the one of a folder right under it.
pub fn detect(root: &Path) -> Option<Tool> {
    if let Some(found) = tool_in(root, "") {
        return Some(found);
    }
    let mut under: Vec<String> = std::fs::read_dir(root)
        .ok()?
        .flatten()
        .filter(|one| one.path().is_dir())
        .map(|one| one.file_name().to_string_lossy().into_owned())
        .filter(|name| !name.starts_with('.') && !NOT_OURS.contains(&name.as_str()))
        .collect();
    under.sort();
    under.truncate(MOST_FOLDERS);
    under.iter().find_map(|name| tool_in(&root.join(name), name))
}

// ----- reading a migration -----

fn is_word(sign: char) -> bool {
    sign.is_alphanumeric() || sign == '_'
}

/// Every call of `name(` in a text: where it starts, and what stands between its brackets.
fn calls<'a>(text: &'a str, name: &str) -> Vec<(usize, &'a str)> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(at) = text[from..].find(name).map(|at| at + from) {
        from = at + name.len();
        let before = text[..at].chars().next_back();
        // `table(` is not found inside `create_table(`.
        if before.is_some_and(is_word) {
            continue;
        }
        let rest = &text[from..];
        let open = rest.len() - rest.trim_start().len();
        if !rest[open..].starts_with('(') {
            continue;
        }
        let inside = &rest[open + 1..];
        let (mut depth, mut quote, mut end) = (1usize, None::<char>, inside.len());
        let mut signs = inside.char_indices().peekable();
        while let Some((index, sign)) = signs.next() {
            match (quote, sign) {
                (Some(_), '\\') => {
                    signs.next();
                }
                (Some(open), sign) if sign == open => quote = None,
                (Some(_), _) => {}
                (None, '\'' | '"' | '`') => quote = Some(sign),
                (None, '(' | '[' | '{') => depth += 1,
                (None, ')' | ']' | '}') => {
                    depth -= 1;
                    if depth == 0 {
                        end = index;
                        break;
                    }
                }
                _ => {}
            }
        }
        found.push((at, &inside[..end]));
    }
    found
}

/// The texts in quotes of a piece of code, in order. A symbol of Ruby or Elixir (`:name`) counts as one.
fn literals(code: &str) -> Vec<String> {
    let mut found = Vec::new();
    let signs: Vec<char> = code.chars().collect();
    let mut at = 0;
    while at < signs.len() {
        let sign = signs[at];
        if sign == '\'' || sign == '"' || sign == '`' {
            // Python's three quotes open a text as one does.
            let triple = at + 2 < signs.len() && signs[at + 1] == sign && signs[at + 2] == sign;
            let start = if triple { at + 3 } else { at + 1 };
            let mut end = start;
            while end < signs.len() {
                if signs[end] == '\\' {
                    end += 2;
                    continue;
                }
                let closes = signs[end] == sign
                    && (!triple || (end + 2 < signs.len() && signs[end + 1] == sign && signs[end + 2] == sign));
                if closes {
                    break;
                }
                end += 1;
            }
            found.push(signs[start..end.min(signs.len())].iter().collect());
            at = end + if triple { 3 } else { 1 };
        } else if sign == ':'
            && at + 1 < signs.len()
            && (signs[at + 1].is_alphabetic() || signs[at + 1] == '_')
            && (at == 0 || !is_word(signs[at - 1]) && signs[at - 1] != ':')
        {
            let end = (at + 1..signs.len()).find(|index| !is_word(signs[*index])).unwrap_or(signs.len());
            found.push(signs[at + 1..end].iter().collect());
            at = end;
        } else {
            at += 1;
        }
    }
    found
}

/// The text given under a name, as in `name="orders"` or `name: "orders"`.
fn keyword(code: &str, key: &str) -> Option<String> {
    let mut from = 0;
    while let Some(at) = code[from..].find(key).map(|at| at + from) {
        from = at + key.len();
        if code[..at].chars().next_back().is_some_and(is_word) {
            continue;
        }
        let rest = code[from..].trim_start();
        let Some(rest) = rest.strip_prefix('=').or_else(|| rest.strip_prefix(':')) else { continue };
        return literals(rest.trim_start()).into_iter().next().filter(|_| rest.trim_start().starts_with(['\'', '"']));
    }
    None
}

/// Whether a piece of code sets something under a name to a value, as in `nullable=False`.
fn sets(code: &str, key: &str, value: &str) -> bool {
    let squeezed: String = code.chars().filter(|sign| !sign.is_whitespace()).collect();
    squeezed.contains(&format!("{key}={value}")) || squeezed.contains(&format!("{key}:{value}"))
}

fn mark(name: &str) -> String {
    format!("`{}`", name.trim_matches(['"', '`', '[', ']', '\'']))
}

// ----- SQL -----

/// A text of SQL without its comments, cut into statements.
fn statements(sql: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut now = String::new();
    let signs: Vec<char> = sql.chars().collect();
    let (mut at, mut quote) = (0, None::<char>);
    while at < signs.len() {
        let (sign, next) = (signs[at], signs.get(at + 1).copied());
        match quote {
            Some(open) => {
                now.push(sign);
                if sign == open {
                    quote = None;
                }
            }
            None if sign == '-' && next == Some('-') => {
                while at < signs.len() && signs[at] != '\n' {
                    at += 1;
                }
                continue;
            }
            None if sign == '/' && next == Some('*') => {
                at += 2;
                while at + 1 < signs.len() && !(signs[at] == '*' && signs[at + 1] == '/') {
                    at += 1;
                }
                at += 2;
                continue;
            }
            None if sign == ';' => found.push(std::mem::take(&mut now)),
            None => {
                if sign == '\'' || sign == '"' || sign == '`' {
                    quote = Some(sign);
                }
                now.push(sign);
            }
        }
        at += 1;
    }
    found.push(now);
    found
        .into_iter()
        .map(|one| one.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|one| !one.is_empty())
        .collect()
}

/// The words of a statement after some leading ones, with the optional ones in between skipped.
fn after<'a>(words: &'a [&'a str], lead: &[&str], skip: &[&str]) -> Option<&'a [&'a str]> {
    if words.len() < lead.len() || !words.iter().zip(lead).all(|(word, lead)| word.eq_ignore_ascii_case(lead)) {
        return None;
    }
    let mut rest = &words[lead.len()..];
    'skipping: loop {
        for phrase in skip {
            let parts: Vec<&str> = phrase.split(' ').collect();
            if rest.len() >= parts.len() && rest.iter().zip(&parts).all(|(word, part)| word.eq_ignore_ascii_case(part))
            {
                rest = &rest[parts.len()..];
                continue 'skipping;
            }
        }
        return Some(rest);
    }
}

/// The name a statement gives first: up to a bracket, a comma or a space.
fn name_of(words: &[&str]) -> String {
    let first = words.first().copied().unwrap_or_default();
    mark(first.split(['(', ',', ';']).next().unwrap_or_default())
}

/// The parts of an `alter table` after the table's name, cut at the commas that are not inside brackets.
fn actions(rest: &str) -> Vec<String> {
    let mut found = vec![String::new()];
    let mut depth = 0;
    for sign in rest.chars() {
        match sign {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                found.push(String::new());
                continue;
            }
            _ => {}
        }
        found.last_mut().unwrap().push(sign);
    }
    found.into_iter().map(|one| one.trim().to_string()).filter(|one| !one.is_empty()).collect()
}

fn alter(table: &str, action: &str) -> Option<Step> {
    let words: Vec<&str> = action.split(' ').collect();
    let lower = action.to_lowercase();
    let column = |rest: &[&str]| format!("`{}.{}`", table.trim_matches('`'), name_of(rest).trim_matches('`'));
    if let Some(rest) = after(&words, &["add"], &["constraint", "primary", "foreign", "unique", "check"])
        .filter(|rest| rest.len() < words.len() - 1)
    {
        let _ = rest;
        return Some(plain(format!("Adds a constraint to {table}")));
    }
    if let Some(rest) = after(&words, &["add"], &["column", "if not exists"]) {
        let kind = rest.get(1..).map(|kind| kind.join(" ")).unwrap_or_default();
        let text = format!("Adds {} ({})", column(rest), kind.to_lowercase());
        let needs = lower.contains("not null")
            && !lower.contains("default")
            && !lower.contains("generated")
            && !lower.contains("serial");
        return Some(if needs { risky(text, NEEDS_VALUE) } else { plain(text) });
    }
    if let Some(rest) = after(&words, &["drop", "constraint"], &["if exists"]) {
        return Some(plain(format!("Drops constraint {} of {table}", name_of(rest))));
    }
    if let Some(rest) = after(&words, &["drop"], &["column", "if exists"]) {
        return Some(risky(format!("Drops {}", column(rest)), LOSES));
    }
    if let Some(rest) = after(&words, &["rename", "to"], &[]) {
        return Some(risky(format!("Renames table {table} to {}", name_of(rest)), RENAMES));
    }
    if let Some(rest) = after(&words, &["rename"], &["column"]).filter(|rest| rest.len() >= 3) {
        return Some(risky(format!("Renames {} to {}", column(rest), name_of(&rest[2..])), RENAMES));
    }
    if let Some(rest) = after(&words, &["alter"], &["column"]) {
        let what = rest.get(1..).map(|what| what.join(" ").to_lowercase()).unwrap_or_default();
        return Some(match what.as_str() {
            what if what.starts_with("type ") || what.starts_with("set data type ") => {
                let kind = what.trim_start_matches("set data ").trim_start_matches("type ");
                risky(
                    format!("Changes the type of {} to {}", column(rest), kind.split(" using ").next().unwrap_or(kind)),
                    RETYPES,
                )
            }
            "set not null" => risky(format!("{} may no longer be empty", column(rest)), NOT_EMPTY),
            "drop not null" => plain(format!("{} may be empty now", column(rest))),
            _ => plain(format!("Changes the default of {}", column(rest))),
        });
    }
    // MySQL's own words for the same.
    if let Some(rest) = after(&words, &["modify"], &["column"]) {
        return Some(risky(format!("Changes the type of {}", column(rest)), RETYPES));
    }
    if let Some(rest) = after(&words, &["change"], &["column"]).filter(|rest| rest.len() >= 2) {
        return Some(risky(format!("Renames {} to {}", column(rest), name_of(&rest[1..])), RENAMES));
    }
    None
}

/// What a text of SQL does, statement by statement.
pub fn read_sql(sql: &str) -> Vec<Step> {
    let mut steps = Vec::new();
    for statement in statements(sql) {
        let words: Vec<&str> = statement.split(' ').collect();
        let is = |lead: &[&str], skip: &[&str]| after(&words, lead, skip);
        if let Some(rest) = is(&["create", "table"], &["if not exists"])
            .or_else(|| is(&["create", "temporary", "table"], &[]).map(|_| &words[..0]))
            .filter(|rest| !rest.is_empty())
        {
            steps.push(plain(format!("Creates table {}", name_of(rest))));
        } else if let Some(rest) = is(&["drop", "table"], &["if exists"]) {
            steps.push(risky(format!("Drops table {}", name_of(rest)), LOSES));
        } else if let Some(rest) = is(&["alter", "table"], &["if exists", "only"]) {
            let table = name_of(rest);
            let found: Vec<Step> = actions(&rest[1.min(rest.len())..].join(" "))
                .iter()
                .filter_map(|action| alter(&table, action))
                .collect();
            match found.is_empty() {
                true => steps.push(plain(format!("Changes table {table}"))),
                false => steps.extend(found),
            }
        } else if let Some(rest) =
            is(&["create"], &["unique"]).and_then(|rest| after(rest, &["index"], &["concurrently", "if not exists"]))
        {
            let on = rest.iter().position(|word| word.eq_ignore_ascii_case("on")).and_then(|at| rest.get(at + 1..));
            let table = on.map(name_of).unwrap_or_default();
            steps.push(plain(format!("Adds index {} on {table}", name_of(rest))));
        } else if let Some(rest) = is(&["drop", "index"], &["concurrently", "if exists"]) {
            steps.push(plain(format!("Drops index {}", name_of(rest))));
        } else if let Some(rest) = is(&["truncate"], &["table", "only"]) {
            steps.push(risky(format!("Empties {}", name_of(rest)), "Every row of it is gone."));
        } else if let Some(rest) = is(&["delete", "from"], &["only"]) {
            steps.push(risky(format!("Deletes rows of {}", name_of(rest)), "Rows are gone for good."));
        } else if let Some(rest) = is(&["update"], &["only"]) {
            steps.push(plain(format!("Changes rows of {}", name_of(rest))));
        } else if let Some(rest) = is(&["insert", "into"], &[]) {
            steps.push(plain(format!("Adds rows to {}", name_of(rest))));
        } else if let Some(rest) = is(&["rename", "table"], &[]) {
            steps.push(risky(format!("Renames table {}", name_of(rest)), RENAMES));
        } else if let Some(rest) = is(&["drop"], &[]).filter(|rest| rest.len() >= 2) {
            let kind = rest[0].to_lowercase();
            let name = name_of(after(&rest[1..], &[], &["if exists"]).unwrap_or(&rest[1..]));
            let danger = matches!(kind.as_str(), "schema" | "database" | "view" | "materialized" | "type");
            steps.push(if danger {
                risky(format!("Drops {kind} {name}"), LOSES)
            } else {
                plain(format!("Drops {kind} {name}"))
            });
        } else if let Some(rest) = is(&["create"], &["or replace"]).filter(|rest| rest.len() >= 2) {
            let kind = rest[0].to_lowercase();
            if matches!(
                kind.as_str(),
                "type" | "extension" | "view" | "function" | "trigger" | "schema" | "sequence" | "materialized"
            ) {
                let name = name_of(after(&rest[1..], &[], &["if not exists", "view"]).unwrap_or(&rest[1..]));
                steps.push(plain(format!("Creates {kind} {name}")));
            }
        }
    }
    steps
}

// ----- the languages migrations are written in -----

/// SQL inside a migration of another language: read as SQL where that gives something, else said to be by hand.
fn by_hand(code: &str) -> Vec<Step> {
    let read: Vec<Step> = literals(code).first().map(|sql| read_sql(sql)).unwrap_or_default();
    match read.is_empty() {
        true => vec![risky("Runs SQL written by hand".into(), BY_HAND)],
        false => read,
    }
}

/// The part of a Python migration between `def name(` and the next `def` at the left edge.
fn function<'a>(text: &'a str, name: &str) -> &'a str {
    let Some(start) = text.find(&format!("def {name}(")) else { return "" };
    let body = &text[start..];
    let end = body[1..].find("\ndef ").map_or(body.len(), |at| at + 1);
    &body[..end]
}

/// Makes the steps of one call out of the texts in it and the whole of what it was given.
type Reader<'a> = dyn Fn(&[String], &str) -> Option<Vec<Step>> + 'a;

fn read_alembic(text: &str) -> Vec<Step> {
    let upgrade = function(text, "upgrade");
    let mut found: Vec<(usize, Vec<Step>)> = Vec::new();
    // Inside `with op.batch_alter_table("orders") as batch_op:` the table is named once, above.
    let batches = calls(upgrade, "op.batch_alter_table");
    let table_at = |at: usize| {
        batches.iter().rev().find(|(start, _)| *start < at).and_then(|(_, args)| literals(args).into_iter().next())
    };
    for via in ["op.", "batch_op."] {
        let batch = via == "batch_op.";
        let mut one = |name: &str, make: &Reader| {
            for (at, args) in calls(upgrade, &format!("{via}{name}")) {
                let mut names = literals(args);
                if batch {
                    names.insert(0, table_at(at).unwrap_or_default());
                }
                if let Some(steps) = make(&names, args) {
                    found.push((at, steps));
                }
            }
        };
        let column = |names: &[String]| format!("`{}.{}`", names[0], names[1]);
        one("create_table", &|names, _| Some(vec![plain(format!("Creates table {}", mark(names.first()?)))]));
        one("drop_table", &|names, _| Some(vec![risky(format!("Drops table {}", mark(names.first()?)), LOSES)]));
        one("rename_table", &|names, _| {
            Some(vec![risky(format!("Renames table {} to {}", mark(names.first()?), mark(names.get(1)?)), RENAMES)])
        });
        one("add_column", &|names, args| {
            names.get(1)?;
            let kind = args
                .match_indices("sa.")
                .nth(1)
                .map(|(at, _)| &args[at + 3..])
                .or_else(|| args.split("Column(").nth(1).and_then(|rest| rest.split(',').nth(1)).map(str::trim));
            let kind: String =
                kind.unwrap_or_default().chars().take_while(|sign| is_word(*sign) || *sign == '.').collect();
            let text = match kind.is_empty() {
                true => format!("Adds {}", column(names)),
                false => format!("Adds {} ({})", column(names), kind.to_lowercase()),
            };
            let needs = sets(args, "nullable", "False") && !args.contains("server_default");
            Some(vec![if needs { risky(text, NEEDS_VALUE) } else { plain(text) }])
        });
        one("drop_column", &|names, _| {
            names.get(1)?;
            Some(vec![risky(format!("Drops {}", column(names)), LOSES)])
        });
        one("alter_column", &|names, args| {
            names.get(1)?;
            let mut steps = Vec::new();
            if let Some(new) = keyword(args, "new_column_name") {
                steps.push(risky(format!("Renames {} to {}", column(names), mark(&new)), RENAMES));
            }
            if args.contains("type_") {
                steps.push(risky(format!("Changes the type of {}", column(names)), RETYPES));
            }
            if sets(args, "nullable", "False") {
                steps.push(risky(format!("{} may no longer be empty", column(names)), NOT_EMPTY));
            }
            if sets(args, "nullable", "True") {
                steps.push(plain(format!("{} may be empty now", column(names))));
            }
            if steps.is_empty() {
                steps.push(plain(format!("Changes {}", column(names))));
            }
            Some(steps)
        });
        one("create_index", &|names, _| {
            let table = if batch { names.first()? } else { names.get(1)? };
            let name = if batch { names.get(1)? } else { names.first()? };
            Some(vec![plain(format!("Adds index {} on {}", mark(name), mark(table)))])
        });
        one("drop_index", &|names, _| {
            Some(vec![plain(format!("Drops index {}", mark(if batch { names.get(1)? } else { names.first()? })))])
        });
        for name in ["create_foreign_key", "create_unique_constraint", "create_check_constraint", "create_primary_key"]
        {
            one(name, &|names, _| {
                Some(vec![plain(format!(
                    "Adds a constraint to {}",
                    mark(if batch { names.first()? } else { names.get(1)? })
                ))])
            });
        }
        one("drop_constraint", &|names, _| {
            let (name, table) = if batch { (names.get(1)?, names.first()?) } else { (names.first()?, names.get(1)?) };
            Some(vec![plain(format!("Drops constraint {} of {}", mark(name), mark(table)))])
        });
        one("bulk_insert", &|_, _| Some(vec![plain("Adds rows to a table".into())]));
        if !batch {
            one("execute", &|_, args| Some(by_hand(args)));
        }
    }
    found.sort_by_key(|(at, _)| *at);
    found.into_iter().flat_map(|(_, steps)| steps).collect()
}

fn read_django(text: &str) -> Vec<Step> {
    let mut found: Vec<(usize, Vec<Step>)> = Vec::new();
    let mut one = |name: &str, make: &dyn Fn(&str) -> Option<Vec<Step>>| {
        for (at, args) in calls(text, &format!("migrations.{name}")) {
            if let Some(steps) = make(args) {
                found.push((at, steps));
            }
        }
    };
    let field = |args: &str| Some(format!("`{}.{}`", keyword(args, "model_name")?, keyword(args, "name")?));
    one("CreateModel", &|args| Some(vec![plain(format!("Creates the table of {}", mark(&keyword(args, "name")?)))]));
    one("DeleteModel", &|args| {
        Some(vec![risky(format!("Drops the table of {}", mark(&keyword(args, "name")?)), LOSES)])
    });
    one("RenameModel", &|args| {
        Some(vec![risky(
            format!("Renames {} to {}", mark(&keyword(args, "old_name")?), mark(&keyword(args, "new_name")?)),
            RENAMES,
        )])
    });
    one("AddField", &|args| Some(vec![plain(format!("Adds {}", field(args)?))]));
    one("RemoveField", &|args| Some(vec![risky(format!("Drops {}", field(args)?), LOSES)]));
    one("AlterField", &|args| {
        Some(vec![risky(
            format!("Changes {}", field(args)?),
            "A changed type or a field that may no longer be empty can fail on the rows there are.",
        )])
    });
    one("RenameField", &|args| {
        let (model, old, new) = (keyword(args, "model_name")?, keyword(args, "old_name")?, keyword(args, "new_name")?);
        Some(vec![risky(format!("Renames `{model}.{old}` to {}", mark(&new)), RENAMES)])
    });
    for (name, what) in [
        ("AddIndex", "Adds an index to"),
        ("RemoveIndex", "Drops an index of"),
        ("AddConstraint", "Adds a constraint to"),
        ("RemoveConstraint", "Drops a constraint of"),
    ] {
        one(name, &|args| Some(vec![plain(format!("{what} {}", mark(&keyword(args, "model_name")?)))]));
    }
    one("RunSQL", &|args| Some(by_hand(args)));
    one("RunPython", &|args| {
        let name: String = args.trim_start().chars().take_while(|sign| is_word(*sign) || *sign == '.').collect();
        let back = args.contains(',') || args.contains("reverse_code");
        let why = if back {
            "It is code written by hand, so what it does is not read here."
        } else {
            "It is code written by hand, and it has no way back."
        };
        Some(vec![risky(format!("Runs {}", mark(&name)), why)])
    });
    found.sort_by_key(|(at, _)| *at);
    found.into_iter().flat_map(|(_, steps)| steps).collect()
}

/// Migrations written as a row of calls, one to a line: Rails, Knex, Laravel and Ecto.
fn read_calls(text: &str) -> Vec<Step> {
    let mut found: Vec<(usize, Step)> = Vec::new();
    // In Ruby and Elixir a call may be written without brackets: they are put in for the reading.
    let bracketed: String = text
        .lines()
        .map(|line| {
            let trimmed = line.trim_start();
            let name: String = trimmed.chars().take_while(|sign| is_word(*sign)).collect();
            let rest = &trimmed[name.len()..];
            match !name.is_empty() && rest.starts_with(' ') && rest.trim_start().starts_with([':', '"', '\'']) {
                true => format!("{name}({})", rest.trim().trim_end_matches(" do").trim_end_matches(" do |t|")),
                false => line.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let text = bracketed.as_str();
    let mut one = |name: &str, make: &dyn Fn(&[String]) -> Option<Step>| {
        for (at, args) in calls(text, name) {
            if let Some(step) = make(&literals(args)) {
                found.push((at, step));
            }
        }
    };
    let column = |names: &[String]| Some(format!("`{}.{}`", names.first()?, names.get(1)?));
    for name in ["create_table", "createTable", "Schema::create"] {
        one(name, &|names| Some(plain(format!("Creates table {}", mark(names.first()?)))));
    }
    for name in ["drop_table", "dropTable", "dropTableIfExists", "Schema::drop", "Schema::dropIfExists"] {
        one(name, &|names| Some(risky(format!("Drops table {}", mark(names.first()?)), LOSES)));
    }
    for name in ["rename_table", "renameTable", "Schema::rename"] {
        one(name, &|names| {
            Some(risky(format!("Renames table {} to {}", mark(names.first()?), mark(names.get(1)?)), RENAMES))
        });
    }
    for name in ["alterTable", "Schema::table", "change_table"] {
        one(name, &|names| Some(plain(format!("Changes table {}", mark(names.first()?)))));
    }
    one("add_column", &|names| {
        Some(plain(format!(
            "Adds {}{}",
            column(names)?,
            names.get(2).map(|kind| format!(" ({kind})")).unwrap_or_default()
        )))
    });
    one("add_reference", &|names| {
        Some(plain(format!("Adds a reference to {} on {}", mark(names.get(1)?), mark(names.first()?))))
    });
    one("remove_column", &|names| Some(risky(format!("Drops {}", column(names)?), LOSES)));
    one("rename_column", &|names| {
        Some(risky(format!("Renames {} to {}", column(names)?, mark(names.get(2)?)), RENAMES))
    });
    one("change_column", &|names| Some(risky(format!("Changes the type of {}", column(names)?), RETYPES)));
    one("change_column_null", &|names| Some(plain(format!("Changes whether {} may be empty", column(names)?))));
    one("add_index", &|names| Some(plain(format!("Adds an index on {}", mark(names.first()?)))));
    one("remove_index", &|names| Some(plain(format!("Drops an index of {}", mark(names.first()?)))));
    for name in ["dropColumn", "dropColumns"] {
        one(name, &|names| Some(risky(format!("Drops column {}", mark(names.first()?)), LOSES)));
    }
    one("renameColumn", &|names| {
        Some(risky(format!("Renames column {} to {}", mark(names.first()?), mark(names.get(1)?)), RENAMES))
    });
    for name in ["execute", "raw", "DB::statement", "DB::unprepared"] {
        for (at, args) in calls(text, name) {
            found.extend(by_hand(args).into_iter().map(|step| (at, step)));
        }
    }
    found.sort_by_key(|(at, _)| *at);
    found.into_iter().map(|(_, step)| step).collect()
}

/// What one migration does, read from its text. `path` says which language it is in.
pub fn read(path: &str, text: &str) -> Vec<Step> {
    let name = path.rsplit('/').next().unwrap_or_default();
    match name.rsplit('.').next().unwrap_or_default() {
        "sql" => read_sql(text),
        "py" if text.contains("migrations.Migration") => read_django(text),
        "py" => read_alembic(text),
        _ => read_calls(text),
    }
}

/// Whether a migration says how to take it back. `None` when that cannot be told from its text.
pub fn has_way_back(path: &str, text: &str) -> Option<bool> {
    if path.ends_with(".py") && !text.contains("migrations.Migration") {
        let body = function(text, "downgrade");
        if body.is_empty() {
            return Some(false);
        }
        // A body of nothing but `pass`, a comment or a refusal takes nothing back.
        let does: Vec<&str> = body
            .lines()
            .skip(1)
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#') && !line.starts_with("\"\"\""))
            .collect();
        return Some(!does.iter().all(|line| *line == "pass" || line.starts_with("raise ")));
    }
    if path.ends_with(".rb") {
        return Some(text.contains("def change") || text.contains("def down"));
    }
    None
}

/// The number a Django migration starts with, as in `0007_add_gift.py`.
pub fn django_number(name: &str) -> Option<&str> {
    let digits = name.len() - name.trim_start_matches(|sign: char| sign.is_ascii_digit()).len();
    (digits > 0).then(|| &name[..digits])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(steps: &[Step]) -> Vec<(&str, bool)> {
        steps.iter().map(|step| (step.text.as_str(), step.danger.is_some())).collect()
    }

    #[test]
    fn sql_is_read_statement_by_statement_and_what_loses_data_is_marked() {
        let sql = r#"
            -- a new table, and the old one goes
            CREATE TABLE IF NOT EXISTS gifts (id serial PRIMARY KEY, note text DEFAULT 'a; b');
            ALTER TABLE orders
                ADD COLUMN count integer NOT NULL,
                ADD COLUMN gift_id integer REFERENCES gifts(id),
                DROP COLUMN total,
                ALTER COLUMN state TYPE varchar(20) USING state::varchar,
                ALTER COLUMN buyer SET NOT NULL;
            /* indexes */
            CREATE UNIQUE INDEX CONCURRENTLY ix_orders_gift ON orders (gift_id);
            ALTER TABLE "users" RENAME COLUMN name TO full_name;
            DROP TABLE IF EXISTS carts;
            UPDATE orders SET count = 1;
            DELETE FROM sessions WHERE old;
        "#;
        assert_eq!(
            texts(&read_sql(sql)),
            [
                ("Creates table `gifts`", false),
                ("Adds `orders.count` (integer not null)", true),
                ("Adds `orders.gift_id` (integer references gifts(id))", false),
                ("Drops `orders.total`", true),
                ("Changes the type of `orders.state` to varchar(20)", true),
                ("`orders.buyer` may no longer be empty", true),
                ("Adds index `ix_orders_gift` on `orders`", false),
                ("Renames `users.name` to `full_name`", true),
                ("Drops table `carts`", true),
                ("Changes rows of `orders`", false),
                ("Deletes rows of `sessions`", true),
            ]
        );
    }

    #[test]
    fn an_alembic_migration_is_read_from_its_upgrade_alone() {
        let text = r#"
revision = "b2"

def upgrade():
    op.create_table("gifts", sa.Column("id", sa.Integer(), primary_key=True))
    op.add_column("orders", sa.Column("count", sa.Integer(), nullable=False))
    op.add_column('orders', sa.Column('note', sa.String(length=80), nullable=True))
    with op.batch_alter_table("orders") as batch_op:
        batch_op.drop_column("total")
        batch_op.alter_column("state", type_=sa.String(20), nullable=False)
    op.create_index("ix_orders_count", "orders", ["count"])
    op.execute("UPDATE orders SET count = 1")
    op.execute(some_statement)


def downgrade():
    op.drop_table("gifts")
    op.drop_column("orders", "count")
"#;
        assert_eq!(
            texts(&read("migrations/versions/b2_gifts.py", text)),
            [
                ("Creates table `gifts`", false),
                ("Adds `orders.count` (integer)", true),
                ("Adds `orders.note` (string)", false),
                ("Drops `orders.total`", true),
                ("Changes the type of `orders.state`", true),
                ("`orders.state` may no longer be empty", true),
                ("Adds index `ix_orders_count` on `orders`", false),
                ("Changes rows of `orders`", false),
                ("Runs SQL written by hand", true),
            ]
        );
        assert_eq!(has_way_back("versions/b2.py", text), Some(true));
        assert_eq!(
            has_way_back(
                "versions/b3.py",
                "def upgrade():\n    op.drop_table('x')\n\ndef downgrade():\n    # nothing\n    pass\n"
            ),
            Some(false)
        );
        assert_eq!(has_way_back("migrations/0001.sql", "create table x ()"), None);
    }

    #[test]
    fn a_django_migration_is_read_from_its_operations() {
        let text = r#"
class Migration(migrations.Migration):
    dependencies = [("shop", "0006_orders")]
    operations = [
        migrations.CreateModel(name="Gift", fields=[("id", models.AutoField(primary_key=True))]),
        migrations.AddField(model_name="order", name="count", field=models.IntegerField(default=1)),
        migrations.RemoveField(model_name="order", name="total"),
        migrations.RenameField(model_name="order", old_name="buyer", new_name="customer"),
        migrations.RunPython(fill_counts),
        migrations.RunSQL("DROP TABLE carts"),
    ]
"#;
        assert_eq!(
            texts(&read("shop/migrations/0007_gifts.py", text)),
            [
                ("Creates the table of `Gift`", false),
                ("Adds `order.count`", false),
                ("Drops `order.total`", true),
                ("Renames `order.buyer` to `customer`", true),
                ("Runs `fill_counts`", true),
                ("Drops table `carts`", true),
            ]
        );
        assert_eq!(django_number("0007_gifts.py"), Some("0007"));
        assert_eq!(django_number("__init__.py"), None);
    }

    #[test]
    fn migrations_written_as_calls_are_read_with_or_without_brackets() {
        let rails = "class AddGifts < ActiveRecord::Migration[7.1]\n  def change\n    create_table :gifts do |t|\n      t.string :name\n    end\n    add_column :orders, :count, :integer\n    remove_column :orders, :total\n    rename_column :orders, :buyer, :customer\n  end\nend\n";
        assert_eq!(
            texts(&read("db/migrate/20261114_add_gifts.rb", rails)),
            [
                ("Creates table `gifts`", false),
                ("Adds `orders.count` (integer)", false),
                ("Drops `orders.total`", true),
                ("Renames `orders.buyer` to `customer`", true),
            ]
        );
        assert_eq!(has_way_back("db/migrate/x.rb", rails), Some(true));
        let knex = "exports.up = (knex) => knex.schema.createTable('gifts', (t) => { t.increments() })\n  .alterTable('orders', (t) => { t.dropColumn('total'); t.renameColumn('buyer', 'customer'); });\n";
        assert_eq!(
            texts(&read("migrations/20261114_gifts.js", knex)),
            [
                ("Creates table `gifts`", false),
                ("Changes table `orders`", false),
                ("Drops column `total`", true),
                ("Renames column `buyer` to `customer`", true),
            ]
        );
    }

    #[test]
    fn the_tool_is_told_from_the_files_a_project_keeps() {
        let dir = std::env::temp_dir().join(format!("ae-migrations-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let write = |path: &str| {
            let file = dir.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, "").unwrap();
        };
        assert_eq!(detect(&dir), None);
        write("backend/alembic.ini");
        write("backend/uv.lock");
        write("node_modules/x/alembic.ini");
        let found = detect(&dir).unwrap();
        assert_eq!((found.kind, found.folder.as_str()), ("alembic", "backend"));
        assert_eq!(
            (found.up.as_str(), found.down.as_str()),
            ("uv run alembic upgrade head", "uv run alembic downgrade -{n}")
        );
        assert!(found.owns("backend/migrations/versions/b2_gifts.py"));
        assert!(!found.owns("backend/app/models.py") && !found.owns("migrations/versions/b2.py"));

        // The top folder is asked first.
        write("prisma/schema.prisma");
        let top = detect(&dir).unwrap();
        assert_eq!((top.kind, top.folder.as_str(), top.down.as_str()), ("prisma", "", ""));
        assert!(top.owns("prisma/migrations/20261114_gifts/migration.sql"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn each_tool_knows_its_own_files() {
        let tool =
            |kind: &'static str| Tool { kind, name: "", folder: String::new(), up: String::new(), down: String::new() };
        assert!(
            tool("django").owns("shop/migrations/0007_gifts.py") && !tool("django").owns("shop/migrations/__init__.py")
        );
        assert!(tool("rails").owns("db/migrate/20261114_add_gifts.rb"));
        assert!(
            tool("migrate").owns("migrations/0002_gifts.up.sql")
                && !tool("migrate").owns("migrations/0002_gifts.down.sql")
        );
        assert!(
            tool("diesel").owns("migrations/2026-11-14_gifts/up.sql")
                && !tool("diesel").owns("migrations/2026-11-14_gifts/down.sql")
        );
        assert!(tool("sql").owns("db/migrations/0002.sql") && !tool("sql").owns("db/seeds.sql"));
    }
}
