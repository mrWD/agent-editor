import { Grip } from '../components/Grip';
import { sizeOf } from '../core/sizes';
import { createEffect, createMemo, createSignal, For, on, onCleanup, onMount, Show, type JSX } from 'solid-js';
import { createStore } from 'solid-js/store';
import { createKept } from '../core/kept';
import { go, taskHref } from '../app/router';
import { ago, Chip } from '../components/bits';
import { RequestTalk } from '../components/DetailPanel';
import { RequestCard } from '../components/RequestCard';
import { PipelineCard, type PipelineBrief } from '../components/PipelineCard';
import { firstTab, tabAfter, tabCount, type ChangesTab } from '../core/changesTabs';
import { CI_WORDS } from '../core/ci';
import { core } from '../core/client';
import type { GitWatch } from '../core/gitstate';
import {
  agentOf,
  createRequest,
  getRequest,
  gitCommit,
  gitDiff,
  gitPush,
  gitRevert,
  gitRevertCommit,
  gitState,
  gitUndoCommit,
  gitUpdate,
  dismissOverlap,
  listOverlaps,
  sendPrompt,
  state,
  type DiffHunk,
  type FileDiff,
  type GitCommit,
  type GitFile,
  type Overlap,
  type Pick,
  type Span,
  type Task,
} from '../core/store';
import { requestName } from '../core/details';
import { refreshRequests, requests, stageOfTask } from '../core/requests';
import { stageWord } from '../core/stages';
import { openInEditor, saveAll, setWork, workOf } from '../core/work';
import { ChangesTabs, createSpan, SpanCenter, SpanFiles } from './Span';

const message = (error: unknown) => (error instanceof Error ? error.message : String(error));
const plural = (count: number, one: string, many = `${one}s`) => `${count} ${count === 1 ? one : many}`;

/**
 * A button for something that cannot be taken back with one click. The first
 * press asks, the second does it; looking away for a few seconds disarms it.
 */
export function Sure(props: {
  label: string;
  sure: string;
  class?: string;
  disabled?: boolean;
  title?: string;
  /** What the press is about, for a button that stays while that changes. A first press is not carried over. */
  about?: string;
  onSure: () => void;
}): JSX.Element {
  const [armed, setArmed] = createSignal(false);
  let timer: ReturnType<typeof setTimeout> | undefined;
  onCleanup(() => clearTimeout(timer));
  createEffect(
    on(
      () => props.about,
      () => {
        clearTimeout(timer);
        setArmed(false);
      },
      { defer: true },
    ),
  );
  const press = () => {
    clearTimeout(timer);
    if (armed()) {
      setArmed(false);
      props.onSure();
    } else {
      setArmed(true);
      timer = setTimeout(() => setArmed(false), 4000);
    }
  };
  return (
    <button
      type="button"
      class={`${props.class ?? 'btn tiny ghost'} ${armed() ? 'armed' : ''}`}
      disabled={props.disabled}
      title={props.title}
      onClick={press}
    >
      {armed() ? props.sure : props.label}
    </button>
  );
}

/**
 * The items of a list that read the same as before are kept as they were, so
 * what is drawn for them stays as well: a button that waits for its second
 * press does not forget the first one, and the keyboard keeps its place, each
 * time the list is read again.
 */
function steady<T>(before: T[], now: T[]): T[] {
  const kept = new Map(before.map((item) => [JSON.stringify(item), item]));
  return now.map((item) => kept.get(JSON.stringify(item)) ?? item);
}

/** What is left out of the commit: a whole file, or some of its changes by number. */
type Off = Record<string, 'file' | number[] | undefined>;

/**
 * The Changes view, in three parts along the path of a change. What is not
 * committed: the files, what of them goes into the next commit, and the
 * commit itself. What is committed and not pushed: those commits and the
 * push. What is pushed: the request, what stands before its merge, what was
 * written under it, and the pipeline. The column holds one part at a time;
 * the middle shows what that part changes.
 */
export function ChangesView(props: { task: Task; git: GitWatch }): JSX.Element {
  // eslint-disable-next-line solid/reactivity -- this component is made anew for each task, so the id never changes under it
  const taskId = props.task.info.id;
  const agent = () => agentOf(props.task);
  const agentName = () => agent()?.name ?? 'the agent';
  const git = () => props.git.state();

  // The file that was picked and its lines are there at once when the view is opened again.
  const [selected, setSelected] = createKept<string | null>(`task:${taskId}:changes.picked`, null);
  const [diff, setDiff] = createKept<FileDiff | null>(`task:${taskId}:changes.diff`, null);
  const [diffProblem, setDiffProblem] = createSignal<string | null>(null);
  const [off, setOff] = createStore<Off>({});
  const [text, setText] = createSignal('');
  const [touched, setTouched] = createSignal(false);
  const [after, setAfter] = createSignal<'stop' | 'push' | 'request'>('stop');
  const [host, setHost] = createKept<{ name: string; word: string } | null>(`task:${taskId}:changes.host`, null);
  const [amend, setAmend] = createSignal(false);
  const [busy, setBusy] = createSignal<string | null>(null);
  const [problem, setProblem] = createSignal<string | null>(null);
  /** How the pipeline of the branch stands, as its card at the foot of the column tells it. */
  const [pipeBrief, setPipeBrief] = createSignal<PipelineBrief | null>(null);
  let column: HTMLElement | undefined;

  const files = createMemo<GitFile[]>((before) => steady(before, git()?.files ?? []), []);
  const commits = createMemo<GitCommit[]>((before) => steady(before, git()?.commits ?? []), []);
  const hunks = createMemo<DiffHunk[]>(
    (before) => steady(before, diff()?.path === selected() ? diff()!.hunks : []),
    [],
  );
  /** The commits that wait for a push, newest first, and the ones the remote has. */
  const ahead = createMemo(() => commits().filter((made) => !made.pushed));
  const pushed = createMemo(() => commits().filter((made) => made.pushed));
  const request = () => requests[taskId]?.request;

  /** The part on show: the one that was picked, else the first that holds something. */
  const tab = (): ChangesTab => {
    const picked = workOf(taskId).changes;
    return picked && picked !== 'visit' ? picked : firstTab(git());
  };
  /** After something was done the view follows the change: a part left empty gives way to the next that is not. */
  const settle = () => setWork(taskId, { changes: tabAfter(tab(), git()) });
  /** What the middle compares in the second and the third part. Nothing while nothing is pushed. */
  const spanOf = (): Span | null =>
    tab() === 'ahead' ? 'ahead' : tab() === 'request' && tabCount('request', git()) > 0 ? 'pushed' : null;
  const span = createSpan(taskId, spanOf);
  // A commit or a push moves the ends of what is compared without touching a file.
  createEffect(
    on(
      () => `${git()?.commits[0]?.hash}:${git()?.notPushed}`,
      () => void span.load(),
      { defer: true },
    ),
  );
  const newest = () => git()?.commits[0];
  const canAmend = () => Boolean(newest() && !newest()!.pushed);
  const canPush = () => Boolean(git()?.remote);

  // What the user typed in the code is saved first, so the view shows what they see.
  // Then the view opens on the first part that holds something, unless a link asked for one. From
  // then on the part stays as picked: a change made by somebody else is counted, not jumped to.
  onMount(
    () =>
      void saveAll(taskId)
        .then(() => props.git.reload())
        .then(() => {
          if (workOf(taskId).changes === null) setWork(taskId, { changes: firstTab(git()) });
        }),
  );
  // Left, the view forgets the part: the next time it opens where there is something to look at.
  onCleanup(() => {
    if (workOf(taskId).changes !== 'visit') setWork(taskId, { changes: null });
  });
  onMount(() => void loadOverlaps());

  // Whether the project lives on a host where a request can be opened decides what is offered after a commit.
  onMount(
    () =>
      void getRequest(taskId)
        .then((info) => setHost(info.host))
        .catch(() => setHost(null)),
  );

  // The commit message starts as the task's title, until the user writes their own.
  createEffect(() => {
    if (!touched()) setText(props.task.info.title);
  });
  createEffect(() => {
    if (!canAmend()) setAmend(false);
    if (!canPush() || (after() === 'request' && !host())) setAfter('stop');
  });

  // Keep a file selected: the one picked, else the first that changed.
  createEffect(() => {
    const list = files();
    const current = selected();
    if (!current || !list.some((file) => file.path === current)) setSelected(list[0]?.path ?? null);
  });

  let asked = 0;
  const loadDiff = async () => {
    const path = selected();
    const mine = ++asked;
    if (!path) {
      setDiff(null);
      return;
    }
    try {
      const next = await gitDiff(taskId, path);
      if (mine !== asked) return;
      setDiff(next);
      setDiffProblem(null);
    } catch (error) {
      if (mine !== asked) return;
      setDiff(null);
      setDiffProblem(message(error));
    }
  };
  // Read the diff again when another file is picked or the files change on disk.
  createEffect(
    on(
      () => [selected(), git()],
      () => void loadDiff(),
    ),
  );

  // A file that is no longer changed leaves nothing behind in the ticks.
  createEffect(() => {
    const present = new Set(files().map((file) => file.path));
    for (const path of Object.keys(off)) {
      if (!present.has(path)) setOff(path, undefined);
    }
  });

  const offChanges = (path: string): number[] => {
    const state = off[path];
    return Array.isArray(state) ? state : [];
  };
  const fileTicked = (path: string) => off[path] !== 'file';
  const changeTicked = (path: string, index: number) => fileTicked(path) && !offChanges(path).includes(index);
  const tickedChanges = (file: GitFile) => (fileTicked(file.path) ? file.changes - offChanges(file.path).length : 0);

  const toggleFile = (path: string) => setOff(path, fileTicked(path) ? 'file' : undefined);
  const toggleChange = (file: GitFile, index: number) => {
    const without = changeTicked(file.path, index)
      ? [...offChanges(file.path), index]
      : offChanges(file.path).filter((n) => n !== index);
    // Every change unticked is the same as the file unticked.
    setOff(file.path, without.length >= file.changes ? 'file' : without.length === 0 ? undefined : without);
  };

  const picks = createMemo<Pick[]>(() =>
    files().flatMap((file) => {
      if (!fileTicked(file.path)) return [];
      const left = offChanges(file.path);
      if (left.length === 0) return [{ path: file.path }];
      return [
        {
          path: file.path,
          changes: Array.from({ length: file.changes }, (_, i) => i).filter((i) => !left.includes(i)),
        },
      ];
    }),
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
      // The state is read here, not left to the watcher: a reading that was already on its way
      // would be the one from before, and the view would follow a change that is not in it yet.
      try {
        props.git.show(await gitState(taskId));
      } catch {
        void props.git.reload();
      }
      settle();
    }
  };

  /** Pushes the commits that wait. With `open`, a request is opened for them, said as the commits say it. */
  const push = (open: boolean) =>
    // eslint-disable-next-line solid/reactivity -- called later, as an event handler: it reads what is current then
    act(open ? 'push-request' : 'push', async () => {
      await gitPush(taskId);
      if (!open) return;
      const info = await getRequest(taskId, true);
      if (!info.request)
        await createRequest(taskId, info.defaults?.title ?? props.task.info.title, info.defaults?.body ?? '');
      void refreshRequests();
    });
  const showPipeline = () => {
    setWork(taskId, { changes: 'request' });
    setTimeout(
      () => column?.querySelector('section.pipeline')?.scrollIntoView({ block: 'start', behavior: 'smooth' }),
      0,
    );
  };

  const commit = () =>
    // eslint-disable-next-line solid/reactivity -- called later, as an event handler: it reads what is current then
    act('commit', async () => {
      // What to do after the commit is read once, at the press. While the commit runs the
      // view reads the git state again, and for a moment that can reset the choice on screen.
      const then = after();
      const next = await gitCommit(taskId, text(), picks(), { amend: amend(), push: then !== 'stop' });
      props.git.show(next);
      if (then === 'request') {
        // The request says what the commits say, as written by the core from the commits and the issue.
        const info = await getRequest(taskId, true);
        if (!info.request) await createRequest(taskId, info.defaults?.title ?? text(), info.defaults?.body ?? '');
      }
      setTouched(false);
      setAmend(false);
    });

  const commitLabel = () => {
    const count = picks().length;
    const what = amend() ? `Add ${plural(count, 'file')} to the last commit` : `Commit ${plural(count, 'file')}`;
    if (after() === 'request') return `${what}, push and open a ${host()?.word ?? 'merge request'}`;
    return after() === 'push' ? `${what} and push` : what;
  };

  const who = (hunk: DiffHunk) => {
    switch (hunk.who) {
      case 'agent':
        return `, written by ${agentName()}`;
      case 'you':
        return ', written by you';
      case 'both':
        return `, written by ${agentName()} and you`;
      default:
        return '';
    }
  };
  // ----- lines that another open task changes too -----
  const [overlaps, setOverlaps] = createKept<Overlap[]>(`task:${taskId}:changes.overlaps`, []);
  const loadOverlaps = async () => {
    try {
      setOverlaps((await listOverlaps(taskId)).overlaps);
    } catch {
      // Finding overlaps is a help, not a need: without it the view works as before.
    }
  };
  const otherShort = (agentId: string) => state.agents.find((one) => one.id === agentId)?.short ?? '··';
  const otherStage = (id: string) => {
    const other = state.tasks[id];
    return other ? stageWord(stageOfTask(other)) : '';
  };
  const canAsk = () => props.task.info.state === 'idle' || props.task.info.state === 'asleep';
  const combinePrompt = (overlap: Overlap) =>
    `Another task, “${overlap.task.title}”, changes the same place of \`${overlap.path}\`, around line ${overlap.line}.\n\n` +
    `This task has there:\n\n\`\`\`\n${overlap.mine.join('\n')}\n\`\`\`\n\n` +
    `The other task has there:\n\n\`\`\`\n${overlap.theirs.join('\n')}\n\`\`\`\n\n` +
    `Change \`${overlap.path}\` here so that both changes are kept, if they do not contradict each other. If they do, change nothing and say why.`;

  // ----- a commit message written by the agent -----
  /** The length of the conversation when the agent was asked; `null` when nothing is awaited. */
  const [awaited, setAwaited] = createSignal<number | null>(null);
  const askForMessage = () =>
    // eslint-disable-next-line solid/reactivity -- called later, as an event handler: it reads what is current then
    act('message', async () => {
      const names = files()
        .filter((file) => fileTicked(file.path))
        .map((file) => `\`${file.path}\``)
        .slice(0, 30)
        .join(', ');
      setAwaited(props.task.feed.length);
      try {
        await sendPrompt(
          taskId,
          `Look at what is not committed in ${names || 'this task'} and write a commit message for it. ` +
            'Answer with the commit message only: one line of at most 72 characters, then, if it helps, an empty line and a few lines on why. Leave every file as it is.',
        );
      } catch (error) {
        setAwaited(null);
        throw error;
      }
    });
  // When that turn has ended, what the agent said last is the message.
  createEffect(() => {
    const from = awaited();
    if (from === null) return;
    const since = props.task.feed.slice(from);
    const asked = since.findIndex((entry) => entry.kind === 'user');
    if (asked < 0) return;
    const after = since.slice(asked + 1);
    // The turn is over when the task is at rest again with something said: an answer, or why there is none.
    const atRest = props.task.info.state === 'idle' || props.task.info.state === 'asleep';
    const said = after.findLast((entry) => entry.kind === 'agent');
    const failed = after.some((entry) => entry.kind === 'notice');
    if (!atRest || (!said && !failed)) return;
    if (said && said.kind === 'agent') {
      // Agents like to wrap such an answer in a code fence; the message is what is inside.
      const message = said.text
        .trim()
        .replace(/^```[a-z]*\n?/, '')
        .replace(/\n?```$/, '')
        .trim();
      if (message) {
        setText(message);
        setTouched(true);
      }
    }
    setAwaited(null);
  });

  const where = (hunk: DiffHunk) =>
    hunk.fromLine === hunk.toLine ? `Line ${hunk.fromLine}` : `Lines ${hunk.fromLine} to ${hunk.toLine}`;

  const reviewOf = (path: string) => props.task.changed.find((file) => file.path === path);
  const badges = (path: string) => (
    <>
      <Show when={(reviewOf(path)?.agentLines ?? 0) > 0}>
        <Chip
          agentId={props.task.info.agentId}
          short={agent()?.short ?? 'Ag'}
          flat
          label={`Changed by ${agentName()}`}
        />
      </Show>
      <Show when={(reviewOf(path)?.yourLines ?? 0) > 0}>
        <Chip agentId="you" short="You" flat label="Changed by you" />
      </Show>
    </>
  );

  /** Asks the agent about one line of the diff: the line goes along with the next message. */
  const ask = (path: string, line: number, lineText: string) => {
    setWork(taskId, { quote: { path, fromLine: line, toLine: line, text: lineText } });
    document.getElementById(`msg-${taskId}`)?.focus();
  };

  const showInEditor = (path: string, line?: number) => {
    go(taskHref(taskId, 'code'));
    setTimeout(() => openInEditor(taskId, path, line), 0);
  };

  // Other windows may change the same task; the core tells every one of them.
  onMount(() => {
    const stop = core.onEvent((event) => {
      if (event.event === 'git.changed' && event.data.taskId === taskId) void loadDiff();
      // Another task of the project may have moved too; its changes are read from disk each time.
      if (event.event === 'git.changed' || event.event === 'review.changed') void loadOverlaps();
    });
    onCleanup(stop);
  });

  return (
    <Show
      when={git()}
      fallback={
        <section class="center">
          <div class="center-note">
            <h2>{props.git.problem() ? 'Nothing to commit here' : 'Reading the changes'}</h2>
            <p>{props.git.problem()}</p>
          </div>
        </section>
      }
    >
      {(state) => (
        <>
          <section
            class="git"
            aria-label="Git: what goes in, commit, push"
            ref={column}
            style={{ flex: `0 0 ${sizeOf('changes')}px` }}
          >
            <ChangesTabs taskId={taskId} git={state()} tab={tab()} pipeline={pipeBrief()?.state} />
            <div class="git-branch">
              <span class="mono">{state().branch ?? 'no branch'}</span>
              <Show
                when={state().base && state().baseAhead > 0}
                fallback={<span class="grow fine">{state().base ? `up to date with ${state().base}` : ''}</span>}
              >
                <span class="grow fine">
                  {state().base} has {plural(state().baseAhead, 'new commit')}
                </span>
                <button
                  type="button"
                  class="btn tiny"
                  disabled={Boolean(busy())}
                  onClick={() => act('update', () => gitUpdate(taskId))}
                >
                  {busy() === 'update' ? 'Updating' : `Update from ${state().base}`}
                </button>
              </Show>
            </div>
            {/* How the pipeline stands, said here because its card is in the third part. */}
            <Show when={tab() !== 'request' && pipeBrief()}>
              {(brief) => (
                <button type="button" class="pipe-brief" title="Show the pipeline" onClick={showPipeline}>
                  <span class={`pipe-dot ${brief().state}`} aria-hidden="true" />
                  <span>
                    Pipeline {CI_WORDS[brief().state]}: {brief().words}
                  </span>
                </button>
              )}
            </Show>

            <Show when={problem()}>
              <p class="banner danger flush" role="alert">
                <span class="grow">{problem()}</span>
                <button type="button" class="btn tiny" onClick={() => setProblem(null)}>
                  Dismiss
                </button>
              </p>
            </Show>

            <div class="git-tab" hidden={tab() !== 'head'}>
              <For each={overlaps()}>
                {(overlap) => (
                  <section class="card overlap" aria-label={`Overlap in ${overlap.path}`}>
                    <h2>Another task changes the same {overlap.mine.length > 1 ? 'lines' : 'line'}</h2>
                    <p>
                      <button type="button" class="link mono" onClick={() => setSelected(overlap.path)}>
                        {overlap.path}
                      </button>
                      , line {overlap.line}, is also changed in:
                    </p>
                    <a class="overlap-task" href={taskHref(overlap.task.id, 'changes')}>
                      <Chip agentId={overlap.task.agentId} short={otherShort(overlap.task.agentId)} flat />
                      <span class="grow">{overlap.task.title}</span>
                      <span class="fine">{otherStage(overlap.task.id)}</span>
                    </a>
                    <Show
                      when={overlap.theirs.length > 0}
                      fallback={<p class="fine">That task takes these lines out.</p>}
                    >
                      <span class="fine">Their line {overlap.theirLine}</span>
                      <pre class="overlap-lines">{overlap.theirs.join('\n')}</pre>
                    </Show>
                    <p class="fine">Whichever merges second gets the conflict. You can commit as it is.</p>
                    <div class="actions">
                      <button
                        type="button"
                        class="btn small"
                        disabled={Boolean(busy()) || !canAsk()}
                        onClick={() => act('combine', () => sendPrompt(taskId, combinePrompt(overlap)))}
                      >
                        Ask {agentName()} to combine them
                      </button>
                      <button
                        type="button"
                        class="btn small ghost"
                        disabled={Boolean(busy())}
                        onClick={() => act('dismiss', () => dismissOverlap(taskId, overlap.mark).then(loadOverlaps))}
                      >
                        Not a problem
                      </button>
                    </div>
                  </section>
                )}
              </For>
              <section class="git-block">
                <div class="git-head">
                  <h2 class="pane-title grow">Not committed</h2>
                  <Show when={files().length > 0}>
                    <span class="fine">Ticked files go into the commit</span>
                  </Show>
                </div>
                <Show when={files().length === 0}>
                  <p class="fine">Everything is committed.</p>
                </Show>
                <For each={files()}>
                  {(file) => (
                    <div class="git-file" aria-current={file.path === selected() ? 'true' : undefined}>
                      <input
                        type="checkbox"
                        checked={fileTicked(file.path)}
                        ref={(el) =>
                          createEffect(
                            () => (el.indeterminate = fileTicked(file.path) && offChanges(file.path).length > 0),
                          )
                        }
                        aria-label={`Put ${file.path} into the commit`}
                        onChange={() => toggleFile(file.path)}
                      />
                      <button type="button" class="git-file-name" onClick={() => setSelected(file.path)}>
                        <span class="path">{file.path}</span>
                        <span class="git-file-meta">
                          <Show when={file.added > 0}>
                            <span class="plus">+{file.added}</span>
                          </Show>
                          <Show when={file.removed > 0}>
                            <span class="minus">−{file.removed}</span>
                          </Show>
                          <Show when={file.kind !== 'modified'}>
                            <span class="word">{file.kind === 'added' ? 'new' : 'deleted'}</span>
                          </Show>
                          {badges(file.path)}
                          <Show when={file.changes > 1 && fileTicked(file.path) && offChanges(file.path).length > 0}>
                            <span class="word">
                              {tickedChanges(file)} of {file.changes} changes ticked
                            </span>
                          </Show>
                        </span>
                      </button>
                      <Sure
                        label="Revert"
                        sure="Really revert?"
                        disabled={Boolean(busy())}
                        title={`Put ${file.path} back as it is in the last commit`}
                        onSure={() => act('revert', () => gitRevert(taskId, file.path))}
                      />
                    </div>
                  )}
                </For>
              </section>

              <Show when={files().length > 0 || amend()}>
                <section class="git-block">
                  <label class="pane-title" for={`commit-${taskId}`}>
                    Commit message
                  </label>
                  <textarea
                    id={`commit-${taskId}`}
                    class="git-message"
                    rows="2"
                    value={text()}
                    onInput={(event) => {
                      setTouched(true);
                      setText(event.currentTarget.value);
                    }}
                  />
                  <div class="git-head">
                    <span class="grow fine">
                      {awaited() !== null
                        ? `${agentName()} is writing it`
                        : touched()
                          ? ''
                          : "Taken from the task's title. Edit it freely."}
                    </span>
                    <button
                      type="button"
                      class="btn tiny ghost"
                      disabled={Boolean(busy()) || !canAsk() || awaited() !== null}
                      title={`${agentName()} reads the changes and writes the message. It is a turn of the conversation.`}
                      onClick={() => void askForMessage()}
                    >
                      Ask {agentName()} to write it
                    </button>
                  </div>

                  <fieldset class="git-after">
                    <legend>After the commit</legend>
                    <label>
                      <input
                        type="radio"
                        name={`after-${taskId}`}
                        checked={after() === 'stop'}
                        onChange={() => setAfter('stop')}
                      />
                      Stop there
                    </label>
                    <label classList={{ dim: !canPush() }}>
                      <input
                        type="radio"
                        name={`after-${taskId}`}
                        checked={after() === 'push'}
                        disabled={!canPush()}
                        onChange={() => setAfter('push')}
                      />
                      Push
                      <Show when={!canPush()}>
                        <span class="fine">this project has no remote</span>
                      </Show>
                    </label>
                    <Show when={canPush() && host()}>
                      <label>
                        <input
                          type="radio"
                          name={`after-${taskId}`}
                          checked={after() === 'request'}
                          onChange={() => setAfter('request')}
                        />
                        Push and open a {host()!.word}
                      </label>
                    </Show>
                  </fieldset>

                  <button
                    type="button"
                    class="btn primary tall"
                    disabled={Boolean(busy()) || picks().length === 0 || text().trim() === ''}
                    onClick={commit}
                  >
                    {busy() === 'commit' ? 'Committing' : commitLabel()}
                  </button>
                  <label class="git-amend" classList={{ dim: !canAmend() }}>
                    <input
                      type="checkbox"
                      checked={amend()}
                      disabled={!canAmend()}
                      onChange={(event) => setAmend(event.currentTarget.checked)}
                    />
                    Add to the last commit, not a new one
                  </label>
                </section>
              </Show>
            </div>

            <div class="git-tab" hidden={tab() !== 'ahead'}>
              <section class="git-block">
                <div class="git-head">
                  <h2 class="pane-title grow">Committed</h2>
                  <Show when={ahead().length > 0}>
                    <span class="fine accent">{ahead().length} not pushed</span>
                  </Show>
                </div>
                <Show when={ahead().length === 0}>
                  <p class="fine">
                    {pushed().length > 0
                      ? 'Every commit is pushed.'
                      : `No commits yet${state().base ? ` since ${state().base}` : ''}.`}
                  </p>
                </Show>
                <For each={ahead()}>
                  {(made, index) => (
                    <div class="git-commit">
                      <div class="grow">
                        <span class="subject">{made.subject}</span>
                        <span class="git-commit-meta">
                          <span class="mono">{made.short}</span>
                          <span>
                            {made.author}, {ago(made.at)}
                          </span>
                        </span>
                      </div>
                      <button
                        type="button"
                        class="btn tiny ghost"
                        disabled={Boolean(busy()) || index() !== 0}
                        title={
                          index() === 0
                            ? 'Take the commit off the branch and keep its changes in the files'
                            : 'Undo the commits above this one first'
                        }
                        onClick={() => act('undo', () => gitUndoCommit(taskId, made.hash))}
                      >
                        Undo
                      </button>
                    </div>
                  )}
                </For>
                <Show when={ahead().length > 0}>
                  <Show when={canPush()} fallback={<p class="fine">There is no remote to push them to.</p>}>
                    <button
                      type="button"
                      class="btn primary tall"
                      disabled={Boolean(busy())}
                      onClick={() => void push(false)}
                    >
                      {busy() === 'push' ? 'Pushing' : 'Push'}
                    </button>
                    <Show when={host() && !request()}>
                      <button type="button" class="btn tall" disabled={Boolean(busy())} onClick={() => void push(true)}>
                        {busy() === 'push-request' ? 'Opening' : `Push and open a ${host()!.word}`}
                      </button>
                    </Show>
                  </Show>
                </Show>
              </section>
              <SpanFiles span={span} title="What they change" none="Nothing waits for a push." />
            </div>

            <div class="git-tab" hidden={tab() !== 'request'}>
              <RequestCard
                task={props.task}
                commits={state().commits.length}
                overlaps={overlaps().length}
                onChanged={() => void props.git.reload()}
              />
              <PipelineCard
                task={props.task}
                pushes={`${state().commits.length}:${state().notPushed}`}
                onBrief={setPipeBrief}
              />
              <Show when={request()}>
                <RequestTalk taskId={taskId} projectId={props.task.info.projectId} shown={tab() === 'request'} />
              </Show>
              <section class="git-block">
                <div class="git-head">
                  <h2 class="pane-title grow">Pushed</h2>
                  <Show when={pushed().length > 0}>
                    <span class="fine">{plural(pushed().length, 'commit')}</span>
                  </Show>
                </div>
                <Show when={pushed().length === 0}>
                  <p class="fine">
                    {canPush() ? 'Nothing is pushed yet.' : 'There is no remote, so nothing can be pushed.'}
                  </p>
                </Show>
                <For each={pushed()}>
                  {(made) => (
                    <div class="git-commit">
                      <div class="grow">
                        <span class="subject">{made.subject}</span>
                        <span class="git-commit-meta">
                          <span class="mono">{made.short}</span>
                          <span>
                            {made.author}, {ago(made.at)}
                          </span>
                        </span>
                      </div>
                      <Sure
                        label="Revert"
                        sure="Really revert?"
                        disabled={Boolean(busy())}
                        title="Add a new commit that takes this one back"
                        onSure={() => act('revert-commit', () => gitRevertCommit(taskId, made.hash))}
                      />
                    </div>
                  )}
                </For>
              </section>
              <Show when={pushed().length > 0}>
                <SpanFiles
                  span={span}
                  title="What is pushed changes"
                  none="No file differs from where the branch started."
                />
              </Show>
            </div>
          </section>
          <Grip name="changes" edge="right" label="Drag to make the column of changes wider or narrower" />

          <Show
            when={tab() === 'head'}
            fallback={
              <Show
                when={tab() === 'ahead'}
                fallback={
                  <SpanCenter
                    taskId={taskId}
                    span={span}
                    title={request() ? `In ${requestName(request()!)}` : 'Pushed'}
                    beside={
                      <Show when={state().base && pushed().length > 0}>
                        <span class="fine">compared with where the branch left {state().base}</span>
                      </Show>
                    }
                    none={pushed().length === 0 ? 'Nothing is pushed yet.' : 'What is pushed changes no file.'}
                    foot="What is pushed: what a reviewer sees."
                  />
                }
              >
                <SpanCenter
                  taskId={taskId}
                  span={span}
                  title="Committed, not pushed"
                  beside={
                    <Show when={ahead().length > 0}>
                      <span class="fine">{plural(ahead().length, 'commit')}</span>
                    </Show>
                  }
                  none={ahead().length === 0 ? 'Nothing waits for a push.' : 'These commits change no file.'}
                  foot="What the commits that are not pushed change."
                />
              </Show>
            }
          >
            <section class="center">
              <Show
                when={selected()}
                fallback={
                  <div class="center-note">
                    <h2>Nothing to show</h2>
                    <p>There are no changes that are not committed.</p>
                  </div>
                }
              >
                {(path) => {
                  const file = () => files().find((entry) => entry.path === path());
                  return (
                    <>
                      <div class="diff-head">
                        <h2 class="mono">{path()}</h2>
                        <Show when={file()}>
                          {(info) => (
                            <>
                              <span class="mono small">
                                <Show when={info().added > 0}>
                                  <span class="plus">+{info().added}</span>{' '}
                                </Show>
                                <Show when={info().removed > 0}>
                                  <span class="minus">−{info().removed}</span>
                                </Show>
                              </span>
                              <span class="fine">
                                {plural(info().changes, 'change')},{' '}
                                {tickedChanges(info()) === info().changes
                                  ? info().changes === 1
                                    ? 'in the commit'
                                    : 'all in the commit'
                                  : tickedChanges(info()) === 0
                                    ? 'not in the commit'
                                    : `${tickedChanges(info())} in the commit`}
                              </span>
                            </>
                          )}
                        </Show>
                        <span class="grow" />
                        <Show when={file()?.kind !== 'deleted'}>
                          <button
                            type="button"
                            class="btn small ghost"
                            onClick={() => showInEditor(path(), diff()?.hunks[0]?.fromLine)}
                          >
                            Open in the editor
                          </button>
                        </Show>
                        <Sure
                          class="btn small ghost"
                          label="Revert this file"
                          about={path()}
                          sure="Really revert?"
                          disabled={Boolean(busy())}
                          onSure={() => act('revert', () => gitRevert(taskId, path()))}
                        />
                      </div>

                      <div class="diff-body">
                        <Show when={diffProblem()}>
                          <p class="diff-note">{diffProblem()}</p>
                        </Show>
                        <Show when={diff()?.binary}>
                          <p class="diff-note">
                            This file is not text, or is too large to show. It can be committed or reverted whole.
                          </p>
                        </Show>
                        <Show when={diff() && !diff()!.binary && diff()!.hunks.length === 0}>
                          <p class="diff-note">
                            {diff()!.kind === 'added'
                              ? 'A new, empty file.'
                              : 'Only the file itself changed, not its lines.'}
                          </p>
                        </Show>
                        <For each={hunks()}>
                          {(hunk) => (
                            <>
                              <div class="hunk-bar">
                                <label>
                                  <input
                                    type="checkbox"
                                    checked={changeTicked(path(), hunk.index)}
                                    disabled={!file()}
                                    onChange={() => file() && toggleChange(file()!, hunk.index)}
                                  />
                                  In the commit
                                </label>
                                <span>
                                  {where(hunk)}
                                  {who(hunk)}
                                </span>
                                <span class="grow" />
                                <Sure
                                  label="Revert this change"
                                  sure="Really revert?"
                                  disabled={Boolean(busy())}
                                  onSure={() => act('revert', () => gitRevert(taskId, path(), hunk.index))}
                                />
                              </div>
                              <For each={hunk.lines}>
                                {(line) => (
                                  <div class={`diff-row ${line.kind}`}>
                                    <button
                                      type="button"
                                      class="no"
                                      title={`Ask ${agentName()} about this line`}
                                      onClick={() => ask(path(), line.newNo ?? line.oldNo ?? 1, line.text)}
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
                      </div>

                      <div class="statusline">
                        <span>Click a line number to ask {agentName()} about that line</span>
                      </div>
                    </>
                  );
                }}
              </Show>
            </section>
          </Show>
        </>
      )}
    </Show>
  );
}
