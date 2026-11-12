import { createSignal } from 'solid-js';
import { createStore, reconcile } from 'solid-js/store';
import { core } from './client';
import { stageOf, type Stage } from './stages';
import { requestsOverview, type MergeRequest, type Task } from './store';

/** What is known about each task's merge request: the request and whether it can merge now. */
export type Known = { request: MergeRequest; ready: boolean };

const [requests, setRequests] = createStore<Record<string, Known>>({});
const [loadedOnce, setLoadedOnce] = createSignal(false);

export { requests };
export const requestsLoaded = loadedOnce;

let asking = false;
let again = false;

/** Asks the core for every task's merge request. Calls that overlap are folded into one more round. */
export async function refreshRequests(): Promise<void> {
  if (asking) {
    again = true;
    return;
  }
  asking = true;
  try {
    const { requests: list } = await requestsOverview();
    setRequests(
      reconcile(
        Object.fromEntries(list.map((entry) => [entry.taskId, { request: entry.request, ready: entry.ready }])),
      ),
    );
    setLoadedOnce(true);
  } catch {
    // Without the hosts the rest of the app still works; the stages simply stay as they were.
  } finally {
    asking = false;
    if (again) {
      again = false;
      void refreshRequests();
    }
  }
}

let started = false;

/** Keeps the merge requests current: on start, when the core says one changed, and once a minute. */
export function watchRequests() {
  if (started) return;
  started = true;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const soon = () => {
    clearTimeout(timer);
    timer = setTimeout(() => void refreshRequests(), 300);
  };
  core.onEvent((event) => {
    if (['link.open', 'mr.changed', 'task.archived', 'task.created'].includes(event.event)) soon();
  });
  // Checks and reviews move on the host without telling anyone here.
  setInterval(() => {
    if (document.visibilityState === 'visible') void refreshRequests();
  }, 60_000);
}

/** Where a task stands, counting what is known of its merge request. */
export function stageOfTask(task: Task): Stage {
  const known = requests[task.info.id];
  return stageOf(
    task.info.state,
    known ? { state: known.request.state, ready: known.ready, checks: known.request.checks.state } : null,
  );
}
