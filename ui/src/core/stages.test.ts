import { describe, expect, it } from 'vitest';
import { listed, restLine, stageOf, summary } from './stages';
import type { TaskState } from './store';

let next = 0;
const task = (state: TaskState, id = `t${next++}`) => ({ info: { id, state } });
const many = (state: TaskState, count: number) => Array.from({ length: count }, () => task(state));

describe('where a task stands', () => {
  it('does not depend on whether an agent process is running', () => {
    expect(stageOf('idle')).toBe('turn');
    expect(stageOf('asleep')).toBe('turn');
    expect(stageOf('waiting_permission')).toBe('needs');
    expect(stageOf('running')).toBe('running');
  });

  it('lets the merge request decide once the agent has nothing to ask', () => {
    const open = { state: 'open' as const, ready: true, checks: 'passed' as const };
    expect(stageOf('idle', open)).toBe('ready');
    expect(stageOf('asleep', { ...open, ready: false, checks: 'pending' })).toBe('checks');
    expect(stageOf('idle', { ...open, ready: false, checks: 'failed' })).toBe('needs');
    expect(stageOf('idle', { state: 'merged', ready: false, checks: 'passed' })).toBe('merged');
    expect(stageOf('idle', { state: 'closed', ready: false, checks: 'none' })).toBe('turn');
    // A question from the agent comes first; a turn under way is shown as running.
    expect(stageOf('waiting_permission', open)).toBe('needs');
    expect(stageOf('running', open)).toBe('running');
    expect(stageOf('idle', null)).toBe('turn');
  });

  it('counts by the stage it is given when a task carries one', () => {
    const tasks = [
      { ...task('idle'), stage: 'ready' as const },
      { ...task('idle'), stage: 'merged' as const },
      task('idle'),
    ];
    expect(summary(tasks)).toBe('1 ready · 1 your turn · 3 tasks');
    expect(summary(tasks, false)).toBe('1 ready to merge · 1 your turn');
    expect(restLine([tasks[1]!, task('running')])).toBe('2 more: 1 merged, 1 running');
  });

  it('sums up a project with only the stages that have something in them', () => {
    const tasks = [task('waiting_permission'), ...many('asleep', 2), ...many('running', 6)];
    expect(summary(tasks)).toBe('1 needs you · 2 your turn · 9 tasks');
    expect(summary([...many('waiting_permission', 3)])).toBe('3 need you · 3 tasks');
    expect(summary([task('running')])).toBe('1 task');
    expect(summary(many('idle', 2), false)).toBe('2 your turn');
    expect(summary(many('running', 2), false)).toBe('');
  });

  it('says what the tasks that are not listed by name are doing', () => {
    expect(restLine(many('running', 3))).toBe('3 more are running');
    expect(restLine(many('running', 1))).toBe('1 more is running');
    expect(restLine([task('idle'), ...many('running', 3)])).toBe('4 more: 1 your turn, 3 running');
    expect(restLine(many('asleep', 2))).toBe('2 more: your turn');
  });

  it('lists the first tasks by name and never leaves out the one on screen', () => {
    const tasks = [task('waiting_permission', 'a'), task('idle', 'b'), task('running', 'c'), task('running', 'd')];
    expect(listed(tasks, 6, null).map((t) => t.info.id)).toEqual(['a', 'b', 'c', 'd']);
    expect(listed(tasks, 2, null).map((t) => t.info.id)).toEqual(['a', 'b']);
    expect(listed(tasks, 2, 'd').map((t) => t.info.id)).toEqual(['a', 'd']);
    expect(listed(tasks, 2, 'a').map((t) => t.info.id)).toEqual(['a', 'b']);
  });
});
