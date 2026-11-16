import { createSignal } from 'solid-js';

/**
 * A list of issues as a person looks through it: only the ones that hold the
 * words typed into the search, in the order picked. The same for the issues of
 * a tracker and for the errors of Sentry: each says what is read in it.
 */

/** How a list is ordered. */
export type IssueSort = 'tracker' | 'newest' | 'oldest' | 'name' | 'key' | 'state';

/** The orders there are, each with the words it is picked by. The first is how the tracker lists them. */
export const ISSUE_SORTS: readonly (readonly [IssueSort, string])[] = [
  ['tracker', 'As the tracker lists them'],
  ['newest', 'Changed last, first'],
  ['oldest', 'Changed longest ago, first'],
  ['name', 'By name'],
  ['key', 'By key'],
  ['state', 'By state'],
];

/** What of an issue is searched and ordered by. */
export type Read = {
  key: string;
  title: string;
  /** How it stands, as its row says it: "In Progress", "open", or the level of an error. */
  state: string;
  /** When it last changed. */
  changedAt: number;
  /** Whatever else its row shows and can be searched for: labels, who has it. */
  more?: readonly (string | null)[];
};

/** The words of a search: what is typed, in small letters, cut at the spaces. */
export function wordsOf(query: string): string[] {
  return query.toLowerCase().split(/\s+/).filter(Boolean);
}

/** Whether an issue holds every word of the search, in its key, its name, its state or the rest its row shows. */
export function holds(read: Read, words: readonly string[]): boolean {
  if (words.length === 0) return true;
  const text = [read.key, read.title, read.state, ...(read.more ?? [])].filter(Boolean).join('\n').toLowerCase();
  return words.every((word) => text.includes(word));
}

const byText = (a: string, b: string) => a.localeCompare(b, undefined, { numeric: true, sensitivity: 'base' });

/**
 * The issues that hold the search, in the order picked. The things in the list stay the very
 * things they were, so a row that is drawn stays drawn. Where two stand the same, the tracker's
 * order decides. "APP-9" comes before "APP-12": numbers in a key or a name are read as numbers.
 */
export function looked<T>(list: readonly T[], query: string, sort: IssueSort, read: (item: T) => Read): T[] {
  const words = wordsOf(query);
  const found = list.filter((item) => holds(read(item), words));
  if (sort === 'tracker') return found;
  const order: Record<Exclude<IssueSort, 'tracker'>, (a: Read, b: Read) => number> = {
    newest: (a, b) => b.changedAt - a.changedAt,
    oldest: (a, b) => a.changedAt - b.changedAt,
    name: (a, b) => byText(a.title, b.title),
    key: (a, b) => byText(a.key, b.key),
    state: (a, b) => byText(a.state, b.state),
  };
  // Sorting keeps the order of what compares the same, in every engine the interface runs in.
  return found.sort((a, b) => order[sort](read(a), read(b)));
}

/** "12 open", or with a search "3 of 12 open": how many of a list are on show. */
export function countWords(shown: number, all: number, query: string, word = 'open'): string {
  return wordsOf(query).length === 0 ? `${all} ${word}` : `${shown} of ${all} ${word}`;
}

/** Where the order is kept: in this browser, as the theme is. */
export const ISSUE_SORT_KEY = 'ae.issues.sort';

/** What was kept, if it is one of the orders there are; else the tracker's. */
export function readSort(kept: string | null): IssueSort {
  return ISSUE_SORTS.find(([sort]) => sort === kept)?.[0] ?? 'tracker';
}

function stored(): IssueSort {
  try {
    return readSort(localStorage.getItem(ISSUE_SORT_KEY));
  } catch {
    // Storage may be refused; the lists are then in the tracker's order.
    return 'tracker';
  }
}

const [issueSort, setSort] = createSignal<IssueSort>(stored());

/** The order the lists of issues are in. */
export { issueSort };

export function setIssueSort(sort: IssueSort) {
  setSort(sort);
  try {
    if (sort === 'tracker') localStorage.removeItem(ISSUE_SORT_KEY);
    else localStorage.setItem(ISSUE_SORT_KEY, sort);
  } catch {
    // The choice then holds for this window only.
  }
}
