import { createEffect, createSignal, For, Match, on, Show, Switch, type JSX } from 'solid-js';
import { go, route, taskHref } from '../app/router';
import { agentOf, answerPermission, cancelTurn, sendPrompt, type Diff, type FeedItem, type Task } from '../core/store';
import { openInEditor, saveAll, setWork, workOf, type Quote } from '../core/work';
import { ago, Check, Chip, Ring, RingDot } from './bits';

const baseName = (path: string) => path.split('/').pop() ?? path;
const linesOf = (quote: Quote) =>
  quote.fromLine === quote.toLine ? `line ${quote.fromLine}` : `lines ${quote.fromLine}–${quote.toLine}`;

function DiffView(props: { diff: Diff }): JSX.Element {
  const lines = (text: string) => text.replace(/\n$/, '').split('\n');
  return (
    <div class="diff" aria-label={`Change to ${props.diff.path}`}>
      <Show when={props.diff.oldText}>
        <For each={lines(props.diff.oldText)}>
          {(line) => (
            <div class="diff-line minus">
              <span class="sign">−</span>
              <span>{line}</span>
            </div>
          )}
        </For>
      </Show>
      <For each={lines(props.diff.newText)}>
        {(line) => (
          <div class="diff-line plus">
            <span class="sign">+</span>
            <span>{line}</span>
          </div>
        )}
      </For>
    </div>
  );
}

/** "Edit src/a.ts" plus its state becomes "wants to edit", "is editing", "edited". */
function toolVerb(kind: string, status: string): string {
  const done = status === 'completed';
  const waiting = status === 'pending';
  switch (kind) {
    case 'read':
      return done ? 'read' : 'is reading';
    case 'edit':
      return done ? 'edited' : waiting ? 'wants to edit' : 'is editing';
    case 'delete':
      return done ? 'deleted' : 'wants to delete';
    case 'execute':
      return done ? 'ran' : waiting ? 'wants to run' : 'is running';
    case 'search':
      return done ? 'searched' : 'is searching';
    case 'fetch':
      return done ? 'fetched' : 'is fetching';
    default:
      return done ? 'did' : 'is doing';
  }
}

function gitVerb(action: string): string {
  switch (action) {
    case 'committed':
      return 'committed';
    case 'pushed':
      return 'pushed';
    case 'undid':
      return 'undid the commit';
    case 'updated':
      return 'updated from';
    default:
      return action;
  }
}

export function AgentPane(props: { task: Task }): JSX.Element {
  const agent = () => agentOf(props.task);
  const name = () => agent()?.name ?? props.task.info.agentId;
  const short = () => agent()?.short ?? '··';
  const agentId = () => props.task.info.agentId;
  const sessionState = () => props.task.info.state;
  const canSend = () => sessionState() === 'idle' || sessionState() === 'asleep';
  const taskId = () => props.task.info.id;
  const work = () => workOf(taskId());

  /** Shows a file of this task in the editor. Paths outside the task's folder are not links. */
  const showFile = (path: string) => {
    const current = route();
    if (current.page !== 'task' || current.view !== 'code') go(taskHref(taskId(), 'code'));
    setTimeout(() => openInEditor(taskId(), path), 0);
  };
  const inFolder = (path: string | null): path is string => Boolean(path) && !/^([a-zA-Z]:)?[\\/]/.test(path!);

  const [draft, setDraft] = createSignal('');
  const [problem, setProblem] = createSignal<string | null>(null);
  const [starting, setStarting] = createSignal(false);
  let feedEl: HTMLDivElement | undefined;

  // Keep the newest line in view, unless the user has scrolled up to read.
  let pinnedToEnd = true;
  const onScroll = () => {
    if (!feedEl) return;
    pinnedToEnd = feedEl.scrollHeight - feedEl.scrollTop - feedEl.clientHeight < 40;
  };
  createEffect(
    on(
      () => [props.task.lastSeq, props.task.info.id, Boolean(workOf(props.task.info.id).editing)],
      () => {
        if (feedEl && pinnedToEnd) feedEl.scrollTop = feedEl.scrollHeight;
      },
    ),
  );

  const act = async (work: () => Promise<unknown>) => {
    setProblem(null);
    try {
      await work();
    } catch (error) {
      setProblem(error instanceof Error ? error.message : String(error));
    }
  };

  const send = (instead?: string) => {
    const typed = (instead ?? draft()).trim();
    if (!typed || !canSend() || starting()) return;
    const quote = work().quote;
    // Code picked in the editor goes first, so the agent knows what the message is about.
    const text = quote
      ? `About \`${quote.path}\`, ${linesOf(quote)}:\n\n\`\`\`\n${quote.text.replace(/\n$/, '')}\n\`\`\`\n\n${typed}`
      : typed;
    if (instead === undefined) setDraft('');
    setWork(taskId(), { quote: null, pausedByTyping: false });
    pinnedToEnd = true;
    // Waking a sleeping task starts an agent first, which can take a while.
    setStarting(sessionState() === 'asleep');
    const id = taskId();
    void act(async () => {
      // What was typed in the code is saved first, so the agent works on what the user sees.
      await saveAll(id);
      await sendPrompt(id, text);
    }).finally(() => setStarting(false));
  };

  const onKeyDown = (event: KeyboardEvent) => {
    if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) {
      event.preventDefault();
      send();
    }
  };

  const item = (entry: FeedItem): JSX.Element => (
    <Switch>
      <Match when={entry.kind === 'user' && entry}>
        {(user) => (
          <>
            <p class="bubble">{user().text}</p>
            <Show when={user().edited.length > 0}>
              <p class="fine sent-with">
                Sent with a note about your edits to {user().edited.map(baseName).join(', ')}.
              </p>
            </Show>
          </>
        )}
      </Match>
      <Match when={entry.kind === 'edit' && entry}>
        {(edit) => (
          <div class="activity">
            <Chip agentId="you" short="You" flat />
            <span class="verb">edited</span>
            <button type="button" class="what link" onClick={() => showFile(edit().path)}>
              {edit().path}
            </button>
          </div>
        )}
      </Match>
      <Match when={entry.kind === 'agent' && entry}>
        {(said) => (
          <article class="said">
            <div class="said-head">
              <Chip agentId={agentId()} short={short()} />
              <span class="who">{name()}</span>
              <span class="muted">{ago(said().at)}</span>
            </div>
            <p>{said().text}</p>
          </article>
        )}
      </Match>
      <Match when={entry.kind === 'git' && entry}>
        {(git) => (
          <div class="activity">
            <Chip agentId="you" short="You" flat />
            <span class="verb">{gitVerb(git().action)}</span>
            <span class="what plain">{git().subject}</span>
            <Show when={git().short}>
              <span class="mono muted">{git().short}</span>
            </Show>
          </div>
        )}
      </Match>
      <Match when={entry.kind === 'thought' && entry}>{(thought) => <p class="thought">{thought().text}</p>}</Match>
      <Match when={entry.kind === 'tool' && entry}>
        {(tool) => (
          <div class="activity">
            <Chip agentId={agentId()} short={short()} flat />
            <span class="verb">{toolVerb(tool().toolKind, tool().status)}</span>
            <Show when={inFolder(tool().path)} fallback={<span class="what">{tool().path ?? tool().title}</span>}>
              <button type="button" class="what link" onClick={() => showFile(tool().path!)}>
                {tool().path}
              </button>
            </Show>
            <Show when={tool().status === 'failed'}>
              <span class="status failed">not done</span>
            </Show>
          </div>
        )}
      </Match>
      <Match when={entry.kind === 'plan' && entry}>
        {(plan) => (
          <section class="plan">
            <h3 class="plan-title">Plan for this turn</h3>
            <For each={plan().entries}>
              {(step) => (
                <div class="plan-entry" classList={{ done: step.status === 'completed' }}>
                  <Switch fallback={<Ring />}>
                    <Match when={step.status === 'completed'}>
                      <Check />
                    </Match>
                    <Match when={step.status === 'in_progress'}>
                      <RingDot />
                    </Match>
                  </Switch>
                  <span>{step.content}</span>
                </div>
              )}
            </For>
          </section>
        )}
      </Match>
      <Match when={entry.kind === 'permission' && entry}>
        {(ask) => (
          <Show
            when={!ask().answered}
            fallback={
              <p class="notice">
                {ask().chosen === null
                  ? `Cancelled: ${ask().title}.`
                  : `You answered “${ask().options.find((o) => o.optionId === ask().chosen)?.name ?? ask().chosen}” to: ${ask().title}.`}
              </p>
            }
          >
            <section class="card needs">
              <h3>
                {name()} asks: {ask().title}
              </h3>
              <Show when={ask().diff}>{(diff) => <DiffView diff={diff()} />}</Show>
              <div class="actions">
                <For each={ask().options}>
                  {(option, index) => (
                    <button
                      type="button"
                      class="btn tall"
                      classList={{ primary: index() === 0 && option.kind.startsWith('allow') }}
                      onClick={() => act(() => answerPermission(props.task.info.id, ask().requestId, option.optionId))}
                    >
                      {option.name}
                    </button>
                  )}
                </For>
              </div>
              <span class="fine">Nothing is changed until you answer.</span>
            </section>
          </Show>
        )}
      </Match>
      <Match when={entry.kind === 'notice' && entry}>
        {(notice) => (
          <p class="notice" classList={{ danger: notice().tone === 'danger' }}>
            {notice().text}
          </p>
        )}
      </Match>
    </Switch>
  );

  return (
    <aside class="agent-pane" aria-label={`Conversation with ${name()}`}>
      <div class="agent-head">
        <span class="agent-name">
          <Chip agentId={agentId()} short={short()} />
          {name()}
        </span>
        <span class="grow" />
        <span class="meta">
          <Switch>
            <Match when={sessionState() === 'running'}>Working</Match>
            <Match when={sessionState() === 'waiting_permission'}>Waiting for you</Match>
            <Match when={sessionState() === 'idle'}>Paused</Match>
            <Match when={starting()}>Starting</Match>
            <Match when={sessionState() === 'asleep'}>Asleep</Match>
          </Switch>
        </span>
      </div>

      <Switch>
        <Match when={sessionState() === 'waiting_permission'}>
          <section class="card pinned">
            <h2>{name()} is waiting for your answer</h2>
            <p>It asked before doing something. The question is at the end of the conversation.</p>
          </section>
        </Match>
        <Match when={sessionState() === 'idle' && work().pausedByTyping}>
          <section class="card pinned">
            <h2>Your turn</h2>
            <p>{name()} paused when you started typing. It will see your edits when it continues.</p>
            <div class="actions">
              <button type="button" class="btn tall primary" onClick={() => send('Continue.')}>
                Let {name()} continue
              </button>
            </div>
          </section>
        </Match>
        <Match when={sessionState() === 'idle' && props.task.feed.length > 0}>
          <section class="card pinned">
            <h2>Your turn</h2>
            <p>{name()} finished this turn. Say what to do next.</p>
          </section>
        </Match>
        <Match when={starting()}>
          <section class="card pinned">
            <h2>Starting {name()}</h2>
            <p>It is told what was said so far, then gets your message.</p>
          </section>
        </Match>
        <Match when={sessionState() === 'asleep'}>
          <section class="card pinned">
            <h2>No agent is running</h2>
            <p>Everything said so far is kept. Your next message starts {name()} again and tells it what was said.</p>
          </section>
        </Match>
      </Switch>

      <div class="feed" ref={feedEl} onScroll={onScroll}>
        <For each={props.task.feed}>{item}</For>
        <Show when={work().editing}>
          {(editing) => (
            <div class="activity live">
              <Chip agentId="you" short="You" flat />
              <span class="verb">are editing</span>
              <span class="what">{editing().path}</span>
              <span class="muted">line {editing().line}</span>
            </div>
          )}
        </Show>
      </div>

      <section class="composer">
        <label for={`msg-${props.task.info.id}`}>Message {name()}</label>
        <Show when={work().quote}>
          {(quote) => (
            <div class="quote">
              <span class="mono grow">
                {baseName(quote().path)} {linesOf(quote())}
              </span>
              <button type="button" class="btn tiny" onClick={() => setWork(taskId(), { quote: null })}>
                Leave it out
              </button>
            </div>
          )}
        </Show>
        <textarea
          id={`msg-${props.task.info.id}`}
          rows="2"
          placeholder={
            work().quote
              ? `Ask ${name()} about the code you picked`
              : work().editing
                ? `Message ${name()} about ${baseName(work().editing!.path)} line ${work().editing!.line}`
                : 'Ask a question or say what to do next'
          }
          value={draft()}
          onInput={(event) => setDraft(event.currentTarget.value)}
          onKeyDown={onKeyDown}
        />
        <Show when={problem()}>
          <p class="notice danger" role="alert">
            {problem()}
          </p>
        </Show>
        <Show when={work().unsaved > 0 && canSend()}>
          <span class="fine">Your unsaved edits are saved when you send, so {name()} sees them.</span>
        </Show>
        <div class="composer-row">
          <Show when={sessionState() === 'running' || sessionState() === 'waiting_permission'}>
            <button type="button" class="btn ghost" onClick={() => act(() => cancelTurn(props.task.info.id))}>
              Stop this turn
            </button>
          </Show>
          <span class="grow" />
          <button
            type="button"
            class="btn strong"
            onClick={() => send()}
            disabled={!canSend() || starting() || draft().trim() === ''}
          >
            Send
          </button>
        </div>
      </section>
    </aside>
  );
}
