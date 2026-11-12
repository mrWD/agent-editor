import { createSignal, onCleanup, type Accessor } from 'solid-js';
import { core } from './client';
import { gitState, type GitState } from './store';

export type GitWatch = {
  state: Accessor<GitState | null>;
  /** Why the state cannot be shown, such as a folder without git. */
  problem: Accessor<string | null>;
  reload: () => Promise<void>;
  /** Shows a state the core just returned, without asking again. */
  show: (state: GitState) => void;
};

/**
 * Keeps the git state of one task current while a component is on screen.
 * It is read again whenever the core says the files or the branch changed.
 */
export function watchGit(taskId: string): GitWatch {
  const [state, setState] = createSignal<GitState | null>(null);
  const [problem, setProblem] = createSignal<string | null>(null);
  let asked = 0;

  const reload = async () => {
    const mine = ++asked;
    try {
      const next = await gitState(taskId);
      if (mine !== asked) return;
      setState(next);
      setProblem(null);
    } catch (error) {
      if (mine !== asked) return;
      setState(null);
      setProblem(error instanceof Error ? error.message : String(error));
    }
  };

  // Several events often arrive together; one read a moment later covers them all.
  let timer: ReturnType<typeof setTimeout> | undefined;
  const soon = () => {
    clearTimeout(timer);
    timer = setTimeout(() => void reload(), 120);
  };
  const stop = core.onEvent((event) => {
    if ((event.event === 'git.changed' || event.event === 'review.changed') && event.data.taskId === taskId) soon();
    if (event.event === 'link.open') soon();
  });
  // Coming back to the window: another program may have changed or committed files meanwhile.
  window.addEventListener('focus', soon);
  onCleanup(() => {
    clearTimeout(timer);
    stop();
    window.removeEventListener('focus', soon);
  });

  void reload();
  return {
    state,
    problem,
    reload,
    show: (next) => {
      asked++;
      setState(next);
      setProblem(null);
    },
  };
}
