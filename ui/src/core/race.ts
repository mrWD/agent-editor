import { createEffect, on, onCleanup } from 'solid-js';
import { createStore, reconcile } from 'solid-js/store';
import { core } from './client';
import { gitDiffOver, state, type FileDiff, type GitFile, type StartedRace, type Task, type TaskState } from './store';

/** How the checks of one entrant stand: lines of "Done when" that are done, of how many. */
export type RaceChecks = { done: number; total: number; failed: number; running: number };

/** One agent's task in a race, as the Compare view shows it. */
export type Entrant = {
  taskId: string;
  agentId: string;
  agentName: string;
  short: string | null;
  state: TaskState;
  startedAt: number | null;
  endedAt: number | null;
  /** From its first message to the end of its last turn. `null` while no turn has ended. */
  ms: number | null;
  /** The first paragraph of what its agent said last. */
  said: string | null;
  added: number;
  removed: number;
  /** The files its branch changed, committed or not. */
  files: GitFile[];
  checks: RaceChecks | null;
  /** Why its changes could not be read. */
  problem: string | null;
};

/** One task text that went to several agents at once. */
export type Race = {
  id: string;
  projectId: string;
  text: string;
  agentIds: string[];
  /** Every task the race started with. `entrants` holds the ones that are still there. */
  tasks: string[];
  startedAt: number;
  entrants: Entrant[];
  /** Nobody is working or waiting for an answer. */
  over: boolean;
  finishedAt: number | null;
  picked: string | null;
  discarded: boolean;
  decidedAt: number | null;
};

const [races, setRaces] = createStore<Record<string, Race>>({});
const [problems, setProblems] = createStore<Record<string, string | null>>({});
const [notices, setNotices] = createStore<Record<string, string | null>>({});

/** What is known of a race. `undefined` until it has been read once. */
export function raceOf(raceId: string | undefined): Race | undefined {
  return raceId ? races[raceId] : undefined;
}

/** Why a race could not be read, if it could not. */
export function raceProblem(raceId: string | undefined): string | null {
  return (raceId && problems[raceId]) || null;
}

/** Something to tell the user about a race once, such as an agent that did not start. */
export function raceNotice(raceId: string | undefined): string | null {
  return (raceId && notices[raceId]) || null;
}

export function setRaceNotice(raceId: string, text: string | null) {
  setNotices(raceId, text);
}

export async function loadRace(raceId: string): Promise<void> {
  try {
    const race = await core.request<Race>('race.get', { raceId });
    setRaces(raceId, reconcile(race, { key: 'taskId' }));
    setProblems(raceId, null);
  } catch (error) {
    setProblems(raceId, error instanceof Error ? error.message : String(error));
  }
}

/** The events of a task that can change what a race says about it. */
const TELLING = ['state', 'turn_end', 'error', 'race_review'];

/** Who is watching each race. A race on screen in two places is still read once. */
const watchers = new Map<string, { count: number; stop: () => void }>();

/**
 * Keeps a race current while something on screen shows it. Returns what stops it.
 * The race is read again when one of its entrants ends a turn, changes a file
 * or gets a new verdict from a check.
 */
export function watchRace(raceId: string): () => void {
  let watcher = watchers.get(raceId);
  if (!watcher) {
    // Several events often arrive together; one read a moment later covers them all.
    let timer: ReturnType<typeof setTimeout> | undefined;
    const soon = () => {
      clearTimeout(timer);
      timer = setTimeout(() => void loadRace(raceId), 150);
    };
    // Before the race has been read once, its tasks are known from what each task says about itself.
    const mine = (taskId: unknown) =>
      (races[raceId]?.tasks ?? []).includes(String(taskId)) || state.tasks[String(taskId)]?.info.race?.id === raceId;
    // eslint-disable-next-line solid/reactivity -- called later, as an event handler: it reads what is current then
    const stopEvents = core.onEvent((event) => {
      switch (event.event) {
        case 'race.changed':
          if (event.data.raceId === raceId) soon();
          return;
        case 'task.event':
          if (TELLING.includes(event.data.type) && mine(event.data.taskId)) soon();
          return;
        case 'review.changed':
        case 'git.changed':
        case 'checks.changed':
        case 'task.archived':
          if (mine(event.data.taskId)) soon();
          return;
        case 'link.open':
          soon();
          return;
      }
    });
    watcher = {
      count: 0,
      stop: () => {
        clearTimeout(timer);
        stopEvents();
      },
    };
    watchers.set(raceId, watcher);
    void loadRace(raceId);
  }
  watcher.count += 1;
  let stopped = false;
  return () => {
    if (stopped) return;
    stopped = true;
    watcher.count -= 1;
    if (watcher.count === 0) {
      watcher.stop();
      watchers.delete(raceId);
    }
  };
}

/** The race of a task, kept current for as long as the component that asks is on screen. */
export function useRace(task: () => Task | undefined): () => Race | undefined {
  const raceId = () => task()?.info.race?.id;
  createEffect(
    on(raceId, (id) => {
      if (id) onCleanup(watchRace(id));
    }),
  );
  return () => raceOf(raceId());
}

/** The entrants of a task's race that are still on the lists, in the order of the race. */
export function entrantTasks(task: Task): Task[] {
  return (task.info.race?.tasks ?? []).map((id) => state.tasks[id]).filter((one): one is Task => Boolean(one));
}

/** The lines of every file an entrant's branch changed, or why they could not be read. */
export type EntrantDiffs = { key: string; files: (FileDiff | { path: string; problem: string })[] };

const [diffs, setDiffs] = createStore<Record<string, EntrantDiffs>>({});

/** More files than this are listed by name only: the page is for comparing, not for a whole review. */
export const SHOWN_FILES = 30;

/** What says whether an entrant's files are still the ones whose lines were read. */
const filesKey = (files: GitFile[]) => files.map((file) => `${file.path}:${file.added}:${file.removed}`).join('|');

/**
 * The lines an entrant changed, as they were last read. While they are being
 * read again after a change, the earlier ones stay on screen.
 */
export function diffsOf(entrant: Entrant): EntrantDiffs | undefined {
  return diffs[entrant.taskId];
}

/** Reads the lines of an entrant's change, file by file. Asking again for the same files does nothing. */
export async function loadDiffs(entrant: Entrant): Promise<void> {
  const key = filesKey(entrant.files);
  const taskId = entrant.taskId;
  if (diffs[taskId]?.key === key) return;
  const paths = entrant.files.slice(0, SHOWN_FILES).map((file) => file.path);
  const files = await Promise.all(
    paths.map((path) =>
      gitDiffOver(taskId, path, 'branch').catch((error) => ({
        path,
        problem: error instanceof Error ? error.message : String(error),
      })),
    ),
  );
  setDiffs(taskId, { key, files });
}

/** Takes one entrant's result: it stays as an ordinary task, the others are removed. */
export function pickResult(raceId: string, taskId: string) {
  return core.request<Race>('race.pick', { raceId, taskId });
}

/** Throws every result of a race away. */
export function discardRace(raceId: string) {
  return core.request<Race>('race.discard', { raceId });
}

/** Asks the agent of `reviewerTaskId` to review the result of `taskId`. Its answer comes in its own task. */
export function askReview(raceId: string, taskId: string, reviewerTaskId: string) {
  return core.request('race.review', { raceId, taskId, reviewerTaskId });
}

/** What to tell the user about the agents that did not start with a race. `null` when all of them did. */
export function failedNote(started: StartedRace): string | null {
  if (started.failed.length === 0) return null;
  const why = started.failed.map((one) => `${one.agentName}: ${one.message}`).join(' ');
  if (started.race) return `Not every agent started, so the race goes on without them. ${why}`;
  const only = state.agents.find((agent) => agent.id === started.tasks[0]?.agentId)?.name ?? 'one agent';
  return `Only ${only} started, so there is no race. Its task is kept as an ordinary task. ${why}`;
}

const numbers = ['no', 'one', 'two', 'three', 'four'];

/** A small count as a word: "two", "three". */
export function countWord(count: number): string {
  return numbers[count] ?? String(count);
}

/** How long an entrant took, in words: "12 s", "4 min", "2 h". */
export function raceTime(ms: number): string {
  if (ms < 60_000) return `${Math.max(1, Math.round(ms / 1000))} s`;
  const minutes = Math.round(ms / 60_000);
  return minutes < 60 ? `${minutes} min` : `${Math.round(minutes / 60)} h`;
}

/** "1 file", "3 files". */
export function filesWord(count: number): string {
  return `${count} ${count === 1 ? 'file' : 'files'}`;
}

/** One line on how an entrant's checks stand, and how much it should stand out. */
export function checksWord(checks: RaceChecks): { text: string; tone: 'good' | 'bad' | 'plain' } {
  if (checks.total === 0) return { text: 'No checks', tone: 'plain' };
  if (checks.running > 0) return { text: 'Checks running', tone: 'plain' };
  if (checks.failed > 0) {
    return { text: `${checks.failed} ${checks.failed === 1 ? 'check' : 'checks'} failed`, tone: 'bad' };
  }
  if (checks.done >= checks.total) return { text: 'All checks passed', tone: 'good' };
  if (checks.done === 0) return { text: 'Checks not run yet', tone: 'plain' };
  return { text: `${checks.done} of ${checks.total} checks done`, tone: 'plain' };
}
