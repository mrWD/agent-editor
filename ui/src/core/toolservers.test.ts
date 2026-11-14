import { describe, expect, it } from 'vitest';
import {
  headersText,
  parseHeaders,
  TOOL_PRESETS,
  toolHanded,
  toolKindWords,
  toolLine,
  type ToolServer,
} from './toolservers';

const server = (over: Partial<ToolServer>): ToolServer => ({
  id: 'a',
  name: 'A',
  kind: 'stdio',
  program: '',
  args: [],
  env: [],
  url: '',
  headers: [],
  on: true,
  ...over,
});

describe('a tool server on one line', () => {
  it('is its command, with an argument that holds a space in quotes, or its address', () => {
    expect(toolLine(server({ program: 'npx', args: ['-y', '@sentry/mcp-server@latest'] }))).toBe(
      'npx -y @sentry/mcp-server@latest',
    );
    expect(toolLine(server({ program: 'run', args: ['a b', ''] }))).toBe('run "a b" ""');
    expect(toolLine(server({ kind: 'http', url: 'https://mcp.notion.com/mcp' }))).toBe('https://mcp.notion.com/mcp');
    expect(toolKindWords('stdio')).toBe('A program');
    expect(toolKindWords('sse')).toBe('At an address, over SSE');
  });

  it('names what it is handed and never the values, and says which are still empty', () => {
    const filled = server({
      env: [
        ['GRAFANA_URL', 'https://grafana.example.com'],
        ['GRAFANA_SERVICE_ACCOUNT_TOKEN', ''],
      ],
    });
    expect(toolHanded(filled)).toEqual({
      names: 'GRAFANA_URL, GRAFANA_SERVICE_ACCOUNT_TOKEN',
      empty: ['GRAFANA_SERVICE_ACCOUNT_TOKEN'],
    });
    // A header that is only "Bearer " has no key in it yet.
    const header = server({ kind: 'http', headers: [['Authorization', 'Bearer ']] });
    expect(toolHanded(header).empty).toEqual(['Authorization']);
    expect(toolHanded(server({ kind: 'http', headers: [['Authorization', 'Bearer abc']] })).empty).toEqual([]);
    // What a program finds in its environment is not what an address is sent, and the other way round.
    expect(toolHanded(server({ kind: 'http', env: [['X', '']] }))).toEqual({ names: '', empty: [] });
  });
});

describe('headers as they are typed', () => {
  it('are a name, a colon and a value on each line', () => {
    const read = parseHeaders('Authorization: Bearer a:b\n\n  X-Team :  shop ');
    expect(read).toEqual({
      headers: [
        ['Authorization', 'Bearer a:b'],
        ['X-Team', 'shop'],
      ],
      problem: null,
    });
    expect(headersText(read.headers)).toBe('Authorization: Bearer a:b\nX-Team: shop');
    expect(parseHeaders('')).toEqual({ headers: [], problem: null });
  });

  it('say which line is not one', () => {
    expect(parseHeaders('no colon here').problem).toContain('"no colon here" is not a header');
    expect(parseHeaders(': value').problem).toContain('is not a header');
  });
});

describe('the servers that are ready to be filled in', () => {
  it('each have a name of their own, a way to be reached and a word on what is left to give', () => {
    const names = TOOL_PRESETS.map((preset) => preset.name);
    expect(new Set(names).size).toBe(names.length);
    for (const preset of TOOL_PRESETS) {
      expect(preset.needs.length, preset.name).toBeGreaterThan(0);
      if (preset.draft.kind === 'stdio') expect(preset.draft.program, preset.name).not.toBe('');
      else expect(preset.draft.url, preset.name).toMatch(/^https:\/\//);
      // The core takes a name of letters, digits, spaces, dots, dashes and underscores.
      expect(preset.draft.name, preset.name).toMatch(/^[A-Za-z0-9 _.-]{1,40}$/);
    }
  });
});
