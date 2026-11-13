import { createStore } from 'solid-js/store';
import { state, taskLeft, taskSince, type Since } from './store';

/** After this long away, "where you stopped" is shown even if nothing happened. */
const LONG_BREAK_MS = 30 * 60 * 1000;
/** A look shorter than this, with nothing done, is a glance: it does not count as a visit. */
const GLANCE_MS = 5000;

type Visit = {
  since: Since | null;
  /** What the reminder said when the user waved it away, in short. See `gist`. */
  hidden: string | null;
  /** When the user last waved the reminder away. */
  hiddenAt: number;
};

/** What a reminder says, in short. Two reminders with the same gist tell the user nothing new. */
function gist(since: Since | null): string | null {
  if (!since) return null;
  const last = since.happened[since.happened.length - 1];
  return [since.leftAt, since.happened.length, last?.text ?? '', since.files.length].join('|');
}

const [visits, setVisits] = createStore<Record<string, Visit>>({});
/** When the user arrived at each task, and the last event it had then. */
const arrived = new Map<string, { at: number; seq: number }>();

/**
 * What to tell the user on coming back to a task: where they stopped and what
 * happened since. `null` when there is nothing worth telling: the user was
 * away for a short while and nothing happened, or they hid the reminder.
 */
export function sinceOf(taskId: string): Since | null {
  const visit = visits[taskId];
  if (!visit?.since || gist(visit.since) === visit.hidden) return null;
  return visit.since.awayMs >= LONG_BREAK_MS || !visit.since.quiet ? visit.since : null;
}

/**
 * Takes the reminder away. It stays away for as long as it would say the same:
 * a glance at another task and back does not bring it up again, news does.
 */
export function hideSince(taskId: string) {
  const visit = visits[taskId];
  setVisits(taskId, {
    since: visit?.since ?? null,
    hidden: gist(visit?.since ?? null) ?? visit?.hidden ?? null,
    hiddenAt: Date.now(),
  });
}

/** Called when a task comes on screen. */
export function arrive(taskId: string) {
  const asked = Date.now();
  arrived.set(taskId, { at: asked, seq: state.tasks[taskId]?.lastSeq ?? -1 });
  void taskSince(taskId)
    .then((since) => {
      const visit = visits[taskId];
      // An answer that comes after the user waved the reminder away must not bring it back.
      const waved = (visit?.hiddenAt ?? 0) > asked;
      setVisits(taskId, {
        since,
        hidden: waved ? gist(since) : (visit?.hidden ?? null),
        hiddenAt: visit?.hiddenAt ?? 0,
      });
    })
    .catch(() => {
      // Without it the task works as before; only the reminder is missing.
    });
}

/**
 * Called when a task goes off screen. From here the next "since your last
 * visit" is counted, unless the user only glanced at the task.
 */
export function leave(taskId: string) {
  const came = arrived.get(taskId);
  arrived.delete(taskId);
  const task = state.tasks[taskId];
  if (!came || !task) return;
  const glance = Date.now() - came.at < GLANCE_MS && task.lastSeq === came.seq;
  if (glance) return;
  void taskLeft(taskId).catch(() => {});
}

/** Counts the next "since your last visit" from this moment: what there is now has been seen. */
export async function seenUpToNow(taskId: string) {
  await taskLeft(taskId);
  arrive(taskId);
}
