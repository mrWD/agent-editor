/**
 * The plain logic of the Preview view: which widths there are and what they
 * are called, how the frames are laid out and scaled, how an address is read,
 * and how what the user pointed at is put into words for the agent.
 * Nothing here touches the screen or the core, so all of it is tested as is.
 */

export type Theme = 'light' | 'dark';

/** The widths offered without asking. The first two are shown on a first visit. */
export const PRESET_WIDTHS = [320, 768, 1024, 1200, 1600] as const;
export const DEFAULT_WIDTHS = [320, 768];
export const MIN_WIDTH = 200;
export const MAX_WIDTH = 3840;

/** The frame around a page, on each side, in the page's own pixels. */
export const BEZEL = 10;
/** The space between two frames, on screen. */
export const GAP = 24;
/** Frames in one row are never drawn smaller than this; past it they go to a second row. */
export const MIN_ROW_SCALE = 0.33;
/** A page is never shown shorter than this, in its own pixels. */
export const MIN_FRAME_HEIGHT = 480;

/** What a width is called: the kind of device that is about that wide. */
export function deviceName(width: number): string {
  if (width < 600) return 'Phone';
  if (width < 1024) return 'Tablet';
  if (width < 1200) return 'Laptop';
  if (width < 1600) return 'Desktop';
  return 'Wide';
}

/** "phone 320": how a frame is named inside a sentence. */
export function frameLabel(width: number): string {
  return `${deviceName(width).toLowerCase()} ${width}`;
}

/** How round the corners of a device are, in the page's own pixels. */
export function cornerRadius(width: number): number {
  if (width < 600) return 36;
  if (width < 1024) return 28;
  return 14;
}

/** Reads a width the user typed. `null` when it is not a width that can be shown. */
export function parseWidth(text: string): number | null {
  const trimmed = text.trim().replace(/px$/i, '').trim();
  if (!/^\d+$/.test(trimmed)) return null;
  const width = Number(trimmed);
  return width >= MIN_WIDTH && width <= MAX_WIDTH ? width : null;
}

/** What the view remembers per task. */
export type PreviewSettings = {
  /** The address the user typed. `null` follows the task's own dev server. */
  address: string | null;
  /** The widths shown, smallest first. */
  widths: number[];
  /** Widths of the user's own, shown or not. */
  custom: number[];
  /** The theme for all frames. */
  theme: Theme;
  /** Frames with a theme of their own, by width. */
  themes: Record<string, Theme>;
};

export function defaultSettings(): PreviewSettings {
  return { address: null, widths: [...DEFAULT_WIDTHS], custom: [], theme: 'light', themes: {} };
}

const isTheme = (value: unknown): value is Theme => value === 'light' || value === 'dark';
const isWidth = (value: unknown): value is number =>
  typeof value === 'number' && Number.isInteger(value) && value >= MIN_WIDTH && value <= MAX_WIDTH;
const ascending = (list: number[]) => [...new Set(list)].sort((a, b) => a - b);

/** Makes whatever was stored into settings that can be shown: wrong or missing parts fall back to the defaults. */
export function readSettings(stored: unknown): PreviewSettings {
  const settings = defaultSettings();
  if (!stored || typeof stored !== 'object') return settings;
  const raw = stored as Record<string, unknown>;
  if (typeof raw.address === 'string' && raw.address.trim()) settings.address = raw.address.trim();
  if (Array.isArray(raw.custom)) {
    settings.custom = ascending(raw.custom.filter(isWidth)).filter(
      (width) => !(PRESET_WIDTHS as readonly number[]).includes(width),
    );
  }
  if (Array.isArray(raw.widths)) {
    const known = new Set<number>([...PRESET_WIDTHS, ...settings.custom]);
    settings.widths = ascending(raw.widths.filter(isWidth)).filter((width) => known.has(width));
  }
  if (isTheme(raw.theme)) settings.theme = raw.theme;
  if (raw.themes && typeof raw.themes === 'object') {
    for (const [width, theme] of Object.entries(raw.themes as Record<string, unknown>)) {
      if (isTheme(theme) && isWidth(Number(width))) settings.themes[width] = theme;
    }
  }
  return settings;
}

/** The theme one frame shows: its own when it has one, else the theme for all. */
export function themeOf(settings: PreviewSettings, width: number): Theme {
  return settings.themes[String(width)] ?? settings.theme;
}

/** An address split into the app it names and the page of that app. */
export type Address = {
  /** The app, as the core takes it: `localhost:3000`. */
  target: string;
  /** The page: `/login?next=1`. Always starts with a slash. */
  path: string;
};

/**
 * Reads an address as people type it: `localhost:3000/login`, `http://127.0.0.1:5173`.
 * `null` when there is nothing to read. Whether the app is on this computer is the core's call.
 */
export function splitAddress(text: string): Address | null {
  const trimmed = text.trim();
  if (!trimmed) return null;
  const scheme = /^[a-z][a-z0-9+.-]*:\/\//i.exec(trimmed)?.[0] ?? '';
  const rest = trimmed.slice(scheme.length);
  const cut = rest.search(/[/?#]/);
  const authority = cut < 0 ? rest : rest.slice(0, cut);
  if (!authority) return null;
  const after = cut < 0 ? '' : rest.slice(cut);
  // A scheme other than http is passed on, so the core can say why it is refused.
  const target = scheme && scheme.toLowerCase() !== 'http://' ? scheme + authority : authority;
  return { target, path: after.startsWith('/') ? after : `/${after}` };
}

/** The address as it is shown: the app, and the page when it is not the first one. */
export function joinAddress(target: string, path: string): string {
  return path === '/' || path === '' ? target : target + path;
}

/** How the frames are put on the stage. */
export type Layout = {
  /** How much smaller than life the frames are drawn: 1 is full size. */
  scale: number;
  /** The widths of each row, in order. One row unless they would get too small. */
  rows: number[][];
};

/** The room a row of frames takes at full size: the pages, their bezels and the gaps (the gaps do not shrink). */
function rowNeeds(widths: number[]): { pages: number; gaps: number } {
  return {
    pages: widths.reduce((sum, width) => sum + width + 2 * BEZEL, 0),
    gaps: GAP * Math.max(0, widths.length - 1),
  };
}

/** The largest scale, at most 1, at which a row fits into `available` pixels. */
function scaleToFit(widths: number[], available: number): number {
  const { pages, gaps } = rowNeeds(widths);
  if (pages <= 0) return 1;
  return Math.max(0.05, Math.min(1, (available - gaps) / pages));
}

/**
 * Lays out the frames so the view never scrolls sideways. They stand in one row,
 * scaled down to fit. A row is not scaled below a third, where a page can no
 * longer be read: past that the frames keep that scale and wrap to more rows.
 * A frame too wide for the stage even at a third is scaled to fit by itself.
 */
export function layoutFrames(widths: number[], available: number): Layout {
  if (widths.length === 0) return { scale: 1, rows: [] };
  const oneRow = scaleToFit(widths, available);
  if (oneRow >= MIN_ROW_SCALE || widths.length === 1) return { scale: oneRow, rows: [widths] };
  const scale = Math.min(MIN_ROW_SCALE, scaleToFit([Math.max(...widths)], available));
  const rows: number[][] = [];
  let row: number[] = [];
  for (const width of widths) {
    const { pages, gaps } = rowNeeds([...row, width]);
    if (row.length > 0 && pages * scale + gaps > available) {
      rows.push(row);
      row = [];
    }
    row.push(width);
  }
  rows.push(row);
  return { scale, rows };
}

/** "Shown at 80%". */
export function scaleLabel(scale: number): string {
  return `Shown at ${Math.round(scale * 100)}%`;
}

/** The room on screen a frame keeps above its page for its heading and a line of notes. */
export const FRAME_HEAD = 64;

/**
 * How tall a page is shown, in its own pixels, so that the frames fill the
 * height of the stage: one row takes all of it, two rows or more take half each.
 */
export function frameHeight(stageHeight: number, rows: number, scale: number): number {
  const onScreen = Math.min(Math.max(rows, 1), 2);
  const forPage = (stageHeight - GAP * (onScreen - 1)) / onScreen - FRAME_HEAD;
  return Math.max(MIN_FRAME_HEIGHT, Math.floor(forPage / scale) - 2 * BEZEL);
}

/** What the page's script says about an element. */
export type PickedElement = {
  tag: string;
  id: string | null;
  classes: string[];
  role: string | null;
  /** What it reads, shortened. */
  text: string;
  width: number;
  height: number;
  disabled: boolean;
  /** A CSS selector that finds it again. */
  selector: string;
  /** The components it is inside, outermost first. Empty when the page leaves no trace. */
  components: string[];
  /** The file it was written in, when the page says so. */
  file: string | null;
  line: number | null;
  column: number | null;
  /** Where the file came from: a framework's notes or an attribute a build plugin wrote. */
  from: 'react' | 'vue' | 'svelte' | 'attribute' | null;
  /** The file is that of an element around this one, not of this one itself. */
  nearby: boolean;
};

/** Makes what a page sent into an element that can be shown. `null` when it is not one. */
export function readElement(raw: unknown): PickedElement | null {
  if (!raw || typeof raw !== 'object') return null;
  const data = raw as Record<string, unknown>;
  if (typeof data.tag !== 'string' || typeof data.selector !== 'string' || !data.selector) return null;
  const text = (value: unknown, limit: number) => (typeof value === 'string' ? value.slice(0, limit) : '');
  const count = (value: unknown) => (typeof value === 'number' && Number.isFinite(value) && value > 0 ? value : null);
  const words = (value: unknown) =>
    Array.isArray(value)
      ? value.filter((word): word is string => typeof word === 'string').map((w) => w.slice(0, 80))
      : [];
  const from = data.from;
  return {
    tag: text(data.tag, 40).toLowerCase(),
    id: text(data.id, 120) || null,
    classes: words(data.classes).slice(0, 3),
    role: text(data.role, 40) || null,
    text: text(data.text, 120),
    width: Math.round(count(data.width) ?? 0),
    height: Math.round(count(data.height) ?? 0),
    disabled: data.disabled === true,
    selector: text(data.selector, 600),
    components: words(data.components).slice(0, 4),
    file: text(data.file, 600) || null,
    line: count(data.line),
    column: count(data.column),
    from: from === 'react' || from === 'vue' || from === 'svelte' || from === 'attribute' ? from : null,
    nearby: data.nearby === true,
  };
}

/** An element the user picked, and the frame it was picked in. */
export type Picked = { element: PickedElement; width: number };

/** A mark the user drew on the page of one frame. */
export type Drawing = {
  /** Its number, as drawn next to it. */
  n: number;
  /** The frame it was drawn in. */
  width: number;
  /** The stroke, as an SVG path in the page's own coordinates. */
  d: string;
  /** The element it is around, when there is one under it. */
  element: PickedElement | null;
  /** What the user wrote about it. */
  note: string;
};

/**
 * The path of a file as the task knows it: from the task's folder, with forward slashes.
 * Build tools write full paths, paths from the project folder, or paths that start with a slash.
 */
export function sourcePath(file: string, cwd: string): string {
  const path = file.replace(/\\/g, '/').replace(/^file:\/\//, '');
  const root = cwd.replace(/\\/g, '/').replace(/\/+$/, '');
  if (root && path.startsWith(`${root}/`)) return path.slice(root.length + 1);
  return path.replace(/^\.\//, '');
}

/** Whether a source path is inside the task's folder, so the editor can open it. */
export function canOpen(path: string): boolean {
  return path !== '' && !/^([a-zA-Z]:)?\//.test(path) && !path.split('/').includes('..');
}

/** "src/components/LoginForm.tsx, line 42", or `null` when the page left no trace of a file. */
export function sourceLabel(element: PickedElement, cwd: string): string | null {
  if (!element.file) return null;
  const path = sourcePath(element.file, cwd);
  return element.line ? `${path}, line ${element.line}` : path;
}

/** "LoginPage › LoginForm › SubmitButton", or `null` when no component is known. */
export function componentLabel(element: PickedElement): string | null {
  return element.components.length > 0 ? element.components.join(' › ') : null;
}

/** The short name of an element: its component when known, else its tag. */
export function elementName(element: PickedElement): string {
  return element.components.at(-1) ?? `<${element.tag}>`;
}

/** "SubmitButton · LoginForm.tsx:42": what is written next to the picked element in the page. */
export function markLabel(element: PickedElement): string {
  const mark = element.id ? `#${element.id}` : element.classes[0] ? `.${element.classes[0]}` : '';
  const name = element.components.at(-1) ?? element.tag + mark;
  if (!element.file || element.nearby) return name;
  const file = element.file.replace(/\\/g, '/').split('/').pop() ?? element.file;
  return `${name} · ${file}${element.line ? `:${element.line}` : ''}`;
}

/** "280 by 46, disabled". */
export function sizeLabel(element: PickedElement): string {
  return `${element.width} by ${element.height}${element.disabled ? ', disabled' : ''}`;
}

/** `<button> that reads "Try again"`: an element in a sentence. */
function inWords(element: PickedElement): string {
  const reads = element.text ? ` that reads "${element.text}"` : '';
  return `<${element.tag}>${reads}`;
}

/** Where an element is, for the agent: its selector, its component and its file, each only when known. */
function whereLines(element: PickedElement, cwd: string): string[] {
  const lines = [`- selector: \`${element.selector}\``];
  const component = componentLabel(element);
  if (component) lines.push(`- component: ${component}`);
  if (element.file) {
    const path = sourcePath(element.file, cwd);
    const place = element.line ? `\`${path}\`, line ${element.line}` : `\`${path}\``;
    lines.push(
      element.nearby
        ? `- source: no file is known for the element itself; it is inside ${place}`
        : `- source: ${place}`,
    );
  } else {
    lines.push('- source: no file is known');
  }
  return lines;
}

/** What goes in front of the user's message about a picked element. */
export function describePicked(picked: Picked, cwd: string): string {
  const { element } = picked;
  return [
    `About an element picked in the preview at ${picked.width} wide (${deviceName(picked.width).toLowerCase()}):`,
    `- it is a ${inWords(element)}, ${sizeLabel(element)}`,
    ...whereLines(element, cwd),
  ].join('\n');
}

/** What goes in front of the user's message about a mark they drew. Agents get words, not pictures. */
export function describeDrawing(drawing: Drawing, cwd: string): string {
  const note = drawing.note.trim();
  const said = note ? ` Their note: "${note}".` : '';
  if (!drawing.element) {
    return `The user marked an area of the page at ${drawing.width} wide (drawing ${drawing.n}); no element was under it.${said}`;
  }
  const head = `The user marked the area around the ${inWords(drawing.element)} at ${drawing.width} wide (drawing ${drawing.n}).${said}`;
  return [head, ...whereLines(drawing.element, cwd)].join('\n');
}

/**
 * Something of the Preview that is text already and that the user wants the agent to see:
 * the answer of a request, the rows of a query, what a migration did.
 */
export type Shown = {
  n: number;
  /** What it is, in a line: "POST /orders answered 500". */
  title: string;
  text: string;
  /** What the user says is wrong with it. */
  note: string;
};

function describeShown(shown: Shown): string {
  const said = shown.note.trim() ? ` They say: "${shown.note.trim()}"` : '';
  return `The user shows you this from the preview: ${shown.title}.${said}\n\n\`\`\`\n${shown.text.replace(/\n$/, '')}\n\`\`\``;
}

/** Everything the user pointed at in the preview, as the start of a message. Empty when there is nothing. */
export function describePointed(picked: Picked | null, drawings: Drawing[], cwd: string, shown: Shown[] = []): string {
  const parts = drawings.map((drawing) => describeDrawing(drawing, cwd));
  if (picked) parts.unshift(describePicked(picked, cwd));
  parts.push(...shown.map(describeShown));
  return parts.join('\n\n');
}

/** What a page's script says about the theme it shows. */
export type ThemeReport = {
  theme: Theme | null;
  /** 'browser': the browser shows it by itself. 'rewritten': the page's styles were rewritten. */
  how: 'browser' | 'rewritten' | 'system';
  /** How many rules of the page depend on the system's theme. */
  media: number;
  /** How many places let the browser choose between a light and a dark colour scheme. */
  schemes: number;
  /** Whether the page's scripts asked for the system's theme. */
  scripts: boolean;
  /** Style sheets from other sites, which cannot be read or rewritten. */
  unreadable: number;
};

export function readThemeReport(raw: unknown): ThemeReport | null {
  if (!raw || typeof raw !== 'object') return null;
  const data = raw as Record<string, unknown>;
  const count = (value: unknown) => (typeof value === 'number' && value > 0 ? Math.round(value) : 0);
  return {
    theme: isTheme(data.theme) ? data.theme : null,
    how: data.how === 'browser' || data.how === 'rewritten' ? data.how : 'system',
    media: count(data.media),
    schemes: count(data.schemes),
    scripts: data.scripts === true,
    unreadable: count(data.unreadable),
  };
}

/**
 * What to say under a frame about its theme, or `null` when the theme is simply shown.
 * `ready` is whether the page's script has spoken; `report` is its last word on the theme.
 */
export function themeNote(asked: Theme, ready: boolean, report: ThemeReport | null): string | null {
  const name = asked === 'dark' ? 'Dark' : 'Light';
  if (!ready) return null;
  if (!report || report.theme !== asked) return `${name} is not set on this page yet.`;
  if (report.how === 'rewritten' && report.unreadable > 0) {
    const sheets = report.unreadable === 1 ? 'One style sheet' : `${report.unreadable} style sheets`;
    return `${name} is set, except in what comes from other sites: ${sheets.toLowerCase()} could not be changed.`;
  }
  if (report.media === 0 && report.schemes === 0 && !report.scripts && report.unreadable === 0) {
    return 'This page has no styles for Light and Dark, so it looks the same in both.';
  }
  return null;
}

// ---------- screenshots ----------

/** The highest page a screenshot can be asked of; the core says the same. */
export const MAX_SHOT_HEIGHT = 4320;

/** "screenshot, phone 320.png": what a picture of a frame is called in a message. */
export function shotName(width: number): string {
  return `screenshot, ${frameLabel(width)}.png`;
}

/** "phone 320", "phone 320 and tablet 768", "phone 320, tablet 768 and laptop 1024". */
export function listed(names: string[]): string {
  if (names.length <= 1) return names.join('');
  return `${names.slice(0, -1).join(', ')} and ${names[names.length - 1]}`;
}

/**
 * Which frames get a picture when the message has room for `room` more:
 * the narrowest first, which is the order they stand in.
 */
export function shotPlan(widths: number[], room: number): { take: number[]; left: number[] } {
  const ascending = [...widths].sort((a, b) => a - b);
  const fits = Math.max(0, room);
  return { take: ascending.slice(0, fits), left: ascending.slice(fits) };
}

/**
 * What a screenshot is, said wherever one is offered. It is not the frames as they stand:
 * the core loads the page anew in a browser that starts empty.
 */
export const SHOT_IS_FRESH = 'It is the page loaded anew: not signed in, and without what you typed into the frames.';

/** Why no screenshot can be taken, in words for the user. `null` when one can. */
export function shotRefusal(known: {
  /** What the core said about the browser; `null` while it has not said. */
  tool: { browser: string | null; needs: string | null } | null;
  takesPictures: boolean;
  agentName: string;
}): string | null {
  // The same words the button for any picture has.
  if (!known.takesPictures) return `${known.agentName} does not take pictures`;
  if (!known.tool) return 'Looking for a browser to take screenshots with.';
  if (!known.tool.browser) return known.tool.needs ?? 'A screenshot needs Chrome, Chromium or Edge on this computer.';
  return null;
}

/** What is said about the frames a screenshot will be of, before it is taken. */
export function shotWords(widths: number[], room: number, most: number): string {
  const { take, left } = shotPlan(widths, room);
  if (take.length === 0)
    return `A message takes ${most} pictures at most. Leave one out to make room for a screenshot.`;
  const of = listed(take.map(frameLabel));
  if (left.length === 0) return `A screenshot of ${of}. ${SHOT_IS_FRESH}`;
  return `A screenshot of ${of} only: a message takes ${most} pictures at most. ${SHOT_IS_FRESH}`;
}

/** What is said once the pictures are in the message. `loading` are the frames whose page had not finished loading. */
export function shotDone(taken: number[], left: number[], loading: number[], browser: string, most: number): string {
  const many = taken.length === 1 ? 'A screenshot is' : `${taken.length} screenshots are`;
  const parts = [`${many} in your message: ${listed(taken.map(frameLabel))}, loaded anew in ${browser}.`];
  if (left.length > 0) {
    parts.push(`${listed(left.map(frameLabel))} did not fit: a message takes ${most} pictures at most.`);
  }
  if (loading.length > 0) {
    parts.push(`The page was still loading on ${listed(loading.map(frameLabel))} when its picture was taken.`);
  }
  return parts.join(' ');
}

/** A picture as the core sends it. */
export type Shot = { width: number; data: string; stillLoading: boolean };

/** Reads the pictures the core took. Anything that is not a picture of a width is left out. */
export function readShots(raw: unknown): Shot[] {
  if (!Array.isArray(raw)) return [];
  return raw.flatMap((one: unknown) => {
    const shot = one as Record<string, unknown> | null;
    if (!shot || typeof shot.width !== 'number' || typeof shot.data !== 'string' || !shot.data) return [];
    return [{ width: shot.width, data: shot.data, stillLoading: shot.stillLoading === true }];
  });
}

// ---------- the iOS simulator and the Android emulator ----------

export type DeviceKind = 'ios' | 'android';

/** A simulator or an emulator as the core lists it. */
export type Device = {
  /** What the system's own tool calls it. */
  id: string;
  name: string;
  /** "iOS 27.0", "Android 15". */
  system: string;
  state: 'running' | 'off' | 'starting';
};

/** What the core found: the devices, or one sentence that says what is needed to have any. */
export type Devices = { needs: string | null; devices: Device[] };

export function readDevices(raw: unknown): Devices {
  const data = (raw ?? {}) as Record<string, unknown>;
  const text = (value: unknown) => (typeof value === 'string' ? value : '');
  const devices = (Array.isArray(data.devices) ? data.devices : []).flatMap((one: unknown) => {
    const device = (one ?? {}) as Record<string, unknown>;
    const id = text(device.id);
    if (!id) return [];
    const state = device.state === 'running' || device.state === 'off' ? device.state : 'starting';
    return [{ id, name: text(device.name) || id, system: text(device.system), state } satisfies Device];
  });
  const needs = text(data.needs) || null;
  // Nothing listed and nothing said would leave the tab empty: say at least that.
  return { needs: needs ?? (devices.length === 0 ? 'No device was found.' : null), devices };
}

/** The words of each kind of device, so the two tabs read alike. */
export function deviceWords(kind: DeviceKind): {
  /** "simulator" or "emulator". */
  one: string;
  /** "iOS simulators" or "Android emulators". */
  many: string;
  /** Where one taps and types. */
  window: string;
} {
  return kind === 'ios'
    ? { one: 'simulator', many: 'iOS simulators', window: 'the Simulator window' }
    : { one: 'emulator', many: 'Android emulators', window: "the emulator's own window" };
}

/** One step towards having a device to show: what to do, and the command for it when there is one. */
export type DeviceStep = { text: string; command?: string };

const SYSTEM_IMAGE = 'system-images;android-35;google_apis;arm64-v8a';

/**
 * How to get what a device tab says is missing, step by step. Read from the
 * sentence the core gave: it names the one thing that is missing first. An
 * empty list when the sentence is about something else.
 */
export function deviceSetup(kind: DeviceKind, needs: string | null): DeviceStep[] {
  const said = needs ?? '';
  if (kind === 'ios') {
    if (said.includes('needs Xcode')) {
      return [
        {
          text: 'Install Xcode from the App Store and open it once. It offers to install the iOS platform: that is what brings the simulators.',
        },
        {
          text: 'If the command line tools were on this Mac before Xcode, point them at Xcode:',
          command: 'sudo xcode-select -s /Applications/Xcode.app/Contents/Developer',
        },
        { text: 'Then press Look again.' },
      ];
    }
    if (said.includes('no iOS simulator yet')) {
      return [{ text: 'In Xcode: Window, Devices and Simulators, Simulators, then the plus at the bottom left.' }];
    }
    return [];
  }
  const anEmulator: DeviceStep[] = [
    {
      text: 'Make an emulator. In Android Studio: More Actions, Virtual Device Manager, Create. Or from a terminal (on an Intel computer write x86_64 for arm64-v8a):',
      command: `sdkmanager "${SYSTEM_IMAGE}"`,
    },
    { text: 'and', command: `avdmanager create avd -n Pixel_7 -d pixel_7 -k "${SYSTEM_IMAGE}"` },
    { text: 'Then press Look again: it is listed here and started from here.' },
  ];
  if (said.includes('needs adb')) {
    return [
      {
        text: 'Android Studio brings everything: install it, then in More Actions, SDK Manager, SDK Tools tick Android SDK Platform-Tools and Android Emulator.',
      },
      {
        text: 'Without Android Studio, the command line tools are enough:',
        command: 'brew install --cask android-commandlinetools',
      },
      { text: 'then adb and the emulator itself:', command: 'sdkmanager "platform-tools" "emulator"' },
      ...anEmulator,
    ];
  }
  if (said.includes('has no emulator yet')) return anEmulator;
  if (said.includes('No Android emulator is running')) {
    return [
      { text: 'Start one from the Device Manager of Android Studio, then press Look again.' },
      {
        text: 'To start emulators from here, this computer also needs the emulator program of the Android tools:',
        command: 'sdkmanager "emulator"',
      },
    ];
  }
  return [];
}

/** What the button of a listed device says, and whether it can be pressed. */
export function deviceButton(device: Device): { label: string; can: boolean } {
  if (device.state === 'running') return { label: 'Show', can: true };
  if (device.state === 'off') return { label: 'Start and show', can: true };
  return { label: 'Starting', can: false };
}

/** What is passed on to a device: a touch on its picture, typed text, one of its buttons. */
export type DeviceTouch =
  | { what: 'tap'; x: number; y: number }
  | { what: 'swipe'; x: number; y: number; x2: number; y2: number }
  | { what: 'text'; text: string }
  | { what: 'key'; key: DeviceKey };

export type DeviceKey = 'home' | 'back' | 'recent' | 'enter' | 'delete';

/** The buttons a device of each kind has. An iPhone has no Back and no Recent: both are swipes there. */
export function deviceKeys(kind: DeviceKind): { key: DeviceKey; label: string }[] {
  const home = { key: 'home' as const, label: 'Home' };
  return kind === 'ios' ? [home] : [{ key: 'back', label: 'Back' }, home, { key: 'recent', label: 'Recent' }];
}

/** How far the pointer may move, in the pixels of the page, and the touch is still a tap. */
const TAP_SLACK = 8;

/**
 * What a press on the picture of a screen is on the device: a tap where it was
 * made, or a swipe when the pointer moved. Places are parts of the screen from
 * 0 to 1, so they hold whatever size the picture is drawn at. `null` for a
 * picture that has no size yet.
 */
export function touchOn(
  box: { left: number; top: number; width: number; height: number },
  down: { x: number; y: number },
  up: { x: number; y: number },
): DeviceTouch | null {
  if (box.width <= 0 || box.height <= 0) return null;
  const part = (at: number, start: number, size: number) => Math.min(1, Math.max(0, (at - start) / size));
  const round = (value: number) => Math.round(value * 10000) / 10000;
  const [x, y] = [round(part(down.x, box.left, box.width)), round(part(down.y, box.top, box.height))];
  if (Math.hypot(up.x - down.x, up.y - down.y) <= TAP_SLACK) return { what: 'tap', x, y };
  return {
    what: 'swipe',
    x,
    y,
    x2: round(part(up.x, box.left, box.width)),
    y2: round(part(up.y, box.top, box.height)),
  };
}

/** "iOS 27.0 · running": what stands next to a listed device's name. */
export function deviceLine(device: Device): string {
  const state = device.state === 'running' ? 'running' : device.state === 'off' ? 'off' : 'starting';
  return device.system ? `${device.system} · ${state}` : state;
}
