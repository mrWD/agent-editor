import { createEffect, createMemo, createSignal, For, on, onCleanup, onMount, Show, type JSX } from 'solid-js';
import { createStore, reconcile } from 'solid-js/store';
import { Chip, clock } from '../components/bits';
import { core } from '../core/client';
import {
  agentOf,
  gitDiffOver,
  gitFiles,
  listCheckpoints,
  rewindTo,
  type Checkpoint,
  type FileDiff,
  type SpanFile,
  type Task,
} from '../core/store';
import { setWork, workOf } from '../core/work';
import { DiffRows, FileCount } from './Span';

const message = (error: unknown) => (error instanceof Error ? error.message : String(error));
const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? '' : 's'}`;

/** The checkpoints of each task that is being looked back at. The middle and the right side both read them. */
const [points, setPoints] = createStore<Record<string, { list: Checkpoint[]; git: boolean; problem: string | null }>>(
  {},
);

async function loadPoints(taskId: string) {
  try {
    const found = await listCheckpoints(taskId);
    if (!points[taskId]) setPoints(taskId, { list: [], git: found.git, problem: null });
    setPoints(taskId, { git: found.git, problem: null });
    setPoints(taskId, 'list', reconcile(found.checkpoints, { key: 'n' }));
  } catch (error) {
    setPoints(taskId, { list: [], git: true, problem: message(error) });
  }
}

/** What a checkpoint is, in a few words: "before your message “…”". */
function pointWords(point: Checkpoint): string {
  switch (point.kind) {
    case 'start':
      return 'at the start of the task';
    case 'message':
      return `before your message “${point.label}”`;
    case 'edit':
      return 'before your edit by hand';
    case 'rewind':
      return 'before a rewind';
  }
}

function pointTitle(point: Checkpoint): string {
  switch (point.kind) {
    case 'start':
      return 'Start of the task';
    case 'message':
      return 'Before your message';
    case 'edit':
      return 'Before your edit by hand';
    case 'rewind':
      return 'Before a rewind';
  }
}

/**
 * The middle of the screen while the user looks back: which files were
 * different at the chosen point and what rewinding to it would change.
 * Nothing is changed by looking.
 */
export function LookBackView(props: { task: Task }): JSX.Element {
  // eslint-disable-next-line solid/reactivity -- this component is made anew for each task
  const taskId = props.task.info.id;
  const n = () => workOf(taskId).lookBack ?? 0;
  const point = () => points[taskId]?.list.find((one) => one.n === n());
  const [files, setFiles] = createSignal<SpanFile[] | null>(null);
  const [diffs, setDiffs] = createSignal<Record<string, FileDiff>>({});
  const [problem, setProblem] = createSignal<string | null>(null);
  const [selected, setSelected] = createSignal<string | null>(null);

  const load = async () => {
    const at = n();
    if (at === 0) return;
    try {
      const listed = await gitFiles(taskId, `checkpoint:${at}`);
      if (at !== n()) return;
      setProblem(null);
      setFiles(listed.files);
      const loaded = await Promise.all(
        listed.files.map((file) =>
          gitDiffOver(taskId, file.path, `checkpoint:${at}`).then((diff) => [file.path, diff] as const),
        ),
      );
      if (at === n()) setDiffs(Object.fromEntries(loaded));
    } catch (error) {
      if (at === n()) setProblem(message(error));
    }
  };

  createEffect(
    on(n, () => {
      setFiles(null);
      setDiffs({});
      setSelected(null);
      void load();
    }),
  );
  onMount(() => {
    void loadPoints(taskId);
    // eslint-disable-next-line solid/reactivity -- called later, as an event handler: it reads what is current then
    const stop = core.onEvent((event) => {
      if ((event.event === 'git.changed' || event.event === 'review.changed') && event.data.taskId === taskId) {
        void loadPoints(taskId);
        void load();
      }
    });
    onCleanup(stop);
  });

  const backToNow = () => setWork(taskId, { lookBack: null });
  const jump = (path: string) => {
    setSelected(path);
    document.getElementById(`back-${path}`)?.scrollIntoView({ block: 'start' });
  };

  return (
    <>
      <section class="git" aria-label="Files that were different at the chosen point">
        <section class="git-block">
          <div class="git-head">
            <h2 class="pane-title grow">{point() ? `Files at ${clock(point()!.at)}` : 'Files then'}</h2>
          </div>
          <Show when={n() === 0}>
            <p class="fine">Pick a point on the right.</p>
          </Show>
          <For each={files() ?? []}>
            {(file) => (
              <div class="git-file" aria-current={file.path === selected() ? 'true' : undefined}>
                <button type="button" class="git-file-name" onClick={() => jump(file.path)}>
                  <span class="path">{file.path}</span>
                  <span class="git-file-meta">
                    <span class="word">differs</span>
                    <Show when={file.yours}>
                      <span class="word">has your lines</span>
                    </Show>
                  </span>
                </button>
              </div>
            )}
          </For>
          <Show when={files()}>
            {(list) => (
              <p class="fine">
                {list().length === 0
                  ? 'Every file is the same as now.'
                  : `${list().length === 1 ? 'One file was' : `${list().length} files were`} different then. The rest are the same as now.`}
              </p>
            )}
          </Show>
        </section>
      </section>

      <section class="center">
        <Show
          when={point()}
          fallback={
            <div class="center-note">
              <h2>Looking back</h2>
              <p>
                {points[taskId]?.problem ??
                  (points[taskId]?.git === false
                    ? 'This task works in a folder without git, so it has no checkpoints.'
                    : 'Pick a point on the right to see the code as it was then. Nothing is changed by looking.')}
              </p>
              <button type="button" class="btn" onClick={backToNow}>
                Back to now
              </button>
            </div>
          }
        >
          {(at) => (
            <>
              <p class="banner review flush">
                <span class="grow">
                  You are looking at the code as it was at <span class="mono">{clock(at().at)}</span>,{' '}
                  {pointWords(at())}. Nothing has been changed yet.
                </span>
                <button type="button" class="btn tiny" onClick={backToNow}>
                  Back to now
                </button>
              </p>
              <div class="diff-head">
                <h2>What rewinding would change</h2>
                <span class="fine">Red lines go away, green lines come back</span>
              </div>
              <div class="diff-body">
                <Show when={problem()}>
                  <p class="diff-note">{problem()}</p>
                </Show>
                <Show when={files()?.length === 0}>
                  <p class="diff-note">Nothing: the code is the same now as it was then.</p>
                </Show>
                <For each={files() ?? []}>
                  {(file) => (
                    <>
                      <div class="hunk-bar span-file" id={`back-${file.path}`}>
                        <span class="mono">{file.path}</span>
                        {/* Whether the file itself comes or goes is said in words next to it. */}
                        <FileCount file={{ ...file, kind: 'modified' }} />
                        <Show when={file.kind === 'added'}>
                          <span class="fine">was there then, is gone now</span>
                        </Show>
                        <Show when={file.kind === 'deleted'}>
                          <span class="fine">was not there then</span>
                        </Show>
                        <Show when={file.yours}>
                          <span class="fine">your lines can stay</span>
                        </Show>
                      </div>
                      <Show when={diffs()[file.path]} fallback={<p class="diff-note">Reading</p>}>
                        {(diff) => <DiffRows diff={diff()} />}
                      </Show>
                    </>
                  )}
                </For>
              </div>
              <div class="statusline">
                <span>A checkpoint is saved before every message you send and before every edit you make by hand</span>
              </div>
            </>
          )}
        </Show>
      </section>
    </>
  );
}

/**
 * The right side while the user looks back: every checkpoint of the task in
 * order, what changed after each, and the rewind itself.
 */
export function Timeline(props: { task: Task }): JSX.Element {
  // eslint-disable-next-line solid/reactivity -- this component is made anew for each task
  const taskId = props.task.info.id;
  const agent = () => agentOf(props.task);
  const name = () => agent()?.name ?? props.task.info.agentId;
  const list = () => points[taskId]?.list ?? [];
  const n = () => workOf(taskId).lookBack ?? 0;
  const point = () => list().find((one) => one.n === n());
  const [keepMine, setKeepMine] = createSignal(true);
  const [dropMessages, setDropMessages] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  const [problem, setProblem] = createSignal<string | null>(null);
  const [yours, setYours] = createSignal<string[]>([]);
  const [changed, setChanged] = createSignal<number | null>(null);

  onMount(() => void loadPoints(taskId));
  createEffect(
    on(n, (at) => {
      setProblem(null);
      setChanged(null);
      setYours([]);
      if (at === 0) return;
      void gitFiles(taskId, `checkpoint:${at}`)
        .then((found) => {
          if (at !== n()) return;
          setChanged(found.files.length);
          setYours(found.files.filter((file) => file.yours).map((file) => file.path));
        })
        .catch(() => {});
    }),
  );

  const working = createMemo(() => ['running', 'waiting_permission'].includes(props.task.info.state));

  const rewind = async () => {
    setBusy(true);
    setProblem(null);
    try {
      await rewindTo(taskId, n(), { keepMine: keepMine(), dropMessages: dropMessages() });
      setWork(taskId, { lookBack: null });
    } catch (error) {
      setProblem(message(error));
    } finally {
      setBusy(false);
    }
  };

  /** What a rewind to the chosen point does to the stretch of work after `one`. */
  const fate = (one: Checkpoint) => {
    if (!point() || one.n < point()!.n || one.files === 0) return null;
    return one.who === 'you' && keepMine() ? 'stays' : 'would be undone';
  };

  return (
    <>
      <div class="feed timeline" aria-label="Checkpoints of this task">
        <Show when={points[taskId]?.problem}>
          <p class="notice danger">{points[taskId]!.problem}</p>
        </Show>
        <Show when={points[taskId] && list().length === 0 && !points[taskId]!.problem}>
          <p class="notice">
            {points[taskId]!.git
              ? 'No checkpoints yet. One is saved before every message you send.'
              : 'This task works in a folder without git, so it has no checkpoints.'}
          </p>
        </Show>
        <For each={list()}>
          {(one) => (
            <>
              <div class="point" aria-current={one.n === n() ? 'true' : undefined}>
                <span class="mono">{clock(one.at)}</span>
                <span class="grow">{one.n === n() ? 'You are looking at this point' : pointTitle(one)}</span>
                <Show when={one.n !== n()}>
                  <button
                    type="button"
                    class="btn tiny"
                    aria-label={`View the code as it was at ${clock(one.at)}, ${pointWords(one)}`}
                    onClick={() => setWork(taskId, { lookBack: one.n })}
                  >
                    View
                  </button>
                </Show>
              </div>
              <Show when={one.kind === 'message' || one.kind === 'start'}>
                <p class="point-said">{one.kind === 'start' ? props.task.info.title : one.label}</p>
              </Show>
              <Show when={one.files > 0}>
                <div class="activity">
                  <Chip
                    agentId={one.who === 'you' ? 'you' : props.task.info.agentId}
                    short={one.who === 'you' ? 'You' : (agent()?.short ?? '··')}
                    flat
                  />
                  <span class="verb">changed</span>
                  <span class="what plain">{one.files === 1 ? one.paths[0] : plural(one.files, 'file')}</span>
                  <Show when={fate(one)} fallback={<FileCount file={{ ...one, kind: 'modified' }} />}>
                    <span class="status" classList={{ failed: fate(one) === 'would be undone' }}>
                      {fate(one)}
                    </span>
                  </Show>
                </div>
              </Show>
            </>
          )}
        </For>
        <div class="point">
          <span class="mono">{clock(Date.now())}</span>
          <span class="grow">Now</span>
        </div>
      </div>

      <Show
        when={point()}
        fallback={
          <section class="composer">
            <span class="fine">
              Pick a point to see the code as it was then. The conversation comes back when you return to now.
            </span>
            <div class="composer-row">
              <button type="button" class="btn" onClick={() => setWork(taskId, { lookBack: null })}>
                Back to now
              </button>
            </div>
          </section>
        }
      >
        {(at) => (
          <section class="card pinned rewind" aria-label="Rewind">
            <h2>Rewind to {clock(at().at)}?</h2>
            <p>
              {changed() === null
                ? 'Reading what would change.'
                : changed() === 0
                  ? 'The code is the same now as it was then: there is nothing to undo.'
                  : `Puts ${plural(changed()!, 'file')} back as ${changed() === 1 ? 'it was' : 'they were'} then.`}
            </p>
            <Show when={yours().length > 0}>
              <label class="check">
                <input
                  type="checkbox"
                  checked={keepMine()}
                  onChange={(event) => setKeepMine(event.currentTarget.checked)}
                />
                Keep my own edits to{' '}
                {yours().length === 1 ? yours()[0]!.split('/').pop() : plural(yours().length, 'file')}
              </label>
            </Show>
            <label class="check">
              <input
                type="checkbox"
                checked={dropMessages()}
                onChange={(event) => setDropMessages(event.currentTarget.checked)}
              />
              Also remove the later messages from the chat
            </label>
            <Show when={dropMessages()}>
              <span class="fine">{name()} is started again and told only what is left of the conversation.</span>
            </Show>
            <Show when={working()}>
              <p class="notice">{name()} is working. Stop its turn before you rewind.</p>
            </Show>
            <Show when={problem()}>
              <p class="notice danger" role="alert">
                {problem()}
              </p>
            </Show>
            <div class="actions">
              <button
                type="button"
                class="btn tall primary"
                disabled={busy() || working() || changed() === null}
                onClick={() => void rewind()}
              >
                {busy() ? 'Rewinding' : 'Rewind'}
              </button>
              <button type="button" class="btn tall" onClick={() => setWork(taskId, { lookBack: null })}>
                Back to now
              </button>
            </div>
            <span class="fine">Commits are not touched. A rewind can itself be undone from this list.</span>
          </section>
        )}
      </Show>
    </>
  );
}
