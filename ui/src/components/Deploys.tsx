import { createSignal, For, onCleanup, onMount, Show, type JSX } from 'solid-js';
import { core } from '../core/client';
import { createKept } from '../core/kept';
import { ago } from './bits';

/** One place the task's branch is deployed to, as its host says. */
export type Deploy = {
  /** What the host calls the place: "Preview", "review/fix-login". */
  name: string;
  /** Where it is; empty when the host gave no address. */
  url: string;
  state: 'ready' | 'going' | 'failed' | 'known';
  /** When it was deployed; 0 when the host did not say. */
  at: number;
  commit: string;
};

type Found = { host: string | null; branch: string | null; deploys: Deploy[] };

const WORDS: Record<Deploy['state'], string> = {
  ready: 'ready',
  going: 'being deployed',
  failed: 'failed',
  known: '',
};

/** "ready, 2 h ago, 955565b6": how a place stands, when it got there and with which commit. */
export function deployLine(deploy: Deploy, when: (at: number) => string): string {
  return [WORDS[deploy.state], deploy.at > 0 ? when(deploy.at) : '', deploy.commit].filter(Boolean).join(', ');
}

/**
 * Where the task's branch is deployed: the previews and review apps its host
 * knows of, each a link that opens in the browser. A row at the head of
 * Preview, there only when the branch is deployed somewhere. The pages are
 * not shown inside the editor: they are on the network, where the editor's
 * own tools for a page (picking an element, drawing on it) cannot reach.
 */
export function Deploys(props: { taskId: string }): JSX.Element {
  // eslint-disable-next-line solid/reactivity -- made anew for each task, so the id never changes under it
  const taskId = props.taskId;
  const [found, setFound] = createKept<Found | null>(`task:${taskId}:deploys`, null);
  const [problem, setProblem] = createSignal<string | null>(null);
  const [looking, setLooking] = createSignal(false);

  let asked = 0;
  const look = async () => {
    const mine = ++asked;
    setLooking(true);
    try {
      const got = await core.request<Found>('deploys.get', { taskId });
      if (mine !== asked) return;
      setFound(got);
      setProblem(null);
    } catch (error) {
      if (mine !== asked) return;
      const why = error instanceof Error ? error.message : String(error);
      // A repository the host does not know has no deployments either: Changes says so, this row stays away.
      setProblem(why.includes('knows no repository') ? null : why);
    } finally {
      if (mine === asked) setLooking(false);
    }
  };

  onMount(() => {
    void look();
    // A request that was opened or changed: a pipeline may deploy for it any moment now.
    const stop = core.onEvent((event) => {
      if (event.event === 'mr.changed' && event.data.taskId === taskId) void look();
    });
    onCleanup(stop);
  });

  const deploys = () => found()?.deploys ?? [];

  return (
    <Show when={deploys().length > 0 || problem()}>
      <section class="preview-bar deploys" aria-label="Where this branch is deployed">
        <span class="fine">Deployed</span>
        <ul class="deploy-list">
          <For each={deploys()}>
            {(deploy) => (
              <li class="deploy" classList={{ [deploy.state]: true }}>
                <span
                  class={`pipe-dot ${deploy.state === 'ready' ? 'passed' : deploy.state === 'going' ? 'running' : deploy.state}`}
                  aria-hidden="true"
                />
                <Show when={deploy.url} fallback={<span class="deploy-name">{deploy.name}</span>}>
                  <a class="deploy-name" href={deploy.url} target="_blank" rel="noreferrer" title={deploy.url}>
                    {deploy.name}
                  </a>
                </Show>
                <span class="fine">{deployLine(deploy, ago)}</span>
              </li>
            )}
          </For>
        </ul>
        <Show when={problem()}>
          <span class="fine text-bad" role="alert">
            {problem()}
          </span>
        </Show>
        <span class="grow" />
        <button type="button" class="btn tiny ghost" disabled={looking()} onClick={() => void look()}>
          {looking() ? 'Looking' : 'Look again'}
        </button>
      </section>
    </Show>
  );
}
