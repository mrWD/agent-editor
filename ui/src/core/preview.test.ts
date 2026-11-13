import { describe, expect, it } from 'vitest';
import {
  BEZEL,
  canOpen,
  componentLabel,
  defaultSettings,
  describeDrawing,
  describePicked,
  describePointed,
  deviceName,
  elementName,
  FRAME_HEAD,
  frameHeight,
  frameLabel,
  GAP,
  joinAddress,
  layoutFrames,
  markLabel,
  MIN_FRAME_HEIGHT,
  MIN_ROW_SCALE,
  parseWidth,
  readElement,
  readSettings,
  readThemeReport,
  scaleLabel,
  sizeLabel,
  sourceLabel,
  sourcePath,
  splitAddress,
  themeNote,
  themeOf,
  type Drawing,
  type PickedElement,
} from './preview';

const element = (over: Partial<PickedElement> = {}): PickedElement => ({
  tag: 'button',
  id: 'submit',
  classes: [],
  role: null,
  text: 'Try again in 42 s',
  width: 280,
  height: 46,
  disabled: true,
  selector: '#submit',
  components: ['LoginPage', 'LoginForm', 'SubmitButton'],
  file: '/work/shop/src/components/LoginForm.tsx',
  line: 42,
  column: 7,
  from: 'react',
  nearby: false,
  ...over,
});

/** How wide a row of frames is on screen at a scale: the gaps between frames do not shrink. */
const rowWidth = (widths: number[], scale: number) =>
  widths.reduce((sum, width) => sum + (width + 2 * BEZEL) * scale, 0) + GAP * (widths.length - 1);

describe('widths and their names', () => {
  it('names a width after the device that is about that wide', () => {
    expect([320, 414, 768, 1024, 1200, 1600, 2560].map(deviceName)).toEqual([
      'Phone',
      'Phone',
      'Tablet',
      'Laptop',
      'Desktop',
      'Wide',
      'Wide',
    ]);
    expect(frameLabel(320)).toBe('phone 320');
    expect(frameLabel(768)).toBe('tablet 768');
  });

  it('reads a width the user typed, and refuses what cannot be shown', () => {
    expect(parseWidth('414')).toBe(414);
    expect(parseWidth(' 1366px ')).toBe(1366);
    expect(parseWidth('199')).toBeNull();
    expect(parseWidth('3841')).toBeNull();
    expect(parseWidth('4e2')).toBeNull();
    expect(parseWidth('wide')).toBeNull();
    expect(parseWidth('')).toBeNull();
    expect(parseWidth('-320')).toBeNull();
  });
});

describe('laying out the frames', () => {
  it('shows the frames at full size when there is room', () => {
    expect(layoutFrames([320, 768], 2000)).toEqual({ scale: 1, rows: [[320, 768]] });
    expect(layoutFrames([], 1000)).toEqual({ scale: 1, rows: [] });
  });

  it('scales one row down just enough to fit, as in the mockup', () => {
    // Phone and tablet with their bezels need 1128 pixels and one gap of 24.
    const { scale, rows } = layoutFrames([320, 768], 930);
    expect(rows).toEqual([[320, 768]]);
    expect(scale).toBeCloseTo((930 - GAP) / 1128, 5);
    expect(rowWidth([320, 768], scale)).toBeCloseTo(930, 5);
    expect(scaleLabel(scale)).toBe('Shown at 80%');
  });

  it('never lets a row be wider than the stage, whatever is chosen', () => {
    const choices = [[320], [1600], [320, 768], [320, 768, 1024], [320, 414, 768, 1024, 1200, 1600], [3840, 3840]];
    for (const widths of choices) {
      for (const available of [240, 500, 760, 1028, 1500, 2600]) {
        const { scale, rows } = layoutFrames(widths, available);
        expect(scale).toBeGreaterThan(0);
        expect(scale).toBeLessThanOrEqual(1);
        expect(rows.flat()).toEqual(widths);
        for (const row of rows) {
          // A row of one frame fits by the scale alone; a longer one was packed to fit.
          expect(rowWidth(row, scale)).toBeLessThanOrEqual(available + 0.001);
        }
      }
    }
  });

  it('wraps to more rows rather than draw pages too small to read', () => {
    const all = [320, 768, 1024, 1200, 1600];
    const { scale, rows } = layoutFrames(all, 1028);
    expect(scale).toBe(MIN_ROW_SCALE);
    expect(rows).toEqual([
      [320, 768, 1024],
      [1200, 1600],
    ]);
    // Three of them still fit in one row above that scale.
    expect(layoutFrames([320, 768, 1024], 1028).rows).toHaveLength(1);
    expect(layoutFrames([320, 768, 1024], 1028).scale).toBeGreaterThan(MIN_ROW_SCALE);
  });

  it('scales a frame that is wider than the stage to fit by itself, however small that is', () => {
    const { scale, rows } = layoutFrames([320, 1600], 400);
    expect(rows).toEqual([[320], [1600]]);
    expect(scale).toBeCloseTo(400 / 1620, 5);
    expect(layoutFrames([1600], 400)).toEqual({ scale: 400 / 1620, rows: [[1600]] });
  });

  it('makes a page as tall as the stage allows, and never shorter than a small phone', () => {
    // One row: the frame's heading and the page's bezel come off, the rest is the page, undone by the scale.
    expect(frameHeight(700, 1, 1)).toBe(700 - FRAME_HEAD - 2 * BEZEL);
    expect(frameHeight(700, 1, 0.5)).toBe((700 - FRAME_HEAD) / 0.5 - 2 * BEZEL);
    // Two rows share the height; a third row does not make them smaller still.
    expect(frameHeight(700, 2, 0.5)).toBe(Math.floor(((700 - GAP) / 2 - FRAME_HEAD) / 0.5) - 2 * BEZEL);
    expect(frameHeight(700, 5, 0.5)).toBe(frameHeight(700, 2, 0.5));
    expect(frameHeight(200, 1, 1)).toBe(MIN_FRAME_HEIGHT);
    expect(frameHeight(0, 0, 1)).toBe(MIN_FRAME_HEIGHT);
  });
});

describe('the address', () => {
  it('splits what was typed into the app and the page', () => {
    expect(splitAddress('localhost:3000')).toEqual({ target: 'localhost:3000', path: '/' });
    expect(splitAddress(' http://localhost:5173/login?next=1#top ')).toEqual({
      target: 'localhost:5173',
      path: '/login?next=1#top',
    });
    expect(splitAddress('127.0.0.1:8000/')).toEqual({ target: '127.0.0.1:8000', path: '/' });
    expect(splitAddress('[::1]:4000/a')).toEqual({ target: '[::1]:4000', path: '/a' });
    expect(splitAddress('localhost:3000?tab=2')).toEqual({ target: 'localhost:3000', path: '/?tab=2' });
    expect(splitAddress('')).toBeNull();
    expect(splitAddress('   ')).toBeNull();
    expect(splitAddress('http://')).toBeNull();
  });

  it('passes an address the core will refuse on as it is, so the core can say why', () => {
    expect(splitAddress('https://localhost:3000/x')).toEqual({ target: 'https://localhost:3000', path: '/x' });
    expect(splitAddress('example.com/login')).toEqual({ target: 'example.com', path: '/login' });
  });

  it('shows the page only when it is not the first one', () => {
    expect(joinAddress('localhost:3000', '/')).toBe('localhost:3000');
    expect(joinAddress('localhost:3000', '/login')).toBe('localhost:3000/login');
  });
});

describe('what is remembered per task', () => {
  it('starts with phone and tablet in the light theme, following the dev server', () => {
    expect(defaultSettings()).toEqual({ address: null, widths: [320, 768], custom: [], theme: 'light', themes: {} });
    expect(readSettings(null)).toEqual(defaultSettings());
    expect(readSettings('dark')).toEqual(defaultSettings());
  });

  it('takes what was stored and leaves out what makes no sense', () => {
    const settings = readSettings({
      address: ' localhost:5173/login ',
      widths: [768, 320, 414, 999, 'wide', 320],
      custom: [414, 414, 1024, 50, 2000],
      theme: 'dark',
      themes: { 320: 'light', 768: 'blue', nonsense: 'dark' },
      extra: true,
    });
    expect(settings).toEqual({
      address: 'localhost:5173/login',
      // 999 is neither a preset nor one of the user's own widths.
      widths: [320, 414, 768],
      // 1024 is a preset already, 50 is too narrow.
      custom: [414, 2000],
      theme: 'dark',
      themes: { '320': 'light' },
    });
    expect(readSettings({ widths: [] }).widths).toEqual([]);
    expect(readSettings({ address: '  ' }).address).toBeNull();
  });

  it('gives a frame its own theme when it has one, else the theme for all', () => {
    const settings = { ...defaultSettings(), theme: 'dark' as const, themes: { '320': 'light' as const } };
    expect(themeOf(settings, 320)).toBe('light');
    expect(themeOf(settings, 768)).toBe('dark');
  });
});

describe('what a page says about an element', () => {
  it('is read with care: a page can send anything', () => {
    const read = readElement({
      tag: 'BUTTON',
      id: 'submit',
      classes: ['a', 'b', 'c', 'd', 7],
      text: 'x'.repeat(500),
      width: 280.4,
      height: 46,
      disabled: true,
      selector: '#submit',
      components: ['LoginForm', null, 'SubmitButton'],
      file: 'src/LoginForm.tsx',
      line: 42,
      column: 0,
      from: 'angular',
      nearby: 'yes',
    });
    expect(read).toMatchObject({
      tag: 'button',
      classes: ['a', 'b', 'c'],
      width: 280,
      components: ['LoginForm', 'SubmitButton'],
      file: 'src/LoginForm.tsx',
      line: 42,
      column: null,
      from: null,
      nearby: false,
    });
    expect(read!.text).toHaveLength(120);
    expect(readElement(null)).toBeNull();
    expect(readElement({ tag: 'p' })).toBeNull();
    expect(readElement({ tag: 'p', selector: '' })).toBeNull();
    expect(readElement('button')).toBeNull();
  });

  it('names the file from the task folder, whichever way the build tool wrote it', () => {
    expect(sourcePath('/work/shop/src/components/LoginForm.tsx', '/work/shop')).toBe('src/components/LoginForm.tsx');
    expect(sourcePath('/work/shop/src/a.ts', '/work/shop/')).toBe('src/a.ts');
    expect(sourcePath('C:\\work\\shop\\src\\a.ts', 'C:\\work\\shop')).toBe('src/a.ts');
    expect(sourcePath('./src/a.ts', '/work/shop')).toBe('src/a.ts');
    expect(sourcePath('src/a.ts', '/work/shop')).toBe('src/a.ts');
    // A file of another folder keeps its whole path, and is not offered to the editor.
    expect(sourcePath('/work/shop-old/src/a.ts', '/work/shop')).toBe('/work/shop-old/src/a.ts');
    expect(canOpen('src/a.ts')).toBe(true);
    expect(canOpen('/work/shop-old/src/a.ts')).toBe(false);
    expect(canOpen('C:/work/a.ts')).toBe(false);
    expect(canOpen('../secrets.txt')).toBe(false);
    expect(canOpen('')).toBe(false);
  });

  it('puts the component, the file and the size into the words of the mockup', () => {
    const picked = element();
    expect(componentLabel(picked)).toBe('LoginPage › LoginForm › SubmitButton');
    expect(sourceLabel(picked, '/work/shop')).toBe('src/components/LoginForm.tsx, line 42');
    expect(sizeLabel(picked)).toBe('280 by 46, disabled');
    expect(elementName(picked)).toBe('SubmitButton');
    expect(markLabel(picked)).toBe('SubmitButton · LoginForm.tsx:42');

    const plain = element({ components: [], file: null, line: null, disabled: false, from: null });
    expect(componentLabel(plain)).toBeNull();
    expect(sourceLabel(plain, '/work/shop')).toBeNull();
    expect(sizeLabel(plain)).toBe('280 by 46');
    expect(elementName(plain)).toBe('<button>');
    expect(markLabel(plain)).toBe('button#submit');
    expect(markLabel(element({ components: [], file: null, id: null, classes: ['primary', 'wide'] }))).toBe(
      'button.primary',
    );
    expect(markLabel(element({ components: [], file: null, id: null }))).toBe('button');
    // Vue names the file without a line; a file of an element around the pick is not the pick's own.
    expect(sourceLabel(element({ file: 'src/Login.vue', line: null }), '/work/shop')).toBe('src/Login.vue');
    expect(markLabel(element({ nearby: true }))).toBe('SubmitButton');
  });
});

describe('what the agent is told', () => {
  it('describes a picked element by what it is, how to find it and where it was written', () => {
    expect(describePicked({ element: element(), width: 320 }, '/work/shop')).toBe(
      [
        'About an element picked in the preview at 320 wide (phone):',
        '- it is a <button> that reads "Try again in 42 s", 280 by 46, disabled',
        '- selector: `#submit`',
        '- component: LoginPage › LoginForm › SubmitButton',
        '- source: `src/components/LoginForm.tsx`, line 42',
      ].join('\n'),
    );
  });

  it('says that no file is known rather than guess one', () => {
    const plain = element({
      components: [],
      file: null,
      line: null,
      text: '',
      disabled: false,
      selector: 'body > main > div',
    });
    const told = describePicked({ element: plain, width: 768 }, '/work/shop');
    expect(told).toBe(
      [
        'About an element picked in the preview at 768 wide (tablet):',
        '- it is a <button>, 280 by 46',
        '- selector: `body > main > div`',
        '- source: no file is known',
      ].join('\n'),
    );
    // The file of an element around the pick is told as exactly that.
    const near = describePicked({ element: element({ nearby: true }), width: 320 }, '/work/shop');
    expect(near).toContain(
      '- source: no file is known for the element itself; it is inside `src/components/LoginForm.tsx`, line 42',
    );
  });

  it('describes a drawing in words, with the user’s note', () => {
    const alert = element({
      tag: 'p',
      text: 'Too many attempts.',
      selector: 'body > p',
      components: [],
      file: null,
      line: null,
    });
    const drawing: Drawing = { n: 1, width: 320, d: 'M0 0 L10 10', element: alert, note: ' Too close to the title ' };
    expect(describeDrawing(drawing, '/work/shop')).toBe(
      [
        'The user marked the area around the <p> that reads "Too many attempts." at 320 wide (drawing 1). Their note: "Too close to the title".',
        '- selector: `body > p`',
        '- source: no file is known',
      ].join('\n'),
    );
    expect(describeDrawing({ ...drawing, n: 2, element: null, note: '' }, '/work/shop')).toBe(
      'The user marked an area of the page at 320 wide (drawing 2); no element was under it.',
    );
  });

  it('puts the pick first, then each drawing, and nothing when nothing was pointed at', () => {
    const drawing: Drawing = { n: 1, width: 768, d: 'M0 0', element: null, note: '' };
    const told = describePointed({ element: element(), width: 320 }, [drawing], '/work/shop');
    expect(told.split('\n\n')).toHaveLength(2);
    expect(told.startsWith('About an element picked in the preview at 320 wide')).toBe(true);
    expect(told.endsWith('at 768 wide (drawing 1); no element was under it.')).toBe(true);
    expect(describePointed(null, [], '/work/shop')).toBe('');
  });
});

describe('what is said about a theme', () => {
  const report = (over: Record<string, unknown> = {}) =>
    readThemeReport({ theme: 'dark', how: 'browser', media: 2, schemes: 1, scripts: true, unreadable: 0, ...over });

  it('says nothing when the theme is simply shown', () => {
    expect(themeNote('dark', true, report())).toBeNull();
    expect(themeNote('dark', true, report({ how: 'rewritten' }))).toBeNull();
    // Before the page's script has spoken there is nothing to say yet.
    expect(themeNote('dark', false, null)).toBeNull();
  });

  it('never claims a theme the page has not confirmed', () => {
    expect(themeNote('dark', true, null)).toBe('Dark is not set on this page yet.');
    expect(themeNote('light', true, report())).toBe('Light is not set on this page yet.');
  });

  it('says what could not be changed when styles come from other sites', () => {
    expect(themeNote('dark', true, report({ how: 'rewritten', unreadable: 1 }))).toBe(
      'Dark is set, except in what comes from other sites: one style sheet could not be changed.',
    );
    expect(themeNote('dark', true, report({ how: 'rewritten', unreadable: 3 }))).toContain('3 style sheets');
    // Where the browser shows the theme by itself, sheets of other sites follow too.
    expect(themeNote('dark', true, report({ how: 'browser', unreadable: 3 }))).toBeNull();
  });

  it('says so when the page has nothing that depends on the theme', () => {
    expect(themeNote('dark', true, report({ media: 0, schemes: 0, scripts: false }))).toBe(
      'This page has no styles for Light and Dark, so it looks the same in both.',
    );
    expect(themeNote('dark', true, report({ media: 0, schemes: 0, scripts: true }))).toBeNull();
  });

  it('reads a report with care', () => {
    expect(readThemeReport(null)).toBeNull();
    expect(readThemeReport({ theme: 'blue', how: 'magic', media: -1, unreadable: '3' })).toEqual({
      theme: null,
      how: 'system',
      media: 0,
      schemes: 0,
      scripts: false,
      unreadable: 0,
    });
  });
});
