import { createEffect, createSignal, on, onCleanup, onMount, Show, type JSX } from 'solid-js';
import { go } from '../app/router';
import { core } from '../core/client';
import { refreshRequests } from '../core/requests';
import { createRequest, getRequest, mergeRequest, type RequestInfo, type Task } from '../core/store';

const message = (error: unknown) => (error instanceof Error ? error.message : String(error));

/** One line of "Before it can merge": a mark, what it is about, and how it stands. */
function Gate(props: { state: 'good' | 'wait' | 'bad'; title: string; note: string }): JSX.Element {
  return (
    <div class="gate">
      <span class={`gate-mark ${props.state}`} aria-hidden="true">
        {props.state === 'good' ? '✓' : props.state === 'bad' ? '!' : '…'}
      </span>
      <span class="gate-title">{props.title}</span>
      <span class="fine">{props.note}</span>
    </div>
  );
}

/**
 * The merge request of a task, at the bottom of the Changes column: open one,
 * see what stands between it and the merge, and merge it with the cleaning up
 * that goes with a finished task.
 */
export function RequestCard(props: { task: Task; commits: number; onChanged: () => void }): JSX.Element {
  // eslint-disable-next-line solid/reactivity -- this component is made anew for each task, so the id never changes under it
  const taskId = props.task.info.id;
  const [info, setInfo] = createSignal<RequestInfo | null>(null);
  const [problem, setProblem] = createSignal<string | null>(null);
  const [form, setForm] = createSignal(false);
  const [title, setTitle] = createSignal('');
  const [body, setBody] = createSignal('');
  const [busy, setBusy] = createSignal<string | null>(null);
  const [squash, setSquash] = createSignal(true);
  const [remove, setRemove] = createSignal(true);
  const [archive, setArchive] = createSignal(true);

  const load = async (fresh = false) => {
    try {
      setInfo(await getRequest(taskId, fresh));
      setProblem(null);
    } catch (error) {
      setProblem(message(error));
    }
  };

  onMount(() => {
    void load();
    const stop = core.onEvent((event) => {
      if (event.event === 'mr.changed' && event.data.taskId === taskId) void load();
    });
    // Checks and reviews move on the host; look again now and then while the card is on screen.
    const timer = setInterval(() => {
      if (document.visibilityState === 'visible' && info()?.request?.state === 'open') void load(true);
    }, 30_000);
    onCleanup(() => {
      stop();
      clearInterval(timer);
    });
  });

  // New commits change what a new request would say.
  createEffect(
    on(
      () => props.commits,
      () => void load(),
      { defer: true },
    ),
  );

  const act = async (name: string, work: () => Promise<unknown>) => {
    if (busy()) return;
    setBusy(name);
    setProblem(null);
    try {
      await work();
    } catch (error) {
      setProblem(message(error));
    } finally {
      setBusy(null);
    }
  };

  const openForm = () =>
    // eslint-disable-next-line solid/reactivity -- called later, as an event handler: it reads what is current then
    act('form', async () => {
      // Read what a new request would say right now: a commit made a moment ago changes it.
      await load();
      setTitle(info()?.defaults?.title ?? props.task.info.title);
      setBody(info()?.defaults?.body ?? '');
      setForm(true);
    });

  const create = () =>
    // eslint-disable-next-line solid/reactivity -- called later, as an event handler: it reads what is current then
    act('create', async () => {
      await createRequest(taskId, title(), body());
      setForm(false);
      await load();
      props.onChanged();
      void refreshRequests();
    });

  const merge = () =>
    // eslint-disable-next-line solid/reactivity -- called later, as an event handler: it reads what is current then
    act('merge', async () => {
      await mergeRequest(taskId, { squash: squash(), deleteBranch: remove(), archive: archive() });
      void refreshRequests();
      // An archived task has no screen any more.
      if (archive()) go('#/');
      else await load();
    });

  const word = () => info()?.host?.word ?? 'merge request';
  const Word = () => word().charAt(0).toUpperCase() + word().slice(1);

  return (
    <section class="git-mr-card" aria-label="Merge request">
      <div class="git-head">
        <h2 class="pane-title">{Word()}</h2>
        <Show when={info()?.request} fallback={<span class="grow fine">{info() ? 'none yet' : ''}</span>}>
          {(request) => (
            <>
              <span class="grow fine">
                Number {request().number} · {request().state}
                {request().draft ? ' · draft' : ''}
              </span>
              <a class="btn tiny" href={request().url} target="_blank" rel="noreferrer">
                Open on {info()!.host!.name}
              </a>
            </>
          )}
        </Show>
        <Show when={info() && info()!.host && !info()!.request && !form()}>
          <button
            type="button"
            class="btn tiny"
            disabled={props.commits === 0}
            title={props.commits === 0 ? 'Commit something first' : undefined}
            onClick={() => void openForm()}
          >
            Create
          </button>
        </Show>
      </div>

      <Show when={problem()}>
        <p class="banner danger flush" role="alert">
          <span class="grow">{problem()}</span>
        </p>
      </Show>
      <Show when={info() && !info()!.host}>
        <p class="fine">This project's remote is not on GitHub or GitLab, so a request cannot be opened from here.</p>
      </Show>

      <Show when={form()}>
        <label class="fine" for={`mr-title-${taskId}`}>
          Title
        </label>
        <input
          id={`mr-title-${taskId}`}
          class="field"
          value={title()}
          onInput={(event) => setTitle(event.currentTarget.value)}
        />
        <label class="fine" for={`mr-body-${taskId}`}>
          Description
        </label>
        <textarea
          id={`mr-body-${taskId}`}
          class="git-message"
          rows="4"
          value={body()}
          onInput={(event) => setBody(event.currentTarget.value)}
        />
        <span class="fine">
          Written from the commits{props.task.info.issue ? ' and the issue' : ''}. It goes into{' '}
          {info()?.defaults?.base ?? 'the base branch'}; the branch is pushed first.
        </span>
        <div class="actions">
          <button
            type="button"
            class="btn primary"
            disabled={Boolean(busy()) || title().trim() === ''}
            onClick={create}
          >
            {busy() === 'create' ? 'Opening' : `Push and open the ${word()}`}
          </button>
          <button type="button" class="btn ghost" onClick={() => setForm(false)}>
            Cancel
          </button>
        </div>
      </Show>

      <Show when={info()?.request}>
        {(request) => (
          <Show
            when={request().state === 'open'}
            fallback={
              <p class="fine">
                {request().state === 'merged' ? 'It is merged.' : 'It was closed without merging.'} This task can be
                archived from Home.
              </p>
            }
          >
            <h3 class="git-sub">Before it can merge</h3>
            <Gate
              state={request().mergeable === false ? 'bad' : request().mergeable === null ? 'wait' : 'good'}
              title="Conflicts"
              note={
                request().mergeable === false
                  ? 'It conflicts with its target. Update the branch.'
                  : request().mergeable === null
                    ? `${info()!.host!.name} is still working it out`
                    : 'None'
              }
            />
            <Gate
              state={
                request().checks.state === 'failed' ? 'bad' : request().checks.state === 'pending' ? 'wait' : 'good'
              }
              title="Checks"
              note={
                request().checks.state === 'none'
                  ? 'This project runs no checks on requests'
                  : `${request().checks.passed} passed` +
                    (request().checks.failed ? `, ${request().checks.failed} failed` : '') +
                    (request().checks.pending ? `, ${request().checks.pending} running` : '')
              }
            />
            <Gate
              state={
                request().review === 'changes_requested'
                  ? 'bad'
                  : request().review === 'review_required'
                    ? 'wait'
                    : 'good'
              }
              title="Review"
              note={
                {
                  approved: 'Approved',
                  changes_requested: 'Changes were asked for',
                  review_required: 'Waiting for a review',
                  none: 'Not required',
                }[request().review]
              }
            />

            <h3 class="git-sub">When it merges</h3>
            <label class="git-amend">
              <input type="checkbox" checked={squash()} onChange={(event) => setSquash(event.currentTarget.checked)} />
              Squash the commits into one
            </label>
            <label class="git-amend">
              <input type="checkbox" checked={remove()} onChange={(event) => setRemove(event.currentTarget.checked)} />
              Delete the branch and its working copy
            </label>
            <label class="git-amend">
              <input
                type="checkbox"
                checked={archive()}
                onChange={(event) => setArchive(event.currentTarget.checked)}
              />
              Archive this task
            </label>
            <Show when={props.task.info.issue}>
              {(issue) => (
                <span class="fine">
                  {issue().source === 'linear'
                    ? `${issue().key} is set to Done in Linear.`
                    : `${issue().key} closes by itself if the description says “Closes ${issue().key}”.`}
                </span>
              )}
            </Show>
            <button type="button" class="btn primary tall" disabled={Boolean(busy()) || !info()!.ready} onClick={merge}>
              {busy() === 'merge' ? 'Merging' : 'Merge now'}
            </button>
            <Show when={!info()!.ready}>
              <span class="fine">Merge now unlocks when nothing above stands in the way.</span>
            </Show>
          </Show>
        )}
      </Show>
    </section>
  );
}
