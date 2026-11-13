import { createSignal, For, onCleanup, onMount, Show, type JSX } from 'solid-js';
import { createStore, reconcile } from 'solid-js/store';
import {
  actOnPipeline,
  CI_WORDS,
  getPipeline,
  jobAct,
  stagesOf,
  needsNobody,
  stillGoing,
  summary,
  type CiAct,
  type CiInfo,
  type CiJob,
  type CiRun,
} from '../core/ci';
import { core } from '../core/client';
import { recallCopy, rememberCopy } from '../core/kept';
import type { Task } from '../core/store';

/** How often a pipeline that is still going is looked at again, while its card is on screen. */
const LOOK_EVERY = 15_000;
/** After a push the host needs a moment to make a pipeline: for this long it is looked for more often. */
const AFTER_A_PUSH = 90_000;
const LOOK_AFTER_A_PUSH = 5_000;
/** A button that was pressed once waits this long for its second press, then forgets the first. */
const ARMED_FOR = 4_000;

const said = (error: unknown) => (error instanceof Error ? error.message : String(error));

/**
 * The pipeline of the task's branch, under its merge request: where it
 * stands, job by job, and what a person does to it without opening the host:
 * run a failed job again, start one that waits, stop or run the whole thing
 * again, start a new one. A long pipeline is told by its stages; the jobs that
 * went well are folded away.
 */
export function PipelineCard(props: { task: Task }): JSX.Element {
  // eslint-disable-next-line solid/reactivity -- this component is made anew for each task, so the id never changes under it
  const taskId = props.task.info.id;
  // Kept in a store and brought in line run by run and job by job: a row stays the same row while
  // its state changes, so the keyboard keeps its place and a first press is not forgotten each time
  // the pipeline is read again. The card opens on what it showed last time.
  const kept = `task:${taskId}:pipeline`;
  const [held, setHeld] = createStore<{ info: CiInfo | null }>({ info: recallCopy<CiInfo>(kept) });
  const info = () => held.info;
  const setInfo = (next: CiInfo) => setHeld('info', reconcile(rememberCopy(kept, next), { key: 'id' }));
  /** True when the task has nowhere to run pipelines: no branch, or a remote that is not GitLab or GitHub. */
  const [nowhere, setNowhere] = createSignal(false);
  /** Why the pipeline could not be read. The next reading that works takes it away. */
  const [problem, setProblem] = createSignal<string | null>(null);
  /** Why the last thing that was asked for did not happen. It stays until it is dismissed or something else is asked. */
  const [refused, setRefused] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal<string | null>(null);
  /** The button that was pressed once and waits for its second press. */
  const [armed, setArmed] = createSignal<string | null>(null);
  /** The stages whose every job is listed, by run and stage. */
  const [unfolded, setUnfolded] = createSignal<string[]>([]);
  let disarm: ReturnType<typeof setTimeout> | undefined;
  const arm = (key: string | null) => {
    clearTimeout(disarm);
    setArmed(key);
    // Not every browser gives a pressed button the keyboard, so leaving it is not always told: time is.
    if (key) disarm = setTimeout(() => setArmed(null), ARMED_FOR);
  };

  let asked = 0;
  const look = async () => {
    const mine = ++asked;
    try {
      const got = await getPipeline(taskId);
      if (mine !== asked) return;
      setInfo(got);
      setNowhere(false);
      setProblem(null);
    } catch (error) {
      if (mine !== asked) return;
      const why = said(error);
      // A folder without git, or a remote that is neither host: there is no pipeline to speak of, and no fault to report.
      // The same for a repository the host does not know (not pushed there yet, or not this account's
      // to see): the request card above says so already, and a second red line would say nothing new.
      const none = ['without git', 'not on GitHub or GitLab', 'knows no repository'];
      if (none.some((words) => why.includes(words))) setNowhere(true);
      else setProblem(why);
    }
  };

  onMount(() => {
    void look();
    let lookedAt = Date.now();
    let pushedAt = 0;
    const again = setInterval(() => {
      if (document.hidden) return;
      const going = stillGoing(info()?.pipeline?.state);
      const justPushed = Date.now() - pushedAt < AFTER_A_PUSH;
      const due = Date.now() - lookedAt >= (justPushed && !going ? LOOK_AFTER_A_PUSH : LOOK_EVERY);
      if ((going || justPushed) && due) {
        lookedAt = Date.now();
        void look();
      }
    }, 1000);
    // A commit, a push or a change to the request: the host may start a pipeline for it any moment now.
    const stop = core.onEvent((event) => {
      const mine = (event.event === 'git.changed' || event.event === 'mr.changed') && event.data.taskId === taskId;
      if (!mine) return;
      pushedAt = Date.now();
      lookedAt = Date.now();
      void look();
    });
    onCleanup(() => {
      clearInterval(again);
      clearTimeout(disarm);
      stop();
    });
  });

  /** Does one thing. With `twice`, the first press only arms the button. */
  const act = async (key: string, what: CiAct, id: string, name: string, twice: boolean) => {
    if (busy()) return;
    if (twice && armed() !== key) {
      arm(key);
      return;
    }
    arm(null);
    setBusy(key);
    setRefused(null);
    try {
      setInfo(await actOnPipeline(taskId, what, id, name));
      // The host shows the new state a moment later: one more look then.
      setTimeout(() => void look(), 2500);
    } catch (error) {
      setRefused(said(error));
    } finally {
      setBusy(null);
    }
  };

  const button = (
    key: string,
    label: string,
    what: CiAct,
    id: string,
    name: string,
    twice: boolean,
    title?: string,
  ) => (
    <button
      type="button"
      class="btn tiny"
      classList={{ armed: armed() === key }}
      disabled={busy() !== null}
      title={title}
      aria-label={`${armed() === key ? `Press again: ${label}` : label}: ${name}`}
      onClick={() => void act(key, what, id, name, twice)}
      onBlur={() => armed() === key && arm(null)}
    >
      {busy() === key ? 'Asking' : armed() === key ? `Press again: ${label}` : label}
    </button>
  );

  const jobRow = (job: CiJob) => {
    const can = () => jobAct(job, info()!.can);
    return (
      <li class="pipe-job">
        <span class={`pipe-dot ${job.state}`} aria-hidden="true" />
        <a class="pipe-name" href={job.url} target="_blank" rel="noreferrer">
          {job.name}
        </a>
        <span class="fine">
          {CI_WORDS[job.state]}
          {job.allowedToFail && job.state === 'failed' ? ', may fail' : ''}
        </span>
        <Show when={can()}>
          {(todo) => button(`job:${job.id}`, todo().label, todo().what, job.id, job.name, todo().twice)}
        </Show>
      </li>
    );
  };

  const run = (one: CiRun) => {
    const broken = () => one.jobs.some((job) => job.state === 'failed' || job.state === 'canceled');
    return (
      <div class="pipe-run" role="group" aria-label={one.name}>
        <div class="pipe-run-head">
          <span class={`pipe-dot ${one.state}`} aria-hidden="true" />
          <a class="pipe-name strong" href={one.url} target="_blank" rel="noreferrer">
            {one.name}
          </a>
          <span class="fine grow">{summary(one.jobs)}</span>
          <Show when={broken() && info()!.can.retryFailed}>
            {button(`failed:${one.id}`, 'Run failed again', 'retryFailed', one.id, one.name, false)}
          </Show>
          <Show when={!stillGoing(one.state) && !info()!.can.runNew}>
            {button(`all:${one.id}`, 'Run all again', 'retryRun', one.id, one.name, true)}
          </Show>
          <Show when={stillGoing(one.state)}>
            {button(`stop:${one.id}`, 'Stop', 'cancelRun', one.id, one.name, true)}
          </Show>
        </div>
        <For each={stagesOf(one.jobs)}>
          {(stage) => {
            const key = `${one.id}:${stage}`;
            const jobs = () => one.jobs.filter((job) => job.stage === stage);
            const open = () => unfolded().includes(key);
            const quiet = () => jobs().filter(needsNobody).length;
            const shown = () => (open() ? jobs() : jobs().filter((job) => !needsNobody(job)));
            return (
              <div class="pipe-stage" role="group" aria-label={stage || 'Jobs'}>
                <div class="pipe-stage-head">
                  <span class="pipe-stage-name">{stage || 'Jobs'}</span>
                  <Show when={quiet() > 0}>
                    <button
                      type="button"
                      class="pipe-fold"
                      aria-expanded={open()}
                      onClick={() => setUnfolded((now) => (open() ? now.filter((one) => one !== key) : [...now, key]))}
                    >
                      {open() ? 'Fold' : `${quiet()} went well`}
                    </button>
                  </Show>
                </div>
                <Show when={shown().length > 0}>
                  <ul class="pipe-jobs">
                    <For each={shown()}>{jobRow}</For>
                  </ul>
                </Show>
              </div>
            );
          }}
        </For>
      </div>
    );
  };

  return (
    <Show when={!nowhere()}>
      <section class="git-mr-card pipeline" aria-label="Pipeline">
        <div class="git-head">
          <h2 class="pane-title">Pipeline</h2>
          <span class="grow fine">
            <Show when={info()} fallback={problem() ? '' : 'Asking'}>
              {(known) => (
                <Show when={known().pipeline} fallback="none yet">
                  {(pipeline) => (
                    <>
                      {CI_WORDS[pipeline().state]} · commit <span class="mono">{pipeline().commit}</span>
                    </>
                  )}
                </Show>
              )}
            </Show>
          </span>
          <button type="button" class="btn tiny ghost" disabled={busy() !== null} onClick={() => void look()}>
            Look again
          </button>
        </div>
        <Show when={refused() ?? problem()}>
          {(why) => (
            <p class="banner danger flush" role="alert">
              <span class="grow">{why()}</span>
              <button
                type="button"
                class="btn tiny"
                onClick={() => {
                  setRefused(null);
                  setProblem(null);
                }}
              >
                Dismiss
              </button>
            </p>
          )}
        </Show>
        <Show when={info()}>
          {(known) => (
            <>
              <Show
                when={known().pipeline}
                fallback={
                  <p class="fine">
                    Nothing has run for <span class="mono">{known().branch}</span> on {known().host} yet. A pipeline
                    starts when the branch is pushed, if the project has one.
                  </p>
                }
              >
                {(pipeline) => <For each={pipeline().runs}>{run}</For>}
              </Show>
              <Show when={known().can.runNew}>
                <div class="actions">
                  {button(
                    'new',
                    'Run a new pipeline',
                    'runNew',
                    '',
                    known().branch,
                    true,
                    'Starts a pipeline for the newest commit of the branch',
                  )}
                </div>
              </Show>
            </>
          )}
        </Show>
      </section>
    </Show>
  );
}
