import { createStore, reconcile } from 'solid-js/store';

/** One thing that is wrong in a file of a task, with where it is. */
export type Problem = {
  /** The file, as a path from the task's folder. */
  path: string;
  /** Counted from 1. */
  line: number;
  column?: number;
  severity: 'error' | 'warning' | 'info';
  message: string;
  /** Who says so, in a word or two: "TypeScript", "Unit tests". */
  from: string;
};

/**
 * The problems of each task, kept by whoever found them. Several sources
 * report into one list: a check that was run, a language server. Each source
 * replaces only what it said itself.
 */
const [problems, setStore] = createStore<Record<string, Record<string, Problem[]>>>({});

/** Says what `source` knows of a task's problems now. An empty list takes its problems away. */
export function setProblems(taskId: string, source: string, list: Problem[]) {
  if (!problems[taskId]) setStore(taskId, {});
  setStore(taskId, source, reconcile(list));
}

/** Every problem of a task: errors first, then by file and line. */
export function problemsOf(taskId: string): Problem[] {
  const order = { error: 0, warning: 1, info: 2 } as const;
  return Object.values(problems[taskId] ?? {})
    .flat()
    .sort(
      (a, b) =>
        order[a.severity] - order[b.severity] ||
        a.path.localeCompare(b.path) ||
        a.line - b.line ||
        (a.column ?? 0) - (b.column ?? 0),
    );
}

/** How many errors and warnings a task has, for the number on the tab. */
export function problemCount(taskId: string): number {
  return problemsOf(taskId).filter((problem) => problem.severity !== 'info').length;
}
