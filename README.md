# Agent Editor

A code editor in which agents from different vendors work on the same tasks.
The plan, the decisions and the design notes are in [PLAN.md](PLAN.md); the
drawn screens are in [design/](design/).

## What is here

| Folder | What it is |
|---|---|
| `crates/core` | The core: one process that starts agents, keeps sessions and does everything heavy. |
| `crates/mock-agent` | A practice agent. It speaks the agent protocol, calls no model and spends nothing. |
| `ui` | The interface: TypeScript and Solid. It talks to the core over one WebSocket. |
| `design` | The design mockups the interface is built from. |

## Run it

You need Rust 1.90 or newer and Node 22 or newer.

```bash
cd ui && npm install && npm run build && cd ..
```

```bash
cargo run -p ae-core -- --ui-dir ui/dist
```

Then open http://127.0.0.1:4477. Pick "Practice agent" to try it without any
account. Claude, Codex and Gemini start through `npx` and spend tokens on your
own accounts.

While working on the interface, run the core without `--ui-dir` and start the
dev server next to it:

```bash
cd ui && npm run dev
```

## Check it

```bash
cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
```

```bash
cd ui && npm run check
```

`npm run check` runs the type check, ESLint, the Prettier check and the unit tests.
The browser tests need the core and the interface built first:

```bash
cd ui && npm run e2e
```

Every automated test uses the practice agent, stand-ins for `gh`, `glab` and
`docker`, and a local stand-in for Linear, so nothing is spent and no real
account is touched. The test cases are listed in [docs/TEST-CASES.md](docs/TEST-CASES.md).

A local SonarQube run (needs Docker):

```bash
scripts/sonar-local.sh
```

## Services

- **GitHub and GitLab** are reached through their own tools, `gh` and `glab`.
  Sign in once with `gh auth login` or `glab auth login`; the core never sees a
  token. Which one a project uses is read from the address of its git remote.
- **Linear** needs a personal API key, pasted in the Issues section, and a team
  key per project.
- **Docker** is used by the Run tab when a project has a compose file.

## Where it stands

Milestones are listed in PLAN.md under "План разработки по вехам". Built so far:

- **M0, the skeleton.** A task is started from the first screen, the agent's
  answer streams in, tool calls and permission questions are shown and answered.
- **M1, tasks and projects.** Projects, tasks and every event are kept in
  SQLite. A task survives a restart of the core. In a git project each task
  works in its own working copy on its own branch.
- **M3, code.** Files open in an editor (CodeMirror) and are saved with
  Cmd/Ctrl+S. Lines are marked by who wrote them since the last review, the
  agent or you; "Keep" accepts the agent's lines, "Undo" takes them back and
  leaves yours. The agent is told what you changed by hand, and its turn stops
  when you start typing in the code. A real terminal runs in the task's folder.
  Cmd/Ctrl+K searches tasks, files, conversations, commands and the text of
  the code; with code selected it attaches that code to your next message.

- **M4, git.** The Changes view: what differs from the last commit, ticks on
  files and on single changes, commit, push, "add to the last commit", the
  branch's commits with Undo and Revert, updating from the base branch. A pull
  or merge request is opened from there, and "Merge now" merges it, deletes the
  branch and the working copy and archives the task.
- **Issues.** Open issues of every project from GitHub, GitLab or a Linear
  team; "Start" turns one into a task.
- **Run.** Dev scripts from `package.json` and services from a compose file,
  each task in a compose project of its own, with taken ports moved to free ones.

Not built yet: real agents have never been started (M2); points to go back to,
"since your last visit" and tasks that touch the same lines (M4); checks and
language servers (M5); teams, races and the Plan view (M6); Preview and Map
(M7); Health, Automations and usage (M8); the desktop shell (M9). Views that
are not built say so on screen.
