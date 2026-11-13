import { describe, expect, it } from 'vitest';
import { mergeShells, shellName, type ShellTab } from './shells';
import type { TermInfo } from './store';

const tab = (key: number, change: Partial<ShellTab> = {}): ShellTab => ({
  key,
  termId: key * 10,
  number: key,
  running: null,
  ended: false,
  used: true,
  ...change,
});
const shell = (termId: number, number: number, running: string | null = null): TermInfo => ({
  termId,
  taskId: 'task',
  title: 'sh',
  purpose: 'shell',
  number,
  running,
});
const keys = (from: number) => () => ++from;

describe('the name of a shell', () => {
  it('is "Terminal" for the first and "Terminal" with its number for the others', () => {
    const all = [tab(1), tab(2), tab(5)];
    expect(all.map((one) => shellName(one, all))).toEqual(['Terminal', 'Terminal 2', 'Terminal 5']);
  });

  it('is the command that runs in it, for as long as it runs', () => {
    const busy = tab(2, { running: 'npm test' });
    expect(shellName(busy, [tab(1), busy])).toBe('npm test');
    expect(shellName({ ...busy, running: null }, [tab(1), busy])).toBe('Terminal 2');
  });

  it('is guessed for a shell that is only now being started: the number after the highest', () => {
    const fresh = tab(9, { termId: null, number: null });
    expect(shellName(fresh, [fresh])).toBe('Terminal');
    expect(shellName(fresh, [tab(1), tab(3), fresh])).toBe('Terminal 4');
  });
});

describe('bringing the tabs in line with the shells of the core', () => {
  it('keeps the tabs it knows, with their new names, and adds the shells it has no tab for', () => {
    const before = [tab(1), tab(2)];
    const after = mergeShells(before, [shell(10, 1), shell(20, 2, 'sleep 300'), shell(70, 3)], keys(2));
    expect(after.map((one) => [one.key, one.termId, one.number, one.running, one.used])).toEqual([
      [1, 10, 1, null, true],
      [2, 20, 2, 'sleep 300', true],
      // Started in another window: it gets a tab here, and is drawn when that tab is looked at.
      [3, 70, 3, null, false],
    ]);
  });

  it('takes away a tab whose shell the core no longer has', () => {
    expect(mergeShells([tab(1), tab(2)], [shell(20, 2)], keys(2)).map((one) => one.key)).toEqual([2]);
  });

  it('leaves a tab that says its shell ended, and one whose shell is being started', () => {
    const ended = tab(1, { termId: null, ended: true });
    const starting = tab(2, { termId: null, number: null });
    expect(mergeShells([ended, starting], [], keys(2))).toEqual([ended, starting]);
  });

  it('does not take the programs of the Run tab for shells', () => {
    const dev: TermInfo = { ...shell(30, 0), purpose: 'run:dev', number: null };
    expect(mergeShells([], [dev, shell(40, 1)], keys(0)).map((one) => one.termId)).toEqual([40]);
  });
});
