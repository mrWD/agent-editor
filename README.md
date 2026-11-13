# Agent Editor

A code editor in which agents from different vendors work on the same tasks.
The plan, the decisions and the design notes are in [PLAN.md](PLAN.md); the
drawn screens are in [design/](design/).

## What is here

| Folder | What it is |
|---|---|
| `crates/core` | The core: one process that starts agents, keeps sessions and does everything heavy. |
| `crates/mock-agent` | A practice agent. It speaks the agent protocol, calls no model and spends nothing. |
| `crates/shell` | The desktop app: a window (Tauri 2) with the core running inside it. A workspace of its own. |
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

## The desktop app

`crates/shell` puts the same core and the same interface into an app for macOS,
Windows and Linux. The core runs inside the app's own process, on a free port
of this computer; the window shows what the core serves. Quitting the app stops
every agent and terminal. On a Mac, closing the window does not quit: the
agents go on working, and the Dock icon brings the window back.

Besides Rust and Node, Linux needs the libraries Tauri lists (WebKitGTK 4.1 and
a few more; the `shell` job in `.github/workflows/ci.yml` installs them).

```bash
cd ui && npm ci && cd ../crates/shell && npm ci
```

```bash
npm run dev      # builds the interface and the practice agent, then opens the app
npm run build    # the bundles for this system, in crates/shell/target/release/bundle
```

`npm run build` makes a `.app` and a `.dmg` on macOS, an installer on Windows,
a `.deb` and an `.AppImage` on Linux. Nothing is signed. If you have Tauri's
own tool installed, `cargo tauri dev` and `cargo tauri build` do the same.

The app keeps its data where `ae-core` keeps it by default, so both see the
same projects and tasks; do not run both on the same data at once. Another
folder: `--data-dir <folder>` or `AE_DATA_DIR`. To show a built interface from
disk instead of the one inside the app: `--ui-dir ../../ui/dist` or `AE_UI_DIR`.

The app is not a member of the main workspace, because Tauri needs system
libraries the core does not. Its own checks run in its folder, after
`node prepare.mjs` has put the interface and the practice agent in place:

```bash
cd crates/shell && node prepare.mjs && cargo fmt -- --check && cargo clippy --all-targets -- -D warnings && cargo test
```

The icon is drawn in `crates/shell/app-icon.svg`; `npx tauri icon app-icon.png`
makes the sizes in `icons/` from its 1024-pixel picture.

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
- **M9, the desktop app.** The core and the interface in one app (Tauri 2) for
  macOS, Windows and Linux: a folder dialog for "Another folder", links that
  open in the system's browser, a window that remembers its size and place, a
  menu, one running copy at a time. Not signed, no auto-update.

Not built yet: real agents have never been started (M2); points to go back to,
"since your last visit" and tasks that touch the same lines (M4); checks and
language servers (M5); teams, races and the Plan view (M6); Preview and Map
(M7); Health, Automations and usage (M8). Views that are not built say so on
screen.
