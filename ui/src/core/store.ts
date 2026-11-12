import { createSignal } from 'solid-js';
import { createStore, produce, reconcile } from 'solid-js/store';
import { core, type CoreEvent } from './client';

export type AgentDef = {
  id: string;
  name: string;
  short: string;
  vendor: string;
  available: boolean;
  note: string | null;
  spendsTokens: boolean;
};

export type Project = { id: string; name: string; path: string; folder: string | null; createdAt: number };

/** `asleep` means no agent process: the next message starts one. */
export type TaskState = 'idle' | 'running' | 'waiting_permission' | 'asleep';

export type TaskInfo = {
  id: string;
  projectId: string;
  title: string;
  agentId: string;
  cwd: string;
  branch: string | null;
  /** The branch the task's branch was started from. */
  baseBranch?: string | null;
  /** The issue this task was started from, if any. */
  issue?: TaskIssue | null;
  createdAt: number;
  seenSeq: number;
  state: TaskState;
};

export type TaskIssue = { key: string; id: string; url: string; title: string; source: 'github' | 'gitlab' | 'linear' };

export type PermissionOption = { optionId: string; name: string; kind: string };
export type Diff = { path: string; oldText: string; newText: string };

/** One thing shown in the agent pane, in the order it happened. */
export type FeedItem =
  | { kind: 'user'; text: string; edited: string[]; at: number }
  | { kind: 'edit'; path: string; at: number }
  | { kind: 'git'; action: string; subject: string; short: string; at: number }
  | { kind: 'tracker'; text: string; failed: boolean; at: number }
  | { kind: 'agent'; text: string; at: number }
  | { kind: 'thought'; text: string; at: number }
  | {
      kind: 'tool';
      id: string;
      title: string;
      toolKind: string;
      status: string;
      path: string | null;
      diff: Diff | null;
      at: number;
    }
  | { kind: 'plan'; entries: { content: string; status: string }[]; at: number }
  | {
      kind: 'permission';
      requestId: string;
      title: string;
      toolKind: string;
      diff: Diff | null;
      options: PermissionOption[];
      answered: boolean;
      chosen: string | null;
      at: number;
    }
  | { kind: 'notice'; text: string; tone: 'muted' | 'danger'; at: number };

/** Who wrote a stretch of lines since the last review. Lines count from 1. */
export type Run = { from: number; to: number; who: 'agent' | 'you' };

/** What is not as it was at the last review, for one file. */
export type FileReview = {
  path: string;
  runs: Run[];
  agentLines: number;
  yourLines: number;
  exists: boolean;
  existed: boolean;
};

/** A task as the interface holds it: what the core says plus the feed built from its events. */
export type Task = {
  info: TaskInfo;
  feed: FeedItem[];
  /** Number of the last event applied, so an event is never applied twice. */
  lastSeq: number;
  /** When the last event arrived, for "where you stopped". */
  lastAt: number;
  /** The files that are not as they were at the last review. */
  changed: FileReview[];
};

type AppInfo = { version: string; platform: string; cwd: string | null; dataDir: string };

const [state, setState] = createStore<{
  ready: boolean;
  app: AppInfo | null;
  agents: AgentDef[];
  projects: Project[];
  tasks: Record<string, Task>;
}>({ ready: false, app: null, agents: [], projects: [], tasks: {} });

export { state };

/** Which project Home shows; `null` means all of them. */
export const [projectFilter, setProjectFilter] = createSignal<string | null>(null);

function firstDiff(content: unknown): Diff | null {
  if (!Array.isArray(content)) return null;
  const found = content.find((block) => block && block.type === 'diff');
  if (!found) return null;
  return { path: String(found.path ?? ''), oldText: String(found.oldText ?? ''), newText: String(found.newText ?? '') };
}

/** A folder's path without the slashes at its end, whichever way they lean. */
function withoutTrailingSlashes(path: string): string {
  let end = path.length;
  while (end > 0 && (path[end - 1] === '/' || path[end - 1] === '\\')) end--;
  return path.slice(0, end);
}

/** Agents name files by their full path; inside a task the path from its folder reads better. */
export function inTask(cwd: string, path: string): string {
  const root = withoutTrailingSlashes(cwd);
  if (path.startsWith(root + '/') || path.startsWith(root + '\\')) {
    return path.slice(root.length + 1).replace(/\\/g, '/');
  }
  return path;
}

function firstPath(call: any, cwd: string): string | null {
  const location = Array.isArray(call?.locations) ? call.locations[0] : null;
  return location?.path ? inTask(cwd, String(location.path)) : null;
}

/** "Edit /long/path/to/src/a.ts" reads better as "Edit src/a.ts". */
function shortTitle(title: string, cwd: string): string {
  const root = withoutTrailingSlashes(cwd);
  return root
    ? title
        .split(root + '/')
        .join('')
        .split(root + '\\')
        .join('')
    : title;
}

function stopReasonText(reason: string): string | null {
  switch (reason) {
    case 'end_turn':
      return null;
    case 'cancelled':
      return 'You stopped this turn.';
    case 'max_tokens':
      return 'The agent ran out of room in its context.';
    case 'max_turn_requests':
      return 'The agent reached its limit of steps for one turn.';
    case 'refusal':
      return 'The agent refused to continue.';
    default:
      return `The turn ended: ${reason}.`;
  }
}

/** Applies one event from the core to a task. Events are applied in order, once. */
export function applyEvent(task: Task, event: any) {
  if (typeof event.seq === 'number') {
    if (event.seq <= task.lastSeq) return;
    task.lastSeq = event.seq;
  }
  const at: number = event.at ?? Date.now();
  task.lastAt = at;
  const feed = task.feed;
  const last = feed[feed.length - 1];

  switch (event.type) {
    case 'state':
      task.info.state = event.state;
      return;
    case 'agent_started':
      if (event.resumed) {
        feed.push({
          kind: 'notice',
          text: 'The agent was started again and told what was said before.',
          tone: 'muted',
          at,
        });
      }
      return;
    case 'user_message':
      feed.push({ kind: 'user', text: event.text, edited: Array.isArray(event.edited) ? event.edited : [], at });
      return;
    case 'user_edit':
      feed.push({ kind: 'edit', path: String(event.path), at });
      return;
    case 'tracker':
      feed.push({ kind: 'tracker', text: String(event.text ?? ''), failed: Boolean(event.failed), at });
      return;
    case 'git':
      feed.push({
        kind: 'git',
        action: String(event.action),
        subject: String(event.subject ?? ''),
        short: String(event.short ?? ''),
        at,
      });
      return;
    case 'update': {
      const update = event.update ?? {};
      switch (update.sessionUpdate) {
        case 'agent_message_chunk': {
          const text = update.content?.text ?? '';
          if (last?.kind === 'agent') last.text += text;
          else feed.push({ kind: 'agent', text, at });
          return;
        }
        case 'agent_thought_chunk': {
          const text = update.content?.text ?? '';
          if (last?.kind === 'thought') last.text += text;
          else feed.push({ kind: 'thought', text, at });
          return;
        }
        case 'tool_call':
          feed.push({
            kind: 'tool',
            id: String(update.toolCallId),
            title: shortTitle(String(update.title ?? 'Tool call'), task.info.cwd),
            toolKind: String(update.kind ?? 'other'),
            status: String(update.status ?? 'pending'),
            path: firstPath(update, task.info.cwd),
            diff: firstDiff(update.content),
            at,
          });
          return;
        case 'tool_call_update': {
          // The newest match wins: a restarted agent numbers its tool calls from one again.
          const item = feed.findLast((entry) => entry.kind === 'tool' && entry.id === update.toolCallId);
          if (item && item.kind === 'tool') {
            if (update.status) item.status = String(update.status);
            if (update.title) item.title = shortTitle(String(update.title), task.info.cwd);
            const diff = firstDiff(update.content);
            if (diff) item.diff = diff;
          }
          return;
        }
        case 'plan': {
          const entries = (update.entries ?? []).map((entry: any) => ({
            content: String(entry.content ?? ''),
            status: String(entry.status ?? 'pending'),
          }));
          // One plan per turn: a newer plan replaces the one shown.
          const lastUser = feed.map((entry) => entry.kind).lastIndexOf('user');
          const existing = feed.findIndex((entry, index) => index > lastUser && entry.kind === 'plan');
          if (existing >= 0) feed[existing] = { kind: 'plan', entries, at };
          else feed.push({ kind: 'plan', entries, at });
          return;
        }
        default:
          return;
      }
    }
    case 'permission_request': {
      const call = event.toolCall ?? {};
      feed.push({
        kind: 'permission',
        requestId: String(event.requestId),
        title: shortTitle(String(call.title ?? 'An action'), task.info.cwd),
        toolKind: String(call.kind ?? 'other'),
        diff: firstDiff(call.content),
        options: (event.options ?? []) as PermissionOption[],
        answered: false,
        chosen: null,
        at,
      });
      return;
    }
    case 'permission_resolved': {
      const item = feed.findLast((entry) => entry.kind === 'permission' && entry.requestId === event.requestId);
      if (item && item.kind === 'permission') {
        item.answered = true;
        item.chosen = event.optionId ?? null;
      }
      return;
    }
    case 'turn_end': {
      const text = stopReasonText(String(event.stopReason));
      if (text) feed.push({ kind: 'notice', text, tone: 'muted', at });
      return;
    }
    case 'error':
      feed.push({ kind: 'notice', text: String(event.message ?? 'The agent failed'), tone: 'danger', at });
      return;
    default:
      return;
  }
}

/**
 * Live events that arrived for a task whose history is still being fetched.
 * They are applied after the history, so nothing is skipped and nothing is shown twice.
 */
const waitingForHistory = new Map<string, any[]>();

function addTask(info: TaskInfo) {
  if (state.tasks[info.id]) return;
  waitingForHistory.set(info.id, []);
  setState('tasks', info.id, { info, feed: [], lastSeq: -1, lastAt: info.createdAt, changed: [] });
  void loadChanged(info.id);
}

/** Asks the core which files of a task are waiting for review. */
async function loadChanged(taskId: string) {
  try {
    const result = await core.request<{ files: FileReview[] }>('review.list', { taskId });
    if (state.tasks[taskId]) setState('tasks', taskId, 'changed', reconcile(result.files, { key: 'path' }));
  } catch {
    // The marks are a convenience; the task works without them.
  }
}

function applyTo(taskId: string, events: any[]) {
  if (!state.tasks[taskId]) return;
  setState(
    'tasks',
    taskId,
    produce((task: Task) => {
      for (const event of events) applyEvent(task, event);
    }),
  );
}

/**
 * Rebuilds a task's feed from its stored history.
 * `current` is the state the core reported before the history was asked for.
 * It is where the task starts from; the history is read later, so a state it
 * holds is newer and wins, and a live event that came after wins over both.
 * (The reported state must not be put on top of the history: a question the
 * agent asked in between would then be shown, but the task would not be
 * "waiting for you" until its next change of state.)
 */
async function loadHistory(taskId: string, current: TaskState) {
  if (!waitingForHistory.has(taskId)) waitingForHistory.set(taskId, []);
  const history = await core.request<{ events: any[] }>('tasks.history', { taskId });
  const live = waitingForHistory.get(taskId) ?? [];
  waitingForHistory.delete(taskId);
  if (!state.tasks[taskId]) return;
  setState(
    'tasks',
    taskId,
    produce((task: Task) => {
      task.feed = [];
      task.lastSeq = -1;
      task.info.state = current;
      for (const event of history.events) applyEvent(task, event);
      for (const event of live) applyEvent(task, event);
    }),
  );
}

async function reloadProjects() {
  setState('projects', reconcile(await core.request<Project[]>('projects.list')));
}

async function reload() {
  const [app, agents, projects, tasks] = await Promise.all([
    core.request<AppInfo>('app.info'),
    core.request<AgentDef[]>('agents.list'),
    core.request<Project[]>('projects.list'),
    core.request<TaskInfo[]>('tasks.list'),
  ]);
  setState({ app, agents });
  setState('projects', reconcile(projects));

  // Tasks that are gone from the core (archived elsewhere) leave the interface too.
  const alive = new Set(tasks.map((task) => task.id));
  for (const id of Object.keys(state.tasks)) {
    if (!alive.has(id)) setState('tasks', id, undefined!);
  }
  for (const info of tasks) {
    const known = Boolean(state.tasks[info.id]);
    addTask(info);
    setState('tasks', info.id, 'info', info);
    if (known) void loadChanged(info.id);
  }
  await Promise.all(tasks.map((info) => loadHistory(info.id, info.state)));
  setState('ready', true);
}

function onEvent(event: CoreEvent) {
  switch (event.event) {
    case 'link.open':
    case 'resync':
      void reload();
      return;
    case 'task.event': {
      const held = waitingForHistory.get(event.data.taskId);
      if (held) held.push(event.data);
      else applyTo(event.data.taskId, [event.data]);
      return;
    }
    case 'task.created': {
      const info = event.data as TaskInfo;
      if (state.tasks[info.id]) return;
      addTask(info);
      void loadHistory(info.id, info.state);
      return;
    }
    case 'task.archived':
      waitingForHistory.delete(event.data.taskId);
      setState('tasks', event.data.taskId, undefined!);
      return;
    case 'projects.changed':
      void reloadProjects();
      return;
    case 'review.changed':
      void loadChanged(event.data.taskId);
      return;
  }
}

/** Connects to the core and keeps the store in step with it. Call once. */
export function startStore() {
  core.onEvent(onEvent);
  core.start();
}

export function agentOf(task: Task): AgentDef | undefined {
  return state.agents.find((agent) => agent.id === task.info.agentId);
}

export function projectName(projectId: string): string {
  return state.projects.find((project) => project.id === projectId)?.name ?? 'project';
}

export function tasksNewestFirst(): Task[] {
  return Object.values(state.tasks).sort((a, b) => b.info.createdAt - a.info.createdAt);
}

export async function addProject(path: string): Promise<Project> {
  const project = await core.request<Project>('projects.add', { path });
  await reloadProjects();
  return project;
}

/** Creates a task, starts its agent and sends the first message. Returns the new task's id. */
export async function createTask(projectId: string, agentId: string, text: string): Promise<string> {
  const info = await core.request<TaskInfo>('tasks.create', { projectId, agentId, text });
  // The "task.created" event may already have added it and started loading; if not, do it here.
  if (!state.tasks[info.id]) {
    addTask(info);
    await loadHistory(info.id, info.state);
  }
  return info.id;
}

export function sendPrompt(taskId: string, text: string) {
  return core.request('tasks.prompt', { taskId, text });
}

export function answerPermission(taskId: string, requestId: string, optionId: string | null) {
  return core.request('tasks.permission', { taskId, requestId, optionId });
}

export function cancelTurn(taskId: string) {
  return core.request('tasks.cancel', { taskId });
}

export function stopAgent(taskId: string) {
  return core.request('tasks.stopAgent', { taskId });
}

export function archiveTask(taskId: string) {
  return core.request('tasks.archive', { taskId });
}

/** Tells the core how far the user has read in a task. */
export function markSeen(taskId: string, seq: number) {
  return core.request('tasks.seen', { taskId, seq });
}

export type FileEntry = { name: string; path: string; isDir: boolean };

export function listFiles(taskId: string, path: string) {
  return core.request<{ entries: FileEntry[] }>('files.list', { taskId, path });
}

export type FileText = { path: string; text: string; size: number; version: string };

/** Reads a file and says which of its lines are waiting for review. */
export function readFile(taskId: string, path: string) {
  return core.request<{ file: FileText; review: FileReview }>('files.read', { taskId, path });
}

/**
 * Saves a file. `version` is the one the text was loaded with: if the file
 * changed on disk since, the core refuses instead of overwriting it.
 */
export function writeFile(taskId: string, path: string, text: string, version: string | null) {
  return core.request<{ file: FileText; review: FileReview }>('files.write', { taskId, path, text, version });
}

export function keepFile(taskId: string, path: string) {
  return core.request<{ review: FileReview }>('review.keep', { taskId, path });
}

export function undoFile(taskId: string, path: string) {
  return core.request<{ review: FileReview }>('review.undo', { taskId, path });
}

export type FileHit = { path: string; positions: number[] };
export type TextHit = { path: string; line: number; column: number; length: number; text: string };
export type ChatHit = {
  taskId: string;
  who: 'you' | 'agent';
  at: number;
  column: number;
  length: number;
  text: string;
};

export function searchFiles(taskId: string, query: string, limit = 20) {
  return core.request<{ hits: FileHit[] }>('search.files', { taskId, query, limit });
}

export function searchText(taskId: string, query: string, limit = 100) {
  return core.request<{ hits: TextHit[] }>('search.text', { taskId, query, limit });
}

export function searchChats(query: string, limit = 20) {
  return core.request<{ hits: ChatHit[] }>('search.chats', { query, limit });
}

/** A file that differs from the last commit. */
export type GitFile = {
  path: string;
  kind: 'modified' | 'added' | 'deleted';
  added: number;
  removed: number;
  binary: boolean;
  /** How many separate changes the file has; each can be committed or reverted on its own. */
  changes: number;
};

export type GitCommit = { hash: string; short: string; subject: string; author: string; at: number; pushed: boolean };

export type GitState = {
  branch: string | null;
  base: string | null;
  baseAhead: number;
  remote: string | null;
  notPushed: number;
  files: GitFile[];
  commits: GitCommit[];
};

export type DiffLine = { kind: 'context' | 'add' | 'remove'; oldNo: number | null; newNo: number | null; text: string };
export type DiffHunk = {
  index: number;
  fromLine: number;
  toLine: number;
  lines: DiffLine[];
  who: 'agent' | 'you' | 'both' | null;
};
export type FileDiff = {
  path: string;
  kind: GitFile['kind'];
  binary: boolean;
  added: number;
  removed: number;
  hunks: DiffHunk[];
};

/** What of one file goes into a commit: all of it (`changes` left out) or some of its changes. */
export type Pick = { path: string; changes?: number[] };

export function gitState(taskId: string) {
  return core.request<GitState>('git.state', { taskId });
}

export function gitDiff(taskId: string, path: string) {
  return core.request<FileDiff>('git.diff', { taskId, path });
}

export function gitCommit(taskId: string, message: string, picks: Pick[], options: { amend: boolean; push: boolean }) {
  return core.request<GitState>('git.commit', { taskId, message, picks, ...options });
}

export function gitPush(taskId: string) {
  return core.request('git.push', { taskId });
}

export function gitUpdate(taskId: string) {
  return core.request('git.update', { taskId });
}

export function gitUndoCommit(taskId: string, hash: string) {
  return core.request('git.undoCommit', { taskId, hash });
}

export function gitRevertCommit(taskId: string, hash: string) {
  return core.request('git.revertCommit', { taskId, hash });
}

/** Puts a file, or one change of it, back as it is in the last commit. */
export function gitRevert(taskId: string, path: string, change?: number) {
  return core.request('git.revert', { taskId, path, change });
}

// ----- services: code hosts, trackers, Docker -----

/** Whether a command-line tool is there and signed in (for Docker: whether its engine runs). */
export type ToolState = { installed: boolean; signedIn: boolean; hint: string | null };
export type Integrations = { github: ToolState; gitlab: ToolState; linear: { hasKey: boolean }; docker: ToolState };

export type Issue = {
  key: string;
  id: string;
  title: string;
  url: string;
  state: string;
  body: string;
  labels: string[];
  assignee: string | null;
  updatedAt: number;
  /** The open task that works on this issue, if there is one. */
  taskId: string | null;
};

export type ProjectIssues = {
  projectId: string;
  source: 'github' | 'gitlab' | 'linear' | 'none';
  label: string;
  linearTeam: string | null;
  issues: Issue[];
  problem: string | null;
};

export function integrationsStatus() {
  return core.request<Integrations>('integrations.status');
}

export function setLinearKey(key: string | null) {
  return core.request('integrations.setLinearKey', { key });
}

export function setLinearTeam(projectId: string, team: string | null) {
  return core.request('integrations.setLinearTeam', { projectId, team });
}

export function listIssues() {
  return core.request<{ projects: ProjectIssues[] }>('issues.list');
}

/** Starts a task for an issue. Returns the new task's id. */
export async function startIssue(projectId: string, key: string, agentId: string): Promise<string> {
  const info = await core.request<TaskInfo>('issues.start', { projectId, key, agentId });
  if (!state.tasks[info.id]) {
    addTask(info);
    await loadHistory(info.id, info.state);
  }
  return info.id;
}

export type Checks = {
  state: 'none' | 'pending' | 'passed' | 'failed';
  passed: number;
  failed: number;
  pending: number;
};

/** A pull request or merge request. */
export type MergeRequest = {
  number: number;
  title: string;
  url: string;
  state: 'open' | 'merged' | 'closed';
  draft: boolean;
  mergeable: boolean | null;
  checks: Checks;
  review: 'approved' | 'changes_requested' | 'review_required' | 'none';
};

export type RequestInfo = {
  host: { kind: 'github' | 'gitlab'; name: string; word: string; slug: string } | null;
  request: MergeRequest | null;
  ready?: boolean;
  defaults?: { title: string; body: string; base: string | null };
};

export function getRequest(taskId: string, fresh = false) {
  return core.request<RequestInfo>('mr.get', { taskId, fresh });
}

export function createRequest(taskId: string, title: string, body: string) {
  return core.request<{ request: MergeRequest }>('mr.create', { taskId, title, body });
}

export function mergeRequest(taskId: string, options: { squash: boolean; deleteBranch: boolean; archive: boolean }) {
  return core.request<{ request: MergeRequest }>('mr.merge', { taskId, ...options });
}

export function requestsOverview() {
  return core.request<{ requests: { taskId: string; ready: boolean; request: MergeRequest }[] }>('mr.overview');
}

/** Removes a merged task's working copy and branch and takes it off the lists. */
export function cleanUpTask(taskId: string) {
  return core.request('tasks.cleanUp', { taskId });
}

export type RunService = {
  name: string;
  kind: 'command' | 'docker';
  runs: string;
  state: 'stopped' | 'starting' | 'running' | 'failed' | 'exited';
  address: string | null;
  note: string | null;
  termId: number | null;
};

export type RunState = {
  services: RunService[];
  composeFile: string | null;
  composeProject: string;
  dockerReady: boolean;
  dockerHint: string | null;
  problem: string | null;
};

export function runState(taskId: string) {
  return core.request<RunState>('run.state', { taskId });
}

export function runStart(taskId: string, service?: string) {
  return core.request<RunState>('run.start', { taskId, service });
}

export function runStop(taskId: string, service?: string) {
  return core.request<RunState>('run.stop', { taskId, service });
}

export function runLogs(taskId: string, service: string) {
  return core.request<{ termId: number }>('run.logs', { taskId, service });
}
