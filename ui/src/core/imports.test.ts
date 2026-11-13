import { describe, expect, it } from 'vitest';
import { matching, placeOf, type ClaudeSession } from './imports';

const session = (title: string, branch: string | null, cwd = '/code/shop'): ClaudeSession => ({
  id: title,
  title,
  startedAt: 1,
  lastAt: 2,
  cwd,
  folderThere: true,
  branch,
  taskId: null,
});

describe('finding a conversation to bring in', () => {
  const all = [
    session('Take the login limit from config', 'fix/login-limit'),
    session('Why is the cart slow?', null, '/code/shop/packages/web'),
    session('займись APP-12706', 'fix/app-12706-employee'),
  ];

  it('keeps every one while nothing is typed', () => {
    expect(matching(all, '')).toEqual(all);
    expect(matching(all, '   ')).toEqual(all);
  });

  it('looks in the first words, the branch and the folder, in any case', () => {
    expect(matching(all, 'LOGIN').map((one) => one.title)).toEqual(['Take the login limit from config']);
    expect(matching(all, 'app-12706').map((one) => one.title)).toEqual(['займись APP-12706']);
    expect(matching(all, 'packages').map((one) => one.title)).toEqual(['Why is the cart slow?']);
    expect(matching(all, 'ЗАЙМИСЬ')).toHaveLength(1);
  });

  it('wants every word that was typed, wherever each is', () => {
    expect(matching(all, 'limit fix/').map((one) => one.title)).toEqual(['Take the login limit from config']);
    expect(matching(all, 'limit cart')).toEqual([]);
  });

  it('says where a conversation worked, from the project’s folder', () => {
    expect(placeOf(all[0]!, '/code/shop')).toBe("the project's folder");
    expect(placeOf(all[1]!, '/code/shop')).toBe('packages/web');
    // A neighbour whose name begins the same is not inside.
    expect(placeOf(session('x', null, '/code/shop-two'), '/code/shop')).toBe('/code/shop-two');
  });
});
