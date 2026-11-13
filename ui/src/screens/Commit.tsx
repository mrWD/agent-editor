import { createSignal, For, onCleanup, onMount, Show, type JSX } from 'solid-js';
import { go, taskHref } from '../app/router';
import { ago, whenLabel } from '../components/bits';
import { commitShown, showCommit } from '../core/commits';
import { gitShow, gitShowFile, projectName, type FileDiff, type ShownCommit, type Task } from '../core/store';
import { openInEditor } from '../core/work';
import { DiffRows, FileCount } from './Span';

const message = (error: unknown) => (error instanceof Error ? error.message : String(error));
const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? '' : 's'}`;

/**
 * One commit in the Changes view: what it says on the left, what it changed
 * in the middle. It is opened from the search over everything and shows a
 * commit of the task's branch or of the project alike. Nothing can be changed
 * here; a commit is history. The view is made anew for each commit.
 */
export function CommitView(props: { task: Task; hash: string }): JSX.Element {
  // eslint-disable-next-line solid/reactivity -- this component is made anew for each task
  const taskId = props.task.info.id;
  // eslint-disable-next-line solid/reactivity -- and for each commit
  const hash = props.hash;
  const [commit, setCommit] = createSignal<ShownCommit | null>(null);
  const [problem, setProblem] = createSignal<string | null>(null);
  const [open, setOpen] = createSignal<Record<string, FileDiff | 'loading' | { problem: string }>>({});

  const show = async (path: string) => {
    setOpen((now) => ({ ...now, [path]: 'loading' }));
    try {
      const diff = await gitShowFile(taskId, hash, path);
      setOpen((now) => ({ ...now, [path]: diff }));
    } catch (error) {
      setOpen((now) => ({ ...now, [path]: { problem: message(error) } }));
    }
  };
  const hide = (path: string) =>
    setOpen((now) => {
      const next = { ...now };
      delete next[path];
      return next;
    });

  onMount(() => {
    void gitShow(taskId, hash)
      .then((found) => {
        setCommit(found);
        // A commit of a few files is read at a glance; a large one is opened file by file.
        for (const file of found.files.slice(0, 5)) void show(file.path);
      })
      .catch((error) => setProblem(message(error)));
  });
  // Leaving the Changes view puts the commit away: coming back shows the changes, as always.
  // When another commit was picked in its place, that one is the one to show.
  onCleanup(() => {
    if (commitShown(taskId) === hash) showCommit(taskId, null);
  });

  const openFile = (path: string) => {
    go(taskHref(taskId, 'code'));
    setTimeout(() => openInEditor(taskId, path), 0);
  };
  // A commit of the project may be older than the task's branch, or newer, made on the branch it started from.
  const where = () =>
    commit()?.onBranch
      ? 'On the branch of this task'
      : `A commit of ${projectName(props.task.info.projectId)}, made outside this task`;

  return (
    <>
      <section class="git" aria-label="Commit">
        <div class="git-head">
          <button type="button" class="btn small" onClick={() => showCommit(taskId, null)}>
            Back to the changes
          </button>
        </div>
        <Show when={problem()}>
          <p class="banner warn flush" role="alert">
            {problem()}
          </p>
        </Show>
        <Show when={!commit() && !problem()}>
          <p class="fine">Reading the commit</p>
        </Show>
        <Show when={commit()}>
          {(found) => (
            <>
              <section class="git-block">
                <h2 class="pane-title">{found().subject}</h2>
                <span class="git-commit-meta">
                  <span class="mono">{found().short}</span>
                  <span>
                    {found().author}, {ago(found().at)}
                  </span>
                </span>
                <span class="fine">{where()}</span>
                <Show when={found().body}>
                  <p class="commit-body">{found().body}</p>
                </Show>
              </section>
              <section class="git-block">
                <div class="git-head">
                  <h2 class="pane-title grow">Files in this commit</h2>
                </div>
                <Show when={found().files.length === 0}>
                  <p class="fine">This commit changed no file.</p>
                </Show>
                <For each={found().files}>
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
            </>
          )}
        </Show>
      </section>

      <section class="center">
        <div class="diff-head">
          <h2>{commit()?.subject ?? 'Commit'}</h2>
          <Show when={commit()}>
            {(found) => (
              <>
                <span class="mono small">{found().short}</span>
                <span class="fine">
                  {found().author}, {whenLabel(found().at)}
                </span>
              </>
            )}
          </Show>
        </div>
        <div class="diff-body">
          <Show when={problem()}>
            <p class="diff-note">{problem()}</p>
          </Show>
          <For each={commit()?.files ?? []}>
            {(file) => {
              const shown = () => open()[file.path];
              return (
                <>
                  <div class="hunk-bar span-file">
                    <span class="mono">{file.path}</span>
                    <FileCount file={file} />
                    <span class="grow" />
                    <Show when={file.kind !== 'deleted'}>
                      <button
                        type="button"
                        class="btn tiny ghost"
                        title="Opens the file as it is now, which may be later than this commit"
                        onClick={() => openFile(file.path)}
                      >
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
                    {(diff) => <DiffRows diff={diff()} />}
                  </Show>
                </>
              );
            }}
          </For>
        </div>
        <div class="statusline">
          <span>
            What this commit changed, compared with the commit before it. {plural(commit()?.files.length ?? 0, 'file')}
          </span>
        </div>
      </section>
    </>
  );
}
