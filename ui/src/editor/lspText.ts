import type { Text } from '@codemirror/state';

/**
 * The plain parts of talking to a language server: turning places, addresses
 * and answers of the protocol into what the editor works with, and back.
 * Nothing here touches the editor or the core, so it is tested on its own.
 */

/** A place in a file as the protocol names it: line and character, both from 0. */
export type Position = { line: number; character: number };
export type Range = { start: Position; end: Position };

/** The protocol's place for an offset in the text. Both count in the same units as the editor does. */
export function positionAt(doc: Text, offset: number): Position {
  const line = doc.lineAt(Math.min(Math.max(0, offset), doc.length));
  return { line: line.number - 1, character: offset - line.from };
}

/**
 * The offset of a place the server named. The server may speak of an older
 * text, so a place past the end of a line or of the file is pulled back in.
 */
export function offsetAt(doc: Text, position: Position | undefined): number {
  if (!position) return 0;
  if (position.line >= doc.lines) return doc.length;
  const line = doc.line(Math.max(0, position.line) + 1);
  return Math.min(line.from + Math.max(0, position.character), line.to);
}

/** A path without the slashes at its end. Written out, because a regular expression for it backtracks. */
function withoutLastSlashes(path: string): string {
  let end = path.length;
  while (end > 0 && path[end - 1] === '/') end--;
  return path.slice(0, end);
}

/** The address of a file of the task, from the address of the task's folder. */
export function uriOf(root: string, path: string): string {
  const folder = withoutLastSlashes(root);
  return `${folder}/${path.split('/').map(encodeURIComponent).join('/')}`;
}

/** A `file://` address as a plain path, so two spellings of one address compare equal. */
function plainPath(uri: string): string | null {
  if (!/^file:\/\//i.test(uri)) return null;
  let path = uri.slice('file://'.length);
  try {
    path = decodeURIComponent(path);
  } catch {
    // Not escaped the usual way: take it as it is written.
  }
  // On Windows servers write the drive letter in either case.
  path = path.replace(/^\/([A-Za-z]):/, (_, drive: string) => `/${drive.toLowerCase()}:`);
  return withoutLastSlashes(path);
}

/**
 * The path of an address from the task's folder, or `null` when the address
 * is not inside that folder. Only files of the task are ever opened.
 */
export function pathInRoot(root: string, uri: string): string | null {
  const folder = plainPath(root);
  const full = plainPath(uri);
  if (folder === null || full === null || !full.startsWith(`${folder}/`)) return null;
  const path = full.slice(folder.length + 1);
  // "inside/../../outside" starts with the folder and is not in it.
  if (path === '' || path.split('/').some((part) => part === '..' || part === '.')) return null;
  return path;
}

/** How bad a problem is, in the editor's words. The protocol counts from 1, worst first. */
export function severityOf(severity: unknown): 'error' | 'warning' | 'info' | 'hint' {
  switch (severity) {
    case 2:
      return 'warning';
    case 3:
      return 'info';
    case 4:
      return 'hint';
    default:
      // A server that does not say how bad it is means it.
      return 'error';
  }
}

/** The small picture next to a completion, from the protocol's number for its kind. */
export function completionType(kind: unknown): string | undefined {
  const names: Record<number, string> = {
    1: 'text',
    2: 'method',
    3: 'function',
    4: 'function',
    5: 'property',
    6: 'variable',
    7: 'class',
    8: 'interface',
    9: 'namespace',
    10: 'property',
    11: 'constant',
    12: 'constant',
    13: 'enum',
    14: 'keyword',
    15: 'text',
    20: 'constant',
    21: 'constant',
    22: 'class',
    23: 'function',
    24: 'keyword',
    25: 'type',
  };
  return typeof kind === 'number' ? names[kind] : undefined;
}

/** What a server says about the word under the pointer, in one text. `markdown` says how to show it. */
export function hoverText(contents: unknown): { text: string; markdown: boolean } | null {
  const one = (part: unknown): { text: string; markdown: boolean } | null => {
    if (typeof part === 'string') return { text: part, markdown: true };
    if (!part || typeof part !== 'object') return null;
    const { kind, language, value } = part as { kind?: unknown; language?: unknown; value?: unknown };
    if (typeof value !== 'string') return null;
    // The older way to send code: a language and the code itself.
    if (typeof language === 'string') return { text: '```' + language + '\n' + value + '\n```', markdown: true };
    return { text: value, markdown: kind !== 'plaintext' };
  };
  const parts = (Array.isArray(contents) ? contents : [contents]).map(one).filter((part) => part !== null);
  const text = parts
    .map((part) => part.text.trim())
    .filter(Boolean)
    .join('\n\n');
  if (text === '') return null;
  return { text, markdown: parts.some((part) => part.markdown) };
}

/** Where a definition is: the first place of an answer, whichever of its three shapes the server chose. */
export function firstLocation(answer: unknown): { uri: string; line: number; character: number } | null {
  const first = Array.isArray(answer) ? answer[0] : answer;
  if (!first || typeof first !== 'object') return null;
  const place = first as {
    uri?: unknown;
    range?: Range;
    targetUri?: unknown;
    targetSelectionRange?: Range;
    targetRange?: Range;
  };
  const uri = place.targetUri ?? place.uri;
  const start = (place.targetSelectionRange ?? place.targetRange ?? place.range)?.start;
  if (typeof uri !== 'string' || !start) return null;
  return { uri, line: Math.max(0, Number(start.line) || 0), character: Math.max(0, Number(start.character) || 0) };
}

export type Change = { from: number; to: number; insert: string };

/**
 * A server's edits as changes to the text they were made for. Each edit is
 * cut down to what it really changes: a formatter that sends the whole file
 * back would otherwise move the caret to the end of it.
 */
export function changesOf(doc: Text, edits: unknown): Change[] {
  if (!Array.isArray(edits)) return [];
  const changes: Change[] = [];
  for (const edit of edits as { range?: Range; newText?: unknown }[]) {
    if (!edit?.range || typeof edit.newText !== 'string') continue;
    let from = offsetAt(doc, edit.range.start);
    let to = Math.max(from, offsetAt(doc, edit.range.end));
    // Servers write line ends their own way; the editor keeps one kind.
    let insert = edit.newText.replace(/\r\n?/g, '\n');
    const old = doc.sliceString(from, to);
    let same = 0;
    while (same < old.length && same < insert.length && old[same] === insert[same]) same++;
    let sameEnd = 0;
    while (
      sameEnd < old.length - same &&
      sameEnd < insert.length - same &&
      old[old.length - 1 - sameEnd] === insert[insert.length - 1 - sameEnd]
    ) {
      sameEnd++;
    }
    from += same;
    to -= sameEnd;
    insert = insert.slice(same, insert.length - sameEnd);
    if (from !== to || insert !== '') changes.push({ from, to, insert });
  }
  return changes;
}

/**
 * Whether a completion could be meant by what was typed: its letters appear
 * in the label in the same order, in either case. This is the question the
 * editor asks before it shows a completion, asked early.
 */
export function couldMean(label: string, typed: string): boolean {
  const wanted = typed.toLowerCase();
  const text = label.toLowerCase();
  let at = 0;
  for (const letter of wanted) {
    at = text.indexOf(letter, at);
    if (at < 0) return false;
    at += 1;
  }
  return true;
}
