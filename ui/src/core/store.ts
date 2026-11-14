import { createSignal } from 'solid-js';
import { createStore, produce, reconcile } from 'solid-js/store';
import { core, type CoreEvent } from './client';
import type { ClaudeSession } from './imports';
import { forget } from './kept';
import { setWork } from './work';

export type AgentDef = {
  id: string;
  name: string;
  short: string;
  vendor: string;
  available: boolean;
  note: string | null;
  spendsTokens: boolean;
  /**
   * How the editor talks to it: the Agent Client Protocol, or not at all. An
   * agent that speaks no protocol runs in a terminal, and messages are typed into it.
   */
  speaks?: 'acp' | 'terminal';
  /** True for the agent picked in Settings as the one a new task starts with. */
  isDefault?: boolean;
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
  /** What the running agent offers to set. Absent while no agent is running. */
  config?: TaskConfig | null;
  /** The race this task is an entrant of, until one result is picked. */
  race?: RaceRef | null;
};

/** A race as its entrants know it: its id and every task it started with, in the order of their agents. */
export type RaceRef = { id: string; tasks: string[] };

/** One setting an agent offers for a session, such as its model or its mode. */
export type ConfigOption = {
  id: string;
  name: string;
  description?: string | null;
  /** "mode", "model", "thought_level" and the like; agents may leave it out. */
  category?: string | null;
  currentValue: string;
  options: { value: string; name: string; description?: string | null }[];
};

export type TaskConfig = {
  options: ConfigOption[];
  commands: { name: string; description?: string }[];
  /** True when the running agent takes pictures in a message. */
  images?: boolean;
  /** The terminal the agent runs in, for one that speaks no protocol. */
  terminal?: number | null;
};

/** A picture of a message, as the conversation names it. Its bytes are asked for by `pictureUrl`. */
export type PictureName = { id: string; name: string; mimeType: string; size: number };
/** A picture on its way to the agent: its bytes in base64. */
export type PictureToSend = { name: string; mimeType: string; data: string };

/** How full the agent's context is and what the session has cost, as the agent reports it. */
export type Usage = { used: number; size: number; cost: number | null; currency: string };

export type TaskIssue = { key: string; id: string; url: string; title: string; source: 'github' | 'gitlab' | 'linear' };

export type PermissionOption = { optionId: string; name: string; kind: string };
export type Diff = { path: string; oldText: string; newText: string };

/** One thing shown in the agent pane, in the order it happened. */
export type FeedItem =
  | { kind: 'user'; text: string; edited: string[]; images: PictureName[]; at: number }
  | { kind: 'edit'; path: string; at: number }
  /** A block of the plan was added by the user or built by an agent. */
  | {
      kind: 'block';
      action: 'added' | 'built';
      blockId: string;
      what: 'block' | 'note';
      text: string;
      by: string;
      at: number;
    }
  | {
      /** The user made, renamed or deleted a file in the tree. */
      kind: 'file';
      action: 'created' | 'renamed' | 'deleted' | 'restored';
      path: string;
      to: string | null;
      /** For a deletion: the name it is kept under, until it is put back. */
      keptAs: string | null;
      at: number;
    }
  | {
      /** What one agent of a team handed to the next, carried by the editor. */
      kind: 'note';
      role: string;
      from: string;
      to: string;
      text: string;
      at: number;
    }
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
  | { kind: 'notice'; text: string; tone: 'muted' | 'danger'; at: number }
  | {
      /** The code was put back as it was at a checkpoint. `undo` is the checkpoint that takes this back. */
      kind: 'rewound';
      label: string;
      toAt: number;
      files: number;
      kept: string[];
      droppedMessages: boolean;
      undo: number;
      at: number;
    };

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
  /** The newest note on the context, or `null` before the agent has sent one. */
  usage: Usage | null;
  /** What the task cost in agent sessions that have ended. The running one is in `usage`. */
  spentBefore: number;
};

/** What a task has cost so far, in every session it has had. */
export function spentOn(task: Task): number {
  return task.spentBefore + (task.usage?.cost ?? 0);
}

type AppInfo = {
  version: string;
  platform: string;
  cwd: string | null;
  dataDir: string;
  /** False until the user has been through the first run, or has put it aside. */
  onboarded?: boolean;
};

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
      // Settings belong to a running agent: a sleeping task has none to show.
      if (event.state === 'asleep') task.info.config = null;
      return;
    case 'agent_started':
      if (event.resumed) {
        feed.push({
          kind: 'notice',
          text: event.loaded
            ? 'The agent was started again and picked its session up where it stopped.'
            : 'The agent was started again and told what was said before.',
          tone: 'muted',
          at,
        });
      }
      return;
    case 'agent_switched':
      task.info.agentId = String(event.to);
      // The new agent starts with an empty context and its own bill.
      task.spentBefore += task.usage?.cost ?? 0;
      task.usage = null;
      feed.push({
        kind: 'notice',
        text: `The task was handed from ${event.fromName ?? event.from} to ${event.toName ?? event.to}. With your next message ${event.toName ?? event.to} is told what was said so far.`,
        tone: 'muted',
        at,
      });
      return;
    case 'user_message':
      feed.push({
        kind: 'user',
        text: event.text,
        edited: Array.isArray(event.edited) ? event.edited : [],
        images: Array.isArray(event.images) ? (event.images as PictureName[]) : [],
        at,
      });
      return;
    case 'user_edit':
      feed.push({ kind: 'edit', path: String(event.path), at });
      return;
    case 'plan_block':
      feed.push({
        kind: 'block',
        action: event.action === 'built' ? 'built' : 'added',
        blockId: String(event.blockId ?? ''),
        what: event.kind === 'note' ? 'note' : 'block',
        text: String(event.text ?? ''),
        by: String(event.by ?? 'you'),
        at,
      });
      return;
    case 'agent_note':
      // A note an agent left through the editor's own tools, for whoever works on the task next.
      feed.push({
        kind: 'note',
        role: 'note',
        from: String(event.from ?? 'An agent'),
        to: '',
        text: String(event.text ?? ''),
        at,
      });
      return;
    case 'team_note':
      feed.push({
        kind: 'note',
        role: String(event.role ?? ''),
        from: String(event.from ?? ''),
        to: String(event.to ?? ''),
        text: String(event.text ?? ''),
        at,
      });
      return;
    case 'user_file': {
      const keptAs = typeof event.keptAs === 'string' ? event.keptAs : null;
      if (event.action === 'restored') {
        // The deletion it takes back has nothing left to put back.
        const deleted = feed.findLast((entry) => entry.kind === 'file' && entry.keptAs === keptAs);
        if (deleted && deleted.kind === 'file') deleted.keptAs = null;
      }
      feed.push({
        kind: 'file',
        action: event.action,
        path: String(event.path),
        to: typeof event.to === 'string' ? event.to : null,
        keptAs: event.action === 'deleted' ? keptAs : null,
        at,
      });
      return;
    }
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
            // Real agents often name the kind and the file only once the call is under way.
            if (update.kind) item.toolKind = String(update.kind);
            item.path = firstPath(update, task.info.cwd) ?? item.path;
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
        case 'usage_update': {
          const used = Number(update.used);
          const size = Number(update.size);
          if (!Number.isFinite(used) || !Number.isFinite(size) || size <= 0) return;
          // Agents send many of these in a turn and name the cost only in some: the last one named stands.
          const amount = update.cost?.amount;
          const before = task.usage?.cost ?? null;
          const cost = typeof amount === 'number' && Number.isFinite(amount) ? amount : before;
          // A new session counts its cost from zero again: what the last one cost is kept.
          if (cost !== null && before !== null && cost < before) task.spentBefore += before;
          const currency = String(update.cost?.currency ?? task.usage?.currency ?? 'USD');
          task.usage = { used, size, cost, currency };
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
    case 'race_review':
      // The message before this one asked the agent to review another result of the same race.
      feed.push({
        kind: 'notice',
        text: `You asked for a review of ${event.ofName ?? 'another agent'}'s result. Its change is in the message above.`,
        tone: 'muted',
        at,
      });
      return;
    case 'rewound':
      feed.push({
        kind: 'rewound',
        label: String(event.label ?? ''),
        toAt: Number(event.toAt ?? at),
        files: Number(event.files ?? 0),
        kept: Array.isArray(event.kept) ? event.kept.map(String) : [],
        droppedMessages: Boolean(event.droppedMessages),
        undo: Number(event.undo),
        at,
      });
      return;
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
  setState('tasks', info.id, {
    info,
    feed: [],
    lastSeq: -1,
    lastAt: info.createdAt,
    changed: [],
    usage: null,
    spentBefore: 0,
  });
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
      task.usage = null;
      task.spentBefore = 0;
      // The state event of a past sleep must not hide what the running agent offers now.
      const config = task.info.config;
      task.info.state = current;
      for (const event of history.events) applyEvent(task, event);
      if (task.info.state !== 'asleep') task.info.config = config;
      for (const event of live) applyEvent(task, event);
    }),
  );
}

async function reloadProjects() {
  setState('projects', reconcile(await core.request<Project[]>('projects.list')));
}

async function reload() {
  // The core cannot know where the user's day begins; "every day at 03:00" is counted on this clock.
  void core.request('automations.clock', { utcOffsetMinutes: -new Date().getTimezoneOffset() }).catch(() => {});
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
      // Which agent a new task starts with follows the one that was used last: the core knows it anew.
      void reloadAgents().catch(() => {});
      if (state.tasks[info.id]) return;
      addTask(info);
      void loadHistory(info.id, info.state);
      return;
    }
    case 'rule.offer':
      // The user has just rewritten a line an agent wrote: the conversation asks whether to keep it as a rule.
      if (state.tasks[event.data.taskId]) {
        setWork(event.data.taskId, {
          ruleOffer: { path: String(event.data.path), was: event.data.was ?? [], now: event.data.now ?? [] },
        });
      }
      return;
    case 'task.updated': {
      const info = event.data as TaskInfo;
      // A task that was handed to another agent changes which agent was used last. Whoever handed it over
      // in this window has the new agent in the task already, so there is nothing here to compare with.
      void reloadAgents().catch(() => {});
      if (state.tasks[info.id]) setState('tasks', info.id, 'info', info);
      return;
    }
    case 'task.reload': {
      // The conversation itself was changed (a rewind took messages out or put them back).
      const task = state.tasks[event.data.taskId];
      if (task) void loadHistory(task.info.id, task.info.state);
      return;
    }
    case 'task.config':
      if (state.tasks[event.data.taskId]) setState('tasks', event.data.taskId, 'info', 'config', event.data.config);
      return;
    case 'race.changed': {
      // A race started or was decided: its tasks are entrants from now on, or ordinary tasks again.
      const race: RaceRef | null = event.data.decided ? null : { id: event.data.raceId, tasks: event.data.tasks };
      for (const id of event.data.tasks as string[]) {
        if (state.tasks[id]) setState('tasks', id, 'info', 'race', race);
      }
      return;
    }
    case 'task.archived':
      forget(`task:${event.data.taskId}:`);
      waitingForHistory.delete(event.data.taskId);
      setState('tasks', event.data.taskId, undefined!);
      void reloadAgents().catch(() => {});
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

/** Puts a project into a folder of projects; an empty name takes it out. The list is read again by the event. */
export function setProjectFolder(projectId: string, folder: string): Promise<Project> {
  return core.request<Project>('projects.setFolder', { projectId, folder });
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

/**
 * Starts a task in which an agent sets the editor up: it follows the editor's setup guide, told what the team uses.
 * Returns the new task's id.
 */
export async function startSetup(projectId: string, agentId: string, text: string): Promise<string> {
  const info = await core.request<TaskInfo>('setup.start', { projectId, agentId, text });
  if (!state.tasks[info.id]) {
    addTask(info);
    await loadHistory(info.id, info.state);
  }
  return info.id;
}

/** Takes in a task the core has just made for this window, when the event about it has not done so yet. */
export async function addStartedTask(info: TaskInfo): Promise<void> {
  if (!state.tasks[info.id]) {
    addTask(info);
    await loadHistory(info.id, info.state);
  }
}

/** The conversations Claude Code had in a project's folder, newest first. */
export async function claudeSessions(projectId: string): Promise<ClaudeSession[]> {
  const found = await core.request<{ sessions: ClaudeSession[] }>('import.claude.list', { projectId });
  return found.sessions;
}

/** Makes a task of one conversation of Claude Code and returns the task's id. */
export async function bringIn(projectId: string, sessionId: string): Promise<string> {
  const info = await core.request<TaskInfo>('import.claude.take', { projectId, sessionId });
  if (!state.tasks[info.id]) {
    addTask(info);
    await loadHistory(info.id, info.state);
  }
  return info.id;
}

/** What the core answers when it was asked to start a race. */
export type StartedRace = {
  /** `null` when fewer than two agents started: one task alone is no race. */
  race: RaceRef | null;
  tasks: TaskInfo[];
  /** The agents that did not start, each with the reason. */
  failed: { agentId: string; agentName: string; message: string }[];
};

/** Takes in the tasks of a race the core has just started, as `createTask` does for one task. */
async function takeIn(started: StartedRace): Promise<StartedRace> {
  for (const info of started.tasks) {
    if (!state.tasks[info.id]) {
      addTask(info);
      await loadHistory(info.id, info.state);
    } else {
      // Only the race is news here: the task's state may have moved on since the core answered.
      setState('tasks', info.id, 'info', 'race', info.race ?? null);
    }
  }
  return started;
}

/** Sends one task text to several agents at once, each in its own copy of the code. */
export async function startRace(projectId: string, agentIds: string[], text: string): Promise<StartedRace> {
  return takeIn(await core.request<StartedRace>('race.start', { projectId, agentIds, text }));
}

/** Starts a new race with the text and the agents of an earlier one. */
export async function raceAgain(raceId: string): Promise<StartedRace> {
  return takeIn(await core.request<StartedRace>('race.again', { raceId }));
}

export function sendPrompt(taskId: string, text: string, images: PictureToSend[] = []) {
  return core.request('tasks.prompt', images.length > 0 ? { taskId, text, images } : { taskId, text });
}

/** Where the core serves a picture that was put into a message. */
export function pictureUrl(taskId: string, picture: PictureName): string {
  return `/api/picture/${encodeURIComponent(taskId)}/${encodeURIComponent(picture.id)}`;
}

export function answerPermission(taskId: string, requestId: string, optionId: string | null) {
  return core.request('tasks.permission', { taskId, requestId, optionId });
}

export function cancelTurn(taskId: string) {
  return core.request('tasks.cancel', { taskId });
}

/** Changes one setting of a task's running agent, such as its model. */
export async function setTaskOption(taskId: string, optionId: string, value: string) {
  const result = await core.request<{ config: TaskConfig }>('tasks.setOption', { taskId, optionId, value });
  if (state.tasks[taskId]) setState('tasks', taskId, 'info', 'config', result.config);
}

/**
 * Hands a task to another agent. The new one is told what was said so far
 * with the user's next message.
 */
export async function switchAgent(taskId: string, agentId: string) {
  const info = await core.request<TaskInfo>('tasks.switchAgent', { taskId, agentId });
  if (state.tasks[taskId]) setState('tasks', taskId, 'info', info);
}

/** One sentence the user wants every agent to keep to. */
export type Rule = { id: string; text: string; from: 'you' | 'edit'; at: number; agents: string[] };

/** Where a rule holds: one project (its id), or all of them. */
export const ALL_PROJECTS = 'all';

export function listRules() {
  return core.request<{ lists: { scope: string; rules: Rule[] }[] }>('rules.list');
}

export function addRule(scope: string, text: string, options: { from?: 'you' | 'edit'; agents?: string[] } = {}) {
  return core.request<{ rule: Rule }>('rules.add', { scope, text, ...options });
}

export function removeRule(scope: string, id: string) {
  return core.request('rules.remove', { scope, id });
}

/** Stops, or starts again, the question "Make this a rule?" for a project. */
export function setRuleOffers(projectId: string, on: boolean) {
  return core.request<{ on: boolean }>('rules.offers', { projectId, on });
}

/** One thing a project is missing, found by looking at its files. */
export type Finding = {
  id: string;
  projectId: string;
  category: 'Security' | 'Tests' | 'Pipeline' | 'Housekeeping' | 'Noticed';
  title: string;
  detail: string;
  /** What an agent is told when the user presses "Fix". */
  fix: string;
};

export type Health = {
  findings: Finding[];
  /** How many findings the user chose not to see. */
  ignored: number;
  /** When the package managers were last asked for their audit. */
  auditedAt: number | null;
  /** The checks of each project, with whether each is on. */
  projects: {
    projectId: string;
    checks: { id: string; title: string; command: string; from: string; on: boolean }[];
  }[];
};

export function getHealth() {
  return core.request<Health>('health.get');
}

/** Asks each project's package manager for its audit. This reaches the network. */
export function auditHealth() {
  return core.request<Health>('health.audit');
}

/** Ignores one finding, or brings every ignored one back when no id is given. */
export function ignoreFinding(id?: string) {
  return core.request('health.ignore', id ? { id } : {});
}

export function turnProjectCheck(projectId: string, id: string, on: boolean) {
  return core.request('health.turnCheck', { projectId, id, on });
}

/** What sets an automation off. */
export type AutomationWhen =
  | { kind: 'merged' }
  | { kind: 'requestOpened' }
  | { kind: 'daily'; at: string }
  | { kind: 'issueLabeled'; label: string };

/** What an automation does. */
export type AutomationThen =
  | { kind: 'cleanUp' }
  | { kind: 'runChecks'; which: 'all' | 'e2e' }
  | { kind: 'startTask'; agentId: string; text: string };

/** Work that starts without the user: "when this happens, do that". */
export type Automation = {
  id: string;
  name: string;
  on: boolean;
  when: AutomationWhen;
  then: AutomationThen;
  /** The projects it is for. Empty means all. */
  projects: string[];
  builtIn: boolean;
  lastRun: number | null;
  lastNote: string | null;
  runsThatDay: number;
};

export function listAutomations() {
  return core.request<{ automations: Automation[] }>('automations.list');
}

export function turnAutomation(id: string, on: boolean) {
  return core.request('automations.turn', { id, on });
}

export function saveAutomation(automation: {
  id?: string;
  name: string;
  when: AutomationWhen;
  then: AutomationThen;
  projects: string[];
}) {
  return core.request<{ id: string }>('automations.save', automation);
}

export function removeAutomation(id: string) {
  return core.request('automations.remove', { id });
}

/** What one agent cost and how close it is to its limits. */
export type AgentUsage = AgentDef & {
  /** Open tasks it has, and how many of them it is working on right now. */
  tasks: number;
  working: number;
  today: number;
  week: number;
  total: number;
  /** What the agent itself last said about its limits, as it said it, and when. */
  limit: { at: number; info: Record<string, unknown> } | null;
};

export type Budget = { daily: number | null; ask: boolean };

export type UsageSummary = {
  agents: AgentUsage[];
  /** Tasks that cost something today, the dearest first. */
  tasks: {
    taskId: string;
    title: string;
    projectId: string;
    agents: string[];
    today: number;
    total: number;
    archived: boolean;
  }[];
  today: number;
  currency: string;
  budget: Budget;
};

/** Cost and limits, counted from the start of the user's own day and week. */
export function usageSummary() {
  const now = new Date();
  const day = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  // The week starts on Monday.
  const week = new Date(day);
  week.setDate(day.getDate() - ((day.getDay() + 6) % 7));
  return core.request<UsageSummary>('usage.summary', { dayStart: day.getTime(), weekStart: week.getTime() });
}

export function setBudget(change: { daily?: number | null; ask?: boolean }) {
  return core.request<Budget>('usage.budget', change);
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

/** Makes an empty file, or a folder, in a task. Returns its path. */
export async function createFile(taskId: string, path: string, folder: boolean): Promise<string> {
  return (await core.request<{ path: string }>('files.create', { taskId, path, folder })).path;
}

/** Renames or moves a file or folder. Returns every file that moved. */
export function renameFile(taskId: string, from: string, to: string) {
  return core.request<{ moved: { from: string; to: string }[]; to: string }>('files.rename', { taskId, from, to });
}

/** Deletes a file or folder. It is kept by the editor and can be put back with `restoreFile`. */
export function deleteFile(taskId: string, path: string) {
  return core.request<{ files: string[]; keptAs: string }>('files.delete', { taskId, path });
}

export function restoreFile(taskId: string, keptAs: string) {
  return core.request<{ path: string; files: string[] }>('files.restore', { taskId, keptAs });
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

/**
 * A stretch of a task's life to compare over: since the user last left
 * ("visit"), since the task's branch started ("branch"), or from now back to
 * a checkpoint ("checkpoint:<n>": what rewinding to it would change).
 */
export type Span = 'visit' | 'branch' | `checkpoint:${number}`;

/** A changed file over a span. `yours` says it holds lines the user wrote, which a rewind can keep. */
export type SpanFile = GitFile & { yours: boolean };

export function gitFiles(taskId: string, from: Span) {
  return core.request<{ files: SpanFile[] }>('git.files', { taskId, from });
}

export function gitDiffOver(taskId: string, path: string, from: Span) {
  return core.request<FileDiff>('git.diff', { taskId, path, from });
}

/** The files of a task as they were at one moment. */
export type Checkpoint = {
  n: number;
  at: number;
  kind: 'start' | 'message' | 'edit' | 'rewind';
  label: string;
  /** Whose work came after this point, up to the next one. */
  who: 'agent' | 'you';
  files: number;
  added: number;
  removed: number;
  paths: string[];
  sameAsNow: boolean;
};

export function listCheckpoints(taskId: string) {
  return core.request<{ checkpoints: Checkpoint[]; git: boolean }>('checkpoints.list', { taskId });
}

export function rewindTo(taskId: string, n: number, options: { keepMine: boolean; dropMessages: boolean }) {
  return core.request<{ files: number; kept: string[]; undo: number }>('checkpoints.rewind', {
    taskId,
    n,
    ...options,
  });
}

/** One thing that happened in a task while the user was away. */
export type SinceLine = { who: 'agent' | 'you' | 'host'; text: string; at?: number; short?: string };

/** Where the user stopped in a task and what happened after they left it. */
export type Since = {
  leftAt: number;
  awayMs: number;
  stopped: string | null;
  happened: SinceLine[];
  /** The first lines of the agent's last message. */
  lastWords: string | null;
  files: GitFile[];
  /** Whether the code can be compared with how it was when the user left. */
  canCompare: boolean;
  /** Nothing happened and no file changed. */
  quiet: boolean;
};

export function taskSince(taskId: string) {
  return core.request<Since | null>('tasks.since', { taskId });
}

export function taskLeft(taskId: string) {
  return core.request('tasks.left', { taskId });
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
export type Integrations = {
  github: ToolState;
  gitlab: ToolState;
  /** `readOnly`: Linear has refused to change anything with the key kept here, so issues are not moved. */
  linear: { hasKey: boolean; readOnly?: boolean };
  docker: ToolState;
};

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

/** The Linear issues assigned to whoever the key belongs to; `null` when Linear is not connected. */
export type LinearMine = { issues: (Issue & { projectId: string | null })[]; problem: string | null } | null;

export function listIssues() {
  return core.request<{ projects: ProjectIssues[]; linear: LinearMine }>('issues.list');
}

/** Who the Linear key belongs to, in which workspace, and the teams there. */
export type LinearAccount = { name: string | null; workspace: string | null; teams: { key: string; name: string }[] };

export function linearAccount() {
  return core.request<LinearAccount>('integrations.linearAccount');
}

/**
 * Starts a task for an issue. Returns the new task's id. A Linear issue of the user's own (`source: 'linear'`)
 * may be started in any project, whatever tracker the project uses.
 */
export async function startIssue(projectId: string, key: string, agentId: string, source?: 'linear'): Promise<string> {
  const info = await core.request<TaskInfo>('issues.start', { projectId, key, agentId, ...(source ? { source } : {}) });
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
  /** Set when the request was asked to merge by itself: what to do at the merge. */
  auto?: { squash: boolean; deleteBranch: boolean; archive: boolean } | null;
};

/** What a reviewer wrote about a line, and nobody has settled yet. */
export type ReviewComment = {
  id: string;
  author: string;
  path: string | null;
  line: number | null;
  body: string;
  url: string;
};

export function requestComments(taskId: string) {
  return core.request<{ comments: ReviewComment[] }>('mr.comments', { taskId });
}

export function replyToComment(taskId: string, commentId: string, body: string) {
  return core.request('mr.reply', { taskId, commentId, body });
}

/** Asks for the request to merge by itself once everything is green, or takes that back. */
export function setAutoMerge(
  taskId: string,
  on: boolean,
  options: { squash: boolean; deleteBranch: boolean; archive: boolean },
) {
  return core.request('mr.auto', { taskId, on, ...options });
}

/** Lines of a task that another open task of the same project changes too. */
export type Overlap = {
  /** Stays the same for as long as the overlap does; names it when it is waved away. */
  mark: string;
  path: string;
  line: number;
  mine: string[];
  theirs: string[];
  before: string[];
  theirLine: number;
  task: { id: string; title: string; agentId: string };
};

export function listOverlaps(taskId: string) {
  return core.request<{ overlaps: Overlap[] }>('overlaps.list', { taskId });
}

export function dismissOverlap(taskId: string, mark: string) {
  return core.request('overlaps.dismiss', { taskId, mark });
}

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
  /** False for a database or a cache: its address is not a page to open. */
  web: boolean;
  /** Whether the chosen profile starts it. */
  inProfile: boolean;
};

export type RunState = {
  services: RunService[];
  composeFile: string | null;
  composeProject: string;
  dockerReady: boolean;
  dockerHint: string | null;
  problem: string | null;
  /** The project's launch profiles, "Full stack" first. */
  profiles: RunProfile[];
  /** The profile "Start all" starts. */
  profile: string;
  /** Null for a project without a package.json. */
  dependencies: RunDependencies | null;
  databases: RunDatabase[];
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

// ----- the Map view -----

/** What a map is centred on: a changed file (`symbol` is null) or one changed definition in it. */
export type MapCenter = { path: string; symbol: string | null; label: string; line: number };

/** Something a file defines. `kind` is a plain word: "function", "const", "class", "route" and so on. */
export type MapDef = { name: string; kind: string; line: number; changed: boolean };

/** A file of the tree. `language` is null for a kind of file the map does not read: it has no details. */
export type MapFile = { path: string; changed: boolean; language: string | null; defs: MapDef[]; tests: number };

/** One box of a picture. `layer` counts steps from the centre; before it is negative. */
export type MapNode = {
  id: string;
  label: string;
  kind: 'file' | 'function' | 'package' | 'other';
  /** Where a click leads. For a package or a call the map cannot place: the line that names it. */
  path: string | null;
  line: number | null;
  changed: boolean;
  layer: number;
  title: string;
};

export type MapGraph = { nodes: MapNode[]; edges: { from: string; to: string }[]; hidden: number };

export type CodeMap = {
  center: MapCenter | null;
  centers: MapCenter[];
  tree: MapFile[];
  deps: MapGraph;
  calls: MapGraph;
  notes: string[];
};

/** Reads the map of a task. The core works it out from the files; no agent is asked. */
export function getMap(taskId: string, center?: { path: string; symbol: string | null } | null) {
  return core.request<CodeMap>('map.get', { taskId, center: center ?? undefined });
}

// ----- the search over everything: commits, and every other task -----

/** A commit as the search lists it. `onBranch` is true for a commit of the task's own branch. */
export type CommitHit = {
  hash: string;
  short: string;
  subject: string;
  author: string;
  at: number;
  onBranch: boolean;
};

/** Commits of a task's branch and of its project whose first line holds `query`, or whose id starts with it. */
export function searchCommits(taskId: string, query: string, limit = 20) {
  return core.request<{ hits: CommitHit[] }>('search.commits', { taskId, query, limit });
}

/** A file, or a line, found in a task other than the one on screen. */
export type FileHitIn = FileHit & { taskId: string; projectId: string };
export type TextHitIn = TextHit & { taskId: string; projectId: string };

/**
 * Files of every open task but `here`, the task on screen, which is searched by
 * itself. The core answers this apart from its other answers, so it may come later.
 */
export function searchFilesEverywhere(query: string, here: string | undefined, limit = 50) {
  return core.request<{ hits: FileHitIn[] }>('everywhere.files', { query, limit, ...(here ? { taskId: here } : {}) });
}

/** Lines of every open task but `here` that hold `query`. */
export function searchTextEverywhere(query: string, here: string | undefined, limit = 100) {
  return core.request<{ hits: TextHitIn[] }>('everywhere.text', { query, limit, ...(here ? { taskId: here } : {}) });
}

/** One commit in full: what it says and which files it changed. */
export type ShownCommit = CommitHit & { body: string; files: GitFile[] };

export function gitShow(taskId: string, hash: string) {
  return core.request<ShownCommit>('git.show', { taskId, hash });
}

/** What one commit changed in one file. Nobody is named as the writer of its lines. */
export function gitShowFile(taskId: string, hash: string, path: string) {
  return core.request<FileDiff>('git.show', { taskId, hash, path });
}

// ----- the terminals of a task -----

/** A terminal as the core lists it. A shell has a number; what the Run tab starts has none. */
export type TermInfo = {
  termId: number;
  taskId: string;
  title: string;
  purpose: string;
  number: number | null;
  /** The command that has been running in a shell for a moment. */
  running: string | null;
};

export function listTerminals(taskId: string) {
  return core.request<{ terminals: TermInfo[] }>('terminal.list', { taskId });
}

/**
 * Closes a terminal. A shell with a command still running in it is closed only
 * with `force`; without it the core answers with that command and leaves the shell be.
 */
export function closeTerminal(termId: number, force = false) {
  return core.request<{ closed: boolean; running?: string }>('terminal.close', { termId, force });
}

// ----- settings -----

/** Reads the agents again: Settings changed a command, added or removed an agent, or picked another default. */
export async function reloadAgents() {
  setState('agents', reconcile(await core.request<AgentDef[]>('agents.list'), { key: 'id' }));
}

/** The agent a new task starts with: the one picked in Settings, else the first that is installed. */
export function defaultAgentId(): string {
  const installed = state.agents.filter((agent) => agent.available);
  return (installed.find((agent) => agent.isDefault) ?? installed[0])?.id ?? '';
}

// ----- the Run tab: profiles, the task's environment, packages, databases, what is used -----

/** Commands and services that start together. "Full stack" is built in: everything the project's files name. */
export type RunProfile = { name: string; services: string[]; builtIn: boolean };

export type RunDependencies = {
  /** What installs the packages, such as `npm ci`. */
  command: string;
  lockFile: string | null;
  /** "none" when package.json names no packages. */
  state: 'none' | 'missing' | 'stale' | 'installing' | 'failed' | 'installed';
  installedAt: number | null;
  note: string | null;
  termId: number | null;
};

/** A database among the Docker services. `own` is false for a volume the compose file names itself. */
export type RunDatabase = { name: string; image: string; volumes: { name: string; own: boolean }[]; seed: boolean };

/** One value of a task's environment. A secret's `value` is masked by the core; the whole value never gets here. */
export type EnvValue = {
  name: string;
  value: string;
  secret: boolean;
  /** Set for this task only. */
  own: boolean;
  /** What the project's own file says, and which file. Null for a name only this task has. */
  project: { value: string; from: string } | null;
};

export type TaskEnv = { files: string[]; values: EnvValue[]; own: number };

/** Something another task has running. */
export type RunOther = {
  taskId: string;
  title: string;
  /** Its agent is at work or waits for an answer. */
  inUse: boolean;
  idleSince: number | null;
  /** In bytes; null where neither Docker nor the system gave a number. */
  memory: number | null;
  running: string[];
};

export type RunResources = {
  /** Memory by service of this task, in bytes. */
  memory: Record<string, number>;
  others: RunOther[];
  othersMemory: number | null;
  /** Finished tasks whose containers and volumes are still there. */
  leftovers: number;
  /** Only in an answer that was asked for it. Null when Docker could not say. */
  disk?: { images: number | null; volumes: number; tasks: number } | null;
  /** The size of each volume of this task, in bytes. Comes with `disk`. */
  volumes?: Record<string, number>;
};

/** Stops and starts again one service, or the chosen profile. */
export function runRestart(taskId: string, service?: string) {
  return core.request<RunState>('run.restart', { taskId, service });
}

/** Opens a terminal inside a service's container. */
export function runShell(taskId: string, service: string) {
  return core.request<{ termId: number }>('run.shell', { taskId, service });
}

export function runPickProfile(taskId: string, name: string) {
  return core.request<RunState>('run.profile', { taskId, name });
}

/** Keeps a profile for the project; `was` names the one it replaces. */
export function runSaveProfile(taskId: string, name: string, services: string[], was?: string) {
  return core.request<RunState>('run.saveProfile', { taskId, name, services, was });
}

export function runRemoveProfile(taskId: string, name: string) {
  return core.request<RunState>('run.removeProfile', { taskId, name });
}

export function envGet(taskId: string) {
  return core.request<TaskEnv>('env.get', { taskId });
}

/** Sets a value for this task only; `null` takes the task's own value away. */
export function envSet(taskId: string, name: string, value: string | null) {
  return core.request<TaskEnv>('env.set', { taskId, name, value });
}

export function depsInstall(taskId: string) {
  return core.request<RunState>('deps.install', { taskId });
}

/** Throws away what a database of the task holds and starts it again if it was running. */
export function dbReset(taskId: string, service: string) {
  return core.request<RunState>('db.reset', { taskId, service });
}

/** What running things use. Docker is asked about its disk only with `disk`: that question is slow. */
export function resourcesGet(taskId: string, disk: boolean) {
  return core.request<RunResources>('resources.get', { taskId, disk });
}

/** Stops what idle tasks run and removes what finished tasks left in Docker. */
export function resourcesCleanUp(taskId: string) {
  return core.request<{ stopped: number; removed: number }>('resources.cleanUp', { taskId });
}
