import { createSignal } from 'solid-js';
import type { FeedItem } from './store';

/**
 * The conversation as it is read, not as it happened.
 *
 * An agent at work reads files, runs commands and says what it is about to
 * do, a hundred times in a turn. Shown one under another, that is a wall
 * with the answer somewhere at its end. So the work is folded:
 *
 * - **A turn that is over and was answered since** shows what was asked and
 *   the agent's last word. Everything between is one line, "Worked for 12m:
 *   ran 40 commands, read 12 files", that opens when pressed.
 * - **The latest turn** shows what the agent says along the way, with each
 *   run of three or more steps between two messages folded into one line.
 *   One step or two stand as they are.
 * - What the agent thought is part of the fold and not shown outside one.
 *
 * What needs the user is never folded: a question that waits, a note of
 * danger, a rewind. "All steps" in the head of the conversation shows
 * everything, one under another, as it happened.
 */

/** One line of the conversation as it is drawn: an entry by itself, or a run of entries folded into one. */
export type Row =
  | {
      kind: 'entry';
      key: string;
      entry: FeedItem;
      /** For what the agent says: whether it is its first word of the turn, which is where its name stands. */
      lead: boolean;
    }
  | {
      kind: 'fold';
      key: string;
      entries: FeedItem[];
      /** The work of a whole earlier turn, as opposed to a run of steps in the latest. */
      whole: boolean;
      /** The agent is at it right now: the last step is said under the line. */
      live: boolean;
    };

/** Where the choice "All steps" is kept: in this browser, as the theme is. */
export const ALL_STEPS_KEY = 'ae.feed.all';

function stored(): boolean {
  try {
    return localStorage.getItem(ALL_STEPS_KEY) === '1';
  } catch {
    // Storage may be refused; the work is then folded, as designed.
    return false;
  }
}

const [allSteps, setAll] = createSignal(stored());

/** Whether every step is shown, unfolded. */
export { allSteps };

export function setAllSteps(on: boolean) {
  setAll(on);
  try {
    if (on) localStorage.setItem(ALL_STEPS_KEY, '1');
    else localStorage.removeItem(ALL_STEPS_KEY);
  } catch {
    // The choice then holds for this window only.
  }
}

/** A run of this many steps stands as it is: a fold would hide two lines to show one. */
const MOST_UNFOLDED = 2;

/** Whether an entry is work to fold rather than something to read. */
function quiet(entry: FeedItem, latest: boolean, last: boolean): boolean {
  switch (entry.kind) {
    case 'tool':
    case 'thought':
      return true;
    // Of an earlier turn only the last word is read; of the latest, every word.
    case 'agent':
      return !latest && !last;
    case 'plan':
      return !latest;
    case 'permission':
      return entry.answered && !latest;
    default:
      return false;
  }
}

/**
 * The rows of a conversation. `working` is whether the agent has the turn right now;
 * `everything` shows each entry by itself, as it happened.
 */
export function rowsOf(feed: readonly FeedItem[], working: boolean, everything: boolean): Row[] {
  const rows: Row[] = [];
  const lastUser = feed.map((entry) => entry.kind).lastIndexOf('user');
  let spoke = false;
  const push = (entry: FeedItem, index: number) => {
    if (entry.kind === 'user') spoke = false;
    rows.push({ kind: 'entry', key: `e:${index}`, entry, lead: entry.kind === 'agent' && !spoke });
    if (entry.kind === 'agent') spoke = true;
  };
  if (everything) {
    feed.forEach(push);
    return rows;
  }

  // Each turn runs from one message of the user up to the next.
  let from = 0;
  while (from < feed.length) {
    let to = from + 1;
    while (to < feed.length && feed[to]!.kind !== 'user') to += 1;
    const latest = from >= lastUser;
    let lastWord = -1;
    for (let index = from; index < to; index += 1) if (feed[index]!.kind === 'agent') lastWord = index;

    let run: { entry: FeedItem; index: number }[] = [];
    const flush = (atEnd: boolean) => {
      // A thought is read only inside a fold: by itself it is not a step.
      const steps = run.filter((one) => one.entry.kind !== 'thought');
      if (steps.length >= MOST_UNFOLDED + 1) {
        const first = run[0]!;
        rows.push({
          kind: 'fold',
          key: `f:${first.index}:${first.entry.at}`,
          entries: run.map((one) => one.entry),
          whole: !latest,
          live: latest && working && atEnd,
        });
      } else {
        for (const one of steps) push(one.entry, one.index);
      }
      run = [];
    };
    for (let index = from; index < to; index += 1) {
      const entry = feed[index]!;
      if (quiet(entry, latest, index === lastWord)) {
        run.push({ entry, index });
        continue;
      }
      flush(false);
      push(entry, index);
    }
    flush(to === feed.length);
    from = to;
  }
  return rows;
}

/**
 * The rows as they were, wherever nothing changed: a row that is the same row keeps what is
 * drawn for it, so a fold that was opened stays open and a message being read does not jump.
 */
export function keepRows(before: readonly Row[], now: Row[]): Row[] {
  const known = new Map(before.map((row) => [row.key, row]));
  return now.map((row) => {
    const was = known.get(row.key);
    if (!was || was.kind !== row.kind) return row;
    if (row.kind === 'entry' && was.kind === 'entry')
      return was.entry === row.entry && was.lead === row.lead ? was : row;
    if (row.kind === 'fold' && was.kind === 'fold') {
      const same =
        was.entries.length === row.entries.length &&
        was.entries.every((entry, index) => entry === row.entries[index]) &&
        was.whole === row.whole &&
        was.live === row.live;
      return same ? was : row;
    }
    return row;
  });
}

const count = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/** "45s", "3m 12s", "1h 4m": how long something took. */
export function tookWords(ms: number): string {
  const seconds = Math.round(ms / 1000);
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return seconds % 60 === 0 ? `${minutes}m` : `${minutes}m ${seconds % 60}s`;
  return minutes % 60 === 0 ? `${Math.floor(minutes / 60)}h` : `${Math.floor(minutes / 60)}h ${minutes % 60}m`;
}

/** What a run of entries came to, counted: "ran 3 commands", "read 2 files". Files are counted once each. */
export function foldParts(entries: readonly FeedItem[]): string[] {
  const tools = entries.filter((entry): entry is Extract<FeedItem, { kind: 'tool' }> => entry.kind === 'tool');
  const of = (kind: string) => tools.filter((tool) => tool.toolKind === kind);
  const files = (kind: string) => new Set(of(kind).map((tool) => tool.path ?? tool.title)).size;
  const known = ['execute', 'read', 'edit', 'delete', 'search', 'fetch'];
  const others = tools.filter((tool) => !known.includes(tool.toolKind)).length;
  const said = entries.filter((entry) => entry.kind === 'agent').length;
  const parts: [number, string][] = [
    [of('execute').length, `ran ${count(of('execute').length, 'command', 'commands')}`],
    [files('read'), `read ${count(files('read'), 'file', 'files')}`],
    [files('edit'), `edited ${count(files('edit'), 'file', 'files')}`],
    [files('delete'), `deleted ${count(files('delete'), 'file', 'files')}`],
    [of('search').length, `searched ${count(of('search').length, 'time', 'times')}`],
    [of('fetch').length, `fetched ${count(of('fetch').length, 'page', 'pages')}`],
    [others, count(others, 'other step', 'other steps')],
    [said, `said ${count(said, 'thing', 'things')} along the way`],
  ];
  return parts.filter(([n]) => n > 0).map(([, words]) => words);
}

/** How many steps of a run were not done. */
export function foldFailed(entries: readonly FeedItem[]): number {
  return entries.filter((entry) => entry.kind === 'tool' && entry.status === 'failed').length;
}

/**
 * The line a fold is closed to. A run of steps says what they were: "Ran 3 commands, read 2 files".
 * The work of a whole turn says how long it took and how much it was: the rest is inside.
 */
export function foldWords(entries: readonly FeedItem[], whole: boolean): string {
  const parts = foldParts(entries);
  if (!whole) {
    const what = parts.length > 0 ? parts.join(', ') : count(entries.length, 'step', 'steps');
    return what.charAt(0).toUpperCase() + what.slice(1);
  }
  const steps = entries.filter((entry) => entry.kind === 'tool').length;
  const said = entries.filter((entry) => entry.kind === 'agent').length;
  const much = [steps > 0 ? count(steps, 'step', 'steps') : '', said > 0 ? count(said, 'message', 'messages') : '']
    .filter(Boolean)
    .join(', ');
  const took = entries.length > 0 ? entries[entries.length - 1]!.at - entries[0]!.at : 0;
  // A conversation that was brought in may not say when each thing happened.
  const worked = took >= 1000 ? `Worked for ${tookWords(took)}` : 'Worked';
  return much ? `${worked} · ${much}` : worked;
}

/** The first line of a text, cut to a length that fits a row. `null` when the text is that line already. */
export function oneLine(text: string, most = 96): string | null {
  const lines = text.split('\n').filter((line) => line.trim() !== '');
  const first = (lines[0] ?? '').trim();
  if (lines.length <= 1 && first.length <= most && first === text.trim()) return null;
  return first.length > most ? `${first.slice(0, most - 1)}…` : `${first} …`;
}
