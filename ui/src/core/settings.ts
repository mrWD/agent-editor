import { core } from './client';
import { reloadAgents } from './store';

/** An agent as Settings shows it: with the command that starts it. */
export type AgentSetting = {
  id: string;
  name: string;
  short: string;
  vendor: string;
  /** The command as it was written: a program, its arguments, and extra environment variables as name and value. */
  program: string;
  args: string[];
  env: [string, string][];
  /** True for an agent the user added; false for one the editor comes with. */
  own: boolean;
  /** True when one the editor comes with has a command the user gave it. */
  changed: boolean;
  available: boolean;
  note: string | null;
  spendsTokens: boolean;
  /** "terminal" for a program that speaks no protocol: it runs in a terminal and messages are typed into it. */
  speaks: 'acp' | 'terminal';
  isDefault: boolean;
  /** How many open tasks have this agent. */
  tasks: number;
};

/** The four things the user can be told about while not looking. */
export const NOTICE_KINDS = ['question', 'turnEnd', 'checksFailed', 'readyToMerge'] as const;
export type NoticeKind = (typeof NOTICE_KINDS)[number];

export type NotificationChoices = Record<NoticeKind, boolean> & {
  /** Off until the user turns notifications on once. */
  on: boolean;
  /** True inside the desktop app: the app shows the notifications, not this page. */
  byApp: boolean;
};

export type KeepAwake = {
  on: boolean;
  /** False when this system has no way to be kept awake; `note` says why. */
  available: boolean;
  /** The helper of the system that does it, such as "caffeinate -i". */
  how: string | null;
  note: string | null;
  /** True while the helper runs: an agent is working right now. */
  active: boolean;
};

export type Settings = {
  agents: AgentSetting[];
  /** The agent a new task starts with; `null` means the first one that is installed. */
  defaultAgent: string | null;
  notifications: NotificationChoices;
  keepAwake: KeepAwake;
  /** What a new project starts with. Each project has its own switch afterwards. */
  formatOnSave: boolean;
  /** Whether rewriting a line an agent wrote offers a rule at all. A project can stop the offer for itself. */
  ruleOffers: boolean;
};

/** What "Try it" found: how the agent introduces itself, or what went wrong. */
export type Tried =
  | {
      ok: true;
      name: string | null;
      version: string | null;
      continues: boolean;
      pictures: boolean;
      tools: boolean;
      says: string;
    }
  | { ok: false; problem: string };

/** A command as the form holds it. */
export type AgentCommand = { program: string; args: string[]; env: [string, string][] };

export function getSettings() {
  return core.request<Settings>('settings.get');
}

/** Changes the settings that are named; the others stay. Answers all of them as they are now. */
export function setSettings(change: {
  defaultAgent?: string | null;
  notifications?: Partial<Record<NoticeKind | 'on', boolean>>;
  keepAwake?: boolean;
  formatOnSave?: boolean;
  ruleOffers?: boolean;
}) {
  return core.request<Settings>('settings.set', change);
}

/**
 * Saves an agent. With the `id` of one the editor comes with, only its command
 * changes. With the `id` of one of the user's own, or with none, the name and
 * the short name are taken too; with none a new agent is added.
 */
export function saveAgent(agent: AgentCommand & { id?: string; name?: string; short?: string; terminal?: boolean }) {
  return core.request<Settings & { saved: string }>('agents.save', agent);
}

/** Puts an agent the editor comes with back to its own command. */
export function resetAgent(id: string) {
  return core.request<Settings>('agents.reset', { id });
}

export function removeAgent(id: string) {
  return core.request<Settings>('agents.remove', { id });
}

/** Starts the agent, reads how it introduces itself and stops it. May take a while the first time. */
export function tryAgent(id: string) {
  return core.request<Tried>('agents.try', { id });
}

let watching = false;

/** Keeps the list of agents current everywhere one is picked, when Settings changed it. Call once. */
export function watchSettings() {
  if (watching) return;
  watching = true;
  core.onEvent((event) => {
    if (event.event === 'settings.changed' && event.data?.agents) void reloadAgents().catch(() => {});
  });
}

// ----- a command line, as text -----

/**
 * Splits a line of arguments the way a shell does: spaces part them, single
 * or double quotes keep one together, a backslash takes the next character as
 * it is. Nothing is expanded: `$HOME` stays `$HOME`.
 */
export function splitArgs(line: string): string[] {
  const args: string[] = [];
  let current = '';
  let started = false;
  let quote: '"' | "'" | null = null;
  for (let i = 0; i < line.length; i++) {
    const char = line[i]!;
    if (quote) {
      if (char === quote) quote = null;
      else if (char === '\\' && quote === '"' && i + 1 < line.length) current += line[++i];
      else current += char;
    } else if (char === '"' || char === "'") {
      quote = char;
      // `""` is an argument too, an empty one.
      started = true;
    } else if (char === '\\' && i + 1 < line.length) {
      current += line[++i];
      started = true;
    } else if (/\s/.test(char)) {
      if (started) args.push(current);
      current = '';
      started = false;
    } else {
      current += char;
      started = true;
    }
  }
  if (started) args.push(current);
  return args;
}

/** One argument as it is typed on a line: quoted only when it has to be. */
function quoted(arg: string): string {
  return arg !== '' && !/[\s"'\\]/.test(arg) ? arg : `"${arg.replace(/(["\\])/g, '\\$1')}"`;
}

/** Arguments as one line that `splitArgs` reads back the same. */
export function joinArgs(args: string[]): string {
  return args.map(quoted).join(' ');
}

/** The whole command on one line, as it would be typed in a terminal. */
export function commandLine(command: { program: string; args: string[] }): string {
  return [command.program, ...command.args].map(quoted).join(' ');
}

/** Environment variables as lines of `NAME=value`. */
export function envText(env: [string, string][]): string {
  return env.map(([name, value]) => `${name}=${value}`).join('\n');
}

/** Reads lines of `NAME=value`. Empty lines are passed over; a line without `=` is said to be wrong. */
export function parseEnv(text: string): { env: [string, string][]; problem: string | null } {
  const env: [string, string][] = [];
  for (const line of text.split('\n')) {
    if (line.trim() === '') continue;
    const at = line.indexOf('=');
    const name = at < 0 ? '' : line.slice(0, at).trim();
    if (name === '' || /\s/.test(name)) {
      return { env: [], problem: `"${line.trim()}" is not NAME=value. Write one variable on a line.` };
    }
    env.push([name, line.slice(at + 1)]);
  }
  return { env, problem: null };
}

/** The first two letters of a name, for the mark next to an agent's lines. */
export function shortFor(name: string): string {
  const letters = name.replace(/[^\p{L}\p{N}]/gu, '');
  return letters.slice(0, 1).toUpperCase() + letters.slice(1, 2).toLowerCase();
}
