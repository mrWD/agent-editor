import { createEffect, createSignal, For, Match, on, onCleanup, Show, Switch, type JSX } from 'solid-js';
import { go, route, taskHref } from '../app/router';
import {
  agentOf,
  answerPermission,
  cancelTurn,
  sendPrompt,
  restoreFile,
  rewindTo,
  setTaskOption,
  spentOn,
  state,
  switchAgent,
  type ConfigOption,
  type Diff,
  type FeedItem,
  type Task,
} from '../core/store';
import { openInEditor, saveAll, setWork, workOf, type Quote } from '../core/work';
import { teamOf, watchTeam } from '../core/team';
import { hideSince, sinceOf } from '../core/visits';
import { Timeline } from '../screens/LookBack';
import { ago, Check, Chip, clock, money, Ring, RingDot } from './bits';
import { Markdown } from './Markdown';
import { RuleOffer } from './RuleOffer';
import { TeamPanel } from './TeamPanel';
import { WhereStopped } from './WhereStopped';

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

const isMode = (option: ConfigOption) => option.category === 'mode' || option.id === 'mode';
const isModel = (option: ConfigOption) => option.category === 'model' || option.id === 'model';

/** One setting of the agent as a drop-down list. */
function OptionSelect(props: {
  option: ConfigOption;
  label: string;
  busy: boolean;
  onPick: (value: string) => void;
}): JSX.Element {
  const chosen = () => props.option.options.find((one) => one.value === props.option.currentValue);
  return (
    <select
      class="pick"
      aria-label={props.label}
      title={chosen()?.description ?? props.option.description ?? props.label}
      disabled={props.busy}
      value={props.option.currentValue}
      onChange={(event) => props.onPick(event.currentTarget.value)}
    >
      <For each={props.option.options}>
        {(one) => (
          <option
            value={one.value}
            title={one.description ?? undefined}
            selected={one.value === props.option.currentValue}
          >
            {one.name}
          </option>
        )}
      </For>
    </select>
  );
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
  const showFile = (path: string, line?: number) => {
    const current = route();
    if (current.page !== 'task' || current.view !== 'code') go(taskHref(taskId(), 'code'));
    setTimeout(() => openInEditor(taskId(), path, line), 0);
  };

  // What the running agent offers to set: its mode by the Send button, its
  // model in the head, everything else behind "More".
  const options = () => props.task.info.config?.options ?? [];
  const modeOption = () => options().find(isMode);
  const modelOption = () => options().find((option) => isModel(option) && !isMode(option));
  const otherOptions = () => options().filter((option) => !isMode(option) && !isModel(option));
  const [moreOpen, setMoreOpen] = createSignal(false);
  const [setting, setSetting] = createSignal(false);
  const pick = (option: ConfigOption, value: string) => {
    if (value === option.currentValue) return;
    setSetting(true);
    const id = taskId();
    void act(() => setTaskOption(id, option.id, value)).finally(() => setSetting(false));
  };

  const lookingBack = () => work().lookBack !== null;
  const team = () => teamOf(taskId());
  const [teamOpen, setTeamOpen] = createSignal(false);
  // The team of the task on screen is kept current for as long as the pane shows it.
  createEffect(
    on(taskId, (id) => {
      setTeamOpen(false);
      onCleanup(watchTeam(id));
    }),
  );
  const since = () => sinceOf(taskId());

  // Any other agent that is installed can take the task over.
  const others = () => state.agents.filter((one) => one.available && one.id !== agentId());
  const hand = (to: string) => {
    if (to === agentId()) return;
    setSetting(true);
    const id = taskId();
    void act(() => switchAgent(id, to)).finally(() => setSetting(false));
  };

  const usage = () => props.task.usage;
  const contextShare = () => Math.min(100, Math.round((usage()!.used / usage()!.size) * 100));
  const usageTitle = () => {
    const now = usage()!;
    const spent = spentOn(props.task);
    const tokens = `${now.used.toLocaleString('en-US')} of ${now.size.toLocaleString('en-US')} tokens of context are in use.`;
    return spent > 0 ? `${tokens} This task has cost ${money(spent, now.currency)} so far.` : tokens;
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
    hideSince(taskId());
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
      <Match when={entry.kind === 'file' && entry}>
        {(did) => (
          <div class="activity">
            <Chip agentId="you" short="You" flat />
            <span class="verb">{did().action === 'restored' ? 'put back' : did().action}</span>
            <Show
              when={did().action !== 'deleted' && did().action !== 'renamed'}
              fallback={<span class="what plain">{did().path}</span>}
            >
              <button type="button" class="what link" onClick={() => showFile(did().path)}>
                {did().path}
              </button>
            </Show>
            <Show when={did().to}>
              <span class="verb">to</span>
              <button type="button" class="what link" onClick={() => showFile(did().to!)}>
                {did().to}
              </button>
            </Show>
            <Show when={did().keptAs}>
              <button
                type="button"
                class="btn tiny"
                title="The editor kept it. This puts it back where it was."
                onClick={() => act(() => restoreFile(taskId(), did().keptAs!))}
              >
                Put it back
              </button>
            </Show>
          </div>
        )}
      </Match>
      <Match when={entry.kind === 'note' && entry}>
        {(note) => (
          <div class="team-note">
            <span class="fine">
              Note · {note().from}
              {note().to ? ` to ${note().to}` : ', for whoever works on this next'}
            </span>
            <Markdown text={note().text} onFile={showFile} />
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
            <Markdown text={said().text} onFile={showFile} />
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
      <Match when={entry.kind === 'rewound' && entry}>
        {(back) => (
          <div class="notice rewound">
            <span class="grow">
              You rewound the code to {clock(back().toAt)}
              {back().label && back().label !== 'Before the rewind' ? ` (${back().label})` : ''}:{' '}
              {back().files === 1 ? 'one file' : `${back().files} files`} went back
              {back().kept.length > 0 ? ', your own lines stayed' : ''}
              {back().droppedMessages ? ', and the later messages left the chat' : ''}.
            </span>
            <button
              type="button"
              class="btn tiny"
              disabled={!canSend()}
              onClick={() => act(() => rewindTo(taskId(), back().undo, { keepMine: false, dropMessages: false }))}
            >
              Undo this rewind
            </button>
          </div>
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
          <Show when={others().length > 0} fallback={name()}>
            <select
              class="pick agent-pick"
              aria-label="The agent of this task"
              title="Hand this task to another agent. It is told what was said so far."
              disabled={setting() || !canSend()}
              value={agentId()}
              onChange={(event) => hand(event.currentTarget.value)}
            >
              <option value={agentId()} selected>
                {name()}
              </option>
              <For each={others()}>{(other) => <option value={other.id}>{other.name}</option>}</For>
            </select>
          </Show>
        </span>
        <Show when={modelOption()}>
          {(option) => (
            <OptionSelect
              option={option()}
              label={`Model of ${name()}`}
              busy={setting()}
              onPick={(value) => pick(option(), value)}
            />
          )}
        </Show>
        <Show when={otherOptions().length > 0}>
          <button
            type="button"
            class="btn small ghost"
            aria-expanded={moreOpen()}
            aria-controls={`more-${taskId()}`}
            onClick={() => setMoreOpen(!moreOpen())}
          >
            More
          </button>
        </Show>
        <button
          type="button"
          class="btn small ghost"
          aria-expanded={teamOpen() || Boolean(team())}
          title="Several agents on this task, one after another: one plans, one builds, one reviews"
          onClick={() => setTeamOpen(!teamOpen())}
        >
          {team() ? (team()!.running ? 'Team at work' : 'Team') : 'Solo'}
        </button>
        <Show when={props.task.info.branch}>
          <button
            type="button"
            class="btn small ghost"
            aria-pressed={lookingBack()}
            title="See the code as it was at an earlier point, and rewind to it"
            onClick={() => setWork(taskId(), { lookBack: lookingBack() ? null : 0 })}
          >
            Look back
          </button>
        </Show>
        <span class="grow" />
        <Show when={usage()}>
          <span class="meta" title={usageTitle()}>
            Context {contextShare()}%
            <Show when={spentOn(props.task) > 0}> · {money(spentOn(props.task), usage()!.currency)}</Show>
          </span>
        </Show>
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
      <Show when={moreOpen() && otherOptions().length > 0}>
        <section class="agent-more" id={`more-${taskId()}`} aria-label={`More settings of ${name()}`}>
          <For each={otherOptions()}>
            {(option) => (
              <label class="agent-more-row">
                <span class="grow" title={option.description ?? undefined}>
                  {option.name}
                </span>
                <OptionSelect
                  option={option}
                  label={option.name}
                  busy={setting()}
                  onPick={(value) => pick(option, value)}
                />
              </label>
            )}
          </For>
          <span class="fine">These go with {name()} to your next tasks. The mode stays with this task.</span>
        </section>
      </Show>

      <Show
        when={!lookingBack()}
        fallback={
          <Show when={taskId()} keyed>
            <Timeline task={props.task} />
          </Show>
        }
      >
        <Show when={teamOpen() || team()}>
          <TeamPanel task={props.task} onClose={() => setTeamOpen(false)} />
        </Show>
        <Show when={since()}>
          {(told) => <WhereStopped task={props.task} since={told()} onHide={() => hideSince(taskId())} />}
        </Show>
        <RuleOffer task={props.task} />
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
            <Show when={modeOption()}>
              {(option) => (
                <OptionSelect
                  option={option()}
                  label={`What ${name()} may do without asking`}
                  busy={setting()}
                  onPick={(value) => pick(option(), value)}
                />
              )}
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
      </Show>
    </aside>
  );
}
