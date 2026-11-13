import { FitAddon } from '@xterm/addon-fit';
import { Terminal as Xterm } from '@xterm/xterm';
import '@xterm/xterm/css/xterm.css';
import { createEffect, createSignal, on, onCleanup, onMount, Show, type JSX } from 'solid-js';
import { core } from '../core/client';
import { listTerminals, type TermInfo } from '../core/store';
import { colourOf, currentTheme } from '../core/theme';

/** Kinds of raw-byte messages; they match the core's. */
const OUTPUT = 1;
const REPLACE = 2;
const INPUT = 1;

/**
 * The colours of the terminal, read from the style sheet as it stands: the
 * terminal draws itself, so it cannot be styled the way the rest is.
 */
function colours() {
  const ansi = (index: number) => colourOf(`--ansi-${index}`);
  return {
    background: colourOf('--term-bg'),
    foreground: colourOf('--term-fg'),
    cursor: colourOf('--accent'),
    cursorAccent: colourOf('--term-bg'),
    selectionBackground: colourOf('--selection'),
    black: ansi(0),
    red: ansi(1),
    green: ansi(2),
    yellow: ansi(3),
    blue: ansi(4),
    magenta: ansi(5),
    cyan: ansi(6),
    white: ansi(7),
    brightBlack: ansi(8),
    brightRed: ansi(9),
    brightGreen: ansi(10),
    brightYellow: ansi(11),
    brightBlue: ansi(12),
    brightMagenta: ansi(13),
    brightCyan: ansi(14),
    brightWhite: ansi(15),
  };
}

const encoder = new TextEncoder();

/**
 * A terminal of a task. Without `attach` it is one of the user's shells in the
 * task's folder: the shell runs in the core and outlives this view, so leaving
 * the task and coming back shows the same terminal with what it printed
 * meanwhile. `shell` names the shell to join; without it a new one is started
 * here, and `onShell` is told which. With `attach` the view shows the output
 * of one program the Run tab started.
 */
export function TerminalView(props: {
  taskId: string;
  visible: boolean;
  attach?: number;
  shell?: number | null;
  onShell?: (info: TermInfo) => void;
  /** For a program the user types into, such as a shell in a container: it is told the size of this view. */
  follow?: boolean;
}): JSX.Element {
  let host: HTMLDivElement | undefined;
  let term: Xterm | undefined;
  let fit: FitAddon | undefined;
  let termId: number | null = null;
  let connecting = false;
  // Set once the shell shown here has ended: the next shell in this view is a new one, whatever `shell` still names.
  let startAnew = false;
  const [ended, setEnded] = createSignal(false);
  const [problem, setProblem] = createSignal<string | null>(null);

  const size = () => ({ cols: term?.cols ?? 80, rows: term?.rows ?? 24 });
  /** Whether the program is told the size of this view. A log is not: it keeps the size it was started with. */
  const follows = () => props.attach === undefined || props.follow === true;

  /** Joins the terminal this view is for, or starts a new shell when it is for none yet. */
  const connect = async () => {
    if (connecting || !term) return;
    connecting = true;
    setProblem(null);
    try {
      // A task has several terminals: its shells, and the programs of the Run tab. This view shows one of them.
      const wanted = props.attach ?? (startAnew ? null : (props.shell ?? null));
      const listed = wanted === null ? [] : (await listTerminals(props.taskId)).terminals;
      const existing = listed.find((info) => info.termId === wanted);
      if (existing) {
        termId = existing.termId;
        await core.request('terminal.attach', { termId });
        // A program's log keeps the size it was started with; only a shell follows this view.
        if (follows()) await core.request('terminal.resize', { termId, ...size() });
      } else if (wanted !== null) {
        // The terminal is gone: its program ended while nobody was looking.
        startAnew = true;
        setEnded(true);
        return;
      } else {
        term.reset();
        const opened = await core.request<TermInfo>('terminal.open', { taskId: props.taskId, ...size() });
        termId = opened.termId;
        startAnew = false;
        props.onShell?.(opened);
      }
      setEnded(false);
    } catch (error) {
      setProblem(error instanceof Error ? error.message : String(error));
    } finally {
      connecting = false;
    }
  };

  const hasSize = () => Boolean(host && host.clientWidth > 0 && host.clientHeight > 0);

  const refit = () => {
    if (!fit || !term || !hasSize()) return;
    const before = size();
    fit.fit();
    const after = size();
    if (termId !== null && follows() && (before.cols !== after.cols || before.rows !== after.rows)) {
      void core.request('terminal.resize', { termId, ...after }).catch(() => {});
    }
  };

  /**
   * Brings the terminal in line with its place on screen, and starts the shell
   * once that place has a size. A shell started at a guessed size redraws its
   * prompt when the real size arrives, which leaves litter on the first line.
   */
  const settle = () => {
    if (!props.visible || !hasSize()) return;
    refit();
    if (termId === null && !ended() && !problem()) void connect();
  };

  onMount(() => {
    term = new Xterm({
      fontFamily: "'JetBrains Mono Variable', 'JetBrains Mono', ui-monospace, monospace",
      fontSize: 12.5,
      lineHeight: 1.3,
      cursorBlink: true,
      theme: colours(),
      scrollback: 5000,
      allowProposedApi: true,
    });
    fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host!);

    // Drawing on the graphics card is faster; where it is not available the plain renderer does the job.
    void import('@xterm/addon-webgl').then(
      ({ WebglAddon }) => {
        if (!term) return;
        try {
          const webgl = new WebglAddon();
          webgl.onContextLoss(() => webgl.dispose());
          term.loadAddon(webgl);
        } catch {
          // Keep the plain renderer.
        }
      },
      () => {},
    );

    term.onData((data) => {
      if (termId !== null) core.sendBytes(INPUT, termId, encoder.encode(data));
    });
    term.onBinary((data) => {
      if (termId === null) return;
      const bytes = new Uint8Array(data.length);
      for (let i = 0; i < data.length; i++) bytes[i] = data.charCodeAt(i) & 0xff;
      core.sendBytes(INPUT, termId, bytes);
    });

    const stopBytes = core.onBytes((kind, channel, bytes) => {
      if (channel !== termId || !term) return;
      if (kind === REPLACE) term.reset();
      if (kind === OUTPUT || kind === REPLACE) term.write(bytes);
    });
    // eslint-disable-next-line solid/reactivity -- called later, as an event handler: it reads what is current then
    const stopEvents = core.onEvent((event) => {
      if (event.event === 'terminal.exit' && event.data.termId === termId) {
        termId = null;
        startAnew = true;
        setEnded(true);
      }
      // After the connection came back the core no longer sends this terminal's output here.
      if (event.event === 'link.open' && (termId !== null || problem())) {
        termId = null;
        void connect();
      }
    });

    const observer = new ResizeObserver(() => settle());
    observer.observe(host!);

    onCleanup(() => {
      observer.disconnect();
      stopBytes();
      stopEvents();
      if (termId !== null) void core.request('terminal.detach', { termId }).catch(() => {});
      term?.dispose();
      term = undefined;
    });
  });

  // The theme changed: the terminal is drawn again in its colours.
  createEffect(
    on(
      currentTheme,
      () => {
        if (term) term.options.theme = colours();
      },
      { defer: true },
    ),
  );

  // The shell starts the first time the terminal is looked at, not before.
  createEffect(
    on(
      () => props.visible,
      (visible) => {
        if (!visible) return;
        requestAnimationFrame(() => {
          settle();
          term?.focus();
        });
      },
    ),
  );

  return (
    <div class="terminal">
      <div class="terminal-host" ref={host} />
      <Show when={ended()}>
        <div class="terminal-note">
          <span>{props.attach === undefined ? 'The shell ended.' : 'This program has ended.'}</span>
          <Show when={props.attach === undefined}>
            <button type="button" class="btn small" onClick={() => void connect()}>
              Start a new one
            </button>
          </Show>
        </div>
      </Show>
      <Show when={problem()}>
        <div class="terminal-note" role="alert">
          <span>{problem()}</span>
          <button type="button" class="btn small" onClick={() => void connect()}>
            Try again
          </button>
        </div>
      </Show>
    </div>
  );
}
