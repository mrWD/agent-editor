import { describe, expect, it } from 'vitest';
import {
  agentFrom,
  agentsHave,
  offersFor,
  PRESETS,
  PROGRAMS_WANTED,
  projectHave,
  servicesHave,
  type Found,
  type Glance,
} from './onboarding';
import type { AgentDef, Integrations } from './store';

/** A computer with nothing on it: none of the programs a ready-made agent may need. */
const nothing: Found = Object.fromEntries(PROGRAMS_WANTED.map((name) => [name, null]));
const preset = (id: string) => PRESETS.find((one) => one.id === id)!;
const ids = (offers: { preset: { id: string; group: string } }[], group: string) =>
  offers.filter((offer) => offer.preset.group === group).map((offer) => offer.preset.id);

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
  it('offers a program of its own only when that program is on this computer', () => {
    const none = offersFor(nothing);
    expect(ids(none, 'terminal')).toEqual([]);
    // Without Node the ones npx fetches are still listed, with what they miss; the ones with a program of their own are not.
    expect(ids(none, 'protocol')).toContain('qwen-code-protocol');
    expect(ids(none, 'protocol')).not.toContain('cursor-protocol');
    expect(none.every((offer) => !offer.ready && offer.missing?.includes('npx'))).toBe(true);

    const some = offersFor({
      ...nothing,
      'cursor-agent': '/usr/local/bin/cursor-agent',
      agy: '/opt/agy',
      aider: '/opt/aider',
    });
    expect(ids(some, 'terminal')).toEqual(['cursor-agent-terminal', 'agy-terminal', 'aider-terminal']);
    // Cursor speaks the protocol with the same program, so it is offered both ways.
    expect(ids(some, 'protocol')).toContain('cursor-protocol');
    expect(some.find((offer) => offer.preset.id === 'cursor-protocol')).toMatchObject({
      ready: true,
      program: 'cursor-agent',
    });
  });

  it('uses whichever of a program’s names is on this computer', () => {
    const offers = offersFor({ ...nothing, 'kimi-code': '/opt/kimi-code', cbc: '/opt/cbc' });
    const kimi = offers.find((offer) => offer.preset.id === 'kimi-terminal')!;
    expect(kimi.program).toBe('kimi-code');
    const made = agentFrom(kimi.preset, {}, kimi.program!);
    expect('agent' in made && made.agent).toMatchObject({
      name: 'Kimi Code (terminal)',
      program: 'kimi-code',
      terminal: true,
    });
  });

  it('knows the agents people use, from the West and from China', () => {
    const titles = PRESETS.map((one) => one.title);
    for (const title of [
      'Codex in a terminal',
      'Cursor in a terminal',
      'Antigravity in a terminal',
      'Qwen Code in a terminal',
      'Trae in a terminal',
      'Cursor',
      'GitHub Copilot',
      'Qwen Code',
      'Kimi Code',
      'CodeBuddy Code',
      'MiniMax Code',
      'DeepSeek models',
      'GLM models',
      'OpenRouter models',
    ]) {
      expect(titles, title).toContain(title);
    }
    // The core is asked for every program any of them may be started with, each once.
    expect(PROGRAMS_WANTED).toEqual([...new Set(PROGRAMS_WANTED)]);
    for (const name of ['npx', 'codex', 'cursor-agent', 'agy', 'qwen', 'kimi', 'traecli', 'opencode']) {
      expect(PROGRAMS_WANTED, name).toContain(name);
    }
  });

  it('says what an agent with a key misses when Node is not there, and lets it be added when it is', () => {
    const without = offersFor(nothing).find((offer) => offer.preset.id === 'claude-key')!;
    expect(without.ready).toBe(false);
    expect(without.missing).toContain('npx');
    const withNode = offersFor({ ...nothing, npx: '/usr/bin/npx' }).find((offer) => offer.preset.id === 'claude-key')!;
    expect(withNode).toMatchObject({ ready: true, missing: null, program: 'npx' });
  });

  it('gives a key to its agent through the environment and to nothing else', () => {
    const made = agentFrom(preset('claude-key'), { key: '  sk-ant-123  ' });
    expect(made).toEqual({
      agent: {
        name: 'Claude (API key)',
        short: 'Ca',
        program: 'npx',
        args: ['-y', '@agentclientprotocol/claude-agent-acp'],
        env: [['ANTHROPIC_API_KEY', 'sk-ant-123']],
        terminal: false,
      },
    });
    expect(agentFrom(preset('codex-key'), {})).toEqual({ problem: 'Fill in: OpenAI API key' });
  });

  it('starts an agent that speaks the protocol as the registry says', () => {
    const cursor = agentFrom(preset('cursor-protocol'), {}, 'cursor-agent');
    expect('agent' in cursor && cursor.agent).toEqual({
      name: 'Cursor',
      short: 'Cu',
      program: 'cursor-agent',
      args: ['acp'],
      env: [],
      terminal: false,
    });
    const qwen = agentFrom(preset('qwen-code-protocol'), {});
    expect('agent' in qwen && qwen.agent).toMatchObject({
      program: 'npx',
      args: ['-y', '@qwen-code/qwen-code', '--acp', '--experimental-skills'],
    });
    const droid = agentFrom(preset('factory-droid-protocol'), {});
    expect('agent' in droid && droid.agent.args).toEqual(['-y', 'droid', 'exec', '--output-format', 'acp-daemon']);
  });

  it('points Claude’s agent at another maker’s server, with the key and the model as that maker names them', () => {
    const made = agentFrom(preset('deepseek-model'), { key: 'sk-ds', model: 'deepseek-chat' });
    if (!('agent' in made)) throw new Error('it is made');
    expect(made.agent).toMatchObject({ name: 'DeepSeek (deepseek-chat)', program: 'npx', terminal: false });
    expect(made.agent.args).toEqual(['-y', '@agentclientprotocol/claude-agent-acp']);
    // The address was left as offered. The agent's own errands go to the same model: the server knows no other.
    expect(made.agent.env).toEqual([
      ['ANTHROPIC_BASE_URL', 'https://api.deepseek.com/anthropic'],
      ['ANTHROPIC_AUTH_TOKEN', 'sk-ds'],
      ['ANTHROPIC_MODEL', 'deepseek-chat'],
      ['ANTHROPIC_SMALL_FAST_MODEL', 'deepseek-chat'],
    ]);
    // Inside China some makers have another address: the field takes it.
    const inChina = agentFrom(preset('glm-model'), {
      key: 'k',
      model: 'glm-4.6',
      address: 'https://open.bigmodel.cn/api/anthropic',
    });
    expect('agent' in inChina && inChina.agent.env[0]).toEqual([
      'ANTHROPIC_BASE_URL',
      'https://open.bigmodel.cn/api/anthropic',
    ]);
    const router = agentFrom(preset('openrouter-model'), { key: 'sk-or', model: 'z-ai/glm-4.6' });
    expect('agent' in router && router.agent.env).toContainEqual(['ANTHROPIC_API_KEY', '']);
    expect(agentFrom(preset('kimi-model'), { key: 'k' })).toEqual({
      problem: 'Fill in: Model, as the service names it',
    });
  });

  it('points Claude’s agent at a server of the user’s own for a local model', () => {
    const made = agentFrom(preset('local'), { model: 'qwen3-coder' });
    if (!('agent' in made)) throw new Error('it is made');
    // The address was left as offered, and no key was given: the server still gets something.
    expect(made.agent.env).toEqual([
      ['ANTHROPIC_BASE_URL', 'http://localhost:11434'],
      ['ANTHROPIC_AUTH_TOKEN', 'local'],
      ['ANTHROPIC_MODEL', 'qwen3-coder'],
      ['ANTHROPIC_SMALL_FAST_MODEL', 'qwen3-coder'],
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
    expect(agentFrom(preset('claude-terminal'), {}, 'claude')).toEqual({
      agent: {
        name: 'Claude Code (terminal)',
        short: 'Cc',
        program: 'claude',
        args: ['{message}'],
        env: [],
        terminal: true,
      },
    });
    const args = (id: string) => {
      const made = agentFrom(preset(id), {});
      return 'agent' in made ? made.agent.args : null;
    };
    expect(args('gemini-terminal')).toEqual(['-i', '{message}']);
    expect(args('opencode-terminal')).toEqual(['--prompt', '{message}']);
    expect(args('traecli-terminal')).toEqual(['--', '{message}']);
    expect(args('kiro-cli-terminal')).toEqual(['chat', '--tui']);
    // Where the way in is not known for sure, the message is typed.
    expect(args('agy-terminal')).toEqual([]);
    expect(args('aider-terminal')).toEqual([]);
  });

  it('gives every ready-made agent an id and a name of its own, and a mark of one or two letters', () => {
    expect(new Set(PRESETS.map((one) => one.id)).size).toBe(PRESETS.length);
    const made = PRESETS.map((one) => {
      const typed = Object.fromEntries(one.asks.map((ask) => [ask.id, 'x']));
      const built = agentFrom(one, typed);
      if (!('agent' in built)) throw new Error(`${one.id} is made`);
      return built.agent;
    });
    // The core refuses two agents of one name, so each of these has its own.
    expect(new Set(made.map((one) => one.name)).size).toBe(made.length);
    for (const one of made) {
      expect(one.name.length, one.name).toBeLessThanOrEqual(40);
      expect(one.short, one.name).toMatch(/^[A-Za-z0-9]{1,2}$/);
    }
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
