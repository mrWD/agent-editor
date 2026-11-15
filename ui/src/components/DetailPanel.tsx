import {
  createEffect,
  createSignal,
  For,
  Match,
  on,
  onCleanup,
  onMount,
  Show,
  Switch,
  type Accessor,
  type JSX,
} from 'solid-js';
import { core } from '../core/client';
import {
  gateOf,
  noteAbout,
  notesWords,
  peopleLines,
  pictureFrom,
  trackerName,
  type Detail,
  type Gate,
} from '../core/details';
import { createKept, recall, remember } from '../core/kept';
import {
  gitDiffOver,
  gitFiles,
  requestComments,
  state,
  type FileDiff,
  type RequestInfo,
  type ReviewComment,
  type SpanFile,
} from '../core/store';
import { sizeOf } from '../core/sizes';
import { ago } from './bits';
import { Grip } from './Grip';
import { Markdown } from './Markdown';
import { PipelineCard } from './PipelineCard';

/** What a panel was opened on: what to call it meanwhile, whose it is, and how the tracker is asked for it. */
export type Opened = {
  /** A name of its own among everything that can be opened: what was read last is kept under it. */
  id: string;
  title: string;
  /** The project it belongs to: its pictures are fetched with that project's sign-in. */
  projectId: string;
  /** For a request: the task whose request it is. Its changes and its checks are read from the task. */
  taskId?: string;
  ask: () => Promise<Detail>;
  /** What can be done with it, shown at the foot of the panel. */
  foot?: () => JSX.Element;
};

export type Details = {
  opened: Accessor<Opened | null>;
  detail: Accessor<Detail | null>;
  loading: Accessor<boolean>;
  problem: Accessor<string | null>;
  open: (what: Opened) => void;
  close: () => void;
  again: () => void;
};

/**
 * What a screen needs to show one issue or request at its side: which is open, what the tracker
 * said about it, and whether it is being asked. What was read last time is shown at once and
 * read again behind that; an answer for something that is no longer open is dropped.
 */
export function createDetails(): Details {
  const [opened, setOpened] = createSignal<Opened | null>(null);
  const [detail, setDetail] = createSignal<Detail | null>(null);
  const [loading, setLoading] = createSignal(false);
  const [problem, setProblem] = createSignal<string | null>(null);
  let asked = 0;

  const read = async (what: Opened) => {
    const mine = ++asked;
    setLoading(true);
    setProblem(null);
    try {
      const found = await what.ask();
      remember(`detail:${what.id}`, found);
      if (mine === asked) setDetail(found);
    } catch (error) {
      if (mine === asked) setProblem(error instanceof Error ? error.message : String(error));
    } finally {
      if (mine === asked) setLoading(false);
    }
  };

  return {
    opened,
    detail,
    loading,
    problem,
    open: (what) => {
      setOpened(what);
      setDetail(recall<Detail>(`detail:${what.id}`));
      void read(what);
    },
    close: () => {
      asked += 1;
      setOpened(null);
      setLoading(false);
    },
    again: () => {
      const what = opened();
      if (what) void read(what);
    },
  };
}

/** The least width left to what a panel stands beside, however wide the panel is dragged. */
const LEAST_BESIDE = 280;

/** The panels that are open, in the order they were opened: Escape closes the newest. */
const open: symbol[] = [];

/**
 * The frame of a side panel: in its head the name of what is open as a link, how it stands, a
 * button that opens it in the tracker, one that reads it again and one that closes the panel.
 */
function Frame(props: {
  details: Details;
  /** What the panel is of, for whoever cannot see it: "issue", "request". Its width is kept under the same name. */
  what: 'issue' | 'request';
  children: JSX.Element;
}): JSX.Element {
  const me = Symbol('panel');
  let closeButton: HTMLButtonElement | undefined;
  const detail = () => props.details.detail();
  const title = () => detail()?.title ?? props.details.opened()?.title ?? '';

  onMount(() => {
    open.push(me);
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== 'Escape' || event.defaultPrevented || open[open.length - 1] !== me) return;
      event.preventDefault();
      props.details.close();
    };
    document.addEventListener('keydown', onKey);
    onCleanup(() => {
      document.removeEventListener('keydown', onKey);
      open.splice(open.indexOf(me), 1);
    });
    closeButton?.focus();
  });

  return (
    <aside
      class="detail"
      aria-label={`Details of ${props.what} ${detail()?.key ?? title()}`}
      style={{ width: `${sizeOf(props.what)}px` }}
    >
      <Grip
        name={props.what}
        edge="left"
        label={`Drag to make the panel of the ${props.what} wider or narrower`}
        room={() => window.innerWidth - LEAST_BESIDE}
      />
      <div class="detail-head">
        <Show
          when={detail()}
          fallback={<span class="grow fine">{props.details.loading() ? 'Asking the tracker' : ''}</span>}
        >
          {(known) => (
            <>
              <a class="issue-key" href={known().url} target="_blank" rel="noreferrer">
                {known().key}
              </a>
              <span class="tag">{known().state}</span>
              <Show when={known().draft}>
                <span class="tag">draft</span>
              </Show>
              <span class="grow" />
              <a class="btn tiny" href={known().url} target="_blank" rel="noreferrer" title="Open it in the browser">
                Open in {trackerName[known().source]}
              </a>
            </>
          )}
        </Show>
        <button
          type="button"
          class="btn tiny ghost"
          disabled={props.details.loading()}
          onClick={() => props.details.again()}
        >
          {props.details.loading() ? 'Reading' : 'Read again'}
        </button>
        <button
          type="button"
          class="btn tiny ghost"
          ref={closeButton}
          aria-label={`Close the ${props.what}`}
          onClick={() => props.details.close()}
        >
          Close
        </button>
      </div>
      <h2 class="detail-title">{title()}</h2>
      <Show when={props.details.problem()}>
        <p class="banner danger detail-flat" role="alert">
          <span class="grow">{props.details.problem()}</span>
        </p>
      </Show>
      {props.children}
      <Show when={props.details.opened()?.foot}>{(foot) => <div class="detail-foot">{foot()()}</div>}</Show>
    </aside>
  );
}

/** What it says and who is on it: the people, the labels, the description and what is attached. */
function Overview(props: { detail: Detail; projectId: string }): JSX.Element {
  return (
    <>
      <p class="fine detail-people">
        <For each={peopleLines(props.detail)}>{(line) => <span>{line}</span>}</For>
        <Show when={props.detail.updatedAt > 0}>
          <span>Changed {ago(props.detail.updatedAt)}</span>
        </Show>
      </p>
      <Show when={props.detail.branch}>
        <p class="fine mono detail-branch">
          {props.detail.branch} → {props.detail.base}
        </p>
      </Show>
      <Show when={props.detail.labels.length > 0}>
        <p class="detail-labels">
          <For each={props.detail.labels}>{(label) => <span class="tag">{label}</span>}</For>
        </p>
      </Show>
      <Show when={props.detail.body.trim() !== ''} fallback={<p class="soft">It has no description.</p>}>
        <section class="detail-text" aria-label="Description">
          <Markdown text={props.detail.body} pictures={pictureFrom(props.projectId)} />
        </section>
      </Show>
      <Show when={props.detail.links.length > 0}>
        <ul class="detail-links" aria-label="Attached">
          <For each={props.detail.links}>
            {(link) => (
              <li>
                <a href={link.url} target="_blank" rel="noreferrer">
                  {link.title || link.url}
                </a>
              </li>
            )}
          </For>
        </ul>
      </Show>
    </>
  );
}

/** What was written under it, oldest first. */
function Notes(props: { detail: Detail; projectId: string }): JSX.Element {
  return (
    <section class="detail-notes" aria-label="Comments">
      <h3 class="detail-sub">{notesWords(props.detail.notes)}</h3>
      <For each={props.detail.notes}>
        {(note) => (
          <article class="detail-note" classList={{ review: note.kind === 'review' }}>
            <p class="detail-note-head">
              <span class="who">{note.author || 'Somebody'}</span>
              <Show when={noteAbout(note)}>
                <span class="tag" classList={{ mono: note.kind === 'comment' }}>
                  {noteAbout(note)}
                </span>
              </Show>
              <span class="fine">{ago(note.at)}</span>
            </p>
            <Show when={note.body.trim() !== ''}>
              <Markdown text={note.body} pictures={pictureFrom(props.projectId)} />
            </Show>
          </article>
        )}
      </For>
    </section>
  );
}

/** An issue in full, in a panel at the side of the screen. The list it was opened from stays where it is. */
export function IssuePanel(props: { details: Details }): JSX.Element {
  let body: HTMLDivElement | undefined;
  // Another issue in the same panel starts at its top.
  createEffect(
    on(
      () => props.details.opened()?.id,
      () => body?.scrollTo({ top: 0 }),
    ),
  );
  return (
    <Show when={props.details.opened()}>
      {(what) => (
        <Frame details={props.details} what="issue">
          <div class="detail-body" ref={body}>
            <Show when={props.details.detail()}>
              {(detail) => (
                <>
                  <Overview detail={detail()} projectId={what().projectId} />
                  <Notes detail={detail()} projectId={what().projectId} />
                </>
              )}
            </Show>
          </div>
        </Frame>
      )}
    </Show>
  );
}

type Tab = 'overview' | 'comments' | 'changes' | 'checks';

/** One file a request changes. Pressed, it shows what changed in it. */
function ChangedFile(props: { taskId: string; file: SpanFile }): JSX.Element {
  const [diff, setDiff] = createSignal<FileDiff | null>(null);
  const [problem, setProblem] = createSignal<string | null>(null);
  const read = (event: Event) => {
    if (!(event.currentTarget as HTMLDetailsElement).open || diff()) return;
    gitDiffOver(props.taskId, props.file.path, 'branch')
      .then(setDiff)
      .catch((error) => setProblem(error instanceof Error ? error.message : String(error)));
  };
  return (
    <details class="detail-file" onToggle={read}>
      <summary>
        <span class="mono grow">{props.file.path}</span>
        <Show when={props.file.kind !== 'modified'}>
          <span class="tag">{props.file.kind === 'added' ? 'new' : 'removed'}</span>
        </Show>
        <span class="fine mono">
          +{props.file.added} −{props.file.removed}
        </span>
      </summary>
      <Show when={problem()}>
        <p class="soft">{problem()}</p>
      </Show>
      <Show
        when={diff()}
        fallback={
          <Show when={!problem()}>
            <p class="fine">Reading the change</p>
          </Show>
        }
      >
        {(known) => (
          <Show
            when={!known().binary}
            fallback={<p class="soft">It is not text, so its change is not shown line by line.</p>}
          >
            <div class="diff" role="group" aria-label={`Change to ${props.file.path}`}>
              <For each={known().hunks}>
                {(hunk) => (
                  <>
                    <div class="diff-line at">
                      <span class="sign" />
                      <span>
                        Lines {hunk.fromLine}–{hunk.toLine}
                      </span>
                    </div>
                    <For each={hunk.lines}>
                      {(line) => (
                        <div class="diff-line" classList={{ plus: line.kind === 'add', minus: line.kind === 'remove' }}>
                          <span class="sign">{line.kind === 'add' ? '+' : line.kind === 'remove' ? '−' : ''}</span>
                          <span>{line.text}</span>
                        </div>
                      )}
                    </For>
                  </>
                )}
              </For>
            </div>
          </Show>
        )}
      </Show>
    </details>
  );
}

/** What the request changes, file by file: everything on the task's branch since it started. */
function Changes(props: { taskId: string }): JSX.Element {
  const [files, setFiles] = createSignal<SpanFile[] | null>(null);
  const [problem, setProblem] = createSignal<string | null>(null);
  onMount(() => {
    gitFiles(props.taskId, 'branch')
      .then((found) => setFiles(found.files))
      .catch((error) => setProblem(error instanceof Error ? error.message : String(error)));
  });
  return (
    <section class="detail-changes" aria-label="Changes">
      <Show when={problem()}>
        <p class="soft">{problem()}</p>
      </Show>
      <Show
        when={files()}
        fallback={
          <Show when={!problem()}>
            <p class="fine">Reading what the branch changes</p>
          </Show>
        }
      >
        {(known) => (
          <>
            <h3 class="detail-sub">
              {known().length === 0
                ? 'The branch changes nothing yet'
                : `${known().length} ${known().length === 1 ? 'file' : 'files'} changed on this computer's copy of the branch`}
            </h3>
            <For each={known()}>{(file) => <ChangedFile taskId={props.taskId} file={file} />}</For>
          </>
        )}
      </Show>
    </section>
  );
}

/** What stands between the request and its merge, and the pipeline of its branch. */
function Checks(props: { taskId: string }): JSX.Element {
  // eslint-disable-next-line solid/reactivity -- made anew for each request, so the task never changes under it
  const [info, setInfo] = createSignal<RequestInfo | null>(recall<RequestInfo>(`task:${props.taskId}:request`));
  const [problem, setProblem] = createSignal<string | null>(null);
  onMount(() => {
    core
      .request<RequestInfo>('mr.get', { taskId: props.taskId })
      .then(setInfo)
      .catch((error) => setProblem(error instanceof Error ? error.message : String(error)));
  });
  const gates = (): Gate[] => {
    const request = info()?.request;
    return request ? gateOf(request, info()?.host?.name ?? 'The host') : [];
  };
  const task = () => state.tasks[props.taskId];
  return (
    <section class="detail-checks" aria-label="Checks">
      <Show when={problem()}>
        <p class="soft">{problem()}</p>
      </Show>
      <h3 class="detail-sub">Before it can merge</h3>
      <For each={gates()}>
        {(gate) => (
          <div class="gate">
            <span class={`gate-mark ${gate.state}`} aria-hidden="true">
              {gate.state === 'good' ? '✓' : gate.state === 'bad' ? '!' : '…'}
            </span>
            <span class="gate-title">{gate.title}</span>
            <span class="fine">{gate.note}</span>
          </div>
        )}
      </For>
      <Show when={task()}>{(known) => <PipelineCard task={known()} pushes="" />}</Show>
    </section>
  );
}

/**
 * What was written under a task's request, for the Changes view: read when it is first looked at,
 * and again when the request changes while it is looked at. What the panel of the request read
 * last is there at once. Nothing is drawn while nobody has written anything.
 */
export function RequestTalk(props: { taskId: string; projectId: string; shown: boolean }): JSX.Element {
  // eslint-disable-next-line solid/reactivity -- made anew for each task, so the id never changes under it
  const taskId = props.taskId;
  const [detail, setDetail] = createKept<Detail | null>(`detail:request:${taskId}`, null);
  let asked = false;
  const read = () =>
    core
      .request<Detail>('mr.detail', { taskId })
      .then(setDetail)
      .catch(() => {
        // The request's card says how it stands; only what was written under it is missing.
      });
  createEffect(() => {
    if (!props.shown || asked) return;
    asked = true;
    void read();
  });
  onMount(() => {
    // eslint-disable-next-line solid/reactivity -- called later, as an event handler: it reads what is current then
    const stop = core.onEvent((event) => {
      if (event.event === 'mr.changed' && event.data.taskId === taskId && props.shown) void read();
    });
    onCleanup(stop);
  });
  return (
    <Show when={(detail()?.notes.length ?? 0) > 0}>
      <div class="git-block request-talk">
        <Notes detail={detail()!} projectId={props.projectId} />
      </div>
    </Show>
  );
}

/** What reviewers wrote about lines of the change and nobody has settled yet. */
function LineComments(props: { taskId: string }): JSX.Element {
  const [comments, setComments] = createSignal<ReviewComment[]>([]);
  onMount(() => {
    requestComments(props.taskId)
      .then((found) => setComments(found.comments))
      .catch(() => {
        // The comments on the request as a whole are still there to read.
      });
  });
  return (
    <Show when={comments().length > 0}>
      <section class="detail-notes" aria-label="Comments on lines">
        <h3 class="detail-sub">{comments().length} on lines of the change, not settled yet</h3>
        <For each={comments()}>
          {(comment) => (
            <article class="detail-note">
              <p class="detail-note-head">
                <span class="who">{comment.author || 'Somebody'}</span>
                <Show when={comment.path}>
                  <span class="tag mono">
                    {comment.path}
                    {comment.line ? `:${comment.line}` : ''}
                  </span>
                </Show>
              </p>
              <Markdown text={comment.body} />
            </article>
          )}
        </For>
      </section>
    </Show>
  );
}

/**
 * A pull or merge request in full, in a panel of its own at the side of the screen: what it
 * says, what was written under it, what it changes and what stands before its merge, a tab each.
 */
export function RequestPanel(props: { details: Details }): JSX.Element {
  const [tab, setTab] = createSignal<Tab>('overview');
  // Another request in the same panel opens on what it says.
  createEffect(
    on(
      () => props.details.opened()?.id,
      () => setTab('overview'),
      { defer: true },
    ),
  );
  const tabs = (): [Tab, string][] => {
    const notes = props.details.detail()?.notes.length ?? 0;
    const more: [Tab, string][] = props.details.opened()?.taskId
      ? [
          ['changes', 'Changes'],
          ['checks', 'Checks'],
        ]
      : [];
    return [['overview', 'Overview'], ['comments', notes > 0 ? `Comments ${notes}` : 'Comments'], ...more];
  };
  return (
    <Show when={props.details.opened()}>
      {(what) => (
        <Frame details={props.details} what="request">
          <div class="switcher detail-tabs" role="tablist" aria-label="Parts of the request">
            <For each={tabs()}>
              {([id, name]) => (
                <button type="button" role="tab" aria-selected={tab() === id} onClick={() => setTab(id)}>
                  {name}
                </button>
              )}
            </For>
          </div>
          <div class="detail-body" role="tabpanel">
            <Switch>
              <Match when={tab() === 'overview'}>
                <Show when={props.details.detail()}>
                  {(detail) => <Overview detail={detail()} projectId={what().projectId} />}
                </Show>
              </Match>
              <Match when={tab() === 'comments'}>
                <Show when={props.details.detail()}>
                  {(detail) => <Notes detail={detail()} projectId={what().projectId} />}
                </Show>
                <Show when={what().taskId} keyed>
                  {(taskId) => <LineComments taskId={taskId} />}
                </Show>
              </Match>
              <Match when={tab() === 'changes' && what().taskId} keyed>
                {(taskId) => <Changes taskId={taskId as string} />}
              </Match>
              <Match when={tab() === 'checks' && what().taskId} keyed>
                {(taskId) => <Checks taskId={taskId as string} />}
              </Match>
            </Switch>
          </div>
        </Frame>
      )}
    </Show>
  );
}

/**
 * Where the side panels stand: at the right edge of the screen. `beside` gives them room of their
 * own next to a list, so every row of the list can still be pressed; without it they lie over
 * what is at that edge.
 */
export function DetailDock(props: { beside?: boolean; children: JSX.Element }): JSX.Element {
  return (
    <div class="detail-dock" classList={{ beside: props.beside }}>
      {props.children}
    </div>
  );
}
