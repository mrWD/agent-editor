import { Deploys } from '../components/Deploys';
import {
  createEffect,
  createMemo,
  createSignal,
  For,
  Match,
  on,
  onCleanup,
  onMount,
  Show,
  Switch,
  type JSX,
} from 'solid-js';
import { createStore, unwrap } from 'solid-js/store';
import { createKept } from '../core/kept';
import { Icon, clock } from '../components/bits';
import { addPictures, pictureCount } from '../components/Pictures';
import { ApiView } from '../components/PreviewApi';
import { DatabaseView } from '../components/PreviewDatabase';
import { DeviceView } from '../components/PreviewDevice';
import { InfraView } from '../components/PreviewInfra';
import { JobsView } from '../components/PreviewJobs';
import { core } from '../core/client';
import { MAX_PICTURES } from '../core/pictures';
import {
  BEZEL,
  cornerRadius,
  deviceName,
  frameHeight,
  frameLabel,
  joinAddress,
  layoutFrames,
  markLabel,
  MAX_SHOT_HEIGHT,
  parseWidth,
  PRESET_WIDTHS,
  readElement,
  readSettings,
  readShots,
  readThemeReport,
  scaleLabel,
  shotDone,
  shotName,
  shotPlan,
  shotRefusal,
  shotWords,
  splitAddress,
  themeNote,
  themeOf,
  defaultSettings,
  MAX_WIDTH,
  MIN_WIDTH,
  type PreviewSettings,
  type Theme,
  type ThemeReport,
} from '../core/preview';
import { addDrawing, parentAsked, pointedOf, setLive, setPicked, setShooter } from '../core/previewState';
import { agentOf, runStart, runState, type RunState, type Task } from '../core/store';

const message = (error: unknown) => (error instanceof Error ? error.message : String(error));

/** Marks on every message between the interface and the script in the page. */
const TO_PAGE = 'ae-preview';
const FROM_PAGE = 'ae-preview-page';

type Relay = { port: number; target: string };
type Tool = 'none' | 'pick' | 'draw';
/** Where the running app is looked at: as a page, or on one of the two devices. Any of them, side by side. */
type Screen = 'web' | 'ios' | 'android';
/** The order they stand in, left to right. */
const SCREENS: readonly Screen[] = ['web', 'ios', 'android'];
type Kind = 'app' | 'api' | 'db' | 'jobs' | 'infra';

/** What is known about the page in one frame. */
type FrameState = {
  /** The page's script has spoken since the page loaded. */
  ready: boolean;
  /** How many pages of this frame have said "ready"; a new one means a new page to set up. */
  loads: number;
  /** A page loaded and its script does not answer: our tools do not work in it. */
  silent: boolean;
  /** How many times a page of this frame answered "here" to being asked after a load. */
  answers: number;
  /** The page keeps itself fresh, so it is not reloaded for it. */
  hot: boolean;
  /** The relay's own page is shown instead of the app, and why. */
  problem: string | null;
  /** The page of the app the frame is on, as the page's script says. `null` until it has. */
  path: string | null;
  report: ThemeReport | null;
  /** Whether the picked element was found in this frame. `null` before the page has said. */
  found: boolean | null;
};

const freshFrame = (): FrameState => ({
  ready: false,
  loads: 0,
  silent: false,
  answers: 0,
  hot: false,
  problem: null,
  path: null,
  report: null,
  found: null,
});

/**
 * The relay listens on 127.0.0.1. A frame is given the name this window itself goes by,
 * so the page in it counts as the same site as the interface and keeps its cookies.
 */
const frameHost = location.hostname === 'localhost' ? 'localhost' : '127.0.0.1';

const Sun = () => (
  <svg
    width="14"
    height="14"
    viewBox="0 0 16 16"
    fill="none"
    stroke="currentColor"
    stroke-width="1.5"
    stroke-linecap="round"
    aria-hidden="true"
  >
    <circle cx="8" cy="8" r="3" />
    <path d="M8 1.5v1.5M8 13v1.5M1.5 8H3M13 8h1.5M3.4 3.4l1.1 1.1M11.5 11.5l1.1 1.1M3.4 12.6l1.1-1.1M11.5 4.5l1.1-1.1" />
  </svg>
);
const Moon = () => (
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
    <path d="M13 9.5A5.5 5.5 0 0 1 6.5 3a5.5 5.5 0 1 0 6.5 6.5z" />
  </svg>
);
const ReloadIcon = () => (
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
    <path d="M13 8a5 5 0 1 1-1.5-3.6" />
    <path d="M13 2.5v3h-3" />
  </svg>
);
const PickIcon = () => (
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
    <path d="M3 3l4.2 10 1.5-4.3L13 7.2 3 3z" />
  </svg>
);
const ShotIcon = () => (
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
    <rect x="2" y="4" width="12" height="9" rx="2" />
    <circle cx="8" cy="8.5" r="2.2" />
  </svg>
);
const DrawIcon = () => (
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
    <path d="M3 13l1-3.5L10.5 3l2.5 2.5L6.5 12 3 13z" />
  </svg>
);

/**
 * The Preview view of a task: the running app in several frames side by side,
 * one per chosen width, each in the theme it was asked to show. The page comes
 * through a relay of the core, which adds the script that sets the theme, picks
 * elements and draws; this view talks to that script in messages between windows.
 */
export function PreviewView(props: { task: Task }): JSX.Element {
  // eslint-disable-next-line solid/reactivity -- this component is made anew for each task, so the id never changes under it
  const taskId = props.task.info.id;
  const [settings, setSettings] = createStore<PreviewSettings>(defaultSettings());
  /**
   * The settings were read from the core. Nothing is shown and nothing can be chosen before that:
   * a choice made earlier would be overwritten by what was stored.
   */
  const [loaded, setLoaded] = createSignal(false);
  // What ran last time is there at once; it is asked anew behind that. The way to the page is not kept:
  // it is closed when this view goes, and opened anew when it comes.
  const [run, setRun] = createKept<RunState | null>(`task:${taskId}:preview.run`, null);
  const [relay, setRelay] = createSignal<Relay | null>(null);
  const [problem, setProblem] = createSignal<string | null>(null);
  // What was looked at last is what opens next: a task on a service is not looked at as a page.
  const [kind, setKind] = createKept<Kind>(`task:${taskId}:preview.kind`, 'app');
  // The page and the two devices are one thing to look at, the app, and they are looked at together:
  // each is turned on or off by itself, and what is on stands side by side, to be compared.
  const [screens, setScreens] = createKept<Screen[]>(`task:${taskId}:preview.screens`, ['web']);
  /** Whether what is on show is data of the task's service or files of its project, not a screen of its app. */
  const isData = () => kind() !== 'app';
  /** Whether the page is on show. */
  const webOn = () => !isData() && screens().includes('web');
  /** The devices that are on show, in the order they stand. */
  const devicesOn = createMemo(() =>
    isData() ? [] : SCREENS.filter((one): one is 'ios' | 'android' => one !== 'web' && screens().includes(one)),
  );
  const toggleScreen = (which: Screen) => {
    const now = screens();
    // The last one stays on: there is always something of the app to look at.
    if (now.includes(which)) {
      if (now.length > 1) setScreens(now.filter((one) => one !== which));
    } else setScreens(SCREENS.filter((one) => one === which || now.includes(one)));
  };
  /**
   * Whether the project has infrastructure files: Infra is offered only then. What was true at the
   * last look is there at once; it is asked anew behind that.
   */
  const [hasInfra, setHasInfra] = createKept<boolean>(`task:${taskId}:preview.infra`, false);
  const loadInfra = () => {
    core
      .request<{ has: boolean }>('infra.has', { taskId })
      .then((answer) => setHasInfra(answer.has))
      // An older core that knows no infrastructure: the kind is not offered.
      .catch(() => {});
  };
  const [tool, setTool] = createSignal<Tool>('none');
  const [adding, setAdding] = createSignal(false);
  const [widthProblem, setWidthProblem] = createSignal<string | null>(null);
  const [starting, setStarting] = createSignal<string | null>(null);
  const [reloadedAt, setReloadedAt] = createSignal<number | null>(null);
  /** What the core said about the browser screenshots are taken with. `null` until it has. */
  const [shotTool, setShotTool] = createSignal<{ browser: string | null; needs: string | null } | null>(null);
  const [shooting, setShooting] = createSignal(false);
  /** What the last screenshot came to, said next to the tools for as long as nothing has changed since (`when`). */
  const [shotSaid, setShotSaid] = createSignal<{ text: string; failed: boolean; when: string } | null>(null);
  /** Goes up with each press of Reload on a device tab: the address is opened on the device again. */
  const [deviceReloads, setDeviceReloads] = createSignal(0);
  const [stage, setStage] = createSignal({ width: 0, height: 0 });
  /** Counts the times the core was reached anew, so the relay is asked for again after a restart. */
  const [links, setLinks] = createSignal(0);
  const [frames, setFrames] = createStore<Record<string, FrameState>>({});
  const iframes = new Map<number, HTMLIFrameElement>();
  // The room the page has: the whole stage, or what the devices beside it leave. The page's part of
  // the stage comes and goes with "Web", so it is watched from the moment it is drawn.
  const stageWatch = new ResizeObserver((entries) => {
    const box = entries[entries.length - 1]?.contentRect;
    if (box) setStage({ width: Math.floor(box.width), height: Math.floor(box.height) });
  });
  const watchStage = (el: HTMLDivElement) => {
    stageWatch.disconnect();
    stageWatch.observe(el);
  };
  onCleanup(() => stageWatch.disconnect());
  let widthInput: HTMLInputElement | undefined;

  // ----- what is remembered -----

  // Sent at once, not a moment later: the window may be closed or loaded anew right after a choice.
  const remember = () => {
    void core.request('preview.remember', { taskId, settings: unwrap(settings) }).catch(() => {});
  };
  const update = (change: Partial<PreviewSettings>) => {
    setSettings(change);
    remember();
  };

  // ----- the address -----

  const loadRun = async () => {
    try {
      setRun(await runState(taskId));
    } catch {
      // Without the run state the address can still be typed by hand.
    }
  };
  const commands = () => run()?.services.filter((service) => service.kind === 'command') ?? [];
  /** Where the task's own dev server serves, as the Run tab knows it. */
  const served = () => commands().find((service) => service.state === 'running' && service.address)?.address ?? null;
  /** A dev script that is up and has not yet said where it serves. */
  const waiting = () => commands().find((service) => service.state === 'running' && !service.address) ?? null;
  const address = () => settings.address ?? served();
  /** Every address a service of the task answers at, for the views that ask a service and not a page. */
  const serving = createMemo(
    () =>
      (run()?.services ?? [])
        .filter((service) => service.state === 'running' && service.address && service.web)
        .map((service) => service.address!),
    [] as string[],
    { equals: (a, b) => a.join(' ') === b.join(' ') },
  );
  const parts = createMemo(() => {
    const now = address();
    return now ? splitAddress(now) : null;
  });
  const target = () => parts()?.target ?? null;

  let asked = 0;
  createEffect(
    on([loaded, target, links], ([isLoaded, wanted]) => {
      const mine = ++asked;
      if (!isLoaded) return;
      if (!wanted) {
        setRelay(null);
        setProblem(null);
        return;
      }
      core
        .request<Relay>('preview.open', { taskId, target: wanted, origin: location.origin })
        .then((opened) => {
          if (mine !== asked) return;
          const before = relay();
          setRelay(opened);
          setProblem(null);
          // The same relay now shows another app: the pages in the frames are of the old one.
          if (before && before.port === opened.port && before.target !== opened.target) reloadAll(true);
        })
        .catch((error) => {
          if (mine !== asked) return;
          setRelay(null);
          setProblem(message(error));
        });
    }),
  );

  const origin = () => (relay() ? `http://${frameHost}:${relay()!.port}` : null);
  const pageUrl = () => (origin() && parts() ? origin()! + parts()!.path : null);

  const commitAddress = (text: string) => {
    const typed = text.trim().replace(/^http:\/\//i, '');
    // The dev server's own address, or nothing at all, means "follow the dev server".
    update({ address: typed === '' || typed === served() ? null : typed });
  };

  // ----- widths and themes -----

  const shown = createMemo(() => [...settings.widths].sort((a, b) => a - b));
  const isShown = (width: number) => settings.widths.includes(width);
  const toggleWidth = (width: number) => {
    update({ widths: isShown(width) ? settings.widths.filter((one) => one !== width) : [...settings.widths, width] });
  };
  const addWidth = (text: string) => {
    const width = parseWidth(text);
    if (width === null) {
      setWidthProblem(`Type a width from ${MIN_WIDTH} to ${MAX_WIDTH} pixels.`);
      return;
    }
    const preset = (PRESET_WIDTHS as readonly number[]).includes(width);
    update({
      custom:
        preset || settings.custom.includes(width) ? settings.custom : [...settings.custom, width].sort((a, b) => a - b),
      widths: isShown(width) ? settings.widths : [...settings.widths, width],
    });
    setAdding(false);
    setWidthProblem(null);
  };
  const removeCustom = (width: number) => {
    const themes = { ...settings.themes };
    delete themes[String(width)];
    update({
      custom: settings.custom.filter((one) => one !== width),
      widths: settings.widths.filter((one) => one !== width),
      themes,
    });
  };
  const setOwnTheme = (width: number, theme: Theme | null) => {
    const themes = { ...settings.themes };
    if (theme === null) delete themes[String(width)];
    else themes[String(width)] = theme;
    update({ themes });
  };

  // ----- the frames -----

  const layout = createMemo(() => layoutFrames(shown(), stage().width));
  /** The height of a page in its own pixels: the frame fills what the stage has under the frame's heading and note. */
  const pageHeight = createMemo(() => frameHeight(stage().height, layout().rows.length, layout().scale));

  const post = (width: number, body: Record<string, unknown>) => {
    const to = origin();
    const frame = iframes.get(width)?.contentWindow;
    if (to && frame) frame.postMessage({ source: TO_PAGE, ...body }, to);
  };

  /** Loads the page of a frame anew. A page that answers reloads where it is; any other is sent to the address. */
  const reload = (width: number, fromAddress: boolean) => {
    const frame = iframes.get(width);
    const url = pageUrl();
    if (!frame || !url) return;
    if (frames[String(width)]?.ready && !fromAddress) post(width, { type: 'reload' });
    else frame.src = url;
  };
  const reloadAll = (fromAddress: boolean) => {
    for (const width of shown()) reload(width, fromAddress);
  };

  /** The files of the task changed: frames whose page does not reload itself are reloaded. */
  let changeTimer: ReturnType<typeof setTimeout> | undefined;
  const changedSoon = () => {
    clearTimeout(changeTimer);
    changeTimer = setTimeout(() => {
      let any = false;
      for (const width of shown()) {
        const frame = frames[String(width)];
        if (frame?.hot) continue;
        reload(width, false);
        any = true;
      }
      if (any && pageUrl()) setReloadedAt(Date.now());
    }, 300);
  };

  const onMessage = (event: MessageEvent) => {
    const data = event.data as { source?: string; type?: string; [key: string]: unknown } | null;
    if (!data || data.source !== FROM_PAGE || event.origin !== origin()) return;
    let width: number | undefined;
    for (const [one, frame] of iframes) if (frame.contentWindow === event.source) width = one;
    if (width === undefined) return;
    const key = String(width);
    switch (data.type) {
      case 'ready':
        setFrames(key, (before) => ({
          ...freshFrame(),
          ready: true,
          loads: (before?.loads ?? 0) + 1,
          answers: before?.answers ?? 0,
        }));
        break;
      case 'here':
        setFrames(key, (before) => ({ answers: (before?.answers ?? 0) + 1, silent: false }));
        break;
      case 'info':
        setFrames(key, {
          hot: data.hot === true,
          problem: typeof data.problem === 'string' ? data.problem : null,
          path: typeof data.path === 'string' && data.path.startsWith('/') ? data.path : null,
        });
        break;
      case 'theme':
        setFrames(key, 'report', readThemeReport(data));
        break;
      case 'marked':
        setFrames(key, 'found', data.found === true);
        break;
      case 'picked': {
        const element = readElement(data.element);
        if (!element) break;
        setPicked(taskId, { element, width });
        document.getElementById(`msg-${taskId}`)?.focus();
        break;
      }
      case 'drawn': {
        if (typeof data.d !== 'string' || data.d.length > 200_000) break;
        addDrawing(taskId, width, data.d, readElement(data.element));
        break;
      }
      case 'escape':
        setTool('none');
        break;
      case 'problem':
        // Our script failed at something inside the page. Worth a line for whoever looks into it.
        console.warn(`Preview, ${frameLabel(width)}: ${String(data.text).slice(0, 300)}`);
        break;
    }
  };

  /** What one frame's page is told to show: everything at once, so a page that just loaded is set up in one go. */
  const setupOf = (width: number) => {
    const pointed = pointedOf(taskId);
    const picked = pointed.picked;
    return {
      type: 'setup',
      theme: themeOf(settings, width),
      tool: tool(),
      mark: picked
        ? { selector: picked.element.selector, own: picked.width === width, label: markLabel(picked.element) }
        : null,
      ink: pointed.drawings
        .filter((drawing) => drawing.width === width)
        .map((drawing) => ({ n: drawing.n, d: drawing.d })),
    };
  };

  // ----- screenshots -----

  const loadShotTool = () => {
    core
      .request<{ browser: string | null; needs: string | null }>('shots.state', { taskId })
      .then(setShotTool)
      // An older core that knows no screenshots: the tool stays off, with the reason.
      .catch(() => setShotTool({ browser: null, needs: null }));
  };
  const shotRefused = () =>
    shotRefusal({
      tool: shotTool(),
      takesPictures: props.task.info.config?.images !== false,
      agentName: agentOf(props.task)?.name ?? props.task.info.agentId,
    });
  /** How many more pictures the unsent message takes. */
  const shotRoom = () => MAX_PICTURES - pictureCount(taskId);
  /** The frames on show and the pictures that wait: when either changes, what was said about the last screenshot is old. */
  const shotMoment = () => `${shown().join(' ')}|${pictureCount(taskId)}`;
  /** What the last screenshot came to, while that still describes what is on screen. */
  const lastShot = () => (shotSaid()?.when === shotMoment() ? shotSaid() : null);

  /**
   * Has the core take a picture of the page at each width shown and puts the pictures into the unsent
   * message. The core loads the page anew for it: each frame's own page, in each frame's own theme.
   */
  const takeShots = async (): Promise<string> => {
    const plan = shotPlan(shown(), shotRoom());
    const typed = parts()?.path ?? '/';
    if (plan.take.length === 0) throw new Error(shotWords(shown(), shotRoom(), MAX_PICTURES));
    const answer = await core.request<{ browser: string; shots: unknown }>('shots.take', {
      taskId,
      shots: plan.take.map((width) => ({
        width,
        height: Math.min(pageHeight(), MAX_SHOT_HEIGHT),
        theme: themeOf(settings, width),
        // A frame may have gone on to another page of the app than the one that was typed.
        path: frames[String(width)]?.path ?? typed,
      })),
    });
    const shots = readShots(answer.shots);
    const files = shots.map((shot) => {
      const bytes = Uint8Array.from(atob(shot.data), (letter) => letter.charCodeAt(0));
      return new File([bytes], shotName(shot.width), { type: 'image/png' });
    });
    await addPictures(taskId, files);
    return shotDone(
      shots.map((shot) => shot.width),
      plan.left,
      shots.filter((shot) => shot.stillLoading).map((shot) => shot.width),
      answer.browser,
      MAX_PICTURES,
    );
  };

  /** The Screenshot tool: the pictures wait above the message box, where each can be left out. */
  const shoot = async () => {
    if (shooting() || shotRefused() !== null) return;
    setShooting(true);
    setShotSaid(null);
    try {
      const text = await takeShots();
      setShotSaid({ text, failed: false, when: shotMoment() });
    } catch (error) {
      setShotSaid({ text: message(error), failed: true, when: shotMoment() });
    } finally {
      setShooting(false);
    }
  };

  // ----- starting the project -----

  const start = async (service: string) => {
    if (starting()) return;
    setStarting(service);
    setProblem(null);
    try {
      setRun(await runStart(taskId, service));
    } catch (error) {
      setProblem(message(error));
    } finally {
      setStarting(null);
    }
  };

  onMount(() => {
    void (async () => {
      try {
        const stored = await core.request<{ relay: Relay | null; settings: unknown }>('preview.state', { taskId });
        setSettings(readSettings(stored.settings));
      } catch (error) {
        setProblem(message(error));
      }
      await loadRun();
      setLoaded(true);
    })();
    loadShotTool();
    loadInfra();

    let infraTimer: ReturnType<typeof setTimeout> | undefined;
    const stopEvents = core.onEvent((event) => {
      if (event.event === 'link.open') {
        void loadRun();
        loadShotTool();
        loadInfra();
        setLinks((n) => n + 1);
        return;
      }
      if (event.data?.taskId !== taskId) return;
      if (event.event === 'run.changed') void loadRun();
      if (event.event === 'review.changed' || event.event === 'files.changed') changedSoon();
      // The task may have written the project's first Dockerfile, or removed its last.
      if (event.event === 'files.changed' || event.event === 'git.changed') {
        clearTimeout(infraTimer);
        infraTimer = setTimeout(loadInfra, 1500);
      }
    });
    // A dev script says where it serves a moment after it starts; the app answers a moment after that.
    const timer = setInterval(() => {
      if (waiting() || starting()) void loadRun();
      for (const width of shown()) {
        if (frames[String(width)]?.problem === 'unreachable' && settings.address === null && served())
          reload(width, true);
      }
    }, 1500);
    window.addEventListener('message', onMessage);
    // Escape puts the tool down, wherever the keyboard is: here, or in a page (which passes it on).
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape' && tool() !== 'none') setTool('none');
    };
    window.addEventListener('keydown', onKey);

    onCleanup(() => {
      stopEvents();
      clearInterval(timer);
      clearTimeout(changeTimer);
      clearTimeout(infraTimer);
      window.removeEventListener('message', onMessage);
      window.removeEventListener('keydown', onKey);
      setLive(taskId, null);
      setShooter(taskId, null);
    });
  });

  // The pane's "Pick its parent" goes to the frame the element was picked in.
  createEffect(
    on(
      parentAsked,
      (request) => {
        const picked = pointedOf(taskId).picked;
        if (request?.taskId === taskId && picked) post(picked.width, { type: 'parent' });
      },
      { defer: true },
    ),
  );

  // The pane says "The preview is live" while a page is on show, and how it is kept fresh.
  createEffect(() => {
    const widths = shown();
    const showing = webOn() && pageUrl() !== null && widths.length > 0;
    const allHot = widths.every((width) => frames[String(width)]?.hot);
    setLive(taskId, showing ? (allHot ? 'itself' : 'reloaded') : null);
  });

  // While a page is on show, the conversation can ask for its picture at the moment a message is sent.
  createEffect(() => {
    const showing = webOn() && pageUrl() !== null && shown().length > 0;
    setShooter(
      taskId,
      showing
        ? {
            refusal: shotRefused,
            words: () => shotWords(shown(), shotRoom(), MAX_PICTURES),
            take: async () => {
              await takeShots();
            },
          }
        : null,
    );
  });

  /** What is said about the screenshot next to its tool: why there is none, what the last one came to, or what one is. */
  const shotLine = () => {
    if (shotRefused()) return shotRefused()!;
    if (shooting()) return 'Loading the page anew at each width to take its picture.';
    return lastShot()?.text ?? shotWords(shown(), shotRoom(), MAX_PICTURES);
  };

  const hint = () => {
    if (tool() === 'pick') return 'Click anything in the page. The pick is marked on every width.';
    if (tool() === 'draw') return 'Draw on the page of one width. The drawing goes along with your next message.';
    return 'Pick an element or draw on the page to talk about it.';
  };

  /** What stands in the place of the page when no app is running: how to start one. */
  const NothingRuns = (): JSX.Element => (
    <div class="center-note">
      <Show
        when={waiting()}
        fallback={
          <>
            <h2>Nothing is running</h2>
            <Show
              when={commands().length > 0}
              fallback={
                <p>
                  This project has no dev script in package.json. Start the app in the terminal, then type its address
                  above.
                </p>
              }
            >
              <p>Start the project here, or type the address of an app that runs already.</p>
              <div class="actions">
                <For each={commands().slice(0, 3)}>
                  {(service, index) => (
                    <button
                      type="button"
                      class="btn tall"
                      classList={{ primary: index() === 0 }}
                      title={service.runs}
                      disabled={starting() !== null}
                      onClick={() => void start(service.name)}
                    >
                      {starting() === service.name ? `Starting ${service.name}` : `Start ${service.name}`}
                    </button>
                  )}
                </For>
              </div>
            </Show>
          </>
        }
      >
        {(service) => (
          <>
            <h2>{service().name} is starting</h2>
            <p>The page shows as soon as it says where it serves. Its output is in the Run tab.</p>
          </>
        )}
      </Show>
    </div>
  );

  function Frame(frameProps: { width: number }): JSX.Element {
    // eslint-disable-next-line solid/reactivity -- a frame is made anew for another width, so the width never changes under it
    const width = frameProps.width;
    const key = String(width);
    const name = `${deviceName(width)} ${width}`;
    const theme = () => themeOf(settings, width);
    const own = () => settings.themes[key] !== undefined;
    const state = () => frames[key];
    const scale = () => layout().scale;
    // A frame is named after its theme when it is made, so the page's script knows the theme before
    // the page's first script runs. Later changes travel as messages, and the script renames the frame.
    const firstName = `ae-preview:${themeOf(settings, width)}`;

    if (!frames[key]) setFrames(key, freshFrame());
    onCleanup(() => {
      iframes.delete(width);
      setFrames(key, freshFrame());
    });

    // Whatever this frame should show is sent whole whenever any of it changes, and to every page that loads.
    createEffect(() => {
      const now = state();
      if (!now?.ready) return;
      void now.loads;
      post(width, setupOf(width));
    });

    // Whatever loaded is asked whether our script is in it. A page that does not answer
    // (a file that is not a page, a page of another site) cannot be themed, picked in or drawn on.
    const onLoad = () => {
      const answers = state()?.answers ?? 0;
      post(width, { type: 'hello' });
      setTimeout(() => {
        if ((state()?.answers ?? 0) === answers) setFrames(key, { ready: false, silent: true });
      }, 1200);
    };

    const note = () => {
      const now = state();
      if (!now) return null;
      if (now.silent) return 'This page did not load our script, so themes, picking and drawing do not work in it.';
      if (now.problem) return null;
      const picked = pointedOf(taskId).picked;
      if (picked && picked.width !== width && now.found === false) return 'The picked element is not on this width.';
      return themeNote(theme(), now.ready, now.report);
    };

    return (
      <section
        class="preview-frame"
        aria-label={name}
        style={{ width: `${Math.floor((width + 2 * BEZEL) * scale())}px` }}
      >
        <div class="preview-frame-head">
          <h3 class="grow">
            {deviceName(width)} <span class="mono">{width}</span>
          </h3>
          <Show when={own()} fallback={<span class="fine preview-follows">Follows the theme for all</span>}>
            <button type="button" class="btn tiny ghost" onClick={() => setOwnTheme(width, null)}>
              Same as all
            </button>
          </Show>
          <div class="preview-theme" role="group" aria-label={`Theme of ${frameLabel(width)}`}>
            <button
              type="button"
              aria-pressed={theme() === 'light'}
              aria-label={`Light theme on ${frameLabel(width)} only`}
              onClick={() => setOwnTheme(width, 'light')}
            >
              <Sun />
            </button>
            <button
              type="button"
              aria-pressed={theme() === 'dark'}
              aria-label={`Dark theme on ${frameLabel(width)} only`}
              onClick={() => setOwnTheme(width, 'dark')}
            >
              <Moon />
            </button>
          </div>
        </div>
        <Show when={note()}>
          <p class="fine preview-frame-note">{note()}</p>
        </Show>
        <div
          class="preview-device-box"
          style={{
            width: `${Math.floor((width + 2 * BEZEL) * scale())}px`,
            height: `${Math.floor((pageHeight() + 2 * BEZEL) * scale())}px`,
          }}
        >
          <div
            class="preview-device"
            style={{
              width: `${width + 2 * BEZEL}px`,
              height: `${pageHeight() + 2 * BEZEL}px`,
              'border-radius': `${cornerRadius(width)}px`,
              transform: `scale(${scale()})`,
            }}
          >
            <iframe
              ref={(el) => iframes.set(width, el)}
              title={`${name}: the page`}
              name={firstName}
              src={pageUrl() ?? 'about:blank'}
              // The page may do what an app does, except take the whole window away from the interface.
              sandbox="allow-scripts allow-same-origin allow-forms allow-modals allow-popups allow-popups-to-escape-sandbox allow-downloads"
              allow="clipboard-read; clipboard-write; fullscreen"
              style={{ width: `${width}px`, height: `${pageHeight()}px`, 'color-scheme': theme() }}
              onLoad={onLoad}
            />
          </div>
        </div>
      </section>
    );
  }

  return (
    <section class="center preview" aria-label="Preview">
      <Deploys taskId={taskId} />
      <div class="preview-bar">
        <div class="switcher" role="group" aria-label="What to preview on">
          <button
            type="button"
            aria-pressed={!isData()}
            title="The running app: as a page, on the iOS simulator and on the Android emulator, side by side"
            onClick={() => setKind('app')}
          >
            App
          </button>
          <button type="button" aria-pressed={kind() === 'api'} onClick={() => setKind('api')}>
            API
          </button>
          <button type="button" aria-pressed={kind() === 'db'} onClick={() => setKind('db')}>
            Database
          </button>
          <button type="button" aria-pressed={kind() === 'jobs'} onClick={() => setKind('jobs')}>
            Jobs
          </button>
          <Show when={hasInfra() || kind() === 'infra'}>
            <button type="button" aria-pressed={kind() === 'infra'} onClick={() => setKind('infra')}>
              Infra
            </button>
          </Show>
        </div>
        <Show when={!isData()}>
          <div class="switcher several" role="group" aria-label="Where the app is shown">
            <For
              each={
                [
                  ['web', 'Web'],
                  ['ios', 'iOS simulator'],
                  ['android', 'Android emulator'],
                ] as const
              }
            >
              {([which, words]) => (
                <button
                  type="button"
                  aria-pressed={screens().includes(which)}
                  title={
                    screens().includes(which)
                      ? screens().length > 1
                        ? `Take ${words} away; the rest stays`
                        : 'The only one on show: turn another on to compare'
                      : `Show ${words} beside what is on show`
                  }
                  onClick={() => toggleScreen(which)}
                >
                  {words}
                </button>
              )}
            </For>
          </div>
        </Show>
        <Show when={webOn() && loaded()}>
          <div class="preview-widths" role="group" aria-label="Widths">
            <span class="fine">Widths</span>
            <For each={PRESET_WIDTHS}>
              {(width) => (
                <button
                  type="button"
                  class="preview-chip mono"
                  aria-pressed={isShown(width)}
                  title={`${deviceName(width)}, ${width} pixels wide`}
                  onClick={() => toggleWidth(width)}
                >
                  {width}
                </button>
              )}
            </For>
            <For each={settings.custom}>
              {(width) => (
                <span class="preview-chip custom" classList={{ on: isShown(width) }}>
                  <button
                    type="button"
                    class="mono"
                    aria-pressed={isShown(width)}
                    title={`Your width, ${width} pixels`}
                    onClick={() => toggleWidth(width)}
                  >
                    {width}
                  </button>
                  <button type="button" aria-label={`Remove your width ${width}`} onClick={() => removeCustom(width)}>
                    <Icon.Close />
                  </button>
                </span>
              )}
            </For>
            <Show
              when={adding()}
              fallback={
                <button
                  type="button"
                  class="preview-chip add"
                  onClick={() => {
                    setAdding(true);
                    queueMicrotask(() => widthInput?.focus());
                  }}
                >
                  <Icon.Plus />
                  Add width
                </button>
              }
            >
              <form
                class="preview-add"
                onSubmit={(event) => {
                  event.preventDefault();
                  addWidth(widthInput?.value ?? '');
                }}
              >
                <input
                  ref={widthInput}
                  class="mono"
                  type="text"
                  inputmode="numeric"
                  aria-label="Your width in pixels"
                  placeholder="414"
                  onKeyDown={(event) => {
                    if (event.key === 'Escape') {
                      setAdding(false);
                      setWidthProblem(null);
                    }
                  }}
                />
                <button type="submit" class="btn tiny">
                  Add
                </button>
              </form>
            </Show>
          </div>
          <div class="preview-all">
            <span class="fine">Theme for all</span>
            <div class="switcher" role="group" aria-label="Theme for all">
              <button
                type="button"
                aria-pressed={settings.theme === 'light'}
                onClick={() => update({ theme: 'light' })}
              >
                <Sun />
                Light
              </button>
              <button type="button" aria-pressed={settings.theme === 'dark'} onClick={() => update({ theme: 'dark' })}>
                <Moon />
                Dark
              </button>
            </div>
          </div>
          <span class="grow" />
          <Show when={pageUrl() && shown().length > 0}>
            <span class="fine">{scaleLabel(layout().scale)}</span>
          </Show>
        </Show>
        <Show when={loaded() && !isData()}>
          <Show when={!webOn()}>
            <span class="grow" />
          </Show>
          <span class="preview-address">
            <input
              class="mono"
              type="text"
              aria-label="Address of the running app"
              placeholder="localhost:3000"
              spellcheck={false}
              value={address() ?? ''}
              onChange={(event) => commitAddress(event.currentTarget.value)}
            />
            {/* One press for all that is on show: the frames are loaded anew, a device opens the address again. */}
            <button
              type="button"
              aria-label={webOn() ? 'Reload the preview' : 'Open the address again'}
              title={
                !webOn()
                  ? 'Open the address on the device again'
                  : devicesOn().length > 0
                    ? 'Load every frame anew, and open the address on the devices again'
                    : 'Load every frame anew'
              }
              disabled={webOn() && devicesOn().length === 0 ? !pageUrl() : !address()}
              onClick={() => {
                if (webOn() && pageUrl()) reloadAll(false);
                if (devicesOn().length > 0) setDeviceReloads((n) => n + 1);
              }}
            >
              <ReloadIcon />
            </button>
          </span>
        </Show>
      </div>
      <Show when={webOn() && loaded()}>
        <div class="preview-bar tools">
          <div class="switcher" role="group" aria-label="Tools">
            <button
              type="button"
              aria-pressed={tool() === 'pick'}
              disabled={!pageUrl()}
              onClick={() => setTool(tool() === 'pick' ? 'none' : 'pick')}
            >
              <PickIcon />
              Pick an element
            </button>
            <button
              type="button"
              aria-pressed={tool() === 'draw'}
              disabled={!pageUrl()}
              onClick={() => setTool(tool() === 'draw' ? 'none' : 'draw')}
            >
              <DrawIcon />
              Draw on the page
            </button>
            <button
              type="button"
              disabled={
                !pageUrl() || shotRefused() !== null || shooting() || shotPlan(shown(), shotRoom()).take.length === 0
              }
              aria-busy={shooting()}
              title={shotRefused() ?? shotWords(shown(), shotRoom(), MAX_PICTURES)}
              onClick={() => void shoot()}
            >
              <ShotIcon />
              {shooting() ? 'Taking a screenshot' : 'Screenshot'}
            </button>
          </div>
          <span class="fine grow">{hint()}</span>
          <span
            class="fine preview-shot-line"
            classList={{ failed: lastShot()?.failed === true && !shooting() && !shotRefused() }}
            role="status"
            aria-label="About the screenshot"
          >
            {shotLine()}
          </span>
        </div>
      </Show>
      <Show when={!isData() && (widthProblem() ?? problem())}>
        <p class="banner danger flush" role="alert">
          <span class="grow">{widthProblem() ?? problem()}</span>
        </p>
      </Show>
      <Show when={kind() === 'api'}>
        <ApiView taskId={taskId} guess={serving()} />
      </Show>
      <Show when={kind() === 'db'}>
        <DatabaseView taskId={taskId} />
      </Show>
      <Show when={kind() === 'jobs'}>
        <JobsView taskId={taskId} services={run()?.services ?? []} />
      </Show>
      <Show when={kind() === 'infra'}>
        <InfraView taskId={taskId} />
      </Show>

      <div class="preview-stage" hidden={isData()}>
        <Show when={loaded()} fallback={<p class="bottom-note">Reading the preview of this task</p>}>
          <Show when={!isData()}>
            {/* What is on show stands side by side: the page first, then each device in a column of its own. */}
            <div class="preview-panes">
              <Show when={webOn()}>
                <div class="preview-pane web" ref={watchStage}>
                  <Switch>
                    <Match when={!address()}>
                      <NothingRuns />
                    </Match>
                    <Match when={!pageUrl()}>
                      <Show when={!problem()}>
                        <p class="bottom-note">Opening the preview</p>
                      </Show>
                    </Match>
                    <Match when={shown().length === 0}>
                      <div class="center-note">
                        <h2>No width is chosen</h2>
                        <p>Pick a width above to see the page.</p>
                      </div>
                    </Match>
                    <Match when={true}>
                      <div class="preview-frames">
                        <For each={shown()}>{(width) => <Frame width={width} />}</For>
                      </div>
                    </Match>
                  </Switch>
                </div>
              </Show>
              <For each={devicesOn()}>
                {(device) => (
                  <div
                    class="preview-pane"
                    role="group"
                    aria-label={device === 'ios' ? 'iOS simulator' : 'Android emulator'}
                  >
                    <DeviceView taskId={taskId} kind={device} address={address()} reloads={deviceReloads()}>
                      {/* How to start the app is said once, by the page beside; alone, a device says it itself. */}
                      <Show when={webOn()} fallback={<NothingRuns />}>
                        <div class="center-note">
                          <h2>{device === 'ios' ? 'iOS simulator' : 'Android emulator'}</h2>
                          <p>Nothing is running yet. It opens the app here once one runs.</p>
                        </div>
                      </Show>
                    </DeviceView>
                  </div>
                )}
              </For>
            </div>
          </Show>
        </Show>
      </div>
      <Show when={webOn() && pageUrl() && shown().length > 0}>
        <div class="statusline">
          <span>
            {joinAddress(relay()!.target, parts()!.path)}
            {settings.address === null ? ', the dev server of this task' : ''}
          </span>
          <span>
            {shown().every((width) => frames[String(width)]?.hot)
              ? 'The page reloads itself when the code changes.'
              : 'Reloads when the code changes.'}
          </span>
          <Show when={reloadedAt()}>{(at) => <span>Reloaded at {clock(at())} after a change.</span>}</Show>
        </div>
      </Show>
    </section>
  );
}
