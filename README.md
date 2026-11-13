# Agent Editor

A code editor in which agents from different vendors work on the same tasks.
The plan, the decisions and the design notes are in [PLAN.md](PLAN.md); the
drawn screens are in [design/](design/).

## What is here

| Folder              | What it is                                                                                   |
| ------------------- | -------------------------------------------------------------------------------------------- |
| `crates/core`       | The core: one process that starts agents, keeps sessions and does everything heavy.          |
| `crates/mock-agent` | A practice agent. It speaks the agent protocol, calls no model and spends nothing.           |
| `crates/shell`      | The desktop app: a window (Tauri 2) with the core running inside it. A workspace of its own. |
| `ui`                | The interface: TypeScript and Solid. It talks to the core over one WebSocket.                |
| `design`            | The design mockups the interface is built from.                                              |

## Run it

You need Rust 1.90 or newer and Node 22 or newer.

**No Rust yet?** `cargo --version` fails with "command not found" when there
is none, or `rustup` has it but the current shell has not picked it up yet.
Install it, then open a new terminal window (not just a new tab) so the
shell sees it:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

**No Node yet?** `node --version` fails the same way. Get it from
[nodejs.org](https://nodejs.org) or, if you use a version manager such as
nvm, `nvm install 22`.

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

Besides Rust and Node (see "Run it" above if you have neither yet), Linux
needs the libraries Tauri lists (WebKitGTK 4.1 and a few more; the `shell`
job in `.github/workflows/ci.yml` installs them).

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
The browser tests need the core and the interface built first; they include a
pass of axe over every screen, so a control without a name or text that does
not stand out from its background fails the build:

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

The first time, an agent can do this for you: **Set up with an agent** on the
home screen starts Claude or Codex with what you say your team uses (for
example "Linear team APP, our own GitLab at gitlab.example.com"). It follows
[docs/AGENT-SETUP.md](docs/AGENT-SETUP.md) through the editor's own tools:
it reads where things stand, adds your repositories as projects and picks each
project's Linear team. Signing in to `gh`, `glab` and Linear stays with you;
the agent names the exact command or screen. It changes no code.

## Where it stands

Milestones are listed in PLAN.md under "План разработки по вехам"; the state
in detail is in the same file under "Состояние на 13.11.2026". Built so far:

- **M0, the skeleton.** A task is started from the first screen, the agent's
  answer streams in, tool calls and permission questions are shown and answered.
- **M1, tasks and projects.** Projects, tasks and every event are kept in
  SQLite. A task survives a restart of the core. In a git project each task
  works in its own working copy on its own branch. The rail at the bottom of
  every screen shows every project and the state of each task; projects go
  into folders, and the ones that do not fit fold into one item, never one
  that needs you.
- **M2, agents.** The model, the mode and the agent's other settings are in the
  head of the conversation and are remembered. An agent that can load a session
  picks its own up after sleep or a restart; one that cannot is told what was
  said. The head shows how full the context is and what the task has cost.
  Answers in Markdown are shown as meant. A picture is attached, pasted or
  dropped into a message and goes to agents that take images. Checked with the
  real Claude on its cheapest model; Codex and Gemini have not been started.
- **M3, code.** Files open in an editor (CodeMirror) and are saved with
  Cmd/Ctrl+S. The tree draws what each entry is: a folder open or closed, and
  a file by what is written in it, coloured by its language. Lines are marked
  by who wrote them since the last review, the agent or you; "Keep" accepts
  the agent's lines, "Undo" takes them back and leaves yours. The agent is told what you changed by hand, and its turn stops
  when you start typing in the code. Files and folders are made, renamed and
  deleted from the tree; a deleted one can be put back. A real terminal runs in
  the task's folder. Cmd/Ctrl+K searches tasks, files, conversations, commands
  and the text of the code; with code selected it attaches that code to your
  next message.
- **M4, git.** The Changes view: what differs from the last commit, ticks on
  files and on single changes, commit, push, "add to the last commit", the
  branch's commits with Undo and Revert, updating from the base branch. A pull
  or merge request is opened from there with its review comments, which go to
  the agent or get a reply; "Merge now" merges it, deletes the branch and the
  working copy and archives the task, and "Merge by itself" does that once the
  checks are green. A checkpoint is taken before every message: "Look back"
  shows any earlier moment and rewinds to it, keeping your own lines. Coming
  back to a task says where you stopped and what happened since. Two open
  tasks that change the same lines are shown to each other.
- **M5, checks and running.** The Checks view runs the project's own lint,
  tests and build, reads the places they name and stands in the way of a merge
  when one fails. Problems lists them. Language servers give completions,
  problems, "go to definition" and formatting on save. The Run tab starts dev
  scripts from `package.json` and services from a compose file, each task in a
  compose project of its own, with taken ports moved to free ones. Launch
  profiles say what starts together; a task has its own values over the
  project's `.env`, with secrets masked; missing packages are installed first;
  a database keeps its data in a volume of the task and can be reset; what
  other tasks run is listed with its memory and can be stopped; Shell opens a
  terminal inside a running container.
- **M6, together.** A team: a planner, a builder and a reviewer take one task
  in turn, then the checks run. A race: the same task goes to two or more
  agents, each in a working copy of its own, and one result is kept. The Plan
  view: the work as blocks the agent keeps to, joined by dragging, each
  built block keeping the very lines it was built in and the test that
  proves it. Rules of a project are told to
  every agent, and rewriting an agent's line offers to make a rule of it. The
  core is a tool server for its agents: they see the other tasks, leave notes
  for the next agent and report what they noticed.
- **M7, Preview and Map.** Preview shows the running app at several widths at
  once, in light and dark, lets you pick an element or draw on the page, and
  sends that to the agent in words. A screenshot of each width goes into the
  message (it needs Chrome, Chromium or Edge on this computer). The iOS
  simulator and the Android emulator each have a tab that opens the app there
  and shows the screen (they need Xcode and the Android tools). Map shows the files the task changed, what
  they define, who needs them and what they call, and draws a flow the agent
  describes.
- **M8, the sections.** Issues from GitHub, GitLab or a Linear team; "Start"
  turns one into a task. Agents: who is doing what, cost by agent and by task,
  limits, a daily budget, handing a task to another agent. Automations: clean
  up after a merge, run checks, start a task every day at a time. Health: what
  each project is missing, read from its files. Settings: the agents and their
  commands, an agent of your own, the one a new task starts with, keeping the
  computer awake while an agent works. Notifications for a question, the end
  of a turn, failed checks and a request that became ready to merge, told once
  and only while no window of the editor is in front.
- **M9, the desktop app.** The core and the interface in one app (Tauri 2) for
  macOS, Windows and Linux: a folder dialog for "Another folder", links that
  open in the system's browser, a window that remembers its size and place, a
  menu, one running copy at a time. Not signed, no auto-update.

Not built: a way to run, in a terminal, an agent that does not speak ACP.

Not checked: anything on Windows or Linux (the CI is written and has never
run); GitLab and Linear against the real services; Codex and Gemini; the
real language servers for Rust, Go and Python (the one for TypeScript and
JavaScript was tried by hand; the tests use a practice server); the
screenshot with a real browser, the real iOS simulator and Android emulator,
and a notification of the real system (the tests use stand-ins for each).
