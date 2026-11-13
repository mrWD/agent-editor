import { createSignal } from 'solid-js';
import { core } from './client';
import type { AgentDef, Integrations } from './store';

/**
 * The first run: the dialog a user sees when they open the editor for the
 * first time. It has three things to connect (agents, services, a project)
 * and after each it says what the user has now.
 *
 * This file holds what the dialog knows without drawing anything: whether it
 * is open, the ready-made agents it offers, and the sentences of "what you
 * have now". The core keeps the one switch (shown already, or not yet).
 */

/** The programs the core looks for on this computer: where each is, or `null`. */
export type Found = Record<'claude' | 'codex' | 'gemini' | 'aider' | 'ollama' | 'npx', string | null>;

export type FirstRun = { done: boolean; programs: Found };

/** What a project's folder holds. */
export type Glance = {
  name: string;
  path: string;
  git: boolean;
  /** "GitHub" or "GitLab", when its remote is on one of them. */
  host: string | null;
  server: string | null;
  repository: string | null;
  /** The scripts the Run tab can start. */
  scripts: string[];
  compose: boolean;
  /** The project's own checks, by title. */
  checks: string[];
};

/** Whether the dialog is on screen. */
export const [onboardingOpen, setOnboardingOpen] = createSignal(false);

export function getFirstRun() {
  return core.request<FirstRun>('onboarding.get');
}

export function glanceAt(projectId: string) {
  return core.request<Glance>('onboarding.project', { projectId });
}

/** Closes the dialog for good: done, or put aside. It can be brought back from Settings. */
export async function finishOnboarding() {
  setOnboardingOpen(false);
  await core.request('onboarding.set', { done: true });
}

/** Brings the dialog back, as Settings offers. */
export async function showOnboardingAgain() {
  await core.request('onboarding.set', { done: false });
  setOnboardingOpen(true);
}

// ----- ready-made agents -----

/** One thing a ready-made agent asks the user for. */
export type Ask = {
  id: string;
  label: string;
  placeholder?: string;
  /** What the field starts with. */
  value?: string;
  /** A key: typed into a field that hides it. */
  secret?: boolean;
  /** May be left empty. */
  optional?: boolean;
};

/** An agent as `agents.save` takes it. */
export type NewAgent = {
  name: string;
  short: string;
  program: string;
  args: string[];
  env: [string, string][];
  terminal: boolean;
};

/**
 * A ready-made agent: a command the user would otherwise have to know and type
 * in Settings. The editor calls no model itself, so a key or a local model is
 * connected by giving it to an agent, in the agent's environment.
 */
export type Preset = {
  id: string;
  /** Where the dialog lists it. */
  group: 'key' | 'local' | 'terminal';
  title: string;
  about: string;
  /** The program that has to be on this computer for the agent to start. */
  needs: keyof Found;
  asks: Ask[];
  build: (given: Record<string, string>) => NewAgent;
};

const CLAUDE_ACP = ['-y', '@agentclientprotocol/claude-agent-acp'];
const CODEX_ACP = ['-y', '@agentclientprotocol/codex-acp'];
const GEMINI_ACP = ['-y', '@google/gemini-cli', '--acp'];

/** An agent that runs in a terminal because its program is on this computer already. */
function inTerminal(
  id: string,
  program: keyof Found,
  name: string,
  short: string,
  args: string[],
  about: string,
): Preset {
  return {
    id,
    group: 'terminal',
    title: `${name} in a terminal`,
    about,
    needs: program,
    asks: [],
    build: () => ({ name: `${name} (terminal)`, short, program, args, env: [], terminal: true }),
  };
}

export const PRESETS: Preset[] = [
  {
    id: 'claude-key',
    group: 'key',
    title: 'Claude with an Anthropic API key',
    about: 'Claude’s agent, paid for by the key instead of a signed-in account.',
    needs: 'npx',
    asks: [{ id: 'key', label: 'Anthropic API key', placeholder: 'sk-ant-…', secret: true }],
    build: (given) => ({
      name: 'Claude (API key)',
      short: 'Ck',
      program: 'npx',
      args: CLAUDE_ACP,
      env: [['ANTHROPIC_API_KEY', given.key ?? '']],
      terminal: false,
    }),
  },
  {
    id: 'codex-key',
    group: 'key',
    title: 'Codex with an OpenAI API key',
    about: 'OpenAI’s agent, paid for by the key.',
    needs: 'npx',
    asks: [{ id: 'key', label: 'OpenAI API key', placeholder: 'sk-…', secret: true }],
    build: (given) => ({
      name: 'Codex (API key)',
      short: 'Xk',
      program: 'npx',
      args: CODEX_ACP,
      env: [['OPENAI_API_KEY', given.key ?? '']],
      terminal: false,
    }),
  },
  {
    id: 'gemini-key',
    group: 'key',
    title: 'Gemini with a Google API key',
    about: 'Google’s agent, paid for by the key.',
    needs: 'npx',
    asks: [{ id: 'key', label: 'Gemini API key', secret: true }],
    build: (given) => ({
      name: 'Gemini (API key)',
      short: 'Gk',
      program: 'npx',
      args: GEMINI_ACP,
      env: [['GEMINI_API_KEY', given.key ?? '']],
      terminal: false,
    }),
  },
  {
    id: 'local',
    group: 'local',
    title: 'A local or self-hosted model',
    about:
      'Claude’s agent, pointed at a server of yours that speaks Anthropic’s API: Ollama, LM Studio, LiteLLM and the like. Nothing leaves this computer when the server is on it. A small model may not manage the tools an agent needs.',
    needs: 'npx',
    asks: [
      { id: 'address', label: 'Address of the server', value: 'http://localhost:11434' },
      { id: 'model', label: 'Model, as the server names it', placeholder: 'qwen3-coder' },
      { id: 'token', label: 'Key, if the server wants one', secret: true, optional: true },
    ],
    build: (given) => ({
      name: `Local model (${given.model ?? ''})`.slice(0, 40),
      short: 'Lm',
      program: 'npx',
      args: CLAUDE_ACP,
      env: [
        ['ANTHROPIC_BASE_URL', given.address ?? ''],
        // A server that asks for no key still has to be given something.
        ['ANTHROPIC_AUTH_TOKEN', given.token || 'local'],
        ['ANTHROPIC_MODEL', given.model ?? ''],
      ],
      terminal: false,
    }),
  },
  inTerminal(
    'claude-terminal',
    'claude',
    'Claude Code',
    'Ct',
    ['{message}'],
    'The claude program you already use, with its own sign-in, skills and commands.',
  ),
  inTerminal(
    'codex-terminal',
    'codex',
    'Codex',
    'Xt',
    ['{message}'],
    'The codex program you already use, with its own sign-in.',
  ),
  inTerminal('gemini-terminal', 'gemini', 'Gemini CLI', 'Gt', [], 'The gemini program you already use.'),
  inTerminal('aider-terminal', 'aider', 'aider', 'Ai', [], 'The aider program you already use, with the keys it has.'),
];

/** A ready-made agent with whether it can be added on this computer, and why not. */
export type Offer = { preset: Preset; ready: boolean; missing: string | null };

const NEEDS: Record<keyof Found, string> = {
  npx: 'Needs Node.js: the npx command was not found.',
  claude: 'The claude program was not found.',
  codex: 'The codex program was not found.',
  gemini: 'The gemini program was not found.',
  aider: 'The aider program was not found.',
  ollama: 'Ollama was not found.',
};

/**
 * The ready-made agents for this computer. One that runs a program in a
 * terminal is offered only when that program is here: there is nothing to
 * connect otherwise. The others are listed either way, with what they miss.
 */
export function offersFor(found: Found): Offer[] {
  return PRESETS.filter((preset) => preset.group !== 'terminal' || found[preset.needs] !== null).map((preset) => {
    const ready = found[preset.needs] !== null;
    return { preset, ready, missing: ready ? null : NEEDS[preset.needs] };
  });
}

/** The agent a preset makes of what was typed, or what is still missing. */
export function agentFrom(preset: Preset, typed: Record<string, string>): { agent: NewAgent } | { problem: string } {
  const given: Record<string, string> = {};
  for (const ask of preset.asks) {
    const value = (typed[ask.id] ?? ask.value ?? '').trim();
    if (value === '' && !ask.optional) return { problem: `Fill in: ${ask.label}` };
    given[ask.id] = value;
  }
  return { agent: preset.build(given) };
}

// ----- what you have now -----

const plural = (count: number, one: string, many: string) => `${count} ${count === 1 ? one : many}`;

/** What the user's agents let them do, said after the step that connects agents. */
export function agentsHave(agents: AgentDef[]): string[] {
  const ready = agents.filter((agent) => agent.available);
  const real = ready.filter((agent) => agent.spendsTokens);
  const lines: string[] = [];
  if (real.length === 0) {
    lines.push(
      'The practice agent: it calls no model and costs nothing, so the editor can be tried from end to end. It writes no real code.',
    );
    return lines;
  }
  const spoken = real.filter((agent) => agent.speaks !== 'terminal');
  const typed = real.filter((agent) => agent.speaks === 'terminal');
  const names = (list: AgentDef[]) => list.map((agent) => agent.name).join(', ');
  if (spoken.length > 0) {
    lines.push(
      `${names(spoken)}: installed here. Once ${spoken.length === 1 ? 'it is' : 'they are'} signed in (“Try it” tells), ${spoken.length === 1 ? 'its' : 'their'} edits, questions and cost show in the conversation of a task, and each line of code says who wrote it.`,
    );
  }
  if (typed.length > 0) {
    lines.push(
      `${names(typed)}: ${typed.length === 1 ? 'it runs' : 'they run'} in a terminal inside the task, and what you write is typed into it.`,
    );
  }
  lines.push(
    real.length >= 2
      ? `${plural(real.length, 'agent', 'agents')} to choose from for every task. With two or more you can race them on one task and keep the best result, or let them work as a team.`
      : 'One agent for every task. Connect a second one to race two on a task or let them work as a team.',
  );
  return lines;
}

/** What the connected services give, said after the step that connects services. */
export function servicesHave(found: Integrations): string[] {
  const lines: string[] = [];
  const hosts = [found.github.signedIn ? 'GitHub' : null, found.gitlab.signedIn ? 'GitLab' : null].filter(
    (name): name is string => name !== null,
  );
  if (hosts.length > 0) {
    lines.push(
      `${hosts.join(' and ')}: the issues of your projects are listed in Issues, and a task goes from its first edit to a merged request without leaving the editor.`,
    );
  }
  if (found.linear.hasKey) {
    lines.push(
      'Linear: the issues assigned to you start tasks; starting one moves the issue on, and merging the task closes it.',
    );
  }
  if (found.docker.installed && found.docker.signedIn) {
    lines.push(
      'Docker: the services of a compose file run for each task by themselves, on ports of their own, from the Run tab.',
    );
  }
  if (lines.length === 0) {
    lines.push(
      'Nothing is connected yet. Tasks, the editor, checks and commits work without any of these; push and merge requests need GitHub or GitLab.',
    );
  }
  return lines;
}

/** What a project's folder gives, said after the step that adds a project. */
export function projectHave(seen: Glance): string[] {
  const lines: string[] = [];
  lines.push(
    seen.git
      ? `${seen.name} is a git repository: every task gets a working copy and a branch of its own, so agents never touch your checkout.`
      : `${seen.name} is a plain folder: tasks work in the folder itself. Make it a git repository to give each task a copy of its own.`,
  );
  if (seen.host && seen.repository) {
    lines.push(`Its code lives on ${seen.host} (${seen.repository}): issues and merge requests come from there.`);
  }
  const runs = [
    ...seen.scripts.map((name) => `"${name}"`),
    ...(seen.compose ? ['the services of its compose file'] : []),
  ];
  if (runs.length > 0) lines.push(`The Run tab starts ${runs.join(', ')}.`);
  if (seen.checks.length > 0) {
    lines.push(
      `${plural(seen.checks.length, 'check', 'checks')} of its own (${seen.checks.join(', ')}) run before a task can merge.`,
    );
  }
  return lines;
}
