import { describe, expect, it } from 'vitest';
import { firstTab, tabAfter, tabCount } from './changesTabs';
import type { GitState } from './store';

const commit = (pushed: boolean) => ({ hash: 'h', short: 'h', subject: 's', author: 'a', at: 0, pushed });
const file = { path: 'a.ts', kind: 'modified', added: 1, removed: 0, binary: false, changes: 1 };

function git(files: number, notPushed: number, pushed: number): GitState {
  return {
    branch: 'task/x',
    base: 'main',
    baseAhead: 0,
    remote: 'origin',
    notPushed,
    files: Array.from({ length: files }, () => file),
    commits: [
      ...Array.from({ length: notPushed }, () => commit(false)),
      ...Array.from({ length: pushed }, () => commit(true)),
    ],
  } as GitState;
}

describe('the parts of the Changes view', () => {
  it('counts files that are not committed, commits that are not pushed and commits that are', () => {
    const state = git(3, 2, 4);
    expect([tabCount('head', state), tabCount('ahead', state), tabCount('request', state)]).toEqual([3, 2, 4]);
    expect(tabCount('head', null)).toBe(0);
  });

  it('opens on the first part that holds something', () => {
    expect(firstTab(git(1, 1, 1))).toBe('head');
    expect(firstTab(git(0, 1, 1))).toBe('ahead');
    expect(firstTab(git(0, 0, 1))).toBe('request');
    // A task with nothing done yet: where the first change will show.
    expect(firstTab(git(0, 0, 0))).toBe('head');
    expect(firstTab(null)).toBe('head');
  });

  it('follows a change along its path, and stays while there is more of the same', () => {
    // Everything was committed: on to the commits.
    expect(tabAfter('head', git(0, 1, 0))).toBe('ahead');
    // Committed and pushed in one go: on to the request.
    expect(tabAfter('head', git(0, 0, 1))).toBe('request');
    // Some files were left out of the commit: they are still to be looked at.
    expect(tabAfter('head', git(2, 1, 0))).toBe('head');
    // The commits were pushed.
    expect(tabAfter('ahead', git(0, 0, 2))).toBe('request');
    // The only commit was undone: its changes are files again.
    expect(tabAfter('ahead', git(2, 0, 0))).toBe('head');
    // A pushed commit was reverted: the new commit waits, but what is pushed is still there.
    expect(tabAfter('request', git(0, 1, 2))).toBe('request');
  });
});
