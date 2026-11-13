import { createStore } from 'solid-js/store';

/**
 * The commit each task's Changes view is showing, if it shows one. A commit
 * is picked in the search over everything; the Changes view then shows that
 * commit in place of what is not committed.
 */
const [shown, setShown] = createStore<Record<string, string | undefined>>({});

export function commitShown(taskId: string): string | null {
  return shown[taskId] ?? null;
}

/** Shows a commit in a task's Changes view; `null` goes back to the changes. */
export function showCommit(taskId: string, hash: string | null) {
  setShown(taskId, hash ?? undefined);
}
