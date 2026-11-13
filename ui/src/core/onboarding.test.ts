import { describe, expect, it } from 'vitest';
import {
  agentFrom,
  agentsHave,
  offersFor,
  PRESETS,
  projectHave,
  servicesHave,
  type Found,
  type Glance,
} from './onboarding';
import type { AgentDef, Integrations } from './store';

const nothing: Found = { claude: null, codex: null, gemini: null, aider: null, ollama: null, npx: null };
const preset = (id: string) => PRESETS.find((one) => one.id === id)!;

const agent = (id: string, more: Partial<AgentDef> = {}): AgentDef => ({
  id,
  name: id,
  short: id.slice(0, 2),
  vendor: 'Someone',
  available: true,
  note: null,
  spendsTokens: true,
  speaks: 'acp',
  ...more,
});

describe('the ready-made agents of the first run', () => {
  it('offers a program in a terminal only when that program is on this computer', () => {
    const none = offersFor(nothing).map((offer) => offer.preset.id);
    expect(none).toEqual(['claude-key', 'codex-key', 'gemini-key', 'local']);
    const some = offersFor({ ...nothing, claude: '/usr/local/bin/claude', aider: '/opt/aider' });
    expect(some.filter((offer) => offer.preset.group === 'terminal').map((offer) => offer.preset.id)).toEqual([
      'claude-terminal',
      'aider-terminal',
    ]);
  });

  it('says what an agent with a key misses when Node is not there, and lets it be added when it is', () => {
    const without = offersFor(nothing).find((offer) => offer.preset.id === 'claude-key')!;
    expect(without.ready).toBe(false);
    expect(without.missing).toContain('npx');
    const withNode = offersFor({ ...nothing, npx: '/usr/bin/npx' }).find((offer) => offer.preset.id === 'claude-key')!;
    expect(withNode).toMatchObject({ ready: true, missing: null });
  });

  it('gives a key to its agent through the environment and to nothing else', () => {
    const made = agentFrom(preset('claude-key'), { key: '  sk-ant-123  ' });
    expect(made).toEqual({
      agent: {
        name: 'Claude (API key)',
        short: 'Ck',
        program: 'npx',
        args: ['-y', '@agentclientprotocol/claude-agent-acp'],
        env: [['ANTHROPIC_API_KEY', 'sk-ant-123']],
        terminal: false,
      },
    });
    expect(agentFrom(preset('codex-key'), {})).toEqual({ problem: 'Fill in: OpenAI API key' });
  });

  it('points Claude’s agent at a server of the user’s own for a local model', () => {
    const made = agentFrom(preset('local'), { model: 'qwen3-coder' });
    if (!('agent' in made)) throw new Error('it is made');
    // The address was left as offered, and no key was given: the server still gets something.
    expect(made.agent.env).toEqual([
      ['ANTHROPIC_BASE_URL', 'http://localhost:11434'],
      ['ANTHROPIC_AUTH_TOKEN', 'local'],
      ['ANTHROPIC_MODEL', 'qwen3-coder'],
    ]);
    expect(made.agent.name).toBe('Local model (qwen3-coder)');
    expect(agentFrom(preset('local'), { address: 'http://box:8080' })).toEqual({
      problem: 'Fill in: Model, as the server names it',
    });
    // The name of an agent is forty characters at most.
    const long = agentFrom(preset('local'), { model: 'a-very-long-name-of-a-model-with-a-quantisation-tag' });
    expect('agent' in long && long.agent.name.length).toBe(40);
  });

  it('starts a program in a terminal with the first message where the program takes one', () => {
    expect(agentFrom(preset('claude-terminal'), {})).toEqual({
      agent: {
        name: 'Claude Code (terminal)',
        short: 'Ct',
        program: 'claude',
        args: ['{message}'],
        env: [],
        terminal: true,
      },
    });
    const aider = agentFrom(preset('aider-terminal'), {});
    expect('agent' in aider && aider.agent.args).toEqual([]);
  });

  it('gives every ready-made agent a name and a mark of its own', () => {
    const made = PRESETS.map((one) => {
      const typed = Object.fromEntries(one.asks.map((ask) => [ask.id, 'x']));
      const built = agentFrom(one, typed);
      if (!('agent' in built)) throw new Error(`${one.id} is made`);
      return built.agent;
    });
    expect(new Set(made.map((one) => one.name)).size).toBe(made.length);
    expect(new Set(made.map((one) => one.short)).size).toBe(made.length);
    for (const one of made) expect(one.short.length).toBeLessThanOrEqual(2);
  });
});

describe('what you have now', () => {
  it('says the practice agent is all there is, and what it is good for', () => {
    const lines = agentsHave([agent('mock', { spendsTokens: false }), agent('claude', { available: false })]);
    expect(lines).toHaveLength(1);
    expect(lines[0]).toContain('practice agent');
  });

  it('tells agents that speak the protocol from those in a terminal, and what two or more allow', () => {
    const one = agentsHave([agent('Claude')]);
    expect(one[0]).toContain('Claude: installed here. Once it is signed in');
    expect(one.at(-1)).toContain('One agent');
    const two = agentsHave([
      agent('Claude'),
      agent('Liner', { speaks: 'terminal' }),
      agent('Gone', { available: false }),
    ]);
    expect(two[1]).toBe('Liner: it runs in a terminal inside the task, and what you write is typed into it.');
    expect(two.at(-1)).toContain('2 agents to choose from');
    expect(two.join(' ')).not.toContain('Gone');
  });

  it('names what each connected service gives, and says so when none is connected', () => {
    const off = { installed: true, signedIn: false, hint: null };
    const on = { installed: true, signedIn: true, hint: null };
    const none: Integrations = { github: off, gitlab: off, linear: { hasKey: false }, docker: off };
    expect(servicesHave(none)).toHaveLength(1);
    expect(servicesHave(none)[0]).toContain('Nothing is connected yet');
    const all: Integrations = { github: on, gitlab: off, linear: { hasKey: true }, docker: on };
    const lines = servicesHave(all);
    expect(lines.map((line) => line.split(':')[0])).toEqual(['GitHub', 'Linear', 'Docker']);
  });

  it('reads a project by what its folder holds', () => {
    const seen: Glance = {
      name: 'shop',
      path: '/code/shop',
      git: true,
      host: 'GitHub',
      server: 'github.com',
      repository: 'mrx/shop',
      scripts: ['dev'],
      compose: true,
      checks: ['Lint is clean', 'Unit tests pass'],
    };
    const lines = projectHave(seen);
    expect(lines[0]).toContain('every task gets a working copy and a branch of its own');
    expect(lines[1]).toBe('Its code lives on GitHub (mrx/shop): issues and merge requests come from there.');
    expect(lines[2]).toBe('The Run tab starts "dev", the services of its compose file.');
    expect(lines[3]).toContain('2 checks of its own (Lint is clean, Unit tests pass)');
    const plain = projectHave({
      ...seen,
      git: false,
      host: null,
      repository: null,
      scripts: [],
      compose: false,
      checks: [],
    });
    expect(plain).toHaveLength(1);
    expect(plain[0]).toContain('plain folder');
  });
});
