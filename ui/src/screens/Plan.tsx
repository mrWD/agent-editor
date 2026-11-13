import { createEffect, createMemo, createSignal, For, Match, on, Show, Switch, type JSX } from 'solid-js';
import { createStore } from 'solid-js/store';
import { go, route, taskHref } from '../app/router';
import { ago, Check, Chip, Icon, RingDot } from '../components/bits';
import {
  acceptBlock,
  addBlock,
  buildBlock,
  connectBlocks,
  countsLine,
  declineBlock,
  describeBlock,
  disconnectBlocks,
  dismissPlanProblem,
  draftPlan,
  editBlock,
  filesOf,
  hasStateLine,
  layoutPlan,
  planOf,
  proofOf,
  removeBlock,
  setOnlyPlan,
  stateWord,
  takeBlock,
  type Block,
  type BlockKind,
  type BlockState,
  type Place,
} from '../core/plan';
import { agentOf, state as app, type Task } from '../core/store';
import { openInEditor, saveAll } from '../core/work';
import { Lines, Stage } from './Map';

const message = (error: unknown) => (error instanceof Error ? error.message : String(error));
const plural = (count: number, one: string, many = `${one}s`) => `${count} ${count === 1 ? one : many}`;
/** The two letters of an agent, by its id. */
const shortOf = (agentId: string | null) => app.agents.find((agent) => agent.id === agentId)?.short ?? 'Ag';

/** The block the user picked in each task, so a look at the code and back does not lose it. */
const [chosen, setChosen] = createStore<Record<string, string | undefined>>({});
/** The block a task's next message is not about after all: the user left it out. */
const [leftOut, setLeftOut] = createStore<Record<string, string | undefined>>({});

/** Shows a block in the Plan view: for the lines of the conversation that name one. */
export function showBlock(taskId: string, blockId: string) {
  setChosen(taskId, blockId);
  go(taskHref(taskId, 'plan'));
}

/** The block the next message is about: the one picked in the Plan view, while that view is on screen. */
function aboutBlock(taskId: string): Block | null {
  const now = route();
  if (now.page !== 'task' || now.id !== taskId || now.view !== 'plan') return null;
  const id = chosen[taskId];
  if (!id || leftOut[taskId] === id) return null;
  return planOf(taskId)?.blocks.find((block) => block.id === id && block.kind !== 'note') ?? null;
}

/** What goes in front of a message about a block of the plan, so the agent knows which one is meant. */
export function planAboutText(taskId: string): string | null {
  const block = aboutBlock(taskId);
  return block ? `About the block \`${block.id}\` of the plan, “${block.text}”:` : null;
}

/** Above the message box: what the message is about, with a way to leave it out. */
export function PlanAbout(props: { taskId: string }): JSX.Element {
  return (
    <Show when={aboutBlock(props.taskId)}>
      {(block) => (
        <div class="quote-chip plan-about">
          <span class="fine">About</span>
          <span class="plan-about-chip">plan</span>
          <span class="plan-about-chip">block: {block().text}</span>
          <button type="button" class="link" onClick={() => setLeftOut(props.taskId, block().id)}>
            Leave it out
          </button>
        </div>
      )}
    </Show>
  );
}

/** The small pictures of the toolbar's three kinds of block. */
const kindIcon: Record<BlockKind, () => JSX.Element> = {
  step: () => (
    <svg
      width="12"
      height="12"
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      stroke-width="1.5"
      aria-hidden="true"
    >
      <rect x="2" y="4" width="12" height="8" rx="2" />
    </svg>
  ),
  ask: () => (
    <svg
      width="12"
      height="12"
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      stroke-width="1.5"
      stroke-linejoin="round"
      aria-hidden="true"
    >
      <path d="M8 2l6 6-6 6-6-6 6-6z" />
    </svg>
  ),
  note: () => (
    <svg
      width="12"
      height="12"
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      stroke-width="1.5"
      stroke-linecap="round"
      aria-hidden="true"
    >
      <path d="M3 4h10M3 8h10M3 12h6" />
    </svg>
  ),
};

const kinds: { kind: BlockKind; word: string; what: string }[] = [
  { kind: 'step', word: 'Step', what: 'a step' },
  { kind: 'ask', word: 'Question', what: 'a question' },
  { kind: 'note', word: 'Note', what: 'a note' },
];

/** What the states look like, in the order the design lists them. */
const legend: { state: BlockState; word: string }[] = [
  { state: 'built', word: 'Built' },
  { state: 'building', word: 'Being built' },
  { state: 'planned', word: 'Planned' },
  { state: 'proposed', word: 'Proposed by an agent' },
  { state: 'before', word: 'Was in the code before' },
];

/** The mark of a state next to its word, in the head of the detail panel. */
function StateMark(props: { state: BlockState }): JSX.Element {
  return (
    <Switch fallback={<span class={`plan-key ${props.state}`} aria-hidden="true" />}>
      <Match when={props.state === 'built'}>
        <Check />
      </Match>
      <Match when={props.state === 'building'}>
        <RingDot />
      </Match>
    </Switch>
  );
}

/**
 * The Plan view: what the task is meant to become, as a flow chart. The user
 * draws it or lets the agent draft it; the agent builds it block by block,
 * when the user says so.
 */
export function PlanView(props: { task: Task }): JSX.Element {
  // eslint-disable-next-line solid/reactivity -- this component is made anew for each task, so the id never changes under it
  const taskId = props.task.info.id;
  const agentName = () => agentOf(props.task)?.name ?? 'the agent';
  const canSend = () => props.task.info.state === 'idle' || props.task.info.state === 'asleep';

  const plan = () => planOf(taskId);
  const blocks = () => plan()?.blocks ?? [];
  const arrows = () => plan()?.arrows ?? [];
  const picked = createMemo<Block | null>(() => blocks().find((block) => block.id === chosen[taskId]) ?? null);
  const layout = createMemo(() => layoutPlan({ blocks: blocks(), arrows: arrows() }));
  const boxOf = (id: string) => layout().boxes.find((box) => box.id === id);
  const stateOf = (id: string) => blocks().find((block) => block.id === id)?.state;
  const textOf = (id: string) => blocks().find((block) => block.id === id)?.text ?? '';
  const waiting = () => blocks().filter((block) => block.state === 'proposed').length;

  const [problem, setProblem] = createSignal<string | null>(null);
  const [pending, setPending] = createSignal(0);
  /** Grows with every "Tidy up": the chart is then made anew, back in its place. */
  const [tidied, setTidied] = createSignal(1);

  const act = async (work: () => Promise<unknown>) => {
    setPending((count) => count + 1);
    setProblem(null);
    try {
      await work();
    } catch (error) {
      setProblem(message(error));
    } finally {
      setPending((count) => count - 1);
    }
  };
  /** For what sends the agent a message: what was typed in the code is saved first, so the agent works on it. */
  const send = (work: () => Promise<unknown>) =>
    act(async () => {
      await saveAll(taskId);
      await work();
    });

  // ----- what stands in the way of a message to the agent -----

  const busy = () => plan()?.busy ?? null;
  const whyNot = () => {
    const now = busy();
    if (now?.kind === 'build') return `${agentName()} is building “${textOf(now.blockId)}”`;
    if (now) return `${agentName()} is drawing the plan`;
    return canSend() ? undefined : `Wait until ${agentName()} has finished its turn`;
  };
  const cannotSend = () => pending() > 0 || whyNot() !== undefined;

  // ----- the chosen block -----

  let nameBox: HTMLInputElement | undefined;
  const [name, setName] = createSignal('');
  const [details, setDetails] = createSignal('');
  const [after, setAfter] = createSignal('');
  const [label, setLabel] = createSignal('');
  // The boxes show what the plan holds. Typing changes them; the plan changes when the box is left.
  createEffect(
    on(
      () => [picked()?.id, picked()?.text, picked()?.details] as const,
      ([, text, more]) => {
        setName(text ?? '');
        setDetails(more ?? '');
      },
    ),
  );
  createEffect(
    on(
      () => picked()?.id,
      () => {
        setAfter('');
        setLabel('');
      },
    ),
  );

  const pick = (id: string) => {
    setChosen(taskId, chosen[taskId] === id ? undefined : id);
    // Picked anew, a block is what the next message is about again.
    setLeftOut(taskId, undefined);
  };

  // ----- joining two blocks by dragging from one to the other -----

  const [join, setJoin] = createSignal<{
    from: string;
    x1: number;
    y1: number;
    x2: number;
    y2: number;
    over: string | null;
  } | null>(null);
  const startJoin = (event: PointerEvent, from: string) => {
    event.preventDefault();
    event.stopPropagation();
    const handle = event.currentTarget as HTMLElement;
    const box = handle.getBoundingClientRect();
    const start = { x1: box.left + box.width / 2, y1: box.top + box.height / 2 };
    // The block under the pointer that an arrow can lead to: any step or question but the one it starts from.
    const under = (x: number, y: number) => {
      const found = document
        .elementsFromPoint(x, y)
        .find((one) => one instanceof HTMLElement && one.dataset.block && one.dataset.block !== from);
      return (found as HTMLElement | undefined)?.dataset.block ?? null;
    };
    const move = (moved: PointerEvent) =>
      setJoin({ from, ...start, x2: moved.clientX, y2: moved.clientY, over: under(moved.clientX, moved.clientY) });
    const stop = () => {
      handle.removeEventListener('pointermove', move);
      handle.removeEventListener('pointerup', end);
      handle.removeEventListener('pointercancel', cancel);
      window.removeEventListener('keydown', key, true);
      setJoin(null);
    };
    function end(ended: PointerEvent) {
      const to = under(ended.clientX, ended.clientY);
      stop();
      if (!to) return;
      void act(async () => {
        await connectBlocks(taskId, from, to, '');
        // The block the arrow leads to is shown next: that is where the arrow gets its label.
        setChosen(taskId, to);
      });
    }
    function cancel() {
      stop();
    }
    function key(pressed: KeyboardEvent) {
      if (pressed.key === 'Escape') stop();
    }
    handle.setPointerCapture(event.pointerId);
    handle.addEventListener('pointermove', move);
    handle.addEventListener('pointerup', end);
    handle.addEventListener('pointercancel', cancel);
    window.addEventListener('keydown', key, true);
    setJoin({ from, ...start, x2: event.clientX, y2: event.clientY, over: null });
  };
  const rename = (id: string) => {
    setChosen(taskId, id);
    // The panel of the block is on screen by now; its name is ready to be typed over.
    setTimeout(() => {
      nameBox?.focus();
      nameBox?.select();
    }, 0);
  };

  const add = (kind: BlockKind, afterId: string | null) =>
    act(async () => {
      const id = await addBlock(taskId, kind, afterId);
      if (id) rename(id);
    });
  /** A new block comes after the chosen one. A note stands on its own, and so does a block added with none chosen. */
  const anchor = (kind: BlockKind) => {
    const at = picked();
    return kind !== 'note' && at && at.kind !== 'note' ? at : null;
  };

  const saveName = () => {
    const block = picked();
    const typed = name().trim();
    if (!block || typed === block.text) return;
    if (typed === '') return void setName(block.text);
    void act(() => editBlock(taskId, block.id, { text: typed }));
  };
  const saveDetails = () => {
    const block = picked();
    const typed = details().trim();
    if (!block || typed === block.details) return;
    void act(() => editBlock(taskId, block.id, { details: typed }));
  };

  const comesAfter = createMemo(() => arrows().filter((arrow) => arrow.to === picked()?.id));
  /** What the chosen block could come after: every step and question it does not come after yet. */
  const canFollow = createMemo(() => {
    const block = picked();
    if (!block) return [];
    const taken = new Set(comesAfter().map((arrow) => arrow.from));
    return blocks().filter((other) => other.kind !== 'note' && other.id !== block.id && !taken.has(other.id));
  });
  const connect = () => {
    const block = picked();
    const [from, text] = [after(), label()];
    if (!block || !from) return;
    void act(async () => {
      await connectBlocks(taskId, from, block.id, text);
      setAfter('');
      setLabel('');
    });
  };

  const showInEditor = (place: Place) => {
    go(taskHref(taskId, 'code'));
    setTimeout(() => openInEditor(taskId, place.path, place.line ?? undefined), 0);
  };

  /** Whether a box is too narrow for a whole sentence under its name: a diamond, or a step beside the main line. */
  const tight = (box: { w: number; h: number }) => box.w < 170 || box.h >= 88;

  /** One arrow of the chart is dashed and coloured when it leads to a block that waits for a yes. */
  const proposedLine = (to: string) => stateOf(to) === 'proposed';

  const fileLink = (place: Place, words: string) => (
    <a
      class="plan-file"
      href={taskHref(taskId, 'code')}
      onClick={(event) => {
        event.preventDefault();
        showInEditor(place);
      }}
    >
      {place.path}
      {words ? `, ${words}` : ''}
    </a>
  );
  const fileLinks = (block: Block) => <For each={filesOf(block)}>{(one) => fileLink(one.place, one.words)}</For>;

  return (
    <section class="center plan-view" aria-label="Plan of this task">
      <div class="plan-tools">
        <div class="plan-add" role="group" aria-label="Add to the plan">
          <For each={kinds}>
            {(entry) => (
              <button
                type="button"
                class="btn small"
                title={
                  anchor(entry.kind)
                    ? `Adds ${entry.what} after “${anchor(entry.kind)!.text}”`
                    : `Adds ${entry.what} that stands on its own`
                }
                onClick={() => void add(entry.kind, anchor(entry.kind)?.id ?? null)}
              >
                {kindIcon[entry.kind]()}
                {entry.word}
              </button>
            )}
          </For>
        </div>
        <button
          type="button"
          class="btn small ghost"
          disabled={cannotSend()}
          title={whyNot() ?? `Asks ${agentName()} for a plan of this task. You say yes to each block.`}
          onClick={() => void send(() => draftPlan(taskId, 'issue'))}
        >
          Draft from the issue
        </button>
        <button
          type="button"
          class="btn small ghost"
          disabled={cannotSend()}
          title={whyNot() ?? `Asks ${agentName()} to draw what the code does now.`}
          onClick={() => void send(() => draftPlan(taskId, 'code'))}
        >
          Draw from the code
        </button>
        <button
          type="button"
          class="btn small ghost"
          title="The chart lays itself out. This puts it back in its place after you dragged it."
          onClick={() => setTidied((count) => count + 1)}
        >
          Tidy up
        </button>
        <div class="plan-go">
          <label class="plan-only">
            <input
              type="checkbox"
              checked={plan()?.only ?? true}
              onChange={(event) => {
                const on = event.currentTarget.checked;
                void act(() => setOnlyPlan(taskId, on));
              }}
            />
            Agents build only what is in the plan
          </label>
          <button
            type="button"
            class="btn plan-next"
            disabled={cannotSend() || !plan()?.next}
            title={whyNot() ?? plan()?.nextWhy ?? `Builds “${textOf(plan()?.next ?? '')}”`}
            onClick={() => void send(() => buildBlock(taskId))}
          >
            Build the next block
          </button>
        </div>
      </div>

      <div class="plan-legend" aria-label="What the blocks look like">
        <For each={legend}>
          {(entry) => (
            <span>
              <span class={`plan-key ${entry.state}`} aria-hidden="true" />
              {entry.word}
            </span>
          )}
        </For>
      </div>

      <Show when={problem()}>
        <p class="banner danger" role="alert">
          <span class="grow">{problem()}</span>
          <button type="button" class="btn tiny" onClick={() => setProblem(null)}>
            Dismiss
          </button>
        </p>
      </Show>
      <Show when={plan()?.problem}>
        <p class="banner danger" role="alert">
          <span class="grow">{plan()!.problem}</span>
          <button type="button" class="btn tiny" onClick={() => void act(() => dismissPlanProblem(taskId))}>
            Dismiss
          </button>
        </p>
      </Show>
      <Show when={busy() && busy()!.kind !== 'build'}>
        <p class="banner" role="status">
          <span class="grow">
            Waiting for {agentName()} to {busy()!.kind === 'draw' ? 'draw the code' : 'draft the plan'}. What it draws
            arrives here.
          </span>
        </p>
      </Show>
      <Show when={waiting() > 0}>
        <p class="banner plan-waits">
          <span class="grow">
            {waiting() === 1 ? '1 block waits' : `${waiting()} blocks wait`} for your yes. {agentName()} builds nothing
            that is not in the plan yet.
          </span>
          <button type="button" class="btn tiny" onClick={() => void act(() => acceptBlock(taskId))}>
            Add all to the plan
          </button>
          <button type="button" class="btn tiny ghost" onClick={() => void act(() => declineBlock(taskId))}>
            Turn all down
          </button>
        </p>
      </Show>

      <div class="plan-body">
        <div class="plan-chart">
          <Show
            when={blocks().length > 0}
            fallback={
              <p class="map-note">
                {plan()
                  ? `Nothing is planned yet. Add a step, or let ${agentName()} draft the plan from the issue.`
                  : 'Reading the plan'}
              </p>
            }
          >
            <Show when={tidied()} keyed>
              {(_round) => (
                <Stage width={layout().width} height={layout().height} label="The plan as a flow chart">
                  <Lines
                    width={layout().width}
                    height={layout().height}
                    paths={layout()
                      .lines.filter((line) => !proposedLine(line.to))
                      .map((line) => line.d)}
                  />
                  {/* The arrows to what is only proposed are drawn apart, so they can look apart. */}
                  <div class="plan-lines-proposed">
                    <Lines
                      width={layout().width}
                      height={layout().height}
                      paths={layout()
                        .lines.filter((line) => proposedLine(line.to))
                        .map((line) => line.d)}
                    />
                  </div>
                  <For each={layout().lines.filter((line) => line.label)}>
                    {(line) => (
                      <span
                        class="map-flow-label"
                        classList={{ over: line.anchor === 'middle' }}
                        style={{ left: `${line.lx}px`, top: `${line.ly}px` }}
                        aria-hidden="true"
                      >
                        {line.label}
                      </span>
                    )}
                  </For>
                  <For each={blocks()}>
                    {(block) => (
                      <Show when={boxOf(block.id)}>
                        {(box) => (
                          <button
                            type="button"
                            class={`map-step plan-block ${block.kind} ${block.state}`}
                            data-box="true"
                            data-block={block.kind === 'note' ? undefined : block.id}
                            classList={{ 'join-target': join()?.over === block.id }}
                            style={{
                              left: `${box().x}px`,
                              top: `${box().y}px`,
                              width: `${box().w}px`,
                              height: `${box().h}px`,
                            }}
                            title={block.kind === 'note' ? undefined : stateWord(block.state)}
                            aria-pressed={picked()?.id === block.id}
                            aria-label={describeBlock(block, { blocks: blocks(), arrows: arrows() })}
                            onClick={() => pick(block.id)}
                            onDblClick={() => rename(block.id)}
                          >
                            <Show when={block.kind === 'ask'}>
                              <svg
                                class="plan-diamond"
                                viewBox={`0 0 ${box().w} ${box().h}`}
                                preserveAspectRatio="none"
                                aria-hidden="true"
                              >
                                <polygon
                                  points={`${box().w / 2},1 ${box().w - 1},${box().h / 2} ${box().w / 2},${box().h - 1} 1,${box().h / 2}`}
                                />
                              </svg>
                            </Show>
                            <span class="plan-block-name">
                              <Show when={block.state === 'built' && block.kind !== 'note'}>
                                <Check />
                              </Show>
                              <span class="map-step-text">{block.text}</span>
                            </span>
                            <Show when={hasStateLine(block)}>
                              <span class="plan-block-state">
                                <Switch>
                                  {/* A diamond and a narrow step have room for a word or two; the rest say more. */}
                                  <Match when={block.state === 'building'}>
                                    <RingDot />
                                    <span>
                                      {tight(box()) ? 'being built' : `${block.byName ?? agentName()} is building`}
                                    </span>
                                  </Match>
                                  <Match when={block.state === 'yours'}>
                                    <Chip agentId="you" short="You" flat />
                                    <span>{block.kind === 'ask' ? 'yours' : 'planned, yours'}</span>
                                  </Match>
                                  <Match when={block.state === 'proposed'}>
                                    <Chip agentId={block.by ?? ''} short={shortOf(block.by)} flat />
                                    <span>
                                      {block.kind === 'ask'
                                        ? 'proposed'
                                        : tight(box())
                                          ? 'waits for you'
                                          : 'proposed, waits for you'}
                                    </span>
                                  </Match>
                                </Switch>
                              </span>
                            </Show>
                          </button>
                        )}
                      </Show>
                    )}
                  </For>
                  {/* Dragging from the dot on a block's side to another block joins them. The panel of a block
                      does the same from the keyboard, so the dots are left out of the tab order. */}
                  <For each={blocks().filter((block) => block.kind !== 'note')}>
                    {(block) => (
                      <Show when={boxOf(block.id)}>
                        {(box) => (
                          <span
                            class="plan-handle"
                            classList={{ on: join()?.from === block.id }}
                            data-box="true"
                            data-handle={block.id}
                            aria-hidden="true"
                            title="Drag to another block to connect them"
                            style={{ left: `${box().x + box().w - 7}px`, top: `${box().y + box().h / 2 - 7}px` }}
                            onPointerDown={(event) => startJoin(event, block.id)}
                          />
                        )}
                      </Show>
                    )}
                  </For>
                  <For each={layout().plus}>
                    {(plus) => (
                      <button
                        type="button"
                        class="plan-plus"
                        data-box="true"
                        style={{ left: `${plus.x}px`, top: `${plus.y}px` }}
                        aria-label={`Add a step after ${textOf(plus.after)}`}
                        title={`Add a step after “${textOf(plus.after)}”`}
                        onClick={() => void add('step', plus.after)}
                      >
                        <Icon.Plus />
                      </button>
                    )}
                  </For>
                </Stage>
              )}
            </Show>
          </Show>
        </div>

        <Show
          when={picked()}
          fallback={
            <section class="plan-detail" aria-label="The selected block">
              <h2>No block is picked</h2>
              <p class="fine">
                Pick a block to say what it should do and what it comes after, to build it, or to take it for yourself.
              </p>
            </section>
          }
        >
          {(block) => (
            <section class="plan-detail" aria-label="The selected block">
              <div class="plan-detail-head">
                <h2>{block().text}</h2>
                <Show when={block().kind !== 'note'} fallback={<span class="plan-state">Note</span>}>
                  <span class="plan-state">
                    <StateMark state={block().state} />
                    {stateWord(block().state)}
                  </span>
                </Show>
              </div>

              <div class="plan-field">
                <label for={`plan-name-${taskId}`}>Name</label>
                <input
                  id={`plan-name-${taskId}`}
                  class="field"
                  ref={nameBox}
                  maxLength={block().kind === 'note' ? 200 : 60}
                  disabled={block().state === 'building'}
                  value={name()}
                  onInput={(event) => setName(event.currentTarget.value)}
                  onChange={saveName}
                  onKeyDown={(event) => {
                    if (event.key === 'Enter') event.currentTarget.blur();
                  }}
                />
              </div>
              <div class="plan-field">
                <label for={`plan-details-${taskId}`}>
                  {block().kind === 'note' ? 'More about it' : 'What this block should do'}
                </label>
                <textarea
                  id={`plan-details-${taskId}`}
                  rows="3"
                  maxLength={1000}
                  placeholder="Say more, when the name is not enough"
                  disabled={block().state === 'building'}
                  value={details()}
                  onInput={(event) => setDetails(event.currentTarget.value)}
                  onChange={saveDetails}
                />
              </div>

              <Show when={block().kind !== 'note'}>
                <div class="plan-field" role="group" aria-label="What it comes after">
                  <span class="plan-field-title">Comes after</span>
                  <For each={comesAfter()} fallback={<span class="fine">Nothing yet. It stands on its own.</span>}>
                    {(arrow) => (
                      <div class="plan-arrow">
                        <span class="grow">{textOf(arrow.from)}</span>
                        <input
                          class="field"
                          maxLength={12}
                          placeholder="label"
                          aria-label={`Label on the arrow from ${textOf(arrow.from)}`}
                          value={arrow.label ?? ''}
                          onChange={(event) => {
                            const typed = event.currentTarget.value;
                            void act(() => connectBlocks(taskId, arrow.from, block().id, typed));
                          }}
                        />
                        <button
                          type="button"
                          class="btn tiny"
                          aria-label={`Disconnect from ${textOf(arrow.from)}`}
                          onClick={() => void act(() => disconnectBlocks(taskId, arrow.from, block().id))}
                        >
                          Disconnect
                        </button>
                      </div>
                    )}
                  </For>
                  <Show when={canFollow().length > 0}>
                    <div class="plan-arrow">
                      <select
                        class="pick grow"
                        aria-label="Connect it after"
                        value={after()}
                        onChange={(event) => setAfter(event.currentTarget.value)}
                      >
                        <option value="">Pick a block</option>
                        <For each={canFollow()}>{(other) => <option value={other.id}>{other.text}</option>}</For>
                      </select>
                      <input
                        class="field"
                        maxLength={12}
                        placeholder="yes, no"
                        aria-label="Label on the new arrow"
                        value={label()}
                        onInput={(event) => setLabel(event.currentTarget.value)}
                      />
                      <button type="button" class="btn tiny" disabled={!after()} onClick={connect}>
                        Connect
                      </button>
                    </div>
                  </Show>
                </div>
              </Show>

              <Show when={block().state === 'built' || block().files.length > 0}>
                <dl class="plan-facts">
                  <Show when={block().state === 'built'}>
                    <dt>Built in</dt>
                    <dd class="plan-files">
                      <Show when={block().files.length > 0} fallback="No file changed in that turn.">
                        {fileLinks(block())}
                      </Show>
                    </dd>
                    <dt>Proved by</dt>
                    <dd class="plan-files">
                      <For each={proofOf(block()).tests}>{(test) => <span>The test “{test}”</span>}</For>
                      <For each={proofOf(block()).files}>{(place) => fileLink(place, '')}</For>
                      <Show when={proofOf(block()).tests.length + proofOf(block()).files.length === 0}>
                        No test was named, and that turn changed no test.
                      </Show>
                    </dd>
                    <dt>Built by</dt>
                    <dd class="plan-by">
                      <Chip agentId={block().by ?? ''} short={shortOf(block().by)} flat />
                      {block().byName ?? agentName()}
                      <Show when={block().at}>, {ago(block().at!)}</Show>
                    </dd>
                  </Show>
                  <Show when={block().state !== 'built' && block().files.length > 0}>
                    <dt>In the code</dt>
                    <dd class="plan-files">{fileLinks(block())}</dd>
                  </Show>
                </dl>
              </Show>

              <div class="plan-actions">
                <Switch>
                  <Match when={block().state === 'proposed'}>
                    <button
                      type="button"
                      class="btn primary"
                      onClick={() => void act(() => acceptBlock(taskId, block().id))}
                    >
                      Add it to the plan
                    </button>
                    <button type="button" class="btn" onClick={() => void act(() => declineBlock(taskId, block().id))}>
                      Not needed
                    </button>
                  </Match>
                  <Match when={block().kind === 'note'}>
                    <button
                      type="button"
                      class="btn ghost"
                      onClick={() => void act(() => removeBlock(taskId, block().id))}
                    >
                      Remove
                    </button>
                  </Match>
                  <Match when={block().state === 'building'}>
                    <span class="fine">
                      {block().byName ?? agentName()} is building this block. Stop its turn to change the block.
                    </span>
                  </Match>
                  <Match when={true}>
                    <Show when={block().state !== 'yours'}>
                      <button
                        type="button"
                        class="btn"
                        disabled={cannotSend()}
                        title={whyNot()}
                        onClick={() => void send(() => buildBlock(taskId, block().id))}
                      >
                        {block().state === 'built' ? 'Build it again' : 'Build this block'}
                      </button>
                    </Show>
                    <button
                      type="button"
                      class="btn ghost"
                      onClick={() => void act(() => takeBlock(taskId, block().id, block().state !== 'yours'))}
                    >
                      {block().state === 'yours' ? 'Give it back to the agent' : 'I will do it myself'}
                    </button>
                    <button
                      type="button"
                      class="btn ghost"
                      onClick={() => void act(() => removeBlock(taskId, block().id))}
                    >
                      Remove
                    </button>
                  </Match>
                </Switch>
              </div>
              <Show when={block().kind !== 'note'}>
                <span class="fine">
                  <Switch>
                    <Match when={block().state === 'proposed'}>
                      It is not in the plan, so {agentName()} will not build it until you say yes.
                    </Match>
                    <Match when={block().state === 'yours'}>
                      This block is yours. {agentName()} leaves it alone and builds what comes after it.
                    </Match>
                    <Match when={true}>
                      Change the text of a built block and it goes back to Planned. When it is built again,{' '}
                      {agentName()} is told how it read before.
                    </Match>
                  </Switch>
                </span>
              </Show>
            </section>
          )}
        </Show>
      </div>

      <Show when={join()}>
        {(line) => (
          <svg class="plan-join" aria-hidden="true">
            <line x1={line().x1} y1={line().y1} x2={line().x2} y2={line().y2} />
          </svg>
        )}
      </Show>

      <div class="statusline">
        <span>
          Draw it yourself or let an agent draft it. Drag from the dot on a block to connect it, double-click to rename.
        </span>
        <span class="grow" />
        <Show when={plan()}>{(known) => <span class="plan-counts">{countsLine(known().counts)}</span>}</Show>
      </div>
    </section>
  );
}

/**
 * The card in the conversation for blocks the agent proposed while it was
 * building: each one waits there for "Add it to the plan" or "Not needed".
 */
export function PlanProposals(props: { task: Task }): JSX.Element {
  const taskId = () => props.task.info.id;
  const agentName = () => agentOf(props.task)?.name ?? 'The agent';
  const proposed = () =>
    (planOf(taskId())?.blocks ?? []).filter((block) => block.state === 'proposed' && block.from === 'build');
  const [problem, setProblem] = createSignal<string | null>(null);
  const answer = (work: Promise<unknown>) => {
    setProblem(null);
    work.catch((error) => setProblem(message(error)));
  };
  /** A name reads as a sentence; a name that was cut short gives way to the whole sentence. */
  const said = (block: Block) =>
    block.details && block.text.endsWith('…')
      ? block.details
      : /[.?!]$/.test(block.text)
        ? block.text
        : `${block.text}.`;

  return (
    <Show when={proposed().length > 0}>
      <section class="card pinned needs plan-proposals" aria-label="Blocks the agent wants to add to the plan">
        <h2>
          {proposed()[0].byName ?? agentName()} wants to add{' '}
          {proposed().length === 1 ? 'one block' : plural(proposed().length, 'block')}
        </h2>
        <For each={proposed()}>
          {(block) => (
            <div class="plan-proposal">
              <p>“{said(block)}”</p>
              <div class="actions">
                <button
                  type="button"
                  class="btn tall primary"
                  aria-label={`Add it to the plan: ${block.text}`}
                  onClick={() => answer(acceptBlock(taskId(), block.id))}
                >
                  Add it to the plan
                </button>
                <button
                  type="button"
                  class="btn tall"
                  aria-label={`Not needed: ${block.text}`}
                  onClick={() => answer(declineBlock(taskId(), block.id))}
                >
                  Not needed
                </button>
              </div>
            </div>
          )}
        </For>
        <Show when={problem()}>
          <p class="notice danger" role="alert">
            {problem()}
          </p>
        </Show>
        <span class="fine">
          {proposed().length === 1 ? 'It is' : 'They are'} not in the plan, so {agentName()} will not build{' '}
          {proposed().length === 1 ? 'it' : 'them'} until you say yes.
        </span>
      </section>
    </Show>
  );
}
