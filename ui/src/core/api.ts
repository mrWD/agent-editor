/**
 * The API view of the Preview: what the core says about a task's service, and
 * the words and addresses made of it. Nothing here asks the core for anything.
 */

/** One thing an operation takes beside its body. */
export type ApiParameter = {
  name: string;
  in: 'path' | 'query' | 'header' | 'cookie';
  required: boolean;
  kind: string;
  example: string;
  about: string;
};

export type ApiOperation = {
  /** The method and the path, such as `GET /users/{id}`. */
  id: string;
  method: string;
  path: string;
  summary: string;
  tags: string[];
  deprecated: boolean;
  parameters: ApiParameter[];
  body: { kind: string; required: boolean; example: string } | null;
  responses: string[];
  /** Against the base, when the two were compared. */
  change: 'new' | 'changed' | null;
};

export type ContractChange = { operation: string; level: 'breaking' | 'careful' | 'safe'; text: string };

export type ApiState = {
  /** Where requests go. `null` when no service is known. */
  address: string | null;
  source: { kind: 'file' | 'service'; where: string } | null;
  title: string;
  version: string;
  /** What stands before every path of the description. */
  prefix: string;
  operations: ApiOperation[];
  note: string | null;
  contract: { against: string; changes: ContractChange[] } | null;
  contractNote: string | null;
  settings: { address: string; spec: string; base: string; headers: [string, string][] };
};

export type ApiAnswer = {
  status: number;
  reason: string;
  headers: [string, string][];
  body: string;
  kind: 'json' | 'text' | 'binary';
  size: number;
  cut: boolean;
  took: number;
  sent: { method: string; url: string };
};

/** What the user typed for one operation, kept while they look at another. */
export type ApiDraft = {
  /** By where it goes and its name, such as `query:page`. */
  values: Record<string, string>;
  /** Headers of this request alone, a line each. */
  headers: string;
  body: string;
};

/** The request that is not in the description: any method and any path. */
export const ANY = 'any';

export const valueKey = (parameter: Pick<ApiParameter, 'in' | 'name'>) => `${parameter.in}:${parameter.name}`;

/** A draft to start an operation from: the examples of its description. */
export function draftOf(operation: ApiOperation): ApiDraft {
  const values: Record<string, string> = {};
  for (const parameter of operation.parameters) {
    // An optional filter starts empty: its example would narrow the answer unasked.
    if (parameter.required && parameter.example) values[valueKey(parameter)] = parameter.example;
  }
  return { values, headers: '', body: operation.body?.example ?? '' };
}

/** A piece of a path, with everything that would mean something else written out. */
const piece = (text: string) =>
  encodeURIComponent(text).replace(/[!'()*]/g, (sign) => `%${sign.charCodeAt(0).toString(16).toUpperCase()}`);

/**
 * The path a request for an operation goes to: the blanks filled, the query added.
 * `missing` names the blanks of the path that have no value yet.
 */
export function requestPath(
  prefix: string,
  operation: ApiOperation,
  draft: ApiDraft,
): { path: string; missing: string[] } {
  const missing: string[] = [];
  const filled = operation.path.replace(/\{([^}]+)\}/g, (blank, name: string) => {
    const value = (draft.values[`path:${name}`] ?? '').trim();
    if (value === '') {
      missing.push(name);
      return blank;
    }
    return piece(value);
  });
  const query = operation.parameters
    .filter((parameter) => parameter.in === 'query')
    .map((parameter) => [parameter.name, (draft.values[valueKey(parameter)] ?? '').trim()] as const)
    .filter(([, value]) => value !== '')
    .map(([name, value]) => `${piece(name)}=${piece(value)}`)
    .join('&');
  return { path: `${prefix}${filled}${query ? `?${query}` : ''}`, missing };
}

/** The headers of a request for an operation: its header parameters, then the lines typed for it. */
export function requestHeaders(
  operation: ApiOperation | null,
  draft: ApiDraft,
): { headers: [string, string][]; problem: string | null } {
  const headers: [string, string][] = [];
  for (const parameter of operation?.parameters ?? []) {
    const value = (draft.values[valueKey(parameter)] ?? '').trim();
    if (parameter.in === 'header' && value !== '') headers.push([parameter.name, value]);
  }
  for (const line of draft.headers.split('\n')) {
    if (line.trim() === '') continue;
    const at = line.indexOf(':');
    if (at <= 0)
      return { headers: [], problem: `"${line.trim()}" is not a header. Write a name, a colon and a value.` };
    headers.push([line.slice(0, at).trim(), line.slice(at + 1).trim()]);
  }
  // A body is said to be what the description says it is, unless the user said otherwise.
  const said = headers.some(([name]) => name.toLowerCase() === 'content-type');
  if (!said && draft.body.trim() !== '') headers.push(['Content-Type', operation?.body?.kind ?? guessKind(draft.body)]);
  return { headers, problem: null };
}

/** What a body typed by hand most likely is. */
export function guessKind(body: string): string {
  const start = body.trim()[0];
  return start === '{' || start === '[' || start === '"' ? 'application/json' : 'text/plain';
}

/** Why a body cannot be sent as it is, or `null`. Only JSON is checked: the rest is the service's to judge. */
export function bodyProblem(kind: string, body: string): string | null {
  if (!kind.includes('json') || body.trim() === '') return null;
  try {
    JSON.parse(body);
    return null;
  } catch (error) {
    return `The body is not JSON: ${error instanceof Error ? error.message : String(error)}`;
  }
}

/** "1.2 KB": a size a person reads. */
export function sizeWords(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(bytes < 10 * 1024 ? 1 : 0)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

/** "12 ms" or "1.4 s". */
export function tookWords(ms: number): string {
  return ms < 1000 ? `${ms} ms` : `${(ms / 1000).toFixed(1)} s`;
}

/** How a status stands: it worked, it was sent elsewhere, the request was wrong, the service failed. */
export function statusTone(status: number): 'good' | 'moved' | 'refused' | 'failed' {
  if (status >= 500) return 'failed';
  if (status >= 400) return 'refused';
  if (status >= 300) return 'moved';
  return 'good';
}

/** The body of an answer the way it is easiest to read: JSON laid out, the rest as it came. */
export function shownBody(answer: Pick<ApiAnswer, 'kind' | 'body'>): string {
  if (answer.kind !== 'json') return answer.body;
  try {
    return JSON.stringify(JSON.parse(answer.body), null, 2);
  } catch {
    return answer.body;
  }
}

/** The operations that match what was typed into the filter: every word is in the method, the path, the summary or a tag. */
export function matching(operations: ApiOperation[], filter: string): ApiOperation[] {
  const words = filter.toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return operations;
  return operations.filter((operation) => {
    const text = `${operation.method} ${operation.path} ${operation.summary} ${operation.tags.join(' ')}`.toLowerCase();
    return words.every((word) => text.includes(word));
  });
}

/** "2 breaking, 1 to check, 3 safe": the changes of a contract, counted by weight. */
export function contractWords(changes: ContractChange[]): string {
  if (changes.length === 0) return 'No change in the contract';
  const count = (level: ContractChange['level']) => changes.filter((change) => change.level === level).length;
  const parts = [
    [count('breaking'), 'breaking'],
    [count('careful'), 'to check'],
    [count('safe'), 'safe'],
  ] as const;
  return parts
    .filter(([n]) => n > 0)
    .map(([n, word]) => `${n} ${word}`)
    .join(', ');
}

export const levelWord: Record<ContractChange['level'], string> = {
  breaking: 'Breaks clients',
  careful: 'Worth a look',
  safe: 'Safe',
};

/** How much of an answer is put before the agent. More than this is cut and said to be cut. */
const MOST_FOR_AGENT = 6000;

/**
 * A request and its answer as text for the agent. The values of the headers kept for the
 * project are left out: one of them is usually a key.
 */
export function answerForAgent(
  answer: ApiAnswer,
  sent: { headers: [string, string][]; body: string },
  kept: [string, string][],
): { title: string; text: string } {
  const secret = new Set(kept.map(([name]) => name.toLowerCase()));
  const lines = [`${answer.sent.method} ${answer.sent.url}`];
  for (const [name] of kept) lines.push(`${name}: (kept for the project, not shown)`);
  for (const [name, value] of sent.headers) if (!secret.has(name.toLowerCase())) lines.push(`${name}: ${value}`);
  if (sent.body.trim() !== '') lines.push('', sent.body.trim());
  const body = answer.kind === 'binary' ? `(${sizeWords(answer.size)} that are not text)` : shownBody(answer);
  const cut = body.length > MOST_FOR_AGENT;
  lines.push(
    '',
    `Answered ${answer.status}${answer.reason ? ` ${answer.reason}` : ''} in ${tookWords(answer.took)}:`,
    cut ? `${body.slice(0, MOST_FOR_AGENT)}\n… (cut: the answer is ${sizeWords(answer.size)})` : body,
  );
  const where = answer.sent.url.replace(/^https?:\/\/[^/]+/, '');
  return { title: `${answer.sent.method} ${where.split('?')[0]} answered ${answer.status}`, text: lines.join('\n') };
}
