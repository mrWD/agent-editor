import { describe, expect, it } from 'vitest';
import { countWords, holds, ISSUE_SORTS, looked, readSort, wordsOf, type Read } from './issueList';

type Row = { key: string; title: string; state: string; at: number; labels: string[]; who: string | null };

const rows: Row[] = [
  {
    key: 'APP-12',
    title: 'Login allows unlimited attempts',
    state: 'In Progress',
    at: 300,
    labels: ['auth'],
    who: 'Ann',
  },
  { key: 'APP-9', title: 'orders need pagination', state: 'Backlog', at: 100, labels: [], who: null },
  { key: 'APP-101', title: 'Billing page is slow', state: 'Backlog', at: 200, labels: ['perf', 'billing'], who: 'Cy' },
];
const read = (row: Row): Read => ({
  key: row.key,
  title: row.title,
  state: row.state,
  changedAt: row.at,
  more: [...row.labels, row.who],
});
const keys = (list: Row[]) => list.map((row) => row.key);

describe('a list of issues that is searched', () => {
  it('finds by the name, the key, the state, a label and who has it, whatever the case', () => {
    expect(keys(looked(rows, 'PAGINATION', 'tracker', read))).toEqual(['APP-9']);
    expect(keys(looked(rows, 'app-12', 'tracker', read))).toEqual(['APP-12']);
    expect(keys(looked(rows, 'backlog', 'tracker', read))).toEqual(['APP-9', 'APP-101']);
    expect(keys(looked(rows, 'perf', 'tracker', read))).toEqual(['APP-101']);
    expect(keys(looked(rows, 'ann', 'tracker', read))).toEqual(['APP-12']);
  });

  it('wants every word of the search, anywhere in the issue', () => {
    expect(wordsOf('  Billing   BACKLOG ')).toEqual(['billing', 'backlog']);
    expect(keys(looked(rows, 'billing backlog', 'tracker', read))).toEqual(['APP-101']);
    expect(keys(looked(rows, 'billing progress', 'tracker', read))).toEqual([]);
    expect(holds(read(rows[0]), [])).toBe(true);
  });

  it('leaves the list as it is for an empty search, and never changes what it was given', () => {
    const before = [...rows];
    expect(looked(rows, '   ', 'tracker', read)).toEqual(rows);
    looked(rows, '', 'name', read);
    expect(rows).toEqual(before);
    // The things in the list are the very things that were given.
    expect(looked(rows, 'login', 'key', read)[0]).toBe(rows[0]);
  });
});

describe('a list of issues that is ordered', () => {
  it('orders by when an issue changed, either way', () => {
    expect(keys(looked(rows, '', 'newest', read))).toEqual(['APP-12', 'APP-101', 'APP-9']);
    expect(keys(looked(rows, '', 'oldest', read))).toEqual(['APP-9', 'APP-101', 'APP-12']);
  });

  it('orders by name without minding capitals, and by key with its number read as a number', () => {
    expect(keys(looked(rows, '', 'name', read))).toEqual(['APP-101', 'APP-12', 'APP-9']);
    expect(keys(looked(rows, '', 'key', read))).toEqual(['APP-9', 'APP-12', 'APP-101']);
  });

  it('orders by state and keeps the tracker’s order among the ones that stand the same', () => {
    expect(keys(looked(rows, '', 'state', read))).toEqual(['APP-9', 'APP-101', 'APP-12']);
  });

  it('orders what a search left', () => {
    expect(keys(looked(rows, 'backlog', 'newest', read))).toEqual(['APP-101', 'APP-9']);
  });

  it('reads a kept order only when it is one there is', () => {
    expect(ISSUE_SORTS.map(([sort]) => sort)).toEqual(['tracker', 'newest', 'oldest', 'name', 'key', 'state']);
    expect(readSort('name')).toBe('name');
    expect(readSort('sideways')).toBe('tracker');
    expect(readSort(null)).toBe('tracker');
  });
});

describe('how many of a list are on show', () => {
  it('says the whole count without a search and "of" with one', () => {
    expect(countWords(12, 12, '')).toBe('12 open');
    expect(countWords(3, 12, 'login')).toBe('3 of 12 open');
    expect(countWords(0, 5, 'x', 'unresolved')).toBe('0 of 5 unresolved');
    expect(countWords(5, 5, '   ', 'unresolved')).toBe('5 unresolved');
  });
});
