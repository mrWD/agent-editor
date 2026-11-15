import type { GitState } from './store';

/**
 * The three parts of the Changes view, along the path of a change: what is not committed, what is
 * committed and not pushed, and what is pushed, where its request is.
 */
export type ChangesTab = 'head' | 'ahead' | 'request';

export const CHANGES_TABS: ChangesTab[] = ['head', 'ahead', 'request'];

/** How many things a part holds: files that are not committed, commits that are not pushed, commits that are. */
export function tabCount(tab: ChangesTab, git: GitState | null): number {
  if (!git) return 0;
  if (tab === 'head') return git.files.length;
  if (tab === 'ahead') return git.notPushed;
  return git.commits.length - git.notPushed;
}

/** The first part that holds something. With nothing anywhere it is the first: that is where work shows up. */
export function firstTab(git: GitState | null): ChangesTab {
  return CHANGES_TABS.find((tab) => tabCount(tab, git) > 0) ?? 'head';
}

/**
 * The part to show after something was done: the same one while it still holds something, else
 * the first that does. A commit of everything leads to the commits, a push of them to the request.
 */
export function tabAfter(now: ChangesTab, git: GitState | null): ChangesTab {
  return tabCount(now, git) > 0 ? now : firstTab(git);
}
