import { core } from './client';

/**
 * Tool servers of the user's own: what agents reach the world outside the
 * code through (a tracker of errors, a design, a database). They speak the
 * Model Context Protocol. The editor keeps the list and names each server to
 * every agent that starts; it calls none of them itself.
 */

/** How a server is reached: a program the agent starts, or an address. */
export type ToolKind = 'stdio' | 'http' | 'sse';

export type ToolServer = {
  id: string;
  /** What agents see it as. */
  name: string;
  kind: ToolKind;
  program: string;
  args: string[];
  env: [string, string][];
  url: string;
  headers: [string, string][];
  /** Off keeps it in the list and hands it to nobody. */
  on: boolean;
};

/** A server as the form holds it, before it has an id. */
export type ToolDraft = Omit<ToolServer, 'id'> & { id?: string };

type Listed = { servers: ToolServer[]; saved: string | null };

export async function listToolServers(): Promise<ToolServer[]> {
  return (await core.request<Listed>('tools.list')).servers;
}

/** Adds a server, or changes the one whose `id` is given. Answers the list as it is now. */
export async function saveToolServer(server: ToolDraft): Promise<ToolServer[]> {
  return (await core.request<Listed>('tools.save', server)).servers;
}

export async function removeToolServer(id: string): Promise<ToolServer[]> {
  return (await core.request<Listed>('tools.remove', { id })).servers;
}

/** A server that is ready to be filled in: where it is, and what the user still has to give. */
export type ToolPreset = {
  name: string;
  /** What agents can do with it, in a few words. */
  gives: string;
  draft: Omit<ToolDraft, 'on' | 'id'>;
  /** What is left for the user: a key to paste, a sign-in, a thing to start. */
  needs: string;
};

const remote = (name: string, url: string): ToolPreset['draft'] => ({
  name,
  kind: 'http',
  url,
  headers: [],
  program: '',
  args: [],
  env: [],
});

const program = (name: string, run: string, args: string[], env: [string, string][] = []): ToolPreset['draft'] => ({
  name,
  kind: 'stdio',
  program: run,
  args,
  env,
  url: '',
  headers: [],
});

/** What each maker's sign-in looks like for a server at an address. */
const SIGNS_IN = 'The agent signs in with its maker the first time it calls a tool, when the agent can do that.';

/**
 * Servers many teams use, as their makers publish them (November 2026). Each
 * is a starting point: the form opens filled in and can be changed before it
 * is saved.
 */
export const TOOL_PRESETS: ToolPreset[] = [
  {
    name: 'Sentry',
    gives: 'Errors of the running app: the stack trace, how often, since which release.',
    draft: remote('Sentry', 'https://mcp.sentry.dev/mcp'),
    needs: SIGNS_IN,
  },
  {
    name: 'Sentry with a token',
    gives: 'The same, as a program on this computer, for an agent that cannot sign in.',
    draft: program('Sentry', 'npx', ['-y', '@sentry/mcp-server@latest'], [['SENTRY_ACCESS_TOKEN', '']]),
    needs: 'Paste a user auth token of Sentry as SENTRY_ACCESS_TOKEN.',
  },
  {
    name: 'Figma',
    gives: 'A frame of a design: what it looks like, its tokens, its code.',
    draft: remote('Figma', 'https://mcp.figma.com/mcp'),
    needs: SIGNS_IN,
  },
  {
    name: 'Notion',
    gives: 'Pages and databases of the workspace, to read and to write.',
    draft: remote('Notion', 'https://mcp.notion.com/mcp'),
    needs: SIGNS_IN,
  },
  {
    name: 'Linear',
    gives: 'Issues, comments, projects and cycles, beyond the list on the Issues screen.',
    draft: remote('Linear', 'https://mcp.linear.app/mcp'),
    needs: SIGNS_IN,
  },
  {
    name: 'Jira and Confluence',
    gives: "Atlassian's issues and pages.",
    draft: remote('Atlassian', 'https://mcp.atlassian.com/v1/mcp'),
    needs: `${SIGNS_IN} Atlassian has moved this address before: check it against their own page if it does not answer.`,
  },
  {
    name: 'Slack',
    gives: 'Channels and threads, to read and to post to.',
    draft: remote('Slack', 'https://mcp.slack.com/mcp'),
    needs: SIGNS_IN,
  },
  {
    name: 'GitHub',
    gives: 'Issues, pull requests, code search and Actions of any repository the token may see.',
    draft: { ...remote('GitHub', 'https://api.githubcopilot.com/mcp/'), headers: [['Authorization', 'Bearer ']] },
    needs: 'Paste a personal access token after "Bearer " in the Authorization header.',
  },
  {
    name: 'Grafana',
    gives: 'Dashboards, logs and alerts of a Grafana.',
    draft: program(
      'Grafana',
      'uvx',
      ['mcp-grafana'],
      [
        ['GRAFANA_URL', ''],
        ['GRAFANA_SERVICE_ACCOUNT_TOKEN', ''],
      ],
    ),
    needs: 'Give the address of your Grafana and a token of a service account.',
  },
  {
    name: 'A browser',
    gives: 'A real browser the agent drives: open a page, click, read what is on it.',
    draft: program('Playwright', 'npx', ['-y', '@playwright/mcp@latest']),
    needs: 'Nothing: it downloads its browser the first time it is used.',
  },
  {
    name: 'Library documentation',
    gives: 'The current documentation of a library, by its name.',
    draft: program('Context7', 'npx', ['-y', '@upstash/context7-mcp@latest']),
    needs: 'Nothing.',
  },
];

/** How a server is reached, on one line: the command, or the address. */
export function toolLine(server: Pick<ToolServer, 'kind' | 'program' | 'args' | 'url'>): string {
  if (server.kind !== 'stdio') return server.url;
  const quoted = server.args.map((arg) => (/[\s"']/.test(arg) || arg === '' ? JSON.stringify(arg) : arg));
  return [server.program, ...quoted].join(' ');
}

/** "A program", "At an address", "At an address, over SSE": said next to a server's name. */
export function toolKindWords(kind: ToolKind): string {
  if (kind === 'stdio') return 'A program';
  return kind === 'http' ? 'At an address' : 'At an address, over SSE';
}

/**
 * What a server is handed, by name only: values are keys and are never put on
 * the screen outside the form. A value that is still empty is said, since the
 * server will not work without it.
 */
export function toolHanded(server: Pick<ToolServer, 'kind' | 'env' | 'headers'>): { names: string; empty: string[] } {
  const pairs = server.kind === 'stdio' ? server.env : server.headers;
  // A header that is only "Bearer " has no key in it yet.
  const blank = (value: string) => value.trim() === '' || /^bearer$/i.test(value.trim());
  return {
    names: pairs.map(([name]) => name).join(', '),
    empty: pairs.filter(([, value]) => blank(value)).map(([name]) => name),
  };
}

/** Pairs written one on a line, as "Name: value": how headers are typed. */
export function headersText(headers: [string, string][]): string {
  return headers.map(([name, value]) => `${name}: ${value}`).join('\n');
}

export function parseHeaders(text: string): { headers: [string, string][]; problem: string | null } {
  const headers: [string, string][] = [];
  for (const line of text.split('\n')) {
    if (line.trim() === '') continue;
    const at = line.indexOf(':');
    if (at <= 0)
      return { headers: [], problem: `"${line.trim()}" is not a header. Write a name, a colon and a value.` };
    headers.push([line.slice(0, at).trim(), line.slice(at + 1).trim()]);
  }
  return { headers, problem: null };
}
