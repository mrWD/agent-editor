import { createDetails, DetailDock, RequestPanel } from './DetailPanel';
import type { Detail } from '../core/details';
import { createEffect, createSignal, For, on, onCleanup, onMount, Show, type JSX } from 'solid-js';
import { createKept, recall, remember } from '../core/kept';
import { createStore, reconcile } from 'solid-js/store';
import { go, taskHref } from '../app/router';
import { checksOf } from '../core/checks';
import { core } from '../core/client';
import { refreshRequests } from '../core/requests';
import {
  agentOf,
  createRequest,
  getRequest,
  joinRequest,
  mergeRequest,
  replyToComment,
  requestComments,
  sendPrompt,
  setAutoMerge,
  taskIssueRequests,
  type IssueRequest,
  type RequestInfo,
  type ReviewComment,
  type Task,
} from '../core/store';
import { openInEditor } from '../core/work';

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
/** Where a comment is, in words: "src/a.ts, line 5". */
const placeOf = (comment: ReviewComment) =>
  comment.path ? `${comment.path}${comment.line ? `, line ${comment.line}` : ''}` : 'the request as a whole';

/** What the agent is told when it is asked to deal with reviewers' comments. */
export function fixPrompt(comments: ReviewComment[], word: string): string {
  const one = (comment: ReviewComment) =>
    `${comment.author} wrote about ${comment.path ? `\`${comment.path}\`${comment.line ? ` line ${comment.line}` : ''}` : 'the request'}:\n\n> ${comment.body.split('\n').join('\n> ')}`;
  if (comments.length === 1) {
    return `A reviewer left a comment on the ${word}. ${one(comments[0]!)}\n\nMake the change it asks for, and say in a sentence what you changed.`;
  }
  return `Reviewers left ${comments.length} comments on the ${word}.\n\n${comments.map((comment, index) => `${index + 1}. ${one(comment)}`).join('\n\n')}\n\nMake the changes they ask for, and say in a sentence for each what you changed.`;
}

export function RequestCard(props: {
  task: Task;
  commits: number;
  /** How many lines of this task another open task changes too. */
  overlaps?: number;
  onChanged: () => void;
}): JSX.Element {
  // eslint-disable-next-line solid/reactivity -- this component is made anew for each task, so the id never changes under it
  const taskId = props.task.info.id;
  // What the request looked like last time is there at once; it is asked anew behind that.
  const [info, setInfo] = createKept<RequestInfo | null>(`task:${taskId}:request`, null);
  const [problem, setProblem] = createSignal<string | null>(null);
  const [form, setForm] = createSignal(false);
  const [title, setTitle] = createSignal('');
  const [body, setBody] = createSignal('');
  const [busy, setBusy] = createSignal<string | null>(null);
  const [squash, setSquash] = createSignal(true);
  const [remove, setRemove] = createSignal(true);
  const [archive, setArchive] = createSignal(true);
  // Kept by id: reading the comments again leaves the rows on screen as they are,
  // so a reply that is being typed, or a press that is on its way, is not lost.
  const [commentList, setCommentList] = createStore<{ list: ReviewComment[] }>({ list: [] });
  const comments = () => commentList.list;
  const setComments = (list: ReviewComment[]) => setCommentList('list', reconcile(list, { key: 'id' }));
  const [replyTo, setReplyTo] = createSignal<string | null>(null);
  const [reply, setReply] = createSignal('');
  /** The request, whole, at the side of the screen. */
  const details = createDetails();
  const agentName = () => agentOf(props.task)?.name ?? 'the agent';
  /** The checks run on this computer. A failed one stands in the way of the merge. */
  const local = () => checksOf(taskId);
  const redHere = () => (local()?.failed ?? 0) > 0;
  const canAsk = () => props.task.info.state === 'idle' || props.task.info.state === 'asleep';

  const loadComments = async () => {
    try {
      setComments((await requestComments(taskId)).comments);
    } catch {
      // The request itself still shows; only the comments are missing.
    }
  };

  /**
   * The open requests of the task's issue that are on another branch: work on the issue that was
   * under way before the task, which the task is not on. Looked for while the task has no request.
   */
  const [others, setOthers] = createKept<IssueRequest[]>(`task:${taskId}:request.others`, []);
  // eslint-disable-next-line solid/reactivity -- the card is made anew for each task, and a task's issue does not change
  const hasIssue = Boolean(props.task.info.issue);
  const loadOthers = async () => {
    if (!hasIssue) return;
    try {
      setOthers(await taskIssueRequests(taskId));
    } catch {
      // Not knowing of them takes nothing away: the task can still open a request of its own.
    }
  };

  const load = async (fresh = false) => {
    try {
      const got = await getRequest(taskId, fresh);
      setInfo(got);
      setProblem(null);
      if (got.request?.state === 'open') void loadComments();
      else setComments([]);
      if (got.request) setOthers([]);
      else if (got.host) void loadOthers();
    } catch (error) {
      setProblem(message(error));
    }
  };

  // Asked for from the list of issues ("Create MR"): the form opens once it is known that there is
  // something to ask a merge for. The commits of the branch are read by the view around this card
  // and may come a moment after the card itself, so the asking waits for them rather than give up.
  const createAsked = `task:${taskId}:request.create`;
  const [asked, setAsked] = createSignal(recall<boolean>(createAsked) === true);
  remember(createAsked, false);
  createEffect(() => {
    const known = info();
    if (!asked() || !known) return;
    if (!known.host || known.request) setAsked(false);
    else if (props.commits > 0 && !form() && !busy()) {
      setAsked(false);
      void openForm();
    }
  });
  onMount(() => {
    void load();
    // An asking that found nothing to merge is forgotten: a commit made later must not open the form by itself.
    const forget = setTimeout(() => setAsked(false), 5000);
    onCleanup(() => clearTimeout(forget));
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

  /** Takes the task over to the branch of a request its issue has already. The core says why when it cannot. */
  const join = (other: IssueRequest) =>
    // eslint-disable-next-line solid/reactivity -- called later, as an event handler: it reads what is current then
    act('join', async () => {
      await joinRequest(taskId, other.number);
      setOthers([]);
      await load(true);
      props.onChanged();
      void refreshRequests();
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

  const options = () => ({ squash: squash(), deleteBranch: remove(), archive: archive() });
  const mergeByItself = (on: boolean) =>
    // eslint-disable-next-line solid/reactivity -- called later, as an event handler: it reads what is current then
    act('auto', async () => {
      await setAutoMerge(taskId, on, options());
      await load();
    });

  const askToFix = (list: ReviewComment[]) => act('fix', () => sendPrompt(taskId, fixPrompt(list, word())));

  const sendReply = (comment: ReviewComment) =>
    // eslint-disable-next-line solid/reactivity -- called later, as an event handler: it reads what is current then
    act('reply', async () => {
      await replyToComment(taskId, comment.id, reply());
      setReply('');
      setReplyTo(null);
    });

  const openInCode = (comment: ReviewComment) => {
    go(taskHref(taskId, 'code'));
    setTimeout(() => openInEditor(taskId, comment.path!, comment.line ?? undefined), 0);
  };

  const word = () => info()?.host?.word ?? 'merge request';
  const Word = () => word().charAt(0).toUpperCase() + word().slice(1);

  return (
    <section class="git-mr-card" aria-label="Merge request">
      <DetailDock>
        <RequestPanel details={details} />
      </DetailDock>
      <div class="git-head">
        <h2 class="pane-title">{Word()}</h2>
        <Show when={info()?.request} fallback={<span class="grow fine">{info() ? 'none yet' : ''}</span>}>
          {(request) => (
            <>
              <span class="grow fine">
                Number {request().number} · {request().state}
                {request().draft ? ' · draft' : ''}
              </span>
              <button
                type="button"
                class="btn tiny"
                title="Read it here: the description, who is on it and what was written under it"
                onClick={() =>
                  details.open({
                    id: `request:${taskId}`,
                    title: `${Word()} ${request().number}`,
                    projectId: props.task.info.projectId,
                    taskId,
                    ask: () => core.request<Detail>('mr.detail', { taskId }),
                  })
                }
              >
                Details
              </button>
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

      {/* Work on the task's issue that is under way on another branch: it can be read here, and the task can go over to it. */}
      <Show when={info()?.host && !info()?.request && !form()}>
        <For each={others()}>
          {(other) => (
            <div class="request-other" role="group" aria-label={`${other.name} of the issue`}>
              <p>
                {props.task.info.issue?.key ?? 'The issue'} has{' '}
                <a href={other.url} target="_blank" rel="noreferrer">
                  {other.name}
                </a>{' '}
                open already: {other.title}
              </p>
              <p class="fine">
                {other.draft ? 'Draft · ' : ''}
                <span class="mono">{other.branch}</span> into <span class="mono">{other.base}</span>. This task is on a
                branch of its own, so that request is not its own yet.
              </p>
              <div class="actions">
                <button
                  type="button"
                  class="btn small primary"
                  disabled={Boolean(busy())}
                  title={`The task's folder goes to ${other.branch} as it stands, and ${other.name} becomes the task's request. Refused while something here is not committed or not on that branch.`}
                  onClick={() => void join(other)}
                >
                  {busy() === 'join' ? 'Going over' : `Go on with ${other.name}`}
                </button>
                <button
                  type="button"
                  class="btn small"
                  title="Read it here: the description, who is on it and what was written under it"
                  onClick={() =>
                    details.open({
                      id: `request:${props.task.info.projectId}:${other.number}`,
                      title: other.title,
                      projectId: props.task.info.projectId,
                      ask: () =>
                        core.request<Detail>('mr.detailOf', {
                          projectId: props.task.info.projectId,
                          number: other.number,
                        }),
                    })
                  }
                >
                  Read {other.name}
                </button>
              </div>
            </div>
          )}
        </For>
        <Show when={props.commits === 0 && others().length === 0}>
          <p class="fine">
            Nothing is committed on this branch yet, so there is nothing to ask to merge: a {word()} can be opened once
            there is a commit.
          </p>
        </Show>
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
            <Show when={comments().length > 0}>
              <div class="git-head">
                <h3 class="git-sub grow">Review comments</h3>
                <Show when={comments().length > 1}>
                  <button
                    type="button"
                    class="btn tiny"
                    disabled={Boolean(busy()) || !canAsk()}
                    onClick={() => void askToFix(comments())}
                  >
                    Let {agentName()} address {comments().length === 2 ? 'both' : `all ${comments().length}`}
                  </button>
                </Show>
              </div>
              <For each={comments()}>
                {(comment) => (
                  <div class="review-comment">
                    <span class="review-where">
                      <span class="mono">{placeOf(comment)}</span>
                      <span class="fine">{comment.author}</span>
                    </span>
                    <p>{comment.body}</p>
                    <div class="actions">
                      <Show when={comment.path}>
                        <button type="button" class="btn tiny ghost" onClick={() => openInCode(comment)}>
                          Open in code
                        </button>
                      </Show>
                      <button
                        type="button"
                        class="btn tiny"
                        disabled={Boolean(busy()) || !canAsk()}
                        title={canAsk() ? undefined : `${agentName()} is working. Ask when its turn ends.`}
                        onClick={() => void askToFix([comment])}
                      >
                        Ask {agentName()} to fix
                      </button>
                      <button
                        type="button"
                        class="btn tiny ghost"
                        aria-expanded={replyTo() === comment.id}
                        onClick={() => setReplyTo(replyTo() === comment.id ? null : comment.id)}
                      >
                        Reply
                      </button>
                    </div>
                    <Show when={replyTo() === comment.id}>
                      <textarea
                        class="git-message"
                        rows="2"
                        aria-label={`Reply to ${comment.author} about ${placeOf(comment)}`}
                        value={reply()}
                        onInput={(event) => setReply(event.currentTarget.value)}
                      />
                      <div class="actions">
                        <button
                          type="button"
                          class="btn tiny"
                          disabled={Boolean(busy()) || reply().trim() === ''}
                          onClick={() => void sendReply(comment)}
                        >
                          {busy() === 'reply' ? 'Sending' : `Send the reply on ${info()!.host!.name}`}
                        </button>
                      </div>
                    </Show>
                  </div>
                )}
              </For>
            </Show>

            <h3 class="git-sub">Before it can merge</h3>
            <Show when={(local()?.total ?? 0) > 0}>
              <Gate
                state={redHere() ? 'bad' : (local()!.running ?? 0) > 0 ? 'wait' : 'good'}
                title="Checks here"
                note={
                  redHere()
                    ? `${local()!.failed} failed on this computer. See the Checks view.`
                    : `${local()!.done} of ${local()!.total} lines of “Done when” are done`
                }
              />
            </Show>
            <Show when={(props.overlaps ?? 0) > 0}>
              <Gate
                state="wait"
                title="Other tasks"
                note={`${props.overlaps === 1 ? 'One place is' : `${props.overlaps} places are`} also changed in another task. Whichever merges second gets the conflict.`}
              />
            </Show>
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
            <Show
              when={info()!.auto}
              fallback={
                <Show when={!info()!.ready}>
                  <button
                    type="button"
                    class="btn tall"
                    disabled={Boolean(busy())}
                    onClick={() => void mergeByItself(true)}
                  >
                    Merge by itself when everything is green
                  </button>
                </Show>
              }
            >
              <p class="banner review flush">
                <span class="grow">It will merge by itself when everything above is green.</span>
                <button
                  type="button"
                  class="btn tiny"
                  disabled={Boolean(busy())}
                  onClick={() => void mergeByItself(false)}
                >
                  Do not
                </button>
              </p>
            </Show>
            <button
              type="button"
              class="btn primary tall"
              disabled={Boolean(busy()) || !info()!.ready || redHere()}
              onClick={merge}
            >
              {busy() === 'merge' ? 'Merging' : 'Merge now'}
            </button>
            <Show when={!info()!.ready || redHere()}>
              <span class="fine">Merge now unlocks when nothing above stands in the way.</span>
            </Show>
          </Show>
        )}
      </Show>
    </section>
  );
}
