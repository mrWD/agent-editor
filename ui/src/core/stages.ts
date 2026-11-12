import type { TaskState } from './store';

/**
 * Where a task stands, as the user thinks of it. Whether an agent process is
 * running right now is not part of that: a task whose agent has stopped waits
 * for the user's next word just like one whose agent is idle.
 */
export type Stage = 'needs' | 'ready' | 'checks' | 'merged' | 'turn' | 'running';

/** The stages in the order they are always listed: what needs the user comes first. */
export const STAGES: Stage[] = ['needs', 'ready', 'checks', 'merged', 'turn', 'running'];

/** What is known of a task's merge request, as far as the stage goes. */
export type RequestStage = {
  state: 'open' | 'merged' | 'closed';
  ready: boolean;
  checks: 'none' | 'pending' | 'passed' | 'failed';
};

/**
 * A question from the agent comes before anything else. After that the merge
 * request decides: merged and not cleaned up, ready to merge, or checks running.
 */
export function stageOf(state: TaskState, request?: RequestStage | null): Stage {
  if (state === 'waiting_permission') return 'needs';
  if (request?.state === 'merged') return 'merged';
  if (state === 'running') return 'running';
  if (request?.state === 'open') {
    if (request.checks === 'failed') return 'needs';
    if (request.ready) return 'ready';
    if (request.checks === 'pending') return 'checks';
  }
  return 'turn';
}

/** What each stage is called next to a task's name. */
export function stageWord(stage: Stage): string {
  switch (stage) {
    case 'needs':
      return 'needs you';
    case 'ready':
      return 'ready';
    case 'checks':
      return 'checks running';
    case 'merged':
      return 'merged';
    case 'turn':
      return 'your turn';
    case 'running':
      return 'running';
  }
}

/** The least of a task these helpers need. */
type Staged = { info: { id: string; state: TaskState }; stage?: Stage };

const stageOfTask = (task: Staged): Stage => task.stage ?? stageOf(task.info.state);

export function countIn(tasks: Staged[], stage: Stage): number {
  return tasks.filter((task) => stageOfTask(task) === stage).length;
}

/** "1 needs you · 2 ready · 9 tasks": what wants the user, then what can be finished. */
export function summary(tasks: Staged[], withTotal = true): string {
  const needs = countIn(tasks, 'needs');
  const ready = countIn(tasks, 'ready');
  const turn = countIn(tasks, 'turn');
  const parts: string[] = [];
  if (needs > 0) parts.push(`${needs} ${needs === 1 ? 'needs' : 'need'} you`);
  if (ready > 0) parts.push(withTotal ? `${ready} ready` : `${ready} ready to merge`);
  if (turn > 0) parts.push(`${turn} your turn`);
  if (withTotal) parts.push(tasks.length === 1 ? '1 task' : `${tasks.length} tasks`);
  return parts.join(' · ');
}

/** "3 more are running", or "4 more: 1 merged, 3 running" when the rest is mixed. */
export function restLine(rest: Staged[]): string {
  const words: Record<Stage, string> = {
    needs: 'need you',
    ready: 'ready',
    checks: 'checks running',
    merged: 'merged',
    turn: 'your turn',
    running: 'running',
  };
  const present = STAGES.filter((stage) => countIn(rest, stage) > 0);
  if (present.length === 1) {
    const only = present[0]!;
    if (only === 'running') return rest.length === 1 ? '1 more is running' : `${rest.length} more are running`;
    return `${rest.length} more: ${words[only]}`;
  }
  return `${rest.length} more: ${present.map((stage) => `${countIn(rest, stage)} ${words[stage]}`).join(', ')}`;
}

/** The tasks listed by name when there is room for `limit`. The task on screen is always among them. */
export function listed<T extends Staged>(tasks: T[], limit: number, currentId: string | null): T[] {
  if (tasks.length <= limit) return tasks;
  const first = tasks.slice(0, limit);
  const current = tasks.find((task) => task.info.id === currentId);
  if (current && !first.includes(current)) first[limit - 1] = current;
  return first;
}
