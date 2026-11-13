import { createStore, reconcile } from 'solid-js/store';
import { core } from './client';
import { setProblems, type Problem } from './problems';

/** A command that checks something about the project, and how its last run ended. */
export type Check = {
  id: string;
  title: string;
  command: string;
  kind: 'lint' | 'types' | 'format' | 'test' | 'e2e' | 'custom';
  /** Where it was found: a file's name, or "you". */
  from: string;
  on: boolean;
  state: 'none' | 'running' | 'passed' | 'failed';
  exitCode?: number;
  /** How long the run took. */
  ms?: number;
  /** When the run started. */
  at?: number;
  /** The files changed after the run: its verdict says nothing about them now. */
  stale?: boolean;
  /** The end of what the command printed. */
  tail?: string;
  problems?: Omit<Problem, 'from'>[];
};

/** A line only a person can confirm. */
export type DoneLine = { id: string; text: string; done: boolean; from: 'issue' | 'you' };

/** "Done when" for a task. */
export type Checks = {
  checks: Check[];
  lines: DoneLine[];
  done: number;
  total: number;
  /** Checks whose last run failed and still counts. */
  failed: number;
  running: number;
};

const [all, setAll] = createStore<Record<string, Checks>>({});

/** What is known of a task's checks. `undefined` until it has been asked for. */
export function checksOf(taskId: string): Checks | undefined {
  return all[taskId];
}

export async function loadChecks(taskId: string): Promise<void> {
  const state = await core.request<Checks>('checks.state', { taskId });
  setAll(taskId, reconcile(state));
  // What the failed checks name as wrong goes to the Problems tab, each check its own share.
  for (const check of state.checks) {
    const found = check.state === 'failed' && !check.stale ? (check.problems ?? []) : [];
    setProblems(
      taskId,
      `check:${check.id}`,
      found.map((problem) => ({ ...problem, from: check.title })),
    );
  }
}

/** Keeps a task's checks current while its screen is shown. Returns what stops it. */
export function watchChecks(taskId: string): () => void {
  const load = () => void loadChecks(taskId).catch(() => {});
  load();
  return core.onEvent((event) => {
    // A change to the files makes a verdict old; a check that ended has a new one.
    if ((event.event === 'checks.changed' || event.event === 'review.changed') && event.data.taskId === taskId) load();
  });
}

export const runChecks = (taskId: string, id?: string) => core.request('checks.run', { taskId, id });
export const stopCheck = (taskId: string, id: string) => core.request('checks.stop', { taskId, id });
export const turnCheck = (taskId: string, id: string, on: boolean) => core.request('checks.turn', { taskId, id, on });
export const addCheck = (taskId: string, command: string, title = '') =>
  core.request('checks.add', { taskId, command, title });
export const removeCheck = (taskId: string, id: string) => core.request('checks.remove', { taskId, id });
export const addLine = (taskId: string, text: string) => core.request('checks.addLine', { taskId, text });
export const setLine = (taskId: string, id: string, done: boolean) =>
  core.request('checks.setLine', { taskId, id, done });
export const removeLine = (taskId: string, id: string) => core.request('checks.removeLine', { taskId, id });

/** How long a run took, in words: "41 ms", "1.2 s", "2 min". */
export function took(ms: number): string {
  if (ms < 1000) return `${ms} ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)} s`;
  return `${Math.round(ms / 60_000)} min`;
}

/** One line on how a check stands. */
export function checkNote(check: Check): string {
  if (!check.on && check.state === 'none') return 'off for this project';
  switch (check.state) {
    case 'running':
      return 'running';
    case 'passed':
      return check.stale ? 'passed before the last change' : `passed in ${took(check.ms ?? 0)}`;
    case 'failed': {
      if (check.stale) return 'failed before the last change';
      const count = check.problems?.length ?? 0;
      return count > 0
        ? `failed, ${count} ${count === 1 ? 'problem' : 'problems'}`
        : `failed with code ${check.exitCode}`;
    }
    case 'none':
      return 'not run yet';
  }
}
