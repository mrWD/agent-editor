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
`docker`, for the browser that takes screenshots, for the device tools
(`xcrun`, `adb`, `emulator`, `idb`) and for `terraform` and `helm`, a local
stand-in for Linear, and a home folder of its own in which the "conversations
of Claude Code" are written by the test, so nothing is spent, no real account
is touched and no real device is started. The test cases are listed in [docs/TEST-CASES.md](docs/TEST-CASES.md).

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
- **An issue or a request in full.** On the Issues screen an issue in work
  opens three ways: "Open" goes to its task with the code, its title opens
  the issue in a panel at the side, and the name of its request ("PR #128",
  "MR !48") opens the request in a panel of its own. Both panels can be open
  at once, beside the list. Each has in its head a button that opens it in
  the browser. The panel of an issue shows the description, who wrote it and
  who has it, the labels, what is attached, and what was written under it,
  oldest first. The panel of a request has a tab for each part: what it
  says, the comments (with the ones on lines that are not settled), what it
  changes file by file, and what stands before its merge with the pipeline
  of its branch. "Details" on the card of a request in Changes opens the
  same panel. A picture in a
  description is kept by the tracker for signed-in people only, so the core
  fetches it with your own sign-in (`gh`, `glab`, the Linear key) and hands
  it on. It does that only for the tracker's own places for uploads, and only
  when what comes back is a picture. Nothing is written: no comment is added
  from here.
- **Docker** is used by the Run tab when a project has a compose file.
- **Sentry** needs an auth token that may read events, projects and the
  organization, pasted on the Issues screen with the organization's name
  (and the address of your own Sentry, if it is not sentry.io). Each project
  is then joined to one of Sentry's projects. Its unresolved errors are
  listed under the tracker's issues, and "Start" makes a task whose first
  message is the error with its numbers and the stack trace of its newest
  event. Sentry is only read; it resolves an issue itself when a commit that
  says "Fixes SHOP-1A" reaches it, which is what the request of such a task
  says.
- **SonarQube or SonarCloud** needs its address and a token, given on the
  Health screen. Which of its projects a project is, is read from
  `sonar-project.properties` or given there. "Check now" then also reads the
  quality gate, the worst open findings and the places to review, and Health
  shows them with the rest; "Fix" starts a task with the rule, the file and
  the line of each.
- **Tool servers** (Model Context Protocol) are listed in Settings: a program
  an agent starts, or an address over HTTP or SSE, with the key each needs.
  Every agent that speaks the protocol is named them when its session opens.
  Sentry, Figma, Notion, Linear, Jira and Confluence, Slack, GitHub, Grafana,
  a browser and library documentation are ready to be filled in. The editor
  calls none of them itself.
- **Where a branch is deployed** is asked of the code host: GitHub's
  deployments (Vercel, Netlify and the like make them) and GitLab's
  environments and deployments (review apps). Preview shows them as links at
  its head.

The first time, an agent can do this for you: **Set up with an agent** on the
home screen starts Claude or Codex with what you say your team uses (for
example "Linear team APP, our own GitLab at gitlab.example.com"). It follows
[docs/AGENT-SETUP.md](docs/AGENT-SETUP.md) through the editor's own tools:
it reads where things stand, adds your repositories as projects and picks each
project's Linear team. Signing in to `gh`, `glab` and Linear stays with you;
the agent names the exact command or screen. It changes no code.

## A service in Preview

Preview shows a page at several widths and an app on a simulator. A service
that answers in data has nothing to show that way, so Preview has three more
things to look at, next to Web and the two devices.

- **API.** The operations of the service, read from its OpenAPI or Swagger
  description: a file of the project (`openapi.yaml`, `docs/openapi.json` and
  the like), or what the running service gives at `/openapi.json` and the other
  paths frameworks serve it under. A request is filled in from the description
  and sent to the task's own service on this computer; the answer is shown
  with its status, the time it took and its size. Headers kept for the project
  (a key, usually) go with every request. **Contract** compares the
  description with the one of the base: the same file where the branch
  started, or a service that runs the base branch, whose address you give
  under Set up. Each difference is weighed by what it does to a client written
  against the base: it breaks clients, it is worth a look, or it is safe. The
  operations the task touched come first in the list.
- **Database.** The migrations the task added or changed, read from their
  text and said step by step, with the steps that can lose data or break the
  code that still runs marked and said why. Alembic, Django, Prisma, Rails,
  Laravel, Knex, Diesel, Ecto, golang-migrate, Drizzle and plain SQL files are
  known. **Try them** runs the migrations on the task's own database, takes
  back the ones the task added and runs them again, and says what they did to
  the tables and whether the way back left the tables as they were. The
  commands run with the task's environment, so they change the database the
  project's own settings point at. The trial is asked for twice, and refused
  on a database other tasks may use and when the task's environment points
  at a database on another computer. Under that are the tables,
  and a query that is run for reading only until you allow changes. A
  database is reached as a service of the task's compose project (through the
  client in its own container, so nothing is installed and no password is
  handled), by the address in the task's environment (`DATABASE_URL`, with
  `psql` or `mysql` of this computer), or as a file of SQLite.
- **Jobs.** Celery tasks, Dramatiq actors, Django and Laravel commands,
  Sidekiq workers, Active Job classes and package scripts named like jobs,
  found by reading the files, the ones the task touched first. One is started
  with its arguments (JSON for a queue, the rest of a command line for a
  command) and what it prints is shown as it comes. Each kind has a command
  line the project can replace, for workers that live in a container.

An answer, the rows of a query, a trial and a run can each be **shown to the
agent**: the text goes along with your next message, with what you say is
wrong with it. The values of the headers kept for the project and the secrets
of the task's environment are left out.

The Run tab starts such a service too. Beside the scripts of `package.json`
it offers the lines of a `Procfile`, and for a project that names nothing
itself the command of its framework: Django, FastAPI, Flask, Rails, Laravel,
a Go program, and a Celery worker where an app is made. `PORT` among the
task's own values moves the port, so two tasks can run side by side.

## Infrastructure in Preview

A change to a Terraform file or a manifest does nothing a page or a request
can show: it takes effect when somebody applies it. For a project that keeps
its infrastructure in its repository, Preview has a seventh thing to look at,
**Infra**. It is offered when the project has any of these files:

- **Terraform**: `*.tf` and the values in `*.tfvars`;
- **Helm**: a `Chart.yaml`, with the values and the templates next to it;
- **Kubernetes**: any other YAML that has an `apiVersion` and a `kind`;
- **Docker**: Dockerfiles and compose files;
- **CI**: `.gitlab-ci.yml` and the workflows under `.github/workflows`.

Infra lists the infrastructure files the task touched since its branch left
the base, grouped by tool, and says what each change does. It reads the files
as they were and as they are; it runs nothing for that. Terraform resources,
modules and outputs are told added, changed (with the arguments that differ)
and removed by their blocks. Kubernetes objects are told by kind and name,
with the fields a deploy turns on marked: image, replicas, resources, env and
ports. Of a Dockerfile, the base images and the exposed ports are said; of a
compose file, its services; of a pipeline, the jobs added, changed and
removed; of a chart, its versions and the values that differ.

What deserves a second look is marked and said why, the way dangerous
migration steps are: a resource, an object, a service or a job that is
removed; a volume or something that looks like a database touched; a
privileged container, or one that shares the node's network or files; a
secret written in plain text. A secret is named and its value is never
repeated; one that is kept encrypted (a sealed secret, a file of SOPS) or
pointed at (a variable, a reference to a vault) is not marked. What only
moved is said to have moved, not to be removed: a block that went from one
Terraform file to another of the same folder, and a file that went elsewhere
with nothing in it changed. A file that stopped being YAML is said to be
unreadable, not read as empty. The reading is by eye, not by the tools' own parsers: what it does
not recognise it leaves out, and it does not know what a provider would
replace rather than update.

**Show it to the agent** puts the whole reading before the agent with your
next message.

Under the reading are the checks, for the folders the task touched first:
`terraform validate` and `terraform plan -lock=false -refresh=false
-input=false` for each folder of Terraform files, `helm template` for each
chart. A check runs only when you press it, in a terminal of the task, and
only when its tool is on this computer; its output is shown as it comes and
can be shown to the agent. Nothing here ever applies, installs or upgrades
anything. A plan is asked not to lock or refresh the state, but a provider
may still want the credentials of its cloud: they are the ones of the task's
environment, as for anything else the task runs.

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
  Every pane at a side or at the bottom has an edge to drag: the files, the
  conversation, the box a message is typed in (by its top edge), the panel
  with the terminal, the open projects panel, the column of Changes, the
  lists of the Preview and the panels of an issue and a request. An arrow key
  moves an edge the way it points, a double click puts the part back as it
  was, and the sizes are kept in the browser.
  The conversation is folded the way it is read. A turn that was answered
  since shows what was asked and the agent's last word, with all the work
  between as one line ("Worked for 1h 6m · 100 steps, 28 messages") that
  opens when pressed. The latest turn shows what the agent says along the
  way, with each run of three steps or more as one line ("Ran 8 commands,
  read 3 files"); while the agent works, the line says what it is at. A
  long command is its first line until it is pressed. What the agent
  thought is inside the folds. A question that waits for you is never
  folded. **All steps** in the head shows everything one under another.
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
  checks are green. Under the request stands the pipeline of the branch
  (GitLab's, or the runs of GitHub Actions for the newest commit), stage by
  stage: what failed, runs or waits is listed, what went well is folded away.
  A failed job is run again with one press; starting a job that waits,
  stopping, running everything again and starting a new pipeline take two. A
  checkpoint is taken before every message: "Look back"
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
  simulator and the Android emulator each have a tab that lists the devices,
  starts one that is off, opens the app there and shows its screen as a
  picture taken again every second. A press on the picture is a tap and a
  drag is a swipe; under it are the device's buttons, a field that types on
  it, and Light and Dark. A device the editor started is shut down when the
  tab is left; one that ran already is left alone. They need Xcode and the
  Android tools (`adb`, and `emulator` to start one that is off); tapping a
  simulator also needs `idb`, and without it stays in the Simulator window.
  Map shows the files the task changed, what
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

A conversation begun in Claude Code can be gone on with here: "Bring in a
conversation from Claude Code" on the home screen lists the conversations
Claude Code had in a project's folder (read from its own files under
`~/.claude`, which are never changed) and makes a task of one, with its words
and tool calls, in the folder it worked in. The task sleeps; the next message
wakes Claude with the session it had.

The editor is dark, as it was designed, and has a light theme: Settings, "The
editor", Theme: Dark, Light, or "As the computer", which follows the system and
changes with it. The code, the terminal and the marks of files change with
everything else. The choice is kept in the browser, so the app and each
browser have their own.

The first time the editor is opened it takes the user through three chapters:
trackers, agents, code. Each is a dialog that connects something, and then
hints on the interface itself: one thing on screen is ringed, a card beside it
says what it is, and "Next" and "Back" move between the hints. Where a hint
says so, pressing the ringed thing is the way on. The last hint of a chapter
opens the next chapter's dialog; the last of all leaves the user in the editor.
A chapter in which nothing was connected has nothing to point at and goes
straight on. Trackers: Linear, with a key that is checked. Agents: the ones
the editor comes with; command-line agents found on this computer, from the
list Orca knows (Claude Code, Codex, Cursor, Antigravity, Gemini, Copilot,
OpenCode, Qwen Code, Kimi, Qoder, CodeBuddy, Trae and more), each added as an
agent in a terminal with one press; agents of the ACP registry (Cursor,
Copilot, Qwen Code, Kimi Code, Qoder, CodeBuddy, MiniMax Code, GLM and
others); agents with an API key; Claude's agent pointed at DeepSeek, Kimi,
GLM, Qwen, MiniMax or OpenRouter; and a local or self-hosted model. The editor
calls no model itself, so a key or a model of another maker is always given
to an agent, in its environment. Code: GitHub, GitLab and Docker, with what is
connected and how to connect the rest, and a folder that becomes a project.
Escape puts the first run aside, and Settings opens it again. Of the
ready-made agents only Claude Code in a terminal was tried against the real
thing; the others are written from the registry and the makers' documentation.

An agent that does not speak ACP runs in a terminal instead. Add it in
Settings as an agent of your own and tick "It runs in a terminal". A task then
shows that terminal in place of the conversation; what you send is typed into
the program, and you can type into it yourself. Write `{message}` among the
arguments where the program takes a first message (`claude --model haiku
{message}`): it is then started with yours. Without that, the first message
is typed, and a program that opens with a question of its own, as Claude does
when it asks whether to trust a new folder, takes the Enter for its answer. The editor cannot read what
such a program means, so a turn is taken to be over when the program has
printed nothing for a moment (2.5 seconds), and "your turn" may also mean that
it is asking you something. Line marks, checkpoints, Changes and checks work
as with any agent. Such a program races like any agent and builds for a team;
it cannot plan or review for one, because the editor cannot read its answer.
There are no tool calls in the conversation, no
questions to answer there, no settings in the head, no pictures, no cost, and
none of the editor's own tools. A program started again is told what was said
before as one long message.

Not checked: anything on Windows or Linux (the CI is written and has never
run); GitLab and Linear against the real services; Codex and Gemini; the
real language servers for Rust, Go and Python (the one for TypeScript and
JavaScript was tried by hand; the tests use a practice server); the
screenshot with a real browser, the real iOS simulator and Android emulator
(this Mac has neither Xcode nor the Android tools; starting an emulator,
touch through `adb` and `idb`, and light and dark are written from the tools'
documentation), and a notification of the real system (the tests use
stand-ins for each); Sentry and SonarQube against the real services (each
is tested against a stand-in server that answers in the documented shapes);
a tool server with a real agent (the practice agent says back what it was
given); GitHub's deployments against the real GitHub (GitLab's were read
from a real project); acting on a real pipeline (reading one was tried
against a GitLab pipeline of fifty jobs; no job was started or stopped
there); waking the real Claude in a conversation that was brought in; the
command-line programs of Codex and Gemini as agents in a terminal (Claude's
was tried by hand: started with a message, its question about the folder
answered in the terminal, a second message of two lines typed into it); the
views for a service against a real project in FastAPI, Django or Rails (the
API view is tested against a server of the tests' own that describes itself
as FastAPI does; the reading of migrations and the finding of jobs, on files
written for the tests; a job was never put on a real Celery queue); a
database in a real container (Docker is off on this Mac; the tests use a
stand-in that answers as `psql` in a container would) and MySQL at all (its
side is written from the client's documentation). PostgreSQL through `psql`
of this computer and SQLite were tried for real: tables, rows, the refusal
to write, and a trial of migrations there and back. The checks of Infra were
never run with a real Terraform or Helm (this Mac has neither; the tests use
stand-ins that say what the real ones might), and its reading was tried only
on files written for the tests, not on the infrastructure of a real project.
