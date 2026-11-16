import { expect, type FrameLocator, type Locator, type Page } from '@playwright/test';
import { createServer, type Server } from 'node:http';
import type { AddressInfo } from 'node:net';
import { unique, type App } from './app';
import { APP_FILE } from './project';

/**
 * The "app" the Preview tests show: one page, served by a server inside the test.
 * It has what a real page has that the Preview must cope with: styles that follow
 * the system's theme in CSS and in script, an element a build plugin marked with
 * its file and line, one without any trace, and elements that carry the notes
 * React, Vue and Svelte leave in development. Every time it is served it says how
 * many times that was, so a reload can be seen.
 *
 * `?root=<folder>` tells the page where the task's files are, as a build tool
 * would know; `?hot=1` makes it open the connection a dev server's page keeps.
 */
export const pageOf = (served: number) => `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="color-scheme" content="light dark">
<title>Shop</title>
<style>
  body { margin: 0; padding: 20px; font: 15px/1.5 system-ui, sans-serif; background: rgb(255, 255, 255); color: rgb(31, 35, 40); }
  .alert { margin: 12px 0; padding: 12px 14px; border-radius: 8px; background: rgb(255, 241, 229); }
  .scheme { color: light-dark(rgb(1, 1, 1), rgb(254, 254, 254)); }
  button { height: 46px; padding: 0 28px; }
  #traces span, #traces em { display: block; margin: 6px 0; }
  @media (prefers-color-scheme: dark) {
    body { background: rgb(20, 22, 26); color: rgb(230, 232, 235); }
    .alert { background: rgb(58, 36, 20); }
  }
</style>
</head>
<body>
<h1>Sign in</h1>
<p class="alert">Too many attempts.</p>
<form>
  <button id="submit" type="button" disabled data-source="${APP_FILE}:9:3">Try again in 42 s</button>
</form>
<p class="scheme">Width <span id="vw"></span>, script sees <span id="js-theme"></span> (at first <span id="first-theme"></span>), served <span id="served">${served}</span></p>
<section id="traces">
  <span id="react-el">Written with React</span>
  <span id="vue-el">Written with Vue</span>
  <span id="svelte-el">Written with Svelte</span>
  <div data-inspector-relative-path="src/limits/config.ts" data-inspector-line="1" data-inspector-column="14"><em id="inside">Inside a marked element</em></div>
</section>
<script>
  document.getElementById('vw').textContent = String(window.innerWidth);
  // Asked once at the start, as many pages do, and again whenever the system's theme changes.
  var dark = window.matchMedia('(prefers-color-scheme: dark)');
  var show = function () { document.getElementById('js-theme').textContent = dark.matches ? 'dark' : 'light'; };
  dark.addEventListener('change', show);
  show();
  // What the very first question was answered with. A page that asks only once lives with this answer.
  document.getElementById('first-theme').textContent = dark.matches ? 'dark' : 'light';

  var query = new URLSearchParams(window.location.search);
  var root = query.get('root') || '';
  // What React (up to 18) keeps on an element in development: where it was written, and the components around it.
  function SubmitButton() {}
  function LoginForm() {}
  function LoginPage() {}
  document.getElementById('react-el')['__reactFiber$e2e'] = {
    type: 'span',
    _debugSource: { fileName: root + '/${APP_FILE}', lineNumber: 14, columnNumber: 3 },
    return: { type: SubmitButton, return: { type: 'div', return: { type: LoginForm, return: { type: LoginPage, return: null } } } },
  };
  // Vue names the component and its file, without a line.
  document.getElementById('vue-el').__vueParentComponent = {
    type: { __name: 'LimitNote', __file: root + '/src/limits/config.ts' },
    parent: { type: { name: 'ShopApp' }, parent: null },
  };
  // Svelte 4 counts lines from 0 and gives the character too.
  document.getElementById('svelte-el').__svelte_meta = { loc: { file: '${APP_FILE}', line: 22, column: 2, char: 512 } };

  if (query.get('hot')) {
    try { new WebSocket('ws://' + window.location.host + '/hmr', 'vite-hmr'); } catch (error) { /* only the asking matters */ }
  }
</script>
</body>
</html>`;

export type TestApp = {
  server: Server;
  /** The address as a dev server prints it: `localhost:PORT`. */
  address: string;
  /** How many times the page was served so far. */
  served: () => number;
};

export async function startPage(): Promise<TestApp> {
  let served = 0;
  const server = createServer((request, response) => {
    const path = (request.url ?? '/').split('?')[0];
    // The icon a browser asks for, and the socket a "hot" page opens: neither is the page.
    if (path === '/favicon.ico' || path === '/hmr') {
      response.writeHead(404).end();
      return;
    }
    if (path === '/notes.txt') {
      // Not a page: nothing of ours can be added to it.
      response.writeHead(200, { 'content-type': 'text/plain; charset=utf-8' }).end('Plain notes, not a page.');
      return;
    }
    served += 1;
    response.writeHead(200, { 'content-type': 'text/html; charset=utf-8', 'cache-control': 'no-store' });
    response.end(pageOf(served));
  });
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  return { server, address: `localhost:${(server.address() as AddressInfo).port}`, served: () => served };
}

/** The page's own colours, as its style sheet has them for the two themes. */
export const DARK = 'rgb(20, 22, 26)';
export const LIGHT = 'rgb(255, 255, 255)';

/** The Preview view on screen: its bars, its frames and the pages in them. */
export class Preview {
  constructor(
    readonly app: App,
    readonly page: Page = app.page,
  ) {}

  get view(): Locator {
    return this.page.getByRole('region', { name: 'Preview', exact: true });
  }

  get addressBox(): Locator {
    return this.view.getByRole('textbox', { name: 'Address of the running app' });
  }

  get widths(): Locator {
    return this.view.getByRole('group', { name: 'Widths' });
  }

  /** A width in the row of widths. */
  chip(width: number): Locator {
    return this.widths.getByRole('button', { name: String(width), exact: true });
  }

  /** The frame of one width: its heading, its theme buttons and the device. */
  frame(name: string): Locator {
    return this.view.getByRole('region', { name, exact: true });
  }

  /** The page inside the frame of one width. */
  pageIn(name: string): FrameLocator {
    return this.view.frameLocator(`iframe[title="${name}: the page"]`);
  }

  /** One of the things our script draws over a page: the outline of the pick, its label, the drawings. */
  drawn(name: string, part: 'catch' | 'hover-label' | 'mark' | 'mark-label' | 'strokes' | 'badges'): Locator {
    return this.pageIn(name).locator(`[data-ae-preview] #${part}`);
  }

  tool(name: 'Pick an element' | 'Draw on the page' | 'Screenshot'): Locator {
    return this.view.getByRole('group', { name: 'Tools' }).getByRole('button', { name });
  }

  /** What is said next to the tools about the screenshot: what one is, why there is none, what the last one came to. */
  get shotLine(): Locator {
    return this.view.getByRole('status', { name: 'About the screenshot' });
  }

  /** "Send a screenshot too", next to Send. */
  get shotToo(): Locator {
    return this.app.conversation.getByRole('button', { name: 'Send a screenshot too' });
  }

  /** The pictures that wait above the message box. */
  get waitingPictures(): Locator {
    return this.app.conversation.getByRole('list', { name: 'Pictures that go with your message' });
  }

  /**
   * One of "Web", "iOS simulator" and "Android emulator": where the app is shown, under "App".
   * Each is turned on and off by itself, and what is on stands side by side.
   */
  kind(name: 'Web' | 'iOS simulator' | 'Android emulator'): Locator {
    return this.view.getByRole('group', { name: 'Where the app is shown' }).getByRole('button', { name });
  }

  /** Shows the app in one place only: turns that one on and the others off. */
  async only(name: 'Web' | 'iOS simulator' | 'Android emulator') {
    if ((await this.kind(name).getAttribute('aria-pressed')) !== 'true') await this.kind(name).click();
    for (const other of ['Web', 'iOS simulator', 'Android emulator'] as const) {
      if (other !== name && (await this.kind(other).getAttribute('aria-pressed')) === 'true') {
        await this.kind(other).click();
      }
    }
    await expect(
      this.view.getByRole('group', { name: 'Where the app is shown' }).locator('[aria-pressed="true"]'),
    ).toHaveText([name]);
  }

  /** The list of simulators or emulators a device tab offers. */
  devices(kind: 'iOS simulators' | 'Android emulators'): Locator {
    return this.view.getByRole('list', { name: kind });
  }

  /** The device a device tab shows: its name, the picture of its screen and what is said about it. */
  device(name: string, kind: 'simulator' | 'emulator'): Locator {
    return this.view.getByRole('region', { name: `${name}, ${kind}`, exact: true });
  }

  /**
   * Takes a tool up and waits until the pages have it: the layer that catches the pointer lies
   * over each of them. A click that comes before that would go to the page itself.
   */
  async useTool(name: 'Pick an element' | 'Draw on the page', frames = ['Phone 320', 'Tablet 768']) {
    await this.tool(name).click();
    await expect(this.tool(name)).toHaveAttribute('aria-pressed', 'true');
    for (const frame of frames) await expect(this.drawn(frame, 'catch')).toBeVisible();
  }

  themeForAll(name: 'Light' | 'Dark'): Locator {
    return this.view.getByRole('group', { name: 'Theme for all' }).getByRole('button', { name });
  }

  get reloadButton(): Locator {
    return this.view.getByRole('button', { name: 'Reload the preview' });
  }

  /** The line under the frames: the address, and how the pages are kept fresh. */
  get status(): Locator {
    return this.view.locator('.statusline');
  }

  /** The card of the picked element, at the top of the conversation. */
  get picked(): Locator {
    return this.app.conversation.getByRole('region', { name: 'Picked element' });
  }

  drawing(n: number): Locator {
    return this.app.conversation.getByRole('region', { name: `Drawing ${n}`, exact: true });
  }

  /** What goes along with the next message, named above the message box. */
  get attached(): Locator {
    return this.app.conversation.getByLabel('Goes along with your message');
  }

  /** Types an address into the address field, as a person does. */
  async show(address: string) {
    await this.addressBox.fill(address);
    await this.addressBox.press('Enter');
  }

  /** The middle of an element of a page, in the window's own coordinates. */
  async middleOf(target: Locator): Promise<{ x: number; y: number }> {
    await expect(target).toBeVisible();
    const box = await target.boundingBox();
    if (!box) throw new Error('The element is not on screen');
    return { x: box.x + box.width / 2, y: box.y + box.height / 2 };
  }

  /** Moves the pointer onto an element of a page. Our own layer lies over the page while a tool is on. */
  async pointAt(target: Locator) {
    const { x, y } = await this.middleOf(target);
    await this.page.mouse.move(x - 2, y - 2);
    await this.page.mouse.move(x, y);
  }

  /** Clicks where an element of a page is. */
  async clickOn(target: Locator) {
    const { x, y } = await this.middleOf(target);
    await this.page.mouse.move(x, y);
    await this.page.mouse.click(x, y);
  }

  /** Draws a loop around an element of a page, as a hand would: not quite closed, a little outside it. */
  async circle(target: Locator) {
    await expect(target).toBeVisible();
    const box = (await target.boundingBox())!;
    const { mouse } = this.page;
    const [left, top, right, bottom] = [box.x - 3, box.y - 3, box.x + box.width + 3, box.y + box.height + 3];
    await mouse.move(left, top);
    await mouse.down();
    await mouse.move(right, top - 1, { steps: 5 });
    await mouse.move(right + 1, bottom, { steps: 5 });
    await mouse.move(left - 1, bottom + 1, { steps: 5 });
    await mouse.move(left, top + 4, { steps: 5 });
    await mouse.up();
  }

  /** The background the page of a frame really has, as the browser worked it out. */
  background(frame: string): Promise<string> {
    return this.pageIn(frame)
      .locator('body')
      .evaluate((body) => getComputedStyle(body).backgroundColor);
  }

  /** The colour of the line that lets the browser choose between a light and a dark value. */
  schemeColor(frame: string): Promise<string> {
    return this.pageIn(frame)
      .locator('.scheme')
      .evaluate((line) => getComputedStyle(line).color);
  }

  /** How many times the page had been served when the one in this frame was. */
  async servedIn(frame: string): Promise<number> {
    return Number(await this.pageIn(frame).locator('#served').textContent());
  }
}

/**
 * A task with the Preview view open and the test page in its frames.
 * `page` is what comes after the address, such as `/?hot=1`.
 */
export async function openPreview(app: App, folder: string, address: string, page = ''): Promise<Preview> {
  await app.openHome();
  const taskId = await app.createTask({ text: unique('Describe the limiter'), folder });
  await app.showView('Preview');
  const preview = new Preview(app);
  const root = page.includes('root=') ? encodeURIComponent(await app.folderOf(taskId)) : '';
  await preview.show(address + page.replace('root=', `root=${root}`));
  // The first page comes through a relay that is started for it; on a busy machine that takes a moment.
  await expect(preview.pageIn('Phone 320').getByRole('heading', { name: 'Sign in' })).toBeVisible({ timeout: 20_000 });
  return preview;
}
