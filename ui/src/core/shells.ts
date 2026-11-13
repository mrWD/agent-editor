import { batch, createEffect, createSignal, onCleanup, onMount, type Accessor } from 'solid-js';
import { createStore, reconcile } from 'solid-js/store';
import { core } from './client';
import { closeTerminal, listTerminals, type TermInfo } from './store';

/**
 * One of a task's shells as the panel under the code holds it. The shell
 * itself runs in the core; this is its tab.
 */
export type ShellTab = {
  /** Stays the same for as long as the tab is there, also when a new shell is started in it. */
  key: number;
  /** The terminal in the core. `null` before the shell has started and after it has ended. */
  termId: number | null;
  /** Which of the task's shells it is, from 1. `null` until the core has said. */
  number: number | null;
  /** The command that has been running in it for a moment. */
  running: string | null;
  /** The shell ended by itself, as after `exit`. The tab stays, to say so. */
  ended: boolean;
  /** The tab has been looked at, so its terminal is drawn. */
  used: boolean;
};

/** How often the names of the shells are read again while they are on screen. */
const LOOK_EVERY_MS = 1200;

/** The number the next new shell is likely to get: the one after the highest. */
function nextNumber(tabs: ShellTab[]): number {
  return Math.max(0, ...tabs.map((tab) => tab.number ?? 0)) + 1;
}

/** What a shell is called: the command running in it, else "Terminal" and its number. */
export function shellName(tab: ShellTab, all: ShellTab[]): string {
  if (tab.running) return tab.running;
  const number = tab.number ?? nextNumber(all.filter((other) => other !== tab));
  return number === 1 ? 'Terminal' : `Terminal ${number}`;
}

/**
 * Brings the tabs in line with the shells the core has. A shell without a tab
 * gets one (it was started in another window, or before this one was opened).
 * A tab whose shell the core no longer has goes away, except one that is there
 * to say its shell ended, or one whose shell is only now being started.
 */
export function mergeShells(tabs: ShellTab[], listed: TermInfo[], newKey: () => number): ShellTab[] {
  const shells = listed.filter((info) => info.purpose === 'shell');
  const alive = new Map(shells.map((info) => [info.termId, info]));
  const kept = tabs
    .filter((tab) => tab.termId === null || alive.has(tab.termId))
    .map((tab) => {
      const info = tab.termId === null ? undefined : alive.get(tab.termId);
      return info ? { ...tab, number: info.number, running: info.running } : tab;
    });
  const known = new Set(kept.map((tab) => tab.termId));
  const fresh = shells
    .filter((info) => !known.has(info.termId))
    .map((info) => ({
      key: newKey(),
      termId: info.termId,
      number: info.number,
      running: info.running,
      ended: false,
      used: false,
    }));
  return [...kept, ...fresh];
}

export type Shells = {
  tabs: ShellTab[];
  /** The key of the tab whose terminal is shown. */
  current: Accessor<number | null>;
  /** The shell the user wants to close, while the question "it still runs something" is open. */
  asking: Accessor<{ key: number; running: string } | null>;
  problem: Accessor<string | null>;
  nameOf: (tab: ShellTab) => string;
  pick: (key: number) => void;
  /** Makes a tab for one more shell and shows it. The shell starts when its terminal is drawn. */
  add: () => void;
  /** A shell was started in a tab. */
  opened: (key: number, info: TermInfo) => void;
  /** Closes a tab and its shell. Asks first when a command is still running there. */
  close: (key: number) => Promise<void>;
  /** The answer to the question: close it anyway, or keep it. */
  answer: (close: boolean) => Promise<void>;
};

/**
 * The shells of one task, kept current while a component is on screen.
 * `visible` says whether the terminals are in view: only then are their names
 * read again and again, and only then is a first shell started by itself.
 * `onEmpty` is called when the user has closed the last one.
 */
export function watchShells(taskId: string, visible: Accessor<boolean>, onEmpty: () => void): Shells {
  const [held, setHeld] = createStore<{ tabs: ShellTab[] }>({ tabs: [] });
  const [current, setCurrent] = createSignal<number | null>(null);
  const [asking, setAsking] = createSignal<{ key: number; running: string } | null>(null);
  const [problem, setProblem] = createSignal<string | null>(null);
  // Until the core has said which shells the task has, none is started: one may be running already.
  const [synced, setSynced] = createSignal(false);
  let lastKey = 0;
  const newKey = () => ++lastKey;

  const find = (key: number) => held.tabs.find((tab) => tab.key === key);
  const show = (tabs: ShellTab[]) => {
    // A tab that stays is the same object as before, so its terminal is not drawn anew.
    setHeld('tabs', reconcile(tabs, { key: 'key' }));
    if (!tabs.some((tab) => tab.key === current())) setCurrent(tabs[0]?.key ?? null);
    const asked = asking();
    if (asked && !tabs.some((tab) => tab.key === asked.key && tab.termId !== null)) setAsking(null);
  };

  let looks = 0;
  const sync = async () => {
    const mine = ++looks;
    try {
      const { terminals } = await listTerminals(taskId);
      // A newer look is on its way: let it speak.
      if (mine !== looks) return;
      show(mergeShells(held.tabs, terminals, newKey));
      setSynced(true);
    } catch {
      // The names are a convenience. The next look, or the next connection, tries again.
    }
  };

  const remove = (key: number, byUser: boolean) => {
    const index = held.tabs.findIndex((tab) => tab.key === key);
    if (index < 0) return;
    const rest = held.tabs.filter((tab) => tab.key !== key);
    // In one step: with the last tab gone and the terminals still in view, a new shell would be started at once.
    batch(() => {
      // The neighbour takes the place of the one that is gone.
      if (current() === key) setCurrent((rest[index] ?? rest[index - 1])?.key ?? null);
      show(rest);
      if (byUser && rest.length === 0) onEmpty();
    });
  };

  const add = () => {
    // A tab whose shell has not started yet is the new one already.
    const waiting = held.tabs.find((tab) => tab.termId === null && !tab.ended);
    const key = waiting?.key ?? newKey();
    if (!waiting) {
      setHeld('tabs', held.tabs.length, { key, termId: null, number: null, running: null, ended: false, used: false });
    }
    setCurrent(key);
  };

  const closeNow = async (key: number, force: boolean) => {
    const tab = find(key);
    if (!tab) return;
    setProblem(null);
    if (tab.termId === null) {
      // Nothing runs behind this tab: there is only the tab to take away.
      remove(key, true);
      return;
    }
    try {
      const answer = await closeTerminal(tab.termId, force);
      if (answer.closed) {
        remove(key, true);
      } else {
        // The question is asked next to the terminal it is about.
        setCurrent(key);
        setAsking({ key, running: answer.running ?? 'A command' });
      }
    } catch (error) {
      const text = error instanceof Error ? error.message : String(error);
      // It ended a moment ago by itself: closing it is done.
      if (text.includes('has ended')) remove(key, true);
      else setProblem(text);
    }
  };

  onMount(() => {
    void sync();
    const timer = setInterval(() => {
      if (visible()) void sync();
    }, LOOK_EVERY_MS);
    // eslint-disable-next-line solid/reactivity -- called later, as an event handler: it reads what is current then
    const stop = core.onEvent((event) => {
      if (event.event === 'link.open') void sync();
      if (event.event !== 'terminal.exit' || event.data.taskId !== taskId || event.data.purpose !== 'shell') return;
      const tab = held.tabs.find((one) => one.termId === event.data.termId);
      if (!tab) return;
      // Closed on purpose, here or in another window: nothing is left to show. Ended by itself: the tab says so.
      if (event.data.closed) remove(tab.key, false);
      else {
        setHeld('tabs', (one) => one.key === tab.key, { termId: null, running: null, ended: true });
        if (asking()?.key === tab.key) setAsking(null);
      }
    });
    onCleanup(() => {
      clearInterval(timer);
      stop();
    });
  });

  // The first time the terminals are looked at, a task without a shell gets one.
  createEffect(() => {
    if (visible() && synced() && held.tabs.length === 0) add();
  });
  // A terminal is drawn the first time its tab is looked at, not before: it needs its place on screen to size itself.
  createEffect(() => {
    const key = current();
    if (visible() && key !== null) setHeld('tabs', (tab) => tab.key === key && !tab.used, 'used', true);
  });

  return {
    get tabs() {
      return held.tabs;
    },
    current,
    asking,
    problem,
    nameOf: (tab) => shellName(tab, held.tabs),
    pick: setCurrent,
    add,
    opened: (key, info) =>
      setHeld('tabs', (tab) => tab.key === key, {
        termId: info.termId,
        number: info.number,
        running: null,
        ended: false,
      }),
    close: (key) => closeNow(key, false),
    answer: async (close) => {
      const about = asking();
      setAsking(null);
      if (close && about) await closeNow(about.key, true);
    },
  };
}
