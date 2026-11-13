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
export type Found = Record<string, string | null>;

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
  return core.request<FirstRun>('onboarding.get', { programs: PROGRAMS_WANTED });
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

/** Where the dialog lists a ready-made agent. */
export type Group = 'terminal' | 'protocol' | 'key' | 'models' | 'local';

/**
 * A ready-made agent: a command the user would otherwise have to know and type
 * in Settings. The editor calls no model itself, so a key or a model of
 * another maker is connected by giving it to an agent, in the agent's
 * environment.
 */
export type Preset = {
  id: string;
  group: Group;
  title: string;
  about: string;
  /** The programs that start it: the first of them found on this computer is used. */
  needs: string[];
  asks: Ask[];
  /** `program` is the one of `needs` that was found. */
  build: (given: Record<string, string>, program: string) => NewAgent;
};

/** The mark next to an agent's lines: the first letters of its first two words, or of its one word. */
function markOf(name: string): string {
  const words = name.split(/[^A-Za-z0-9]+/).filter(Boolean);
  const mark = words.length > 1 ? `${words[0]![0]}${words[1]![0]}` : (words[0] ?? 'Ag').slice(0, 2);
  return mark.charAt(0).toUpperCase() + mark.slice(1).toLowerCase();
}

const NPX = 'npx';
/** What `npx` is given to start an agent that is fetched when first used. */
const fetched = (...rest: string[]) => ['-y', ...rest];
const CLAUDE_ACP = fetched('@agentclientprotocol/claude-agent-acp');

// ----- programs in a terminal -----

/**
 * Command-line agents, by the program each is started with. One that is on
 * this computer can be an agent in a terminal as it is, with its own sign-in.
 * `first` is where the program takes a first message on its command line;
 * without it the message is typed once the program has started.
 */
const PROGRAMS: { name: string; programs: string[]; args?: string[]; first?: string[] }[] = [
  { name: 'Claude Code', programs: ['claude'], first: ['{message}'] },
  { name: 'Codex', programs: ['codex'], first: ['{message}'] },
  { name: 'Cursor', programs: ['cursor-agent'], first: ['{message}'] },
  { name: 'Antigravity', programs: ['agy'] },
  { name: 'Gemini CLI', programs: ['gemini'], first: ['-i', '{message}'] },
  { name: 'GitHub Copilot', programs: ['copilot'], first: ['-i', '{message}'] },
  { name: 'OpenCode', programs: ['opencode'], first: ['--prompt', '{message}'] },
  { name: 'Qwen Code', programs: ['qwen'] },
  { name: 'Kimi Code', programs: ['kimi', 'kimi-code'] },
  { name: 'Qoder', programs: ['qodercli', 'qoder'] },
  { name: 'CodeBuddy', programs: ['codebuddy', 'cbc'], first: ['{message}'] },
  { name: 'Trae', programs: ['traecli'], first: ['--', '{message}'] },
  { name: 'MiMo Code', programs: ['mimo'] },
  { name: 'Grok', programs: ['grok'], first: ['--', '{message}'] },
  { name: 'Factory Droid', programs: ['droid'], first: ['{message}'] },
  { name: 'Amp', programs: ['amp'] },
  { name: 'goose', programs: ['goose'] },
  { name: 'aider', programs: ['aider'] },
  { name: 'Crush', programs: ['crush'] },
  { name: 'Auggie', programs: ['auggie'] },
  { name: 'Cline', programs: ['cline'] },
  { name: 'Kiro', programs: ['kiro-cli'], args: ['chat', '--tui'] },
  { name: 'Kilo', programs: ['kilo'] },
  { name: 'Mistral Vibe', programs: ['vibe', 'mistral-vibe'] },
  { name: 'Devin', programs: ['devin'], first: ['--', '{message}'] },
  { name: 'Pi', programs: ['pi'], first: ['{message}'] },
  { name: 'Continue', programs: ['cn'] },
  { name: 'OpenClaude', programs: ['openclaude'], first: ['{message}'] },
];

const TERMINAL: Preset[] = PROGRAMS.map((one) => ({
  id: `${one.programs[0]}-terminal`,
  group: 'terminal',
  title: `${one.name} in a terminal`,
  about: `The ${one.programs[0]} program you already use, with its own sign-in and settings.`,
  needs: one.programs,
  asks: [],
  build: (_given, program) => ({
    name: `${one.name} (terminal)`,
    short: markOf(one.name),
    program,
    args: [...(one.args ?? []), ...(one.first ?? [])],
    env: [],
    terminal: true,
  }),
}));

// ----- agents that speak the protocol -----

/**
 * Agents that speak the Agent Client Protocol, as its registry lists them
 * (agentclientprotocol.com, November 2026). `npx` fetches the ones that come
 * as a package; the others need their own program on this computer.
 */
const SPEAKERS: { name: string; maker: string; program: string; args: string[]; env?: [string, string][] }[] = [
  { name: 'Cursor', maker: 'Cursor', program: 'cursor-agent', args: ['acp'] },
  { name: 'GitHub Copilot', maker: 'GitHub', program: NPX, args: fetched('@github/copilot', '--acp') },
  {
    name: 'Qwen Code',
    maker: 'Alibaba',
    program: NPX,
    args: fetched('@qwen-code/qwen-code', '--acp', '--experimental-skills'),
  },
  { name: 'Kimi Code', maker: 'Moonshot AI', program: NPX, args: fetched('@moonshot-ai/kimi-code', 'acp') },
  { name: 'Qoder', maker: 'Qoder AI', program: NPX, args: fetched('@qoder-ai/qodercli', '--acp') },
  {
    name: 'CodeBuddy Code',
    maker: 'Tencent Cloud',
    program: NPX,
    args: fetched('@tencent-ai/codebuddy-code', '--acp'),
  },
  { name: 'MiniMax Code', maker: 'MiniMax', program: NPX, args: fetched('@minimax-ai/code', 'acp') },
  {
    name: 'GLM Agent',
    maker: 'a community agent for Zhipu’s GLM models',
    program: NPX,
    args: fetched('glm-acp-agent'),
  },
  { name: 'Grok Build', maker: 'xAI', program: NPX, args: fetched('@xai-official/grok', 'agent', 'stdio') },
  { name: 'Cline', maker: 'Cline', program: NPX, args: fetched('cline', '--acp') },
  {
    name: 'Auggie',
    maker: 'Augment Code',
    program: NPX,
    args: fetched('@augmentcode/auggie', '--acp'),
    env: [['AUGMENT_DISABLE_AUTO_UPDATE', '1']],
  },
  {
    name: 'Factory Droid',
    maker: 'Factory AI',
    program: NPX,
    args: fetched('droid', 'exec', '--output-format', 'acp-daemon'),
    env: [
      ['DROID_DISABLE_AUTO_UPDATE', 'true'],
      ['FACTORY_DROID_AUTO_UPDATE_ENABLED', 'false'],
    ],
  },
  { name: 'Kilo', maker: 'Kilo Code', program: NPX, args: fetched('@kilocode/cli', 'acp') },
  { name: 'OpenCode', maker: 'Anomaly', program: 'opencode', args: ['acp'] },
  { name: 'goose', maker: 'Block', program: 'goose', args: ['acp'] },
  { name: 'Kiro', maker: 'Amazon', program: 'kiro-cli', args: ['acp', '--agent-engine=v3'] },
  { name: 'Mistral Vibe', maker: 'Mistral AI', program: 'vibe-acp', args: [] },
  { name: 'Devin', maker: 'Cognition', program: 'devin', args: ['acp'] },
  { name: 'Amp', maker: 'a community wrapper for Amp', program: 'amp-acp', args: [] },
  { name: 'Junie', maker: 'JetBrains', program: 'junie', args: ['--acp=true'] },
];

const PROTOCOL: Preset[] = SPEAKERS.map((one) => ({
  id: `${one.name.toLowerCase().replace(/[^a-z0-9]+/g, '-')}-protocol`,
  group: 'protocol',
  title: one.name,
  about:
    one.program === NPX
      ? `From ${one.maker}. Fetched with npx at its first start; it signs in its own way.`
      : `From ${one.maker}. Uses the ${one.program} program on this computer, with its own sign-in.`,
  needs: [one.program],
  asks: [],
  build: (_given, program) => ({
    name: one.name,
    short: markOf(one.name),
    program,
    args: one.args,
    env: one.env ?? [],
    terminal: false,
  }),
}));

// ----- with a key -----

const withKey = (
  id: string,
  title: string,
  about: string,
  name: string,
  args: string[],
  variable: string,
  label: string,
  placeholder?: string,
): Preset => ({
  id,
  group: 'key',
  title,
  about,
  needs: [NPX],
  asks: [{ id: 'key', label, placeholder, secret: true }],
  build: (given) => ({
    name,
    short: markOf(name),
    program: NPX,
    args,
    env: [[variable, given.key ?? '']],
    terminal: false,
  }),
});

const KEYS: Preset[] = [
  withKey(
    'claude-key',
    'Claude with an Anthropic API key',
    'Claude’s agent, paid for by the key instead of a signed-in account.',
    'Claude (API key)',
    CLAUDE_ACP,
    'ANTHROPIC_API_KEY',
    'Anthropic API key',
    'sk-ant-…',
  ),
  withKey(
    'codex-key',
    'Codex with an OpenAI API key',
    'OpenAI’s agent, paid for by the key.',
    'Codex (API key)',
    fetched('@agentclientprotocol/codex-acp'),
    'OPENAI_API_KEY',
    'OpenAI API key',
    'sk-…',
  ),
  withKey(
    'gemini-key',
    'Gemini with a Google API key',
    'Google’s agent, paid for by the key.',
    'Gemini (API key)',
    fetched('@google/gemini-cli', '--acp'),
    'GEMINI_API_KEY',
    'Gemini API key',
  ),
];

// ----- models of other makers, behind Claude's agent -----

/**
 * Claude's agent pointed at another server that speaks Anthropic's API. The
 * agent asks for a small, fast model for its own errands as well; here that
 * is the same model, for the server knows no other by Claude's names.
 */
function behindClaude(
  address: string,
  token: string,
  model: string,
  more: [string, string][] = [],
): [string, string][] {
  return [
    ['ANTHROPIC_BASE_URL', address],
    ['ANTHROPIC_AUTH_TOKEN', token],
    ['ANTHROPIC_MODEL', model],
    ['ANTHROPIC_SMALL_FAST_MODEL', model],
    ...more,
  ];
}

/**
 * Makers whose servers speak Anthropic's API, with the address each gives for
 * it. Some have another address inside China; the field can be changed.
 */
const MAKERS: { id: string; name: string; who: string; address: string; model: string; more?: [string, string][] }[] = [
  {
    id: 'deepseek',
    name: 'DeepSeek',
    who: 'DeepSeek',
    address: 'https://api.deepseek.com/anthropic',
    model: 'deepseek-chat',
  },
  {
    id: 'kimi',
    name: 'Kimi',
    who: 'Moonshot AI',
    address: 'https://api.moonshot.ai/anthropic',
    model: 'kimi-k2-turbo-preview',
  },
  { id: 'glm', name: 'GLM', who: 'Zhipu, through Z.ai', address: 'https://api.z.ai/api/anthropic', model: 'glm-4.6' },
  {
    id: 'qwen',
    name: 'Qwen',
    who: 'Alibaba Cloud Model Studio',
    address: 'https://dashscope-intl.aliyuncs.com/apps/anthropic',
    model: 'qwen3-coder-plus',
  },
  { id: 'minimax', name: 'MiniMax', who: 'MiniMax', address: 'https://api.minimax.io/anthropic', model: 'MiniMax-M2' },
  {
    id: 'openrouter',
    name: 'OpenRouter',
    who: 'OpenRouter, which passes on to many makers’ models',
    address: 'https://openrouter.ai/api',
    model: 'deepseek/deepseek-chat',
    // OpenRouter asks for this one to be there and empty.
    more: [['ANTHROPIC_API_KEY', '']],
  },
];

const MODELS: Preset[] = MAKERS.map((one) => ({
  id: `${one.id}-model`,
  group: 'models',
  title: `${one.name} models`,
  about: `Claude’s agent, talking to ${one.who} with a key of yours. What you write and your code go to that service.`,
  needs: [NPX],
  asks: [
    { id: 'key', label: `${one.name} API key`, secret: true },
    { id: 'model', label: 'Model, as the service names it', placeholder: one.model },
    { id: 'address', label: 'Address of the service', value: one.address },
  ],
  build: (given) => ({
    name: `${one.name} (${given.model ?? ''})`.slice(0, 40),
    short: markOf(one.name),
    program: NPX,
    args: CLAUDE_ACP,
    env: behindClaude(given.address ?? '', given.key ?? '', given.model ?? '', one.more),
    terminal: false,
  }),
}));

const LOCAL: Preset = {
  id: 'local',
  group: 'local',
  title: 'A local or self-hosted model',
  about:
    'Claude’s agent, pointed at a server of yours that speaks Anthropic’s API: Ollama, LM Studio, LiteLLM and the like. Nothing leaves this computer when the server is on it. A small model may not manage the tools an agent needs.',
  needs: [NPX],
  asks: [
    { id: 'address', label: 'Address of the server', value: 'http://localhost:11434' },
    { id: 'model', label: 'Model, as the server names it', placeholder: 'qwen3-coder' },
    { id: 'token', label: 'Key, if the server wants one', secret: true, optional: true },
  ],
  build: (given) => ({
    name: `Local model (${given.model ?? ''})`.slice(0, 40),
    short: 'Lm',
    program: NPX,
    args: CLAUDE_ACP,
    // A server that asks for no key still has to be given something.
    env: behindClaude(given.address ?? '', given.token || 'local', given.model ?? ''),
    terminal: false,
  }),
};

export const PRESETS: Preset[] = [...TERMINAL, ...PROTOCOL, ...KEYS, ...MODELS, LOCAL];

/** Every program a ready-made agent may be started with: what the core is asked to look for. */
export const PROGRAMS_WANTED: string[] = [...new Set(PRESETS.flatMap((preset) => preset.needs))];

/** A ready-made agent with whether it can be added on this computer, why not, and the program it would use. */
export type Offer = { preset: Preset; ready: boolean; missing: string | null; program: string | null };

/**
 * The ready-made agents for this computer. One that needs a program of its
 * own (a command-line agent, in a terminal or speaking the protocol) is
 * offered only when that program is here: there is nothing to connect
 * otherwise. The ones `npx` fetches are listed either way, and say so when
 * Node is missing.
 */
export function offersFor(found: Found): Offer[] {
  return PRESETS.flatMap((preset): Offer[] => {
    const program = preset.needs.find((name) => (found[name] ?? null) !== null) ?? null;
    if (program !== null) return [{ preset, ready: true, missing: null, program }];
    if (preset.needs[0] !== NPX) return [];
    return [{ preset, ready: false, missing: 'Needs Node.js: the npx command was not found.', program: null }];
  });
}

/** The agent a preset makes of what was typed, or what is still missing. `program` is the one that was found. */
export function agentFrom(
  preset: Preset,
  typed: Record<string, string>,
  program: string = preset.needs[0] ?? '',
): { agent: NewAgent } | { problem: string } {
  const given: Record<string, string> = {};
  for (const ask of preset.asks) {
    const value = (typed[ask.id] ?? ask.value ?? '').trim();
    if (value === '' && !ask.optional) return { problem: `Fill in: ${ask.label}` };
    given[ask.id] = value;
  }
  return { agent: preset.build(given, program) };
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
