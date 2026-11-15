import { describe, expect, it } from 'vitest';
import { foldFailed, foldParts, foldWords, keepRows, oneLine, rowsOf, tookWords, type Row } from './feed';
import type { FeedItem } from './store';

let clock = 0;
const at = () => (clock += 1000);
const user = (text: string): FeedItem => ({ kind: 'user', text, edited: [], images: [], at: at() });
const agent = (text: string): FeedItem => ({ kind: 'agent', text, at: at() });
const thought = (text: string): FeedItem => ({ kind: 'thought', text, at: at() });
const tool = (toolKind: string, title: string, path: string | null = null, status = 'completed'): FeedItem => ({
  kind: 'tool',
  id: `${toolKind}-${title}`,
  title,
  toolKind,
  status,
  path,
  diff: null,
  at: at(),
});
const plan = (): FeedItem => ({ kind: 'plan', entries: [{ content: 'Read', status: 'completed' }], at: at() });
const asked = (answered: boolean): FeedItem => ({
  kind: 'permission',
  requestId: 'r1',
  title: 'Edit src/a.ts',
  toolKind: 'edit',
  diff: null,
  options: [],
  answered,
  chosen: answered ? 'allow' : null,
  at: at(),
});

/** The rows in a few signs: an entry by its kind (with `*` where the agent's name stands), a fold by what it holds. */
const drawn = (rows: Row[]) =>
  rows.map((row) =>
    row.kind === 'entry'
      ? `${row.entry.kind}${row.lead ? '*' : ''}`
      : `[${row.entries.map((entry) => entry.kind).join(' ')}]${row.whole ? ' whole' : ''}${row.live ? ' live' : ''}`,
  );

describe('the conversation as it is read', () => {
  it('folds each run of three steps or more in the latest turn and leaves what the agent says open', () => {
    const feed = [
      user('Fix the limit'),
      plan(),
      thought('Where is it?'),
      tool('read', 'Read a.ts', 'src/a.ts'),
      tool('search', 'grep limit'),
      tool('execute', 'npm test'),
      agent('It is in a.ts.'),
      tool('edit', 'Edit a.ts', 'src/a.ts'),
      tool('execute', 'npm test'),
      agent('Done.'),
    ];
    expect(drawn(rowsOf(feed, false, false))).toEqual([
      'user',
      'plan',
      '[thought tool tool tool]',
      'agent*',
      'tool',
      'tool',
      'agent',
    ]);
  });

  it('shows one step or two as they are, and a thought only inside a fold', () => {
    const feed = [user('Look'), thought('Hm'), tool('read', 'Read a.ts', 'src/a.ts'), thought('I see'), agent('Here.')];
    expect(drawn(rowsOf(feed, false, false))).toEqual(['user', 'tool', 'agent*']);
    expect(drawn(rowsOf([user('Look'), thought('Hm'), agent('Here.')], false, false))).toEqual(['user', 'agent*']);
  });

  it('folds a turn that was answered since down to what was asked and the last word', () => {
    const feed = [
      user('First'),
      plan(),
      tool('read', 'Read a.ts', 'src/a.ts'),
      agent('Looking.'),
      asked(true),
      tool('edit', 'Edit a.ts', 'src/a.ts'),
      agent('Changed it.'),
      user('Second'),
      tool('execute', 'npm test'),
      agent('Green.'),
    ];
    expect(drawn(rowsOf(feed, false, false))).toEqual([
      'user',
      '[plan tool agent permission tool] whole',
      'agent*',
      'user',
      'tool',
      'agent*',
    ]);
  });

  it('never folds what needs the user', () => {
    const notice: FeedItem = { kind: 'notice', text: 'The agent stopped', tone: 'danger', at: at() };
    const steps = [tool('read', 'a', 'a'), tool('read', 'b', 'b'), tool('read', 'c', 'c')];
    const feed = [user('First'), ...steps, asked(false), notice, user('Second')];
    expect(drawn(rowsOf(feed, false, false))).toEqual([
      'user',
      '[tool tool tool] whole',
      'permission',
      'notice',
      'user',
    ]);
  });

  it('says what the agent is at while the last fold is still growing', () => {
    const steps = [
      tool('execute', 'npm ci'),
      tool('execute', 'npm run build'),
      tool('execute', 'npm test', null, 'in_progress'),
    ];
    const feed = [user('Go'), ...steps];
    expect(drawn(rowsOf(feed, true, false))).toEqual(['user', '[tool tool tool] live']);
    expect(drawn(rowsOf(feed, false, false))).toEqual(['user', '[tool tool tool]']);
    // A fold that a message has closed is not the one being worked in.
    expect(drawn(rowsOf([...feed, agent('Tests run.')], true, false))).toEqual(['user', '[tool tool tool]', 'agent*']);
  });

  it('shows everything one under another when asked to, with the name once in a turn', () => {
    const feed = [
      user('Go'),
      thought('Hm'),
      tool('read', 'a', 'a'),
      agent('One.'),
      tool('read', 'b', 'b'),
      agent('Two.'),
    ];
    expect(drawn(rowsOf(feed, false, true))).toEqual(['user', 'thought', 'tool', 'agent*', 'tool', 'agent']);
  });

  it('keeps a row that did not change, so what is drawn for it stays', () => {
    const feed = [user('Go'), tool('read', 'a', 'a'), tool('read', 'b', 'b'), tool('read', 'c', 'c'), agent('One.')];
    const before = keepRows([], rowsOf(feed, true, false));
    const grown = [...feed, tool('execute', 'npm ci'), tool('execute', 'npm test'), tool('execute', 'npm run lint')];
    const after = keepRows(before, rowsOf(grown, true, false));
    expect(drawn(after)).toEqual(['user', '[tool tool tool]', 'agent*', '[tool tool tool] live']);
    expect(after.slice(0, 3).every((row, index) => row === before[index])).toBe(true);
    // The fold that grew is a new row under the same name: it stays open if it was.
    const more = keepRows(after, rowsOf([...grown, tool('execute', 'npm run build')], true, false));
    expect(more[3]).not.toBe(after[3]);
    expect(more[3]!.key).toBe(after[3]!.key);
  });
});

describe('the line a fold is closed to', () => {
  it('counts the steps by what they were, each file once', () => {
    const entries = [
      tool('execute', 'npm ci'),
      tool('execute', 'npm test'),
      tool('read', 'Read a.ts', 'src/a.ts'),
      tool('read', 'Read a.ts again', 'src/a.ts'),
      tool('read', 'Read b.ts', 'src/b.ts'),
      tool('edit', 'Edit a.ts', 'src/a.ts'),
      tool('search', 'grep'),
      tool('fetch', 'https://example.com'),
      tool('think', 'Plan'),
      thought('Hm'),
    ];
    expect(foldParts(entries)).toEqual([
      'ran 2 commands',
      'read 2 files',
      'edited 1 file',
      'searched 1 time',
      'fetched 1 page',
      '1 other step',
    ]);
    expect(foldWords(entries.slice(0, 5), false)).toBe('Ran 2 commands, read 2 files');
    expect(foldWords([thought('a'), thought('b')], false)).toBe('2 steps');
  });

  it('says of a whole turn how long it took and how much it was', () => {
    clock = 0;
    const entries = [tool('execute', 'npm ci'), agent('Installing.'), tool('execute', 'npm test', null, 'failed')];
    expect(foldWords(entries, true)).toBe('Worked for 2s · 2 steps, 1 message');
    expect(foldFailed(entries)).toBe(1);
    // A conversation that was brought in may give every entry the same moment.
    const sameMoment = entries.map((entry) => ({ ...entry, at: 5 }));
    expect(foldWords(sameMoment, true)).toBe('Worked · 2 steps, 1 message');
  });

  it('reads a time as a person says it', () => {
    expect([
      tookWords(45_400),
      tookWords(192_000),
      tookWords(180_000),
      tookWords(3_960_000),
      tookWords(7_200_000),
    ]).toEqual(['45s', '3m 12s', '3m', '1h 6m', '2h']);
  });

  it('cuts a long command to its first line, and leaves a short one alone', () => {
    expect(oneLine('npm test')).toBeNull();
    expect(oneLine('cd ~/app &&\n  npm test\n')).toBe('cd ~/app && …');
    expect(oneLine('x'.repeat(200))).toBe(`${'x'.repeat(95)}…`);
    expect(oneLine('\n\nnpm test\n')).toBeNull();
  });
});
