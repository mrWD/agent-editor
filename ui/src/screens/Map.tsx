import {
  createEffect,
  createMemo,
  createSignal,
  createUniqueId,
  For,
  on,
  onCleanup,
  onMount,
  Show,
  type JSX,
} from 'solid-js';
import { createStore, reconcile } from 'solid-js/store';
import { go, taskHref } from '../app/router';
import { ago } from '../components/bits';
import { core } from '../core/client';
import { asksFlowOf, flowRequest, layoutFlow, parseFlow, type Flow, type FlowBox } from '../core/flow';
import { layoutGraph } from '../core/graph';
import {
  agentOf,
  getMap,
  sendPrompt,
  type CodeMap,
  type MapCenter,
  type MapDef,
  type MapFile,
  type MapGraph,
  type MapNode,
  type Task,
} from '../core/store';
import { openInEditor, saveAll } from '../core/work';

const message = (error: unknown) => (error instanceof Error ? error.message : String(error));
const baseName = (path: string) => path.split('/').pop() ?? path;
const plural = (count: number, one: string, many = `${one}s`) => `${count} ${count === 1 ? one : many}`;

/** A picture is never shrunk below this: smaller letters cannot be read. Past it, the picture is dragged. */
const SMALLEST = 0.6;
/** A picture taller than this is dragged up and down inside its card. */
const TALLEST = 480;
/** How many definitions a file of the tree shows before "and N more". */
const DEFS_SHOWN = 8;

type Picked = { path: string; symbol: string | null };

/** The centre the user picked for each task, so a look at the code and back does not lose it. */
const [picked, setPicked] = createStore<Record<string, Picked | undefined>>({});

/** The centre is also remembered between visits, like the open files of a task. */
function rememberedCenter(taskId: string): Picked | undefined {
  try {
    const kept = JSON.parse(localStorage.getItem(`ae.map.${taskId}`) ?? 'null') as Partial<Picked> | null;
    if (kept && typeof kept.path === 'string') return { path: kept.path, symbol: kept.symbol ?? null };
  } catch {
    // Storage may be refused or hold something else; the map then starts where the core suggests.
  }
  return undefined;
}

function rememberCenter(taskId: string, center: Picked) {
  setPicked(taskId, center);
  try {
    localStorage.setItem(`ae.map.${taskId}`, JSON.stringify(center));
  } catch {
    // The centre still holds for this visit.
  }
}

/** The lines between the boxes of a picture, with an arrowhead at each end. */
function Lines(props: { width: number; height: number; paths: string[] }): JSX.Element {
  const head = createUniqueId();
  return (
    <svg
      class="map-lines"
      width={props.width}
      height={props.height}
      viewBox={`0 0 ${props.width} ${props.height}`}
      aria-hidden="true"
    >
      <defs>
        <marker id={head} orient="auto" markerWidth="5" markerHeight="5" refX="2" refY="2" overflow="visible">
          <path d="M0 0 L4 2 L0 4 Z" />
        </marker>
      </defs>
      <For each={props.paths}>{(d) => <path d={d} marker-end={`url(#${head})`} />}</For>
    </svg>
  );
}

/**
 * The frame a picture is drawn in. A picture wider than its card is shrunk to
 * fit; one that would get too small, or is very tall, is dragged instead. The
 * card never gets a scroll bar of its own.
 */
function Stage(props: {
  width: number;
  height: number;
  label: string;
  /** Told how wide the frame is, so the picture can be laid out for it. */
  onRoom?: (room: number) => void;
  children: JSX.Element;
}): JSX.Element {
  let frame!: HTMLDivElement;
  const [room, setRoom] = createSignal(0);
  const [shift, setShift] = createSignal({ x: 0, y: 0 });

  const scale = () => (room() > 0 && props.width > room() ? Math.max(SMALLEST, room() / props.width) : 1);
  const shownHeight = () => Math.min(props.height * scale(), TALLEST);
  /** How far the picture can be dragged each way. */
  const slack = () => ({
    x: Math.max(0, props.width * scale() - room()),
    y: Math.max(0, props.height * scale() - TALLEST),
  });
  const movable = () => room() > 0 && (slack().x > 0.5 || slack().y > 0.5);
  const within = (x: number, y: number) => ({
    x: Math.min(0, Math.max(-slack().x, x)),
    y: Math.min(0, Math.max(-slack().y, y)),
  });

  // A card that grows or a picture that changes must not leave the picture dragged out of sight.
  createEffect(() => {
    const now = shift();
    const kept = within(now.x, now.y);
    if (kept.x !== now.x || kept.y !== now.y) setShift(kept);
  });

  let drag: { x: number; y: number; from: { x: number; y: number }; moved: boolean } | null = null;
  let justDragged = false;
  const down = (event: PointerEvent) => {
    if (!movable() || event.button !== 0) return;
    drag = { x: event.clientX, y: event.clientY, from: shift(), moved: false };
  };
  const move = (event: PointerEvent) => {
    if (!drag) return;
    const dx = event.clientX - drag.x;
    const dy = event.clientY - drag.y;
    // A few pixels of wobble are still a click.
    if (!drag.moved && Math.hypot(dx, dy) < 4) return;
    if (!drag.moved) {
      drag.moved = true;
      frame.setPointerCapture(event.pointerId);
    }
    setShift(within(drag.from.x + dx, drag.from.y + dy));
  };
  const up = () => {
    if (drag?.moved) {
      justDragged = true;
      setTimeout(() => (justDragged = false), 0);
    }
    drag = null;
  };

  onMount(() => {
    const measure = () => {
      const width = frame.clientWidth;
      if (width === room()) return;
      setRoom(width);
      props.onRoom?.(width);
    };
    measure();
    // The picture is laid out again one frame later: changing what is being measured
    // in the same turn makes browsers report a loop.
    let waiting = 0;
    const observer = new ResizeObserver(() => {
      cancelAnimationFrame(waiting);
      waiting = requestAnimationFrame(measure);
    });
    observer.observe(frame);
    // A drag that ends on a box must not open it. Caught on the way down, before the box hears it.
    const swallow = (event: MouseEvent) => {
      if (!justDragged) return;
      event.stopPropagation();
      event.preventDefault();
    };
    frame.addEventListener('click', swallow, true);
    onCleanup(() => {
      cancelAnimationFrame(waiting);
      observer.disconnect();
      frame.removeEventListener('click', swallow, true);
    });
  });

  /** A box reached with the keyboard is brought into view, as a drag would. */
  const reveal = (event: FocusEvent) => {
    // A browser scrolls even a frame without scroll bars to show what got the focus. The picture moves instead.
    frame.scrollLeft = 0;
    frame.scrollTop = 0;
    const box = event.target as HTMLElement;
    if (!movable() || !box.dataset.box) return;
    const size = scale();
    const left = box.offsetLeft * size;
    const top = box.offsetTop * size;
    const right = left + box.offsetWidth * size;
    const bottom = top + box.offsetHeight * size;
    let { x, y } = shift();
    if (right + x > room()) x = room() - right - 8;
    if (left + x < 0) x = 8 - left;
    if (bottom + y > shownHeight()) y = shownHeight() - bottom - 8;
    if (top + y < 0) y = 8 - top;
    setShift(within(x, y));
  };

  return (
    <>
      <div
        class="map-stage"
        classList={{ movable: movable() }}
        ref={frame}
        style={{ height: `${shownHeight()}px` }}
        role="group"
        aria-label={props.label}
        onPointerDown={down}
        onPointerMove={move}
        onPointerUp={up}
        onPointerCancel={up}
        onFocusIn={reveal}
        onScroll={() => {
          // Nothing scrolls here: the picture is moved, and only by a drag or the keyboard.
          frame.scrollLeft = 0;
          frame.scrollTop = 0;
        }}
      >
        <div
          class="map-canvas"
          style={{
            width: `${props.width}px`,
            height: `${props.height}px`,
            transform: `translate(${shift().x}px, ${shift().y}px) scale(${scale()})`,
          }}
        >
          {props.children}
        </div>
      </div>
      <Show when={movable()}>
        <span class="fine">The picture is larger than this card. Drag it to see the rest.</span>
      </Show>
    </>
  );
}

/** What a box of a picture is called for someone who cannot see it: its name, what marks it, and how it is tied in. */
function describe(node: MapNode, graph: MapGraph, verbs: { before: string; after: string }): string {
  const label = (id: string) => graph.nodes.find((other) => other.id === id)?.label ?? '';
  const parts = [node.label];
  if (node.changed) parts.push('changed in this task');
  if (node.kind === 'package') parts.push('external package');
  const from = graph.edges.filter((edge) => edge.to === node.id).map((edge) => label(edge.from));
  const to = graph.edges.filter((edge) => edge.from === node.id).map((edge) => label(edge.to));
  if (node.layer < 0 && to.length > 0) parts.push(`${verbs.before} ${to.join(' and ')}`);
  if (node.layer > 0 && from.length > 0) parts.push(`${verbs.after} ${from.join(' and ')}`);
  return parts.join(', ');
}

/** One of the two pictures the core works out: boxes in layers with arrows between them. */
function Picture(props: {
  label: string;
  graph: MapGraph;
  verbs: { before: string; after: string };
  onOpen: (node: MapNode) => void;
}): JSX.Element {
  // Until the card is measured, the width the design draws for.
  const [room, setRoom] = createSignal(470);
  const layout = createMemo(() => layoutGraph(props.graph.nodes, props.graph.edges, room()));
  const placeOf = (id: string) => layout().boxes.find((box) => box.id === id);
  return (
    <Stage width={layout().width} height={layout().height} label={props.label} onRoom={setRoom}>
      <Lines width={layout().width} height={layout().height} paths={layout().lines} />
      <For each={props.graph.nodes}>
        {(node) => (
          <Show when={placeOf(node.id)}>
            {(box) => (
              <button
                type="button"
                class="map-node"
                classList={{ changed: node.changed, package: node.kind === 'package' }}
                data-box="true"
                data-layer={node.layer}
                style={{ left: `${box().x}px`, top: `${box().y}px`, width: `${box().w}px`, height: `${box().h}px` }}
                title={node.title}
                aria-label={describe(node, props.graph, props.verbs)}
                onClick={() => props.onOpen(node)}
              >
                <span>{node.label}</span>
              </button>
            )}
          </Show>
        )}
      </For>
    </Stage>
  );
}

/** The chart an agent described: steps, questions as diamonds, and labelled arrows. */
function FlowChart(props: { flow: Flow; label: string; onOpen: (path: string, line: number) => void }): JSX.Element {
  const layout = createMemo(() => layoutFlow(props.flow));
  /** Where a step leads, in words, for someone who cannot see the arrows. */
  const leadsTo = (box: FlowBox) => {
    const next = props.flow.arrows
      .filter((arrow) => arrow.from === box.id)
      .map((arrow) => {
        const text = props.flow.nodes.find((node) => node.id === arrow.to)?.text ?? '';
        return arrow.label ? `${arrow.label}: ${text}` : text;
      });
    const marks = box.changed ? ', changed in this task' : '';
    // A question already ends its own sentence.
    const stop = /[.?!]$/.test(box.text + marks) ? '' : '.';
    return next.length > 0 ? `${marks}${stop} Then ${next.join('; ')}.` : `${marks}${stop}`;
  };
  // A step that is a button says where it leads in its name; one that is plain text carries the words unseen.
  const inside = (box: FlowBox, withWords: boolean) => (
    <>
      <Show when={box.kind === 'ask'}>
        <span class="map-diamond" aria-hidden="true" />
      </Show>
      <span class="map-step-text">{box.text}</span>
      <Show when={withWords}>
        <span class="map-unseen">{leadsTo(box)}</span>
      </Show>
    </>
  );
  const frame = (box: FlowBox) => ({
    left: `${box.x}px`,
    top: `${box.y}px`,
    width: `${box.w}px`,
    height: `${box.h}px`,
  });
  return (
    <Stage width={layout().width} height={layout().height} label={props.label}>
      <Lines width={layout().width} height={layout().height} paths={layout().lines.map((line) => line.d)} />
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
      <For each={layout().boxes}>
        {(box) => (
          <Show
            when={box.path}
            fallback={
              <div class="map-step" classList={{ ask: box.kind === 'ask', changed: box.changed }} style={frame(box)}>
                {inside(box, true)}
              </div>
            }
          >
            {/* A step that says where it happens opens that place. */}
            <button
              type="button"
              class="map-step"
              classList={{ ask: box.kind === 'ask', changed: box.changed }}
              data-box="true"
              style={frame(box)}
              title={`${box.path}, line ${box.line}`}
              aria-label={`${box.text}${leadsTo(box)}`}
              onClick={() => props.onOpen(box.path!, box.line ?? 1)}
            >
              {inside(box, false)}
            </button>
          </Show>
        )}
      </For>
    </Stage>
  );
}

type Folder = { name: string; folders: Folder[]; files: MapFile[] };

/** Sorts files into their folders. A folder that holds nothing but one folder is shown on one line with it. */
function foldersOf(files: MapFile[]): Folder {
  const root: Folder = { name: '', folders: [], files: [] };
  for (const file of files) {
    const parts = file.path.split('/');
    parts.pop();
    let at = root;
    for (const part of parts) {
      let next = at.folders.find((folder) => folder.name === part);
      if (!next) {
        next = { name: part, folders: [], files: [] };
        at.folders.push(next);
      }
      at = next;
    }
    at.files.push(file);
  }
  const squeeze = (folder: Folder): Folder => {
    let current = folder;
    while (current.files.length === 0 && current.folders.length === 1 && current !== root) {
      const only = current.folders[0];
      current = { ...only, name: `${current.name}/${only.name}` };
    }
    return { ...current, folders: current.folders.map(squeeze) };
  };
  return { ...root, folders: root.folders.map(squeeze) };
}

/** One file of the tree: its name, then what it defines. */
function FileRow(props: { file: MapFile; onOpen: (path: string, line?: number) => void }): JSX.Element {
  const [all, setAll] = createSignal(false);
  const shown = createMemo<MapDef[]>(() => {
    const defs = props.file.defs;
    if (all() || defs.length <= DEFS_SHOWN) return defs;
    // Room for a few: what the task changed first, then the rest, each in the order of the file.
    const keep = new Set([...defs.filter((def) => def.changed), ...defs].slice(0, DEFS_SHOWN));
    return defs.filter((def) => keep.has(def));
  });
  const more = () => props.file.defs.length - shown().length;
  const name = () => baseName(props.file.path);
  return (
    <li class="map-file-row">
      <button
        type="button"
        class="map-file"
        classList={{ changed: props.file.changed }}
        title={props.file.path}
        aria-label={`${name()}${props.file.changed ? ', changed in this task' : ''}`}
        onClick={() => props.onOpen(props.file.path)}
      >
        {name()}
      </button>
      <Show when={props.file.defs.length > 0 || props.file.tests > 0}>
        <span class="map-defs">
          <For each={shown()}>
            {(def, index) => (
              <>
                <Show when={index() > 0}>, </Show>
                <button
                  type="button"
                  class="map-def"
                  title={`${props.file.path}, line ${def.line}`}
                  aria-label={`${def.kind} ${def.name}${def.changed ? ', changed in this task' : ''}`}
                  onClick={() => props.onOpen(props.file.path, def.line)}
                >
                  {def.kind} {def.name}
                </button>
              </>
            )}
          </For>
          <Show when={more() > 0}>
            {' '}
            <button type="button" class="map-def more" onClick={() => setAll(true)}>
              and {more()} more
            </button>
          </Show>
          <Show when={props.file.tests > 0}>
            {shown().length > 0 ? ', ' : ''}
            {plural(props.file.tests, 'test')}
          </Show>
        </span>
      </Show>
    </li>
  );
}

function Card(props: { title: string; about: string; children: JSX.Element }): JSX.Element {
  return (
    <section class="map-card" aria-label={props.title}>
      <div class="map-card-head">
        <h2>{props.title}</h2>
        <span>{props.about}</span>
      </div>
      {props.children}
    </section>
  );
}

/**
 * The Map view: pictures of the code this task changed. The tree, the
 * dependencies and the calls are worked out by the core from the files. The
 * flow takes understanding the code, so the task's agent is asked to draw it.
 */
export function MapView(props: { task: Task }): JSX.Element {
  // eslint-disable-next-line solid/reactivity -- this component is made anew for each task, so the id never changes under it
  const taskId = props.task.info.id;
  const agentName = () => agentOf(props.task)?.name ?? 'the agent';
  const canSend = () => props.task.info.state === 'idle' || props.task.info.state === 'asleep';

  const [state, setState] = createStore<{ map: CodeMap | null }>({ map: null });
  const [problem, setProblem] = createSignal<string | null>(null);
  const [sendProblem, setSendProblem] = createSignal<string | null>(null);
  const [sending, setSending] = createSignal(false);

  let asked = 0;
  const load = async () => {
    const mine = ++asked;
    try {
      const next = await getMap(taskId, picked[taskId]);
      if (mine !== asked) return;
      // Boxes that are the same as before stay as they are, so the keyboard keeps its place.
      setState('map', reconcile(next));
      setProblem(null);
    } catch (error) {
      if (mine !== asked) return;
      setProblem(message(error));
    }
  };

  // Several events often arrive together; one read a moment later covers them all.
  let timer: ReturnType<typeof setTimeout> | undefined;
  const soon = () => {
    clearTimeout(timer);
    timer = setTimeout(() => void load(), 150);
  };
  onMount(() => {
    if (!picked[taskId]) {
      const kept = rememberedCenter(taskId);
      if (kept) setPicked(taskId, kept);
    }
    // What the user typed in the code is saved first, so the map shows what they see.
    void saveAll(taskId).then(load);
    const stop = core.onEvent((event) => {
      const mine = event.data?.taskId === taskId;
      if ((event.event === 'git.changed' || event.event === 'review.changed') && mine) soon();
      if (event.event === 'link.open') soon();
    });
    // Coming back to the window: another program may have changed the files meanwhile.
    window.addEventListener('focus', soon);
    onCleanup(() => {
      clearTimeout(timer);
      stop();
      window.removeEventListener('focus', soon);
    });
  });

  const center = () => state.map?.center ?? null;
  const pick = (index: number) => {
    const next = state.map?.centers[index];
    if (!next) return;
    rememberCenter(taskId, { path: next.path, symbol: next.symbol });
    void load();
  };

  const showInEditor = (path: string, line?: number) => {
    go(taskHref(taskId, 'code'));
    setTimeout(() => openInEditor(taskId, path, line), 0);
  };
  const openNode = (node: MapNode) => {
    if (node.path) showInEditor(node.path, node.line ?? undefined);
  };

  const send = async (text: string) => {
    if (!canSend() || sending()) return;
    setSending(true);
    setSendProblem(null);
    try {
      // What was typed in the code is saved first, so the agent works on what the user sees.
      await saveAll(taskId);
      await sendPrompt(taskId, text);
    } catch (error) {
      setSendProblem(message(error));
    } finally {
      setSending(false);
    }
  };

  /** "`loginLimiter` in `src/a.ts`", the way a message names the centre. */
  const named = (at: MapCenter) => (at.symbol ? `\`${at.symbol}\` in \`${at.path}\`` : `\`${at.path}\``);
  const explain = () => {
    const at = center();
    if (at) void send(`Map, ${named(at)}\n\nExplain what changed here.`);
  };
  const askFlow = () => {
    const at = center();
    if (at) void send(flowRequest(at));
  };

  /**
   * Where the flow of the centre stands: never asked for, asked and not yet
   * answered, drawn, or answered in a way that cannot be drawn. It is read
   * from the conversation, so it is still there after a reload.
   */
  const flow = createMemo<
    | { kind: 'none' }
    | { kind: 'waiting' }
    | { kind: 'drawn'; flow: Flow; at: number }
    | { kind: 'bad'; problem: string }
  >(() => {
    const at = center();
    const feed = props.task.feed;
    if (!at) return { kind: 'none' };
    const request = feed.findLastIndex((entry) => entry.kind === 'user' && asksFlowOf(entry.text, at));
    if (request < 0) return { kind: 'none' };
    // Everything the agent said from the request up to the user's next message is its answer.
    let answer = '';
    let answeredAt = 0;
    let over = false;
    for (const entry of feed.slice(request + 1)) {
      if (entry.kind === 'user') {
        over = true;
        break;
      }
      if (entry.kind === 'agent') {
        answer += `${entry.text}\n`;
        answeredAt = entry.at;
      }
    }
    const read = parseFlow(answer);
    if (read.ok) return { kind: 'drawn', flow: read.flow, at: answeredAt };
    // While the agent is still at it, a half-written block is not a wrong one.
    if (!over && !canSend()) return { kind: 'waiting' };
    return { kind: 'bad', problem: read.problem };
  });

  const drawn = createMemo(() => {
    const now = flow();
    return now.kind === 'drawn' ? now : null;
  });
  const wrong = createMemo(() => {
    const now = flow();
    return now.kind === 'bad' ? now.problem : null;
  });

  const tree = createMemo(() => foldersOf(state.map?.tree ?? []));
  const level = (folder: Folder): JSX.Element => (
    <>
      <For each={folder.folders}>
        {(inner) => (
          <li class="map-folder">
            <span class="map-folder-name">{inner.name}/</span>
            <ul class="map-branch">{level(inner)}</ul>
          </li>
        )}
      </For>
      <For each={folder.files}>{(file) => <FileRow file={file} onOpen={showInEditor} />}</For>
    </>
  );
  const unread = () => (state.map?.tree ?? []).some((file) => file.language === null);

  /** What stands under a picture that left boxes out. */
  const leftOut = (graph: MapGraph) => (
    <Show when={graph.hidden > 0}>
      <span class="fine">{plural(graph.hidden, 'more box', 'more boxes')} left out to keep the picture readable.</span>
    </Show>
  );
  const busyTitle = () => (canSend() ? undefined : `Wait until ${agentName()} has finished its turn`);

  // A message that could not be sent was about the old centre.
  createEffect(on(center, () => setSendProblem(null), { defer: true }));

  return (
    <section class="center map" aria-label="Map of this task">
      <Show
        when={state.map}
        fallback={
          <div class="center-note">
            <h2>{problem() ? 'The map cannot be drawn' : 'Reading the code'}</h2>
            <p>{problem()}</p>
          </div>
        }
      >
        {(map) => (
          <Show
            when={map().tree.length > 0}
            fallback={
              <div class="center-note">
                <h2>Nothing to map yet</h2>
                <p>The map shows the files this task changed and the code around them.</p>
                <p>
                  {props.task.info.branch
                    ? `When ${agentName()} or you change a file, it appears here.`
                    : 'This folder has no git, so the map only knows about files you save in the editor.'}
                </p>
              </div>
            }
          >
            <div class="map-head">
              <Show when={center()}>
                {(at) => (
                  <label class="map-center">
                    <span class="soft" aria-hidden="true">
                      Centered on
                    </span>
                    <span class="mono" aria-hidden="true">
                      {at().label}
                    </span>
                    <svg
                      width="14"
                      height="14"
                      viewBox="0 0 16 16"
                      fill="none"
                      stroke="currentColor"
                      stroke-width="1.5"
                      stroke-linecap="round"
                      stroke-linejoin="round"
                      aria-hidden="true"
                    >
                      <path d="M4 6l4 4 4-4" />
                    </svg>
                    {/* A real list under the drawn button: the keyboard and screen readers get it for free. */}
                    <select
                      aria-label="Centered on"
                      title={`${at().path}${at().symbol ? `, line ${at().line}` : ''}`}
                      onChange={(event) => pick(Number(event.currentTarget.value))}
                    >
                      <For each={map().centers}>
                        {(option, index) => (
                          <option value={index()} selected={option.path === at().path && option.symbol === at().symbol}>
                            {option.symbol
                              ? `${option.label} in ${baseName(option.path)}`
                              : `${option.path}, the whole file`}
                          </option>
                        )}
                      </For>
                    </select>
                  </label>
                )}
              </Show>
              <span class="map-legend">
                <span class="map-key changed" aria-hidden="true" />
                Changed in this task
              </span>
              <span class="map-legend">
                <span class="map-key package" aria-hidden="true" />
                External package
              </span>
              <span class="grow" />
              <Show when={center()}>
                <button
                  type="button"
                  class="btn small map-ask"
                  disabled={!canSend() || sending()}
                  title={busyTitle()}
                  onClick={explain}
                >
                  Ask {agentName()} to explain this
                </button>
              </Show>
            </div>

            <Show when={sendProblem()}>
              <p class="banner danger" role="alert">
                <span class="grow">{sendProblem()}</span>
                <button type="button" class="btn tiny" onClick={() => setSendProblem(null)}>
                  Dismiss
                </button>
              </p>
            </Show>
            {/* The map on screen is from the last reading that worked; say so when a newer one failed. */}
            <Show when={problem()}>
              <p class="banner danger" role="alert">
                <span class="grow">The map could not be read again, so this one may be out of date. {problem()}</span>
                <button type="button" class="btn tiny" onClick={() => void load()}>
                  Read it again
                </button>
              </p>
            </Show>

            <div class="map-body">
              <div class="map-grid">
                <Show when={center()}>
                  {(at) => (
                    <Card title="Flow" about="What happens, step by step, when the request comes in">
                      <Show when={flow().kind === 'none'}>
                        <p class="map-note">
                          A flow takes understanding the code, so {agentName()} draws it. Asking sends it one message
                          about {at().label}.
                        </p>
                      </Show>
                      <Show when={flow().kind === 'waiting'}>
                        <p class="map-note" role="status">
                          Waiting for {agentName()} to draw the flow of {at().label}.
                        </p>
                      </Show>
                      <Show when={drawn()}>
                        {(now) => (
                          <>
                            <FlowChart flow={now().flow} label={`Flow of ${at().label}`} onOpen={showInEditor} />
                            <span class="fine">
                              Drawn by {agentName()}, {ago(now().at)}. A step that names a file opens it.
                            </span>
                          </>
                        )}
                      </Show>
                      <Show when={wrong()}>
                        <p class="map-note" role="alert">
                          {agentName()} answered, but not in the flow format, so there is nothing to draw. {wrong()}
                        </p>
                      </Show>
                      <Show when={flow().kind !== 'waiting'}>
                        <div>
                          <button
                            type="button"
                            class="btn small map-ask"
                            disabled={!canSend() || sending()}
                            title={busyTitle()}
                            onClick={askFlow}
                          >
                            {flow().kind === 'none'
                              ? `Ask ${agentName()} to draw the flow`
                              : `Ask ${agentName()} to draw it again`}
                          </button>
                        </div>
                      </Show>
                    </Card>
                  )}
                </Show>

                <Card title="Tree" about="Files and what each one defines">
                  <ul class="map-tree">{level(tree())}</ul>
                  <Show when={unread()}>
                    <span class="fine">A file with nothing after its name is of a kind the map does not read yet.</span>
                  </Show>
                  <For each={map().notes}>{(note) => <span class="fine">{note}</span>}</For>
                </Card>

                <Show
                  when={center()}
                  fallback={
                    <Card title="Dependencies" about="Which files and packages this code needs">
                      <p class="map-note">
                        This task has not changed a file the map can read, so there is nothing to draw around.
                      </p>
                    </Card>
                  }
                >
                  {(at) => (
                    <>
                      <Card title="Dependencies" about="Which files and packages this code needs">
                        <Picture
                          label={`Dependencies of ${baseName(at().path)}`}
                          graph={map().deps}
                          verbs={{ before: 'needs', after: 'needed by' }}
                          onOpen={openNode}
                        />
                        <Show when={map().deps.nodes.length === 1}>
                          <span class="fine">
                            {baseName(at().path)} imports nothing the map can see, and no file imports it.
                          </span>
                        </Show>
                        {leftOut(map().deps)}
                      </Card>
                      <Card title="Calls" about="Who calls the new function and what it calls">
                        <Show
                          when={map().calls.nodes.length > 0}
                          fallback={
                            <p class="map-note">
                              No function of {baseName(at().path)} was changed, so there are no calls to draw.
                            </p>
                          }
                        >
                          <Picture
                            label={`Calls around ${at().label}`}
                            graph={map().calls}
                            verbs={{ before: 'uses', after: 'used by' }}
                            onOpen={openNode}
                          />
                          <Show
                            when={map().calls.nodes.length === map().calls.nodes.filter((n) => n.layer === 0).length}
                          >
                            <span class="fine">Nothing the map can see uses it, and it calls nothing.</span>
                          </Show>
                          {leftOut(map().calls)}
                        </Show>
                      </Card>
                    </>
                  )}
                </Show>
              </div>
            </div>
          </Show>
        )}
      </Show>
    </section>
  );
}
