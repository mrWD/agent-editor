import { createEffect, createSignal, For, on, onCleanup, onMount, Show, type JSX } from 'solid-js';
import { recall, remember } from '../core/kept';
import { go, taskHref } from '../app/router';
import { ago, span as lengthOf, whenLabel } from '../components/bits';
import { core } from '../core/client';
import { seenUpToNow } from '../core/visits';
import {
  agentOf,
  gitDiffOver,
  gitFiles,
  taskSince,
  type FileDiff,
  type Since,
  type Span,
  type SpanFile,
  type Task,
} from '../core/store';
import { openInEditor, setWork, workOf, type Work } from '../core/work';

const message = (error: unknown) => (error instanceof Error ? error.message : String(error));
const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? '' : 's'}`;

/** The three things the Changes view can compare the code with. */
export function ChangesMode(props: { taskId: string }): JSX.Element {
  const mode = () => workOf(props.taskId).changes;
  const pick = (changes: Work['changes']) => setWork(props.taskId, { changes });
  return (
    <div class="span-mode">
      <span class="fine">Show changes</span>
      <div class="switcher" role="group" aria-label="Which changes to show">
        <button type="button" aria-pressed={mode() === 'visit'} onClick={() => pick('visit')}>
          Since last visit
        </button>
        <button type="button" aria-pressed={mode() === 'head'} onClick={() => pick('head')}>
          Not committed
        </button>
        <button type="button" aria-pressed={mode() === 'branch'} onClick={() => pick('branch')}>
          Branch
        </button>
      </div>
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

/**
 * The Changes view over a stretch of the task's life: what is new since the
 * user's last visit, or everything the task's branch holds.
 */
export function SpanView(props: { task: Task; mode: 'visit' | 'branch' }): JSX.Element {
  // eslint-disable-next-line solid/reactivity -- this component is made anew for each task
  const taskId = props.task.info.id;
  const agentName = () => agentOf(props.task)?.name ?? 'the agent';
  // What each of the two looks showed last time is there at once; it is read anew behind that.
  const keptFiles = (mode: Span) => `task:${taskId}:span.${mode}`;
  // eslint-disable-next-line solid/reactivity -- only where it starts: the effect below follows the mode from then on
  const [files, setFiles] = createSignal<SpanFile[] | null>(recall<SpanFile[]>(keptFiles(props.mode)));
  const [problem, setProblem] = createSignal<string | null>(null);
  const [since, setSince] = createSignal<Since | null>(null);
  const [open, setOpen] = createSignal<Record<string, FileDiff | 'loading' | { problem: string }>>({});
  const from = (): Span => props.mode;

  const show = async (path: string) => {
    setOpen((now) => ({ ...now, [path]: now[path] && now[path] !== 'loading' ? now[path]! : 'loading' }));
    const mode = from();
    try {
      const diff = await gitDiffOver(taskId, path, mode);
      if (mode === from()) setOpen((now) => ({ ...now, [path]: diff }));
    } catch (error) {
      if (mode === from()) setOpen((now) => ({ ...now, [path]: { problem: message(error) } }));
    }
  };
  const hide = (path: string) =>
    setOpen((now) => {
      const next = { ...now };
      delete next[path];
      return next;
    });

  const load = async () => {
    const mode = from();
    try {
      const [listed, told] = await Promise.all([
        gitFiles(taskId, mode),
        mode === 'visit' ? taskSince(taskId) : Promise.resolve(null),
      ]);
      if (mode !== from()) return;
      setProblem(null);
      setFiles(remember(keptFiles(mode), listed.files));
      setSince(told);
      // The files that were open stay open with fresh lines; with none open, the first one is shown.
      const shown = Object.keys(open()).filter((path) => listed.files.some((file) => file.path === path));
      setOpen((now) => Object.fromEntries(shown.map((path) => [path, now[path]!])));
      const first = listed.files[0];
      for (const path of shown.length > 0 ? shown : first ? [first.path] : []) void show(path);
    } catch (error) {
      if (mode !== from()) return;
      setFiles([]);
      setProblem(message(error));
    }
  };

  createEffect(
    on(from, (mode) => {
      setFiles(recall<SpanFile[]>(keptFiles(mode)));
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

  const openFile = (path: string, line?: number) => {
    go(taskHref(taskId, 'code'));
    setTimeout(() => openInEditor(taskId, path, line), 0);
  };

  /** From here the next visit is counted: what is shown now has been seen. */
  const markSeen = () => {
    void seenUpToNow(taskId)
      .then(load)
      .catch((error) => setProblem(message(error)));
  };

  return (
    <>
      <section
        class="git"
        aria-label={props.mode === 'visit' ? 'What happened while you were away' : 'What this branch holds'}
      >
        <ChangesMode taskId={taskId} />
        <Show when={problem()}>
          <p class="banner warn flush">{problem()}</p>
        </Show>

        <Show when={props.mode === 'visit' && since()}>
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

        <section class="git-block">
          <div class="git-head">
            <h2 class="pane-title grow">
              {props.mode === 'visit' ? 'Files changed since then' : 'Files changed on this branch'}
            </h2>
          </div>
          <Show when={files() === null}>
            <p class="fine">Reading the changes</p>
          </Show>
          <Show when={files()?.length === 0 && !problem()}>
            <p class="fine">
              {props.mode === 'visit'
                ? 'No file has changed since your last visit.'
                : 'This branch holds no changes yet.'}
            </p>
          </Show>
          <For each={files() ?? []}>
            {(file) => (
              <div class="git-file" aria-current={open()[file.path] ? 'true' : undefined}>
                <button
                  type="button"
                  class="git-file-name"
                  aria-expanded={Boolean(open()[file.path])}
                  onClick={() => (open()[file.path] ? hide(file.path) : void show(file.path))}
                >
                  <span class="path">{file.path}</span>
                  <FileCount file={file} />
                </button>
              </div>
            )}
          </For>
        </section>

        <Show when={props.mode === 'visit'}>
          <span class="grow" />
          <div class="git-block">
            <button type="button" class="btn small" disabled={(files()?.length ?? 0) === 0} onClick={markSeen}>
              Mark all as seen
            </button>
            <span class="fine">The next visit starts counting from here.</span>
          </div>
        </Show>
      </section>

      <section class="center">
        <div class="diff-head">
          <h2>{props.mode === 'visit' ? 'Since you were last here' : 'Everything on this branch'}</h2>
          <Show when={props.mode === 'visit' && since()}>
            {(told) => <span class="fine">{whenLabel(told().leftAt)}</span>}
          </Show>
          <Show when={props.mode === 'branch' && props.task.info.baseBranch}>
            <span class="fine">compared with where it left {props.task.info.baseBranch}</span>
          </Show>
        </div>
        <div class="diff-body">
          <Show when={files()?.length === 0}>
            <p class="diff-note">
              {problem() ??
                (props.mode === 'visit'
                  ? `Nothing is new. What ${agentName()} or you change from now on shows here.`
                  : 'Nothing to show.')}
            </p>
          </Show>
          <For each={files() ?? []}>
            {(file) => {
              const shown = () => open()[file.path];
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
                      onClick={() => (shown() ? hide(file.path) : void show(file.path))}
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
            {props.mode === 'visit'
              ? 'Only what is new since your last visit is shown. ' + plural(files()?.length ?? 0, 'file')
              : 'Commits and what is not committed yet, together. ' + plural(files()?.length ?? 0, 'file')}
          </span>
        </div>
      </section>
    </>
  );
}
