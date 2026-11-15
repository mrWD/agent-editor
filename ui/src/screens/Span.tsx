import { createEffect, createSignal, For, on, onCleanup, onMount, Show, type Accessor, type JSX } from 'solid-js';
import { recall, remember } from '../core/kept';
import { go, taskHref } from '../app/router';
import { ago, span as lengthOf, whenLabel } from '../components/bits';
import { CHANGES_TABS, tabCount, type ChangesTab } from '../core/changesTabs';
import { CI_WORDS, type CiPipeline } from '../core/ci';
import { core } from '../core/client';
import { requestName } from '../core/details';
import type { GitWatch } from '../core/gitstate';
import { requests } from '../core/requests';
import { seenUpToNow } from '../core/visits';
import {
  agentOf,
  gitDiffOver,
  gitFiles,
  taskSince,
  type FileDiff,
  type GitState,
  type Since,
  type Span,
  type SpanFile,
  type Task,
} from '../core/store';
import { openInEditor, setWork, type Work } from '../core/work';

const message = (error: unknown) => (error instanceof Error ? error.message : String(error));
const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? '' : 's'}`;

/**
 * What the Changes view shows, picked at the head of its column. Three parts along the path of a
 * change, each with how much it holds: what is not committed, what is committed and not pushed,
 * and what is pushed, under the name of its request once there is one. Beside them, apart: what
 * is new since the last visit.
 */
export function ChangesTabs(props: {
  taskId: string;
  git: GitState | null;
  /** The part on show. `visit` when the view shows what is new since the last visit. */
  tab: ChangesTab | 'visit';
  /** How the pipeline of the branch stands, to mark the part it is in. */
  pipeline?: CiPipeline['state'] | null;
}): JSX.Element {
  const pick = (changes: Work['changes']) => setWork(props.taskId, { changes });
  const request = () => requests[props.taskId]?.request;
  const names: Record<ChangesTab, () => string> = {
    head: () => 'Not committed',
    ahead: () => 'Committed',
    request: () => (request() ? requestName(request()!) : 'Pushed'),
  };
  const hints: Record<ChangesTab, () => string> = {
    head: () => 'What differs from the last commit, and the commit itself',
    ahead: () => 'The commits that are not pushed yet, and the push',
    request: () =>
      request()
        ? 'What is pushed: the request, what stands before its merge, its comments and its pipeline'
        : 'What is pushed, and the request to open for it',
  };
  return (
    <div class="span-mode">
      <div class="switcher changes-tabs" role="group" aria-label="Which changes to show">
        <For each={CHANGES_TABS}>
          {(tab) => (
            <button type="button" aria-pressed={props.tab === tab} title={hints[tab]()} onClick={() => pick(tab)}>
              {names[tab]()}
              <Show when={tab !== 'request' && tabCount(tab, props.git) > 0}>
                <span class="count">{tabCount(tab, props.git)}</span>
              </Show>
              <Show when={tab === 'request' && props.pipeline}>
                {(state) => (
                  <span
                    class={`pipe-dot ${state()}`}
                    role="img"
                    aria-label={`Pipeline ${CI_WORDS[state()]}`}
                    title={`Pipeline ${CI_WORDS[state()]}`}
                  />
                )}
              </Show>
            </button>
          )}
        </For>
      </div>
      <button
        type="button"
        class="btn tiny ghost"
        aria-pressed={props.tab === 'visit'}
        title="Only what changed since you last left this task"
        onClick={() => pick('visit')}
      >
        Since last visit
      </button>
    </div>
  );
}

/** The lines of one file's changes. A press on a line number opens the file there. */
export function DiffRows(props: { diff: FileDiff; onLine?: (line: number) => void }): JSX.Element {
  return (
    <>
      <Show when={props.diff.binary}>
        <p class="diff-note">This file is not text, or is too large to show line by line.</p>
      </Show>
      <Show when={!props.diff.binary && props.diff.hunks.length === 0}>
        <p class="diff-note">
          {props.diff.kind === 'added' ? 'A new, empty file.' : 'Only the file itself changed, not its lines.'}
        </p>
      </Show>
      <For each={props.diff.hunks}>
        {(hunk, index) => (
          <>
            <Show when={index() > 0}>
              <div class="diff-gap">unchanged lines</div>
            </Show>
            <For each={hunk.lines}>
              {(line) => (
                <div class={`diff-row ${line.kind}`}>
                  <button
                    type="button"
                    class="no"
                    disabled={!props.onLine || line.newNo === null}
                    title={props.onLine && line.newNo !== null ? 'Open the file at this line' : undefined}
                    onClick={() => line.newNo !== null && props.onLine?.(line.newNo)}
                  >
                    {line.newNo ?? line.oldNo}
                  </button>
                  <span class="sign" aria-hidden="true">
                    {line.kind === 'add' ? '+' : line.kind === 'remove' ? '−' : ' '}
                  </span>
                  <span class="src">{line.text || ' '}</span>
                </div>
              )}
            </For>
          </>
        )}
      </For>
    </>
  );
}

/** A file's name with how many lines came and went. */
export function FileCount(props: { file: { added: number; removed: number; kind: string } }): JSX.Element {
  return (
    <span class="git-file-meta">
      <Show when={props.file.added > 0}>
        <span class="plus">+{props.file.added}</span>
      </Show>
      <Show when={props.file.removed > 0}>
        <span class="minus">−{props.file.removed}</span>
      </Show>
      <Show when={props.file.kind !== 'modified'}>
        <span class="word">{props.file.kind === 'added' ? 'new' : 'deleted'}</span>
      </Show>
    </span>
  );
}

/** One file of a span on show: its lines, or that they are being read, or why they cannot be. */
type Shown = FileDiff | 'loading' | { problem: string };

/** The files that differ over a span, and the ones whose lines are on show. */
export type SpanWatch = {
  /** `null` until they were read once. */
  files: Accessor<SpanFile[] | null>;
  problem: Accessor<string | null>;
  open: Accessor<Record<string, Shown>>;
  show: (path: string) => Promise<void>;
  hide: (path: string) => void;
  load: () => Promise<void>;
};

/**
 * Reads the files that differ over a span and keeps them current while a component is on screen.
 * What the span showed last time is there at once; it is read anew behind that. With `from` at
 * `null` nothing is read: there is nothing to compare.
 */
export function createSpan(taskId: string, from: Accessor<Span | null>): SpanWatch {
  const kept = (span: Span) => `task:${taskId}:span.${span}`;
  const [files, setFiles] = createSignal<SpanFile[] | null>(null);
  const [problem, setProblem] = createSignal<string | null>(null);
  const [open, setOpen] = createSignal<Record<string, Shown>>({});

  const show = async (path: string) => {
    const span = from();
    if (!span) return;
    setOpen((now) => ({ ...now, [path]: now[path] && now[path] !== 'loading' ? now[path]! : 'loading' }));
    try {
      const diff = await gitDiffOver(taskId, path, span);
      if (span === from()) setOpen((now) => ({ ...now, [path]: diff }));
    } catch (error) {
      if (span === from()) setOpen((now) => ({ ...now, [path]: { problem: message(error) } }));
    }
  };
  const hide = (path: string) =>
    setOpen((now) => {
      const next = { ...now };
      delete next[path];
      return next;
    });

  const load = async () => {
    const span = from();
    if (!span) return;
    try {
      const listed = await gitFiles(taskId, span);
      if (span !== from()) return;
      setProblem(null);
      setFiles(remember(kept(span), listed.files));
      // The files that were open stay open with fresh lines; with none open, the first one is shown.
      const shown = Object.keys(open()).filter((path) => listed.files.some((file) => file.path === path));
      setOpen((now) => Object.fromEntries(shown.map((path) => [path, now[path]!])));
      const first = listed.files[0];
      for (const path of shown.length > 0 ? shown : first ? [first.path] : []) void show(path);
    } catch (error) {
      if (span !== from()) return;
      setFiles([]);
      setProblem(message(error));
    }
  };

  createEffect(
    on(from, (span) => {
      setFiles(span ? recall<SpanFile[]>(kept(span)) : []);
      setProblem(null);
      setOpen({});
      void load();
    }),
  );
  onMount(() => {
    // eslint-disable-next-line solid/reactivity -- called later, as an event handler: it reads what is current then
    const stop = core.onEvent((event) => {
      if ((event.event === 'git.changed' || event.event === 'review.changed') && event.data.taskId === taskId)
        void load();
    });
    onCleanup(stop);
  });

  return { files, problem, open, show, hide, load };
}

/** The files of a span, in the column: a press on one shows or hides its lines in the middle. */
export function SpanFiles(props: { span: SpanWatch; title: string; none: string }): JSX.Element {
  return (
    <section class="git-block">
      <div class="git-head">
        <h2 class="pane-title grow">{props.title}</h2>
      </div>
      <Show when={props.span.files() === null}>
        <p class="fine">Reading the changes</p>
      </Show>
      <Show when={props.span.files()?.length === 0 && !props.span.problem()}>
        <p class="fine">{props.none}</p>
      </Show>
      <For each={props.span.files() ?? []}>
        {(file) => (
          <div class="git-file" aria-current={props.span.open()[file.path] ? 'true' : undefined}>
            <button
              type="button"
              class="git-file-name"
              aria-expanded={Boolean(props.span.open()[file.path])}
              onClick={() =>
                props.span.open()[file.path] ? props.span.hide(file.path) : void props.span.show(file.path)
              }
            >
              <span class="path">{file.path}</span>
              <FileCount file={file} />
            </button>
          </div>
        )}
      </For>
    </section>
  );
}

/** The middle of the view for a span: every file that differs, one under another, each with its lines. */
export function SpanCenter(props: {
  taskId: string;
  span: SpanWatch;
  title: string;
  /** Said next to the title, in small letters. */
  beside?: JSX.Element;
  /** Said in the middle when no file differs. */
  none: string;
  /** Said at the foot, before the count of files. */
  foot: string;
}): JSX.Element {
  const openFile = (path: string, line?: number) => {
    go(taskHref(props.taskId, 'code'));
    setTimeout(() => openInEditor(props.taskId, path, line), 0);
  };
  return (
    <section class="center">
      <div class="diff-head">
        <h2>{props.title}</h2>
        {props.beside}
      </div>
      <div class="diff-body">
        <Show when={props.span.files()?.length === 0}>
          <p class="diff-note">{props.span.problem() ?? props.none}</p>
        </Show>
        <For each={props.span.files() ?? []}>
          {(file) => {
            const shown = () => props.span.open()[file.path];
            return (
              <>
                <div class="hunk-bar span-file">
                  <span class="mono">{file.path}</span>
                  <FileCount file={file} />
                  <span class="grow" />
                  <Show when={file.kind !== 'deleted'}>
                    <button type="button" class="btn tiny ghost" onClick={() => openFile(file.path)}>
                      Open in the editor
                    </button>
                  </Show>
                  <button
                    type="button"
                    class="btn tiny"
                    aria-expanded={Boolean(shown())}
                    aria-label={`${shown() ? 'Hide' : 'Show'} ${file.path}`}
                    onClick={() => (shown() ? props.span.hide(file.path) : void props.span.show(file.path))}
                  >
                    {shown() ? 'Hide' : 'Show'}
                  </button>
                </div>
                <Show when={shown() === 'loading'}>
                  <p class="diff-note">Reading</p>
                </Show>
                <Show when={typeof shown() === 'object' && 'problem' in (shown() as object)}>
                  <p class="diff-note">{(shown() as { problem: string }).problem}</p>
                </Show>
                <Show when={typeof shown() === 'object' && 'hunks' in (shown() as object) && (shown() as FileDiff)}>
                  {(diff) => <DiffRows diff={diff()} onLine={(line) => openFile(file.path, line)} />}
                </Show>
              </>
            );
          }}
        </For>
      </div>
      <div class="statusline">
        <span>
          {props.foot} {plural(props.span.files()?.length ?? 0, 'file')}
        </span>
      </div>
    </section>
  );
}

/** The Changes view over the time the user was away: what was done in the task, and what it changed. */
export function SpanView(props: { task: Task; git: GitWatch }): JSX.Element {
  // eslint-disable-next-line solid/reactivity -- this component is made anew for each task
  const taskId = props.task.info.id;
  const agentName = () => agentOf(props.task)?.name ?? 'the agent';
  const span = createSpan(taskId, () => 'visit');
  const [since, setSince] = createSignal<Since | null>(null);
  const [problem, setProblem] = createSignal<string | null>(null);

  const loadSince = async () => {
    try {
      setSince(await taskSince(taskId));
    } catch {
      // The files that changed still show; only the story of what happened is missing.
    }
  };
  onMount(() => {
    void loadSince();
    const stop = core.onEvent((event) => {
      if ((event.event === 'git.changed' || event.event === 'review.changed') && event.data.taskId === taskId)
        void loadSince();
    });
    onCleanup(stop);
  });

  /** From here the next visit is counted: what is shown now has been seen. */
  const markSeen = () => {
    void seenUpToNow(taskId)
      .then(() => Promise.all([span.load(), loadSince()]))
      .catch((error) => setProblem(message(error)));
  };

  return (
    <>
      <section class="git" aria-label="What happened while you were away">
        <ChangesTabs taskId={taskId} git={props.git.state()} tab="visit" />
        <Show when={problem() ?? span.problem()}>
          <p class="banner warn flush">{problem() ?? span.problem()}</p>
        </Show>

        <Show when={since()}>
          {(told) => (
            <section class="git-block">
              <div class="git-head">
                <h2 class="pane-title grow">While you were away</h2>
                <span class="fine">{lengthOf(told().awayMs)}</span>
              </div>
              <Show when={told().happened.length === 0}>
                <p class="fine">Nothing was done in this task.</p>
              </Show>
              <For each={told().happened}>
                {(line) => (
                  <div class="away-row">
                    <span class="grow">
                      {line.text}
                      <Show when={line.short}>
                        {' '}
                        <span class="mono muted">{line.short}</span>
                      </Show>
                    </span>
                    <Show when={line.at}>
                      <span class="muted">{ago(line.at!)}</span>
                    </Show>
                  </div>
                )}
              </For>
            </section>
          )}
        </Show>

        <SpanFiles span={span} title="Files changed since then" none="No file has changed since your last visit." />

        <span class="grow" />
        <div class="git-block">
          <button type="button" class="btn small" disabled={(span.files()?.length ?? 0) === 0} onClick={markSeen}>
            Mark all as seen
          </button>
          <span class="fine">The next visit starts counting from here.</span>
        </div>
      </section>

      <SpanCenter
        taskId={taskId}
        span={span}
        title="Since you were last here"
        beside={<Show when={since()}>{(told) => <span class="fine">{whenLabel(told().leftAt)}</span>}</Show>}
        none={`Nothing is new. What ${agentName()} or you change from now on shows here.`}
        foot="Only what is new since your last visit is shown."
      />
    </>
  );
}
