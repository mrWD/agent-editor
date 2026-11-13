import { describe, expect, it } from 'vitest';
import { alwaysShown, byFolder, finds, fitCount, foldOrder, totals } from './rail';

const item = (id: string, lastActive: number, flags: { needs?: boolean; current?: boolean } = {}) => ({
  id,
  lastActive,
  needs: flags.needs ?? false,
  current: flags.current ?? false,
});

describe('which projects stay on the line', () => {
  it('puts the ones that need the user first, then the one on screen, then the most recent', () => {
    const order = foldOrder([
      item('old', 1),
      item('asks-old', 2, { needs: true }),
      item('here', 3, { current: true }),
      item('new', 9),
      item('asks-new', 8, { needs: true }),
    ]).map((one) => one.id);
    expect(order).toEqual(['asks-new', 'asks-old', 'here', 'new', 'old']);
  });

  it('never folds away what needs the user, the project on screen, or the one worked in last', () => {
    const ordered = foldOrder([
      item('a', 1, { needs: true }),
      item('b', 2, { current: true }),
      item('c', 3),
      item('d', 4),
    ]);
    expect(alwaysShown(ordered)).toBe(3);
    expect(alwaysShown(foldOrder([item('a', 1), item('b', 2)]))).toBe(1);
    expect(alwaysShown([])).toBe(0);
    // Everything needs the user: nothing can fold.
    expect(alwaysShown([item('a', 1, { needs: true }), item('b', 2, { needs: true })])).toBe(2);
  });

  it('counts how many fit, and keeps the ones that must stay even when they do not', () => {
    expect(fitCount([100, 100, 100, 100], 0, 250)).toBe(2);
    expect(fitCount([100, 100, 100, 100], 3, 250)).toBe(3);
    expect(fitCount([100, 100], 0, 1000)).toBe(2);
    expect(fitCount([100, 100], 5, 0)).toBe(2);
    expect(fitCount([300, 10], 0, 250)).toBe(0);
  });
});

describe('folders of projects', () => {
  it('groups by folder, folders by name, and leaves the rest on their own', () => {
    const grouped = byFolder([
      { name: 'shop', folder: 'client work' },
      { name: 'notes', folder: null },
      { name: 'appose', folder: 'Products' },
      { name: 'billing', folder: 'client work' },
      { name: 'blank', folder: '  ' },
    ]);
    expect(grouped.folders.map((folder) => [folder.name, folder.projects.map((project) => project.name)])).toEqual([
      ['client work', ['shop', 'billing']],
      ['Products', ['appose']],
    ]);
    expect(grouped.loose.map((project) => project.name)).toEqual(['notes', 'blank']);
  });
});

describe('the Find box', () => {
  it('finds every typed word in any order and any case', () => {
    expect(finds('rate login', 'Add rate limiting to /login')).toBe(true);
    expect(finds('LOGIN', 'Add rate limiting to /login')).toBe(true);
    expect(finds('rate signup', 'Add rate limiting to /login')).toBe(false);
    expect(finds('   ', 'anything')).toBe(true);
  });

  it('says how much there is', () => {
    expect(totals(86, 12)).toBe('86 tasks in 12 projects');
    expect(totals(1, 1)).toBe('1 task in 1 project');
  });
});
