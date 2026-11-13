import { describe, expect, it } from 'vitest';
import { checksWord, countWord, failedNote, filesWord, raceTime } from './race';
import { applyEvent, type StartedRace, type Task, type TaskInfo } from './store';

const info = (id: string, agentId: string): TaskInfo => ({
  id,
  projectId: 'p1',
  title: 'Fix the test',
  agentId,
  cwd: '/tmp',
  branch: null,
  createdAt: 0,
  seenSeq: -1,
  state: 'idle',
});

describe('what a race says about its entrants', () => {
  it('tells how the checks of a result stand, and how much that should stand out', () => {
    expect(checksWord({ done: 0, total: 0, failed: 0, running: 0 })).toEqual({ text: 'No checks', tone: 'plain' });
    expect(checksWord({ done: 0, total: 3, failed: 0, running: 0 }).text).toBe('Checks not run yet');
    expect(checksWord({ done: 1, total: 3, failed: 0, running: 2 }).text).toBe('Checks running');
    expect(checksWord({ done: 2, total: 3, failed: 0, running: 0 }).text).toBe('2 of 3 checks done');
    expect(checksWord({ done: 3, total: 3, failed: 0, running: 0 })).toEqual({
      text: 'All checks passed',
      tone: 'good',
    });
    // A failed check wins over the ones that passed.
    expect(checksWord({ done: 2, total: 3, failed: 1, running: 0 })).toEqual({ text: '1 check failed', tone: 'bad' });
    expect(checksWord({ done: 0, total: 3, failed: 2, running: 0 }).text).toBe('2 checks failed');
  });

  it('says how long an entrant took in seconds, minutes or hours', () => {
    expect(raceTime(0)).toBe('1 s');
    expect(raceTime(12_400)).toBe('12 s');
    expect(raceTime(59_000)).toBe('59 s');
    expect(raceTime(4 * 60_000 + 10_000)).toBe('4 min');
    expect(raceTime(3 * 3_600_000)).toBe('3 h');
  });

  it('counts in words', () => {
    expect([2, 3, 4].map(countWord)).toEqual(['two', 'three', 'four']);
    expect(countWord(12)).toBe('12');
    expect([filesWord(1), filesWord(3)]).toEqual(['1 file', '3 files']);
  });
});

describe('agents that did not start with a race', () => {
  const failed = [{ agentId: 'codex', agentName: 'Codex', message: 'Codex did not start: no such program' }];

  it('are named with the reason, and the race goes on with the rest', () => {
    const started: StartedRace = { race: { id: 'p1:r1', tasks: ['t1', 't2'] }, tasks: [], failed };
    expect(failedNote(started)).toBe(
      'Not every agent started, so the race goes on without them. Codex: Codex did not start: no such program',
    );
    expect(failedNote({ ...started, failed: [] })).toBeNull();
  });

  it('leave no race when only one agent is left', () => {
    const note = failedNote({ race: null, tasks: [info('t1', 'mock')], failed });
    expect(note).toContain('so there is no race. Its task is kept as an ordinary task.');
    expect(note).toContain('Codex did not start');
  });
});

describe('a request for a review in the conversation', () => {
  it('is told as a line of its own after the message that asked', () => {
    const task: Task = {
      info: info('t2', 'codex'),
      feed: [],
      lastSeq: -1,
      lastAt: 0,
      changed: [],
      usage: null,
      spentBefore: 0,
    };
    applyEvent(task, { type: 'user_message', text: 'Another agent, Claude, worked on the same task', seq: 0, at: 1 });
    applyEvent(task, { type: 'race_review', of: 't1', ofName: 'Claude', seq: 1, at: 2 });
    expect(task.feed.map((entry) => entry.kind)).toEqual(['user', 'notice']);
    expect(task.feed[1]).toMatchObject({
      text: "You asked for a review of Claude's result. Its change is in the message above.",
      tone: 'muted',
    });
  });
});
