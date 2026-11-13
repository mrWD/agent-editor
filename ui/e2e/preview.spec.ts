import { existsSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:net';
import path from 'node:path';
import {
  APP_FILE,
  expect,
  setAvds,
  setEmulators,
  setSimulators,
  simulatorRuns,
  standInNote,
  test,
  unique,
} from './support/fixtures';
import { DARK, LIGHT, openPreview, pageOf, Preview, startPage, type TestApp } from './support/previewPage';

/**
 * A dev script that really serves the test page and says where, as a dev server does.
 * It takes any free port, so two tests never meet.
 */
const serveScript = (page: string) => `import { createServer } from 'node:http';
const page = ${JSON.stringify(page)};
const server = createServer((request, response) => {
  response.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
  response.end(page);
});
server.listen(0, '127.0.0.1', () => console.log('  Local:   http://localhost:' + server.address().port + '/'));
`;

/** A port nothing listens on, held for a moment and let go. */
async function unusedPort(): Promise<number> {
  const server = createServer();
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  const { port } = server.address() as { port: number };
  await new Promise((resolve) => server.close(resolve));
  return port;
}

test.describe('The Preview of a task', () => {
  let shop: TestApp;
  test.beforeEach(async () => {
    shop = await startPage();
  });
  test.afterEach(async () => {
    await new Promise((resolve) => shop.server.close(resolve));
  });

  test('PV-01 without a running app the view says so, starts the dev script and shows the page it serves', async ({
    app,
    project,
  }) => {
    const site = await project({ devScript: 'node serve.mjs', git: false });
    site.write('serve.mjs', serveScript(pageOf(1)));
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: site.dir });
    await app.showView('Preview');
    const preview = new Preview(app);

    await expect(preview.view.getByRole('heading', { name: 'Nothing is running' })).toBeVisible();
    await expect(preview.view).toContainText(
      'Start the project here, or type the address of an app that runs already.',
    );
    await expect(preview.addressBox).toHaveValue('');
    await expect(preview.tool('Pick an element')).toBeDisabled();

    // The same start the Run tab makes; the address is the one the script prints.
    await preview.view.getByRole('button', { name: 'Start dev' }).click();
    // The script has to start, say where it serves and be asked through a new relay: on a busy machine that takes a while.
    await expect(preview.pageIn('Phone 320').getByRole('heading', { name: 'Sign in' })).toBeVisible({
      timeout: 30_000,
    });
    await expect(preview.pageIn('Tablet 768').getByRole('heading', { name: 'Sign in' })).toBeVisible();
    await expect(preview.addressBox).toHaveValue(/^localhost:\d+$/);
    await expect(preview.status).toContainText(/localhost:\d+, the dev server of this task/);
    await expect(app.conversation.getByRole('region', { name: 'The preview is live' })).toContainText(
      'The preview is live',
    );

    // The Run tab shows the same script as running, at the same address.
    await app.showView('Code');
    await app.openBottom('Run');
    await expect(app.service('dev').getByRole('link', { name: /^localhost:\d+$/ })).toBeVisible();
    await app.core.request('run.stop', { taskId });
  });

  test('PV-02 the page is shown at each chosen width, scaled to fit, and never scrolls sideways', async ({
    app,
    project,
  }) => {
    const preview = await openPreview(app, (await project()).dir, shop.address);

    // Phone and tablet to begin with; each page is laid out at exactly its width.
    await expect(preview.chip(320)).toHaveAttribute('aria-pressed', 'true');
    await expect(preview.chip(768)).toHaveAttribute('aria-pressed', 'true');
    await expect(preview.chip(1024)).toHaveAttribute('aria-pressed', 'false');
    await expect(preview.pageIn('Phone 320').locator('#vw')).toHaveText('320');
    await expect(preview.pageIn('Tablet 768').locator('#vw')).toHaveText('768');
    await expect(preview.view.getByText(/^Shown at \d+%$/)).toBeVisible();
    await expect(preview.status).toContainText('Reloads when the code changes.');

    // More widths than fit at that size: the frames get smaller, the view still does not scroll sideways.
    await preview.chip(1024).click();
    await preview.chip(1600).click();
    await expect(preview.pageIn('Laptop 1024').locator('#vw')).toHaveText('1024');
    await expect(preview.pageIn('Wide 1600').locator('#vw')).toHaveText('1600');
    const stage = preview.view.locator('.preview-stage');
    expect(await stage.evaluate((el) => el.scrollWidth <= el.clientWidth)).toBe(true);
    expect(await preview.page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    const label = (await preview.view.getByText(/^Shown at \d+%$/).textContent()) ?? '';
    const scale = Number(/(\d+)%/.exec(label)?.[1]);
    expect(scale).toBeLessThan(100);
    // What is drawn is the page's width times the scale that is named.
    const drawn = (await preview.pageIn('Wide 1600').locator('body').boundingBox())!.width;
    expect(Math.abs(drawn - (1600 * scale) / 100)).toBeLessThan(1600 * 0.011);

    // A width that is switched off leaves; the others stay as they were, without loading anew.
    const servedBefore = await preview.servedIn('Phone 320');
    await preview.chip(1600).click();
    await expect(preview.frame('Wide 1600')).toHaveCount(0);
    await expect(preview.pageIn('Phone 320').locator('#vw')).toHaveText('320');
    expect(await preview.servedIn('Phone 320')).toBe(servedBefore);
  });

  test('PV-03 a width of one’s own is added and removed, and the choices are remembered for the task', async ({
    app,
    project,
  }) => {
    const preview = await openPreview(app, (await project()).dir, shop.address, '/login');

    const tabletServed = await preview.servedIn('Tablet 768');
    await preview.widths.getByRole('button', { name: 'Add width' }).click();
    const box = preview.widths.getByRole('textbox', { name: 'Your width in pixels' });
    await expect(box).toBeFocused();
    // What is not a width is refused in words.
    await box.fill('wide');
    await box.press('Enter');
    await expect(preview.view.getByRole('alert')).toHaveText('Type a width from 200 to 3840 pixels.');
    await box.fill('414');
    await box.press('Enter');
    await expect(preview.view.getByRole('alert')).toHaveCount(0);

    // A chip with a dashed border and a cross, and a frame of that width.
    const own = preview.chip(414);
    await expect(own).toHaveAttribute('aria-pressed', 'true');
    const border = await own.locator('xpath=..').evaluate((chip) => getComputedStyle(chip).borderTopStyle);
    expect(border).toBe('dashed');
    await expect(preview.pageIn('Phone 414').locator('#vw')).toHaveText('414');
    // The new frame stands between the two that were there; neither of them loaded anew for it.
    expect(await preview.servedIn('Tablet 768')).toBe(tabletServed);
    await preview.themeForAll('Dark').click();
    await expect.poll(() => preview.background('Phone 414')).toBe(DARK);

    // Coming back to the task: the same widths, theme and address, with nothing typed again.
    await app.page.reload();
    await expect(preview.addressBox).toHaveValue(`${shop.address}/login`);
    await expect(preview.chip(414)).toHaveAttribute('aria-pressed', 'true');
    await expect(preview.themeForAll('Dark')).toHaveAttribute('aria-pressed', 'true');
    await expect(preview.pageIn('Phone 414').locator('#vw')).toHaveText('414');
    await expect.poll(() => preview.background('Phone 414')).toBe(DARK);

    // Its own width can be switched off like any other, and the cross takes it away for good.
    await own.click();
    await expect(preview.frame('Phone 414')).toHaveCount(0);
    await expect(own).toHaveAttribute('aria-pressed', 'false');
    await preview.widths.getByRole('button', { name: 'Remove your width 414' }).click();
    await expect(preview.chip(414)).toHaveCount(0);
    await app.page.reload();
    await expect(preview.pageIn('Phone 320').locator('#vw')).toHaveText('320');
    await expect(preview.chip(414)).toHaveCount(0);
  });

  test('PV-04 the theme for all reaches every page, one frame can differ, and "Same as all" brings it back', async ({
    app,
    project,
  }) => {
    const preview = await openPreview(app, (await project()).dir, shop.address);
    const phone = preview.frame('Phone 320');
    const tablet = preview.frame('Tablet 768');

    // Light to begin with: in the style sheet, and in what the page's script is told.
    expect(await preview.background('Phone 320')).toBe(LIGHT);
    await expect(preview.pageIn('Phone 320').locator('#js-theme')).toHaveText('light');
    expect(await preview.schemeColor('Phone 320')).toBe('rgb(1, 1, 1)');
    await expect(tablet).toContainText('Follows the theme for all');

    await preview.themeForAll('Dark').click();
    for (const frame of ['Phone 320', 'Tablet 768']) {
      await expect.poll(() => preview.background(frame)).toBe(DARK);
      // A script that listens is told; a colour that lets the browser choose turns too.
      await expect(preview.pageIn(frame).locator('#js-theme')).toHaveText('dark');
      await expect.poll(() => preview.schemeColor(frame)).toBe('rgb(254, 254, 254)');
    }
    // Nothing is said under a frame whose theme is simply shown.
    await expect(tablet.locator('.preview-frame-note')).toHaveCount(0);

    // The phone alone in light.
    await phone.getByRole('button', { name: 'Light theme on phone 320 only' }).click();
    await expect.poll(() => preview.background('Phone 320')).toBe(LIGHT);
    await expect(preview.pageIn('Phone 320').locator('#js-theme')).toHaveText('light');
    expect(await preview.background('Tablet 768')).toBe(DARK);
    await expect(phone.getByRole('button', { name: 'Light theme on phone 320 only' })).toHaveAttribute(
      'aria-pressed',
      'true',
    );
    await expect(tablet.getByRole('button', { name: 'Dark theme on tablet 768 only' })).toHaveAttribute(
      'aria-pressed',
      'true',
    );

    await phone.getByRole('button', { name: 'Same as all' }).click();
    await expect.poll(() => preview.background('Phone 320')).toBe(DARK);
    await expect(phone).toContainText('Follows the theme for all');

    // A page that loads anew is dark from its first script on: many pages ask once, at the start.
    const before = await preview.servedIn('Tablet 768');
    await preview.reloadButton.click();
    await expect.poll(() => preview.servedIn('Tablet 768')).toBeGreaterThan(before);
    await expect(preview.pageIn('Tablet 768').locator('#first-theme')).toHaveText('dark');
    await expect(preview.pageIn('Tablet 768').locator('#js-theme')).toHaveText('dark');
    await expect.poll(() => preview.background('Tablet 768')).toBe(DARK);
  });

  test('PV-06 picking an element outlines and names it, shows it in the conversation and marks it on every width', async ({
    app,
    project,
  }) => {
    const preview = await openPreview(app, (await project()).dir, shop.address);
    await expect(preview.view).toContainText('Pick an element or draw on the page to talk about it.');
    await preview.useTool('Pick an element');
    await expect(preview.view).toContainText('Click anything in the page. The pick is marked on every width.');

    // Under the pointer: an outline and a name with the size.
    const alert = preview.pageIn('Phone 320').locator('p.alert');
    await preview.pointAt(alert);
    await expect(preview.drawn('Phone 320', 'hover-label')).toHaveText(/^p\.alert\s+280 × \d+$/);

    // A click picks it. The page leaves no trace of a file for it, and none is made up.
    await preview.clickOn(alert);
    await expect(preview.picked.getByRole('heading')).toHaveText('<p> “Too many attempts.”');
    await expect(preview.picked).toContainText('picked on phone 320');
    await expect(preview.picked).toContainText('No file is known');
    await expect(preview.picked).toContainText(/Here\s*280 by \d+/);
    await expect(preview.picked.getByRole('button', { name: 'Open in code' })).toHaveCount(0);
    await expect(app.messageBox).toBeFocused();

    // Marked where it was picked, and found again on the other width.
    await expect(preview.drawn('Phone 320', 'mark-label')).toHaveText('p.alert');
    await expect(preview.drawn('Tablet 768', 'mark-label')).toHaveText('the same element here');
    const mark = await preview.drawn('Tablet 768', 'mark').boundingBox();
    const there = await preview.pageIn('Tablet 768').locator('p.alert').boundingBox();
    expect(Math.abs(mark!.x - there!.x)).toBeLessThan(2);
    expect(Math.abs(mark!.width - there!.width)).toBeLessThan(2);

    // "Pick its parent" goes one element out, in the frame it was picked in.
    await preview.picked.getByRole('button', { name: 'Pick its parent' }).click();
    await expect(preview.picked.getByRole('heading')).toContainText('<body>');
    await expect(preview.picked).toContainText('picked on phone 320');

    // Escape puts the tool down; the pick stays until it is sent or left out.
    await app.page.keyboard.press('Escape');
    await expect(preview.tool('Pick an element')).toHaveAttribute('aria-pressed', 'false');
    await expect(preview.picked).toBeVisible();
    await preview.attached.getByRole('button', { name: 'Leave it out' }).click();
    await expect(preview.picked).toHaveCount(0);
    await expect(preview.drawn('Tablet 768', 'mark-label')).toBeHidden();
  });

  test('PV-07 an element that a build plugin marked with its file and line opens there in the editor', async ({
    app,
    project,
  }) => {
    const preview = await openPreview(app, (await project()).dir, shop.address);
    await preview.useTool('Pick an element');
    // A disabled button takes no clicks of its own; it is picked all the same.
    await preview.clickOn(preview.pageIn('Tablet 768').locator('#submit'));

    await expect(preview.picked.getByRole('heading')).toHaveText('<button> “Try again in 42 s”');
    await expect(preview.picked).toContainText('picked on tablet 768');
    await expect(preview.picked).toContainText(/Here\s*\d+ by 46, disabled/);
    await expect(preview.drawn('Tablet 768', 'mark-label')).toHaveText('button#submit · app.ts:9');
    const source = preview.picked.getByRole('button', { name: `${APP_FILE}, line 9` });
    await expect(source).toBeVisible();

    await source.click();
    await expect(app.page).toHaveURL(/#\/task\/[^/]+\/code$/);
    await expect(app.tab('app.ts')).toHaveAttribute('aria-selected', 'true');
    await expect(app.page.getByText(/^Line 9, column \d+$/)).toBeVisible();

    // The pick is still there on the way back, and "Open in code" goes to the same place.
    await app.showView('Preview');
    await expect(preview.drawn('Tablet 768', 'mark-label')).toHaveText('button#submit · app.ts:9');
    await preview.picked.getByRole('button', { name: 'Open in code' }).click();
    await expect(app.page).toHaveURL(/#\/task\/[^/]+\/code$/);
    await expect(app.page.getByText(/^Line 9, column \d+$/)).toBeVisible();
  });

  test('PV-08 the notes React, Vue and Svelte leave on an element are read; a file nearby is told as nearby', async ({
    app,
    project,
  }) => {
    const preview = await openPreview(app, (await project()).dir, shop.address, '/?root=');
    await preview.useTool('Pick an element');
    const page = preview.pageIn('Tablet 768');

    // React: where the element was written, and the components around it, outermost first.
    await preview.clickOn(page.locator('#react-el'));
    await expect(preview.picked.getByRole('heading')).toHaveText('SubmitButton “Written with React”');
    await expect(preview.picked).toContainText(/Component\s*LoginPage › LoginForm › SubmitButton/);
    await expect(preview.picked.getByRole('button', { name: `${APP_FILE}, line 14` })).toBeVisible();
    await expect(preview.drawn('Tablet 768', 'mark-label')).toHaveText('SubmitButton · app.ts:14');

    // Vue: the component and its file, and no line, because Vue keeps none.
    await preview.clickOn(page.locator('#vue-el'));
    await expect(preview.picked).toContainText(/Component\s*ShopApp › LimitNote/);
    await expect(preview.picked.getByRole('button', { name: 'src/limits/config.ts', exact: true })).toBeVisible();
    await expect(preview.picked).not.toContainText(', line');

    // Svelte 4 counts lines from 0; the editor counts from 1.
    await preview.clickOn(page.locator('#svelte-el'));
    await expect(preview.picked.getByRole('button', { name: `${APP_FILE}, line 23` })).toBeVisible();

    // An element without a mark of its own inside a marked one: the file is given as the one around it.
    await preview.clickOn(page.locator('#inside'));
    await expect(preview.picked.getByRole('heading')).toHaveText('<em> “Inside a marked element”');
    await expect(preview.picked).toContainText(/Source\s*around it:\s*src\/limits\/config\.ts, line 1/);
    await expect(preview.drawn('Tablet 768', 'mark-label')).toHaveText('em#inside');
  });

  test('PV-09 the picked element goes to the agent with the next message, in words it can use', async ({
    app,
    project,
  }) => {
    const preview = await openPreview(app, (await project()).dir, shop.address);
    await preview.useTool('Pick an element');
    await preview.clickOn(preview.pageIn('Phone 320').locator('#submit'));

    // What goes along is named above the message box.
    await expect(preview.attached).toContainText('<button>');
    await expect(preview.attached).toContainText('phone 320');

    await app.say('What is this for?');
    const told = app.thoughts.last();
    await expect(told).toContainText('About an element picked in the preview at 320 wide (phone):');
    await expect(told).toContainText('- it is a <button> that reads "Try again in 42 s", ');
    await expect(told).toContainText('46, disabled');
    await expect(told).toContainText('- selector: `#submit`');
    await expect(told).toContainText(`- source: \`${APP_FILE}\`, line 9`);
    await expect(told).toContainText('What is this for?');
    await expect(app.messages.last()).toContainText('- selector: `#submit`');

    // Sent once: the next message is about nothing in particular again.
    await expect(preview.attached).toHaveCount(0);
    await expect(preview.picked).toHaveCount(0);
    await expect(preview.drawn('Phone 320', 'mark-label')).toBeHidden();
    await app.say('And what else is here?');
    await expect(app.thoughts.last()).not.toContainText('About an element picked');
  });

  test('PV-10 a drawing on the page is kept with a note and sent to the agent as words', async ({ app, project }) => {
    const preview = await openPreview(app, (await project()).dir, shop.address);
    await preview.useTool('Draw on the page');
    await expect(preview.view).toContainText('Draw on the page of one width.');

    // A loop around the message on the phone.
    await preview.circle(preview.pageIn('Phone 320').locator('p.alert'));
    await expect(preview.drawing(1)).toContainText('Your drawing on phone 320, around <p> “Too many attempts.”');
    await expect(preview.drawn('Phone 320', 'strokes').locator('path')).toHaveCount(1);
    await expect(preview.drawn('Phone 320', 'badges')).toHaveText('1');
    await expect(preview.drawn('Tablet 768', 'strokes').locator('path')).toHaveCount(0);

    // A second one on the tablet; the first can be taken back.
    await preview.circle(preview.pageIn('Tablet 768').getByRole('heading', { name: 'Sign in' }));
    await expect(preview.drawing(2)).toContainText('Your drawing on tablet 768, around <h1> “Sign in”');
    await preview.drawing(1).getByRole('button', { name: 'Remove drawing 1' }).click();
    await expect(preview.drawing(1)).toHaveCount(0);
    await expect(preview.drawn('Phone 320', 'strokes').locator('path')).toHaveCount(0);

    // A drawing belongs to the interface, so it is still on the page after the page loads anew.
    const before = await preview.servedIn('Tablet 768');
    await preview.reloadButton.click();
    await expect.poll(() => preview.servedIn('Tablet 768')).toBeGreaterThan(before);
    await expect(preview.drawn('Tablet 768', 'strokes').locator('path')).toHaveCount(1);
    await expect(preview.drawn('Tablet 768', 'badges')).toHaveText('2');

    await preview.drawing(2).getByRole('textbox').fill('Too close to the top');
    await expect(preview.attached).toContainText('drawing 2');
    await expect(preview.attached).toContainText('tablet 768');
    await app.say('What do you see there?');
    await expect(app.thoughts.last()).toContainText(
      'The user marked the area around the <h1> that reads "Sign in" at 768 wide (drawing 2). Their note: "Too close to the top".',
    );
    await expect(app.thoughts.last()).toContainText('- selector: `body > h1`');
    await expect(app.thoughts.last()).toContainText('- source: no file is known');
    await expect(preview.drawing(2)).toHaveCount(0);
    await expect(preview.drawn('Tablet 768', 'strokes').locator('path')).toHaveCount(0);
  });

  test('PV-11 the frames load anew when a file of the task changes, and with the Reload button', async ({
    app,
    project,
  }) => {
    const preview = await openPreview(app, (await project()).dir, shop.address);
    const taskId = app.taskId();
    const first = await preview.servedIn('Phone 320');

    // A file saved by hand.
    await app.core.request('files.write', { taskId, path: 'README.md', text: '# Shop\n\nChanged by hand.\n' });
    await expect.poll(() => preview.servedIn('Phone 320')).toBeGreaterThan(first);
    await expect.poll(() => preview.servedIn('Tablet 768')).toBeGreaterThan(first);
    await expect(preview.status).toContainText(/Reloaded at \d\d:\d\d after a change\./);

    // A file the agent changes.
    const second = await preview.servedIn('Phone 320');
    await app.letAgentEdit('Change the limit');
    await expect.poll(() => preview.servedIn('Phone 320')).toBeGreaterThan(second);

    // A new file.
    const third = await preview.servedIn('Phone 320');
    await app.core.request('files.create', { taskId, path: 'src/extra.ts', folder: false });
    await expect.poll(() => preview.servedIn('Phone 320')).toBeGreaterThan(third);

    const fourth = await preview.servedIn('Phone 320');
    await preview.reloadButton.click();
    await expect.poll(() => preview.servedIn('Phone 320')).toBeGreaterThan(fourth);
  });

  test('PV-12 a page that reloads itself is left to do so', async ({ app, project }) => {
    const preview = await openPreview(app, (await project()).dir, shop.address, '/?hot=1');
    await expect(preview.status).toContainText('The page reloads itself when the code changes.');
    await expect(app.conversation.getByRole('region', { name: 'The preview is live' })).toContainText(
      'The page reloads itself when you or Practice agent change the code.',
    );

    const before = shop.served();
    await app.core.request('files.write', { taskId: app.taskId(), path: 'README.md', text: '# Shop\n\nChanged.\n' });
    // Long enough for a reload to have happened, had one been asked for.
    await app.page.waitForTimeout(1500);
    expect(shop.served()).toBe(before);
    // The button still loads the page anew when the user says so.
    await preview.reloadButton.click();
    await expect.poll(() => shop.served()).toBeGreaterThan(before);
  });

  test('PV-13 an address that is not on this computer is refused in plain words; a project without a dev script says so', async ({
    app,
    project,
  }) => {
    const bare = await project({ git: false });
    bare.write('package.json', '{ "name": "shop", "scripts": { "build": "true" } }\n');
    await app.openHome();
    await app.createTask({ text: unique('Describe the limiter'), folder: bare.dir });
    await app.showView('Preview');
    const preview = new Preview(app);
    await expect(preview.view).toContainText(
      'This project has no dev script in package.json. Start the app in the terminal, then type its address above.',
    );
    await expect(preview.view.getByRole('button', { name: /^Start/ })).toHaveCount(0);

    await preview.show('example.com/login');
    await expect(preview.view.getByRole('alert')).toHaveText(
      'Only an app on this computer can be shown: localhost, 127.0.0.1 or [::1]. "example.com" is somewhere else.',
    );
    await expect(preview.view.locator('iframe')).toHaveCount(0);
    await preview.show('https://localhost:3000');
    await expect(preview.view.getByRole('alert')).toHaveText(
      'Only http addresses can be shown. https is not supported yet.',
    );

    // A good address after a refused one shows the page, and the refusal is gone.
    await preview.show(`http://${shop.address}`);
    await expect(preview.pageIn('Phone 320').getByRole('heading', { name: 'Sign in' })).toBeVisible();
    await expect(preview.view.getByRole('alert')).toHaveCount(0);
    await expect(preview.addressBox).toHaveValue(shop.address);

    // An empty address goes back to "nothing is running".
    await preview.show('');
    await expect(preview.view.locator('iframe')).toHaveCount(0);
    await expect(preview.view.getByRole('heading', { name: 'Nothing is running' })).toBeVisible();
  });

  test('PV-14 a page our script is not in, and an address nothing answers at, are told as they are', async ({
    app,
    project,
  }) => {
    const preview = await openPreview(app, (await project()).dir, shop.address);

    // Not an HTML page: it is shown, and the frame says our tools do not work in it.
    await preview.show(`${shop.address}/notes.txt`);
    await expect(preview.pageIn('Phone 320').getByText('Plain notes, not a page.')).toBeVisible();
    await expect(preview.frame('Phone 320')).toContainText(
      'This page did not load our script, so themes, picking and drawing do not work in it.',
    );

    // Back on a page, the note is gone.
    await preview.show(shop.address);
    await expect(preview.pageIn('Phone 320').getByRole('heading', { name: 'Sign in' })).toBeVisible();
    await expect(preview.frame('Phone 320')).not.toContainText('did not load our script');

    // Nothing listens there: the frame shows a page of ours that says so.
    const nobody = `localhost:${await unusedPort()}`;
    await preview.show(nobody);
    await expect(preview.pageIn('Phone 320').getByRole('heading', { name: 'The app is not running' })).toBeVisible();
    await expect(preview.pageIn('Phone 320').getByText(`Nothing answers at ${nobody}.`)).toBeVisible();
  });
});

test.describe('The Preview on a system set to dark', () => {
  test.use({ colorScheme: 'dark' });

  test('PV-05 Light is shown on a dark system, and Dark is the page’s own dark', async ({ app, project }) => {
    const shop = await startPage();
    try {
      const preview = await openPreview(app, (await project()).dir, shop.address);
      // The theme for all starts as Light, whatever the system says.
      for (const frame of ['Phone 320', 'Tablet 768']) {
        await expect.poll(() => preview.background(frame)).toBe(LIGHT);
        // Light from the page's first script on, not dark first and light a moment later.
        await expect(preview.pageIn(frame).locator('#first-theme')).toHaveText('light');
        await expect(preview.pageIn(frame).locator('#js-theme')).toHaveText('light');
        await expect.poll(() => preview.schemeColor(frame)).toBe('rgb(1, 1, 1)');
      }
      await preview.frame('Tablet 768').getByRole('button', { name: 'Dark theme on tablet 768 only' }).click();
      await expect.poll(() => preview.background('Tablet 768')).toBe(DARK);
      await expect(preview.pageIn('Tablet 768').locator('#js-theme')).toHaveText('dark');
      await expect.poll(() => preview.schemeColor('Tablet 768')).toBe('rgb(254, 254, 254)');
      expect(await preview.background('Phone 320')).toBe(LIGHT);
    } finally {
      await new Promise((resolve) => shop.server.close(resolve));
    }
  });
});

/** What a screenshot is said to be, wherever one is offered. */
const FRESH = 'It is the page loaded anew: not signed in, and without what you typed into the frames.';

test.describe('Screenshots of the page', () => {
  // The browser is a stand-in that writes down each of its runs: with a core of its own, a test sees only its own.
  test.use({ freshCore: true });
  let shop: TestApp;
  test.beforeEach(async () => {
    shop = await startPage();
  });
  test.afterEach(async () => {
    await new Promise((resolve) => shop.server.close(resolve));
  });

  test('PV-19 the Screenshot tool puts a picture of each width into the message, where each can be left out', async ({
    app,
    core,
    project,
  }) => {
    const preview = await openPreview(app, (await project()).dir, shop.address, '/login');
    // What a screenshot is, and what it is not, is said before one is taken.
    await expect(preview.shotLine).toHaveText(`A screenshot of phone 320 and tablet 768. ${FRESH}`);
    await preview.themeForAll('Dark').click();
    await preview.frame('Tablet 768').getByRole('button', { name: 'Light theme on tablet 768 only' }).click();

    await preview.tool('Screenshot').click();
    const waiting = preview.waitingPictures.getByRole('listitem');
    await expect(waiting).toHaveCount(2);
    await expect(waiting.nth(0)).toContainText('screenshot, phone 320.png');
    await expect(waiting.nth(1)).toContainText('screenshot, tablet 768.png');
    await expect(preview.shotLine).toHaveText(
      '2 screenshots are in your message: phone 320 and tablet 768, loaded anew in the browser.',
    );
    await expect(preview.tool('Screenshot')).toBeEnabled();

    // The browser ran once for each width: without a window, as wide as the frame and higher than it,
    // in the theme of that frame, and sent to a page of the task's own preview.
    const runs = core.calls('browser');
    expect(runs.filter((run) => run.startsWith('--headless=new'))).toHaveLength(2);
    const phone = runs.find((run) => run.includes('--window-size=320,'))!;
    const tablet = runs.find((run) => run.includes('--window-size=768,'))!;
    expect(phone).toContain('--hide-scrollbars');
    expect(phone).toContain('--blink-settings=preferredColorScheme=0');
    expect(tablet).toContain('--blink-settings=preferredColorScheme=1');
    expect(phone).toMatch(/--screenshot=\S+/);
    expect(phone).toMatch(/ http:\/\/127\.0\.0\.1:\d+\/__ae_preview\/shot\/[0-9a-f]{32}$/);
    const frameHeight = (await preview
      .pageIn('Phone 320')
      .locator('body')
      .evaluate(() => window.innerHeight)) as number;
    expect(Number(/--window-size=320,(\d+)/.exec(phone)![1])).toBe(frameHeight + 240);
    // What the core is set up with stays with the core, and nothing of a run is kept.
    expect(runs.join('\n')).not.toContain('the settings of the core were passed on');
    const kept = path.join(core.dataDir, 'tmp', 'shots');
    expect(existsSync(kept) ? readdirSync(kept) : []).toEqual([]);

    // One is left out; the other goes to the agent with the message and is shown in the conversation.
    await preview.waitingPictures.getByRole('button', { name: 'Leave screenshot, tablet 768.png out' }).click();
    await expect(waiting).toHaveCount(1);
    await app.messageBox.fill('What is wrong here?');
    await app.sendButton.click();
    await expect(app.answers.last()).toHaveText(/^I see one picture: image\/png, \d+ bytes\. Nothing was changed\.$/);
    const sent = app.conversation.getByRole('list', { name: 'Pictures sent with this message' });
    await expect(sent.getByRole('img', { name: 'screenshot, phone 320.png' })).toBeVisible();
    await expect(waiting).toHaveCount(0);
    // No screenshot was taken for the sending itself: "Send a screenshot too" is off unless asked for.
    expect(core.calls('browser').filter((run) => run.startsWith('--headless=new'))).toHaveLength(2);

    // A page that has not finished loading when the browser gives up waiting: the picture is taken, and that is said.
    await app.turnEnds();
    writeFileSync(path.join(core.toolsDir, 'state', 'browser', 'never-loads'), '');
    await preview.tool('Screenshot').click();
    await expect(waiting).toHaveCount(2);
    await expect(preview.shotLine).toHaveText(
      '2 screenshots are in your message: phone 320 and tablet 768, loaded anew in the browser. ' +
        'The page was still loading on phone 320 and tablet 768 when its picture was taken.',
    );
  });

  test('PV-20 "Send a screenshot too" takes the pictures at the moment of sending, while the Preview shows the page', async ({
    app,
    core,
    project,
  }) => {
    const preview = await openPreview(app, (await project()).dir, shop.address);
    const runs = () => core.calls('browser').filter((run) => run.startsWith('--headless=new')).length;
    const goesAlong = app.conversation.getByRole('status', { name: 'About the screenshot that goes along' });
    const sent = app.conversation.getByRole('list', { name: 'Pictures sent with this message' });

    // Off to begin with: a message goes as it is.
    await expect(preview.shotToo).toHaveAttribute('aria-pressed', 'false');
    await expect(goesAlong).toHaveCount(0);
    await app.say('What does this page do?');
    expect(runs()).toBe(0);

    // On: said in words what goes along, and taken when Send is pressed, not before.
    await preview.shotToo.click();
    await expect(preview.shotToo).toHaveAttribute('aria-pressed', 'true');
    await expect(goesAlong).toHaveText(`Goes with your message: A screenshot of phone 320 and tablet 768. ${FRESH}`);
    expect(runs()).toBe(0);
    await app.messageBox.fill('And how does it look?');
    await app.sendButton.click();
    await expect(app.answers.last()).toHaveText(/^I see 2 pictures: image\/png, \d+ bytes, image\/png, \d+ bytes\./);
    expect(runs()).toBe(2);
    await expect(sent.last().getByRole('img', { name: 'screenshot, phone 320.png' })).toBeVisible();
    await expect(sent.last().getByRole('img', { name: 'screenshot, tablet 768.png' })).toBeVisible();
    await expect(app.messageBox).toHaveValue('');
    await app.turnEnds();

    // A browser that fails: the message is not sent, and the words stay in the box.
    writeFileSync(path.join(core.toolsDir, 'state', 'browser', 'fails'), '');
    const before = await app.messages.count();
    await app.messageBox.fill('And now?');
    await app.sendButton.click();
    await expect(app.conversation.getByRole('alert')).toHaveText(
      /^the browser made no picture of the page \d+ pixels wide\. It said: The browser stand-in was told to fail Your message was not sent\.$/,
    );
    await expect(app.messageBox).toHaveValue('And now?');
    expect(await app.messages.count()).toBe(before);
    rmSync(path.join(core.toolsDir, 'state', 'browser', 'fails'));

    // Away from the Preview there is no page on show: the control is gone and no picture is taken.
    const taken = runs();
    await app.showView('Code');
    await expect(preview.shotToo).toHaveCount(0);
    await expect(goesAlong).toHaveCount(0);
    await app.sendButton.click();
    await expect(app.messages.last()).toHaveText('And now?');
    await app.turnEnds();
    expect(runs()).toBe(taken);

    // Back on the Preview it is still on for this task.
    await app.showView('Preview');
    await expect(preview.shotToo).toHaveAttribute('aria-pressed', 'true');
  });

  test('PV-21 with more frames than a message takes pictures, the screenshot is of the first five and says so', async ({
    app,
    project,
  }) => {
    const preview = await openPreview(app, (await project()).dir, shop.address);
    for (const width of [1024, 1200, 1600]) await preview.chip(width).click();
    await preview.widths.getByRole('button', { name: 'Add width' }).click();
    await preview.widths.getByRole('textbox', { name: 'Your width in pixels' }).fill('414');
    await preview.widths.getByRole('textbox', { name: 'Your width in pixels' }).press('Enter');
    await expect(preview.pageIn('Wide 1600').getByRole('heading', { name: 'Sign in' })).toBeVisible();

    const five = 'phone 320, phone 414, tablet 768, laptop 1024 and desktop 1200';
    await expect(preview.shotLine).toHaveText(
      `A screenshot of ${five} only: a message takes 5 pictures at most. ${FRESH}`,
    );
    await preview.tool('Screenshot').click();
    await expect(preview.waitingPictures.getByRole('listitem')).toHaveCount(5);
    await expect(preview.waitingPictures).not.toContainText('wide 1600');
    await expect(preview.shotLine).toHaveText(
      `5 screenshots are in your message: ${five}, loaded anew in the browser. wide 1600 did not fit: a message takes 5 pictures at most.`,
    );

    // A message that is full takes no more: the tool is off until a picture is left out.
    await preview.chip(1600).click();
    await expect(preview.tool('Screenshot')).toBeDisabled();
    await expect(preview.shotLine).toHaveText(
      'A message takes 5 pictures at most. Leave one out to make room for a screenshot.',
    );
    await preview.waitingPictures.getByRole('button', { name: 'Leave screenshot, phone 414.png out' }).click();
    await expect(preview.tool('Screenshot')).toBeEnabled();
    await expect(preview.shotLine).toContainText('A screenshot of phone 320 only: a message takes 5 pictures at most.');
  });

  test('PV-22 without Chrome, Chromium or Edge the screenshot is off and says what it needs', async ({
    app,
    core,
    project,
  }) => {
    // This computer has no browser of that family: the program the core was pointed at is not there.
    rmSync(path.join(core.toolsDir, 'browser'));
    const preview = await openPreview(app, (await project()).dir, shop.address);
    const needs = 'A screenshot needs Chrome, Chromium or Edge on this computer.';

    await expect(preview.tool('Screenshot')).toBeDisabled();
    await expect(preview.shotLine).toHaveText(needs);
    await expect(preview.shotToo).toBeDisabled();
    await expect(preview.shotToo).toHaveAttribute('title', needs);
    await expect(preview.shotToo).toHaveAttribute('aria-pressed', 'false');
    // The other tools work as before, and a message goes without a picture.
    await expect(preview.tool('Pick an element')).toBeEnabled();
    await app.say('What does this page do?');
    await expect(preview.waitingPictures).toHaveCount(0);
  });
});

test.describe('Screenshots for an agent that takes no pictures', () => {
  test.use({ freshCore: true });

  test('PV-23 an agent that takes no pictures gets no screenshot, and the tool says so', async ({
    app,
    core,
    project,
  }) => {
    // The practice agent of the older kind takes no pictures.
    core.env.AE_MOCK_MODES = '1';
    await core.restart();
    const shop = await startPage();
    try {
      const preview = await openPreview(app, (await project()).dir, shop.address);
      const why = 'Practice agent does not take pictures';
      // The same rule as for any picture.
      await expect(app.conversation.getByRole('button', { name: 'Picture', exact: true })).toBeDisabled();
      await expect(preview.tool('Screenshot')).toBeDisabled();
      await expect(preview.shotLine).toHaveText(why);
      await expect(preview.shotToo).toBeDisabled();
      await expect(preview.shotToo).toHaveAttribute('title', why);
      expect(core.calls('browser')).toEqual([]);
    } finally {
      await new Promise((resolve) => shop.server.close(resolve));
    }
  });
});

test.describe('The iOS simulator and the Android emulator', () => {
  // `xcrun` and `adb` are stand-ins that write down what they are asked: with a core of its own, a test sees only its own.
  test.use({ freshCore: true });
  let shop: TestApp;
  test.beforeEach(async () => {
    shop = await startPage();
  });
  test.afterEach(async () => {
    await new Promise((resolve) => shop.server.close(resolve));
  });

  const PHONE = 'AAAA-1111';
  const PAD = 'BBBB-2222';

  test('PV-15 the iOS simulator tab lists the simulators, starts the one that is picked, opens the app in it and shows its screen', async ({
    app,
    core,
    project,
  }) => {
    setSimulators(core.toolsDir, [
      { udid: PHONE, name: 'iPhone 17 Pro' },
      { udid: PAD, name: 'iPad Air', running: true },
    ]);
    const preview = await openPreview(app, (await project()).dir, shop.address, '/login?next=1&tab=2');
    const asked = (start: string) => core.calls('xcrun').filter((call) => call.startsWith(start)).length;

    await preview.kind('iOS simulator').click();
    await expect(preview.view.locator('iframe')).toHaveCount(0);
    // What runs already comes first. Looking at the tab starts nothing.
    const rows = preview.devices('iOS simulators').getByRole('listitem');
    await expect(rows).toHaveText([
      /^iPad AiriOS 27\.0 · runningShow$/,
      /^iPhone 17 ProiOS 27\.0 · offStart and show$/,
    ]);
    await expect(preview.view).toContainText(
      'One that is off is started here and shut down again when you leave this tab.',
    );
    expect(core.calls('xcrun')).toEqual(['simctl list devices available -j']);
    // The widths and the tools are the web page's; the address is still there to be changed.
    await expect(preview.widths).toHaveCount(0);
    await expect(preview.view.getByRole('group', { name: 'Tools' })).toHaveCount(0);
    await expect(preview.addressBox).toHaveValue(`${shop.address}/login?next=1&tab=2`);

    // The one that is off is started, and the app's own address is opened in it, page and all.
    await rows.getByRole('button', { name: 'Start and show iPhone 17 Pro' }).click();
    const device = preview.device('iPhone 17 Pro', 'simulator');
    await expect(device).toBeVisible();
    expect(simulatorRuns(core.toolsDir, PHONE)).toBe(true);
    expect(standInNote(core.toolsDir, 'xcrun', `${PHONE}.url`)).toBe(`http://${shop.address}/login?next=1&tab=2`);
    await expect(device).toContainText(
      "This is a picture of the simulator's screen, taken again every second. Press on it to tap, drag to swipe, and type in the field under it.",
    );
    await expect(device).toContainText(
      'Agent Editor started this simulator and shuts it down when you leave this tab.',
    );

    // What is shown is the picture the simulator gave of its screen, and it is taken again.
    const screen = device.getByRole('img', { name: 'The screen of iPhone 17 Pro' });
    await expect(screen).toBeVisible();
    await expect
      .poll(() => screen.evaluate((img: HTMLImageElement) => [img.naturalWidth, img.naturalHeight]))
      .toEqual([60, 130]);
    const first = await screen.getAttribute('src');
    await expect.poll(() => screen.getAttribute('src')).not.toBe(first);
    await expect(device).toContainText(/Picture taken at \d\d:\d\d:\d\d\./);
    await expect(device).toContainText(`${shop.address}/login?next=1&tab=2 on iPhone 17 Pro`);
    expect(asked(`simctl io ${PHONE} screenshot --type=png /`)).toBeGreaterThanOrEqual(2);

    // PV-30: a press on the picture is a tap on the simulator, where it was made. This Mac has idb, which
    // counts in points: the picture has three pixels to one.
    // (Pressed in the middle of a pixel of the picture, which is drawn at its own size: on the very
    // line between two pixels a browser may round either way.)
    await screen.click({ position: { x: 30.5, y: 65.5 } });
    await expect.poll(() => core.calls('idb')).toContain(`ui tap --udid ${PHONE} 10 22`);
    const keys = device.getByRole('group', { name: 'The buttons of iPhone 17 Pro' });
    await keys.getByRole('textbox', { name: 'Type on iPhone 17 Pro' }).fill('привет, it’s me');
    await keys.getByRole('button', { name: 'Type', exact: true }).click();
    await expect.poll(() => core.calls('idb')).toContain(`ui text --udid ${PHONE} -- привет, it’s me`);
    await expect(keys.getByRole('textbox', { name: 'Type on iPhone 17 Pro' })).toHaveValue('');
    await keys.getByRole('button', { name: 'Home' }).click();
    await expect.poll(() => core.calls('idb')).toContain(`ui button --udid ${PHONE} HOME`);
    // An iPhone has no Back and no Recent.
    await expect(keys.getByRole('button', { name: 'Back' })).toHaveCount(0);
    // Dark is the simulator's own setting, and the button says which was asked for last.
    const looks = keys.getByRole('group', { name: 'How iPhone 17 Pro looks' });
    await expect(looks.getByRole('button', { name: 'Dark' })).toHaveAttribute('aria-pressed', 'false');
    await looks.getByRole('button', { name: 'Dark' }).click();
    await expect.poll(() => asked(`simctl ui ${PHONE} appearance dark`)).toBe(1);
    await expect(looks.getByRole('button', { name: 'Dark' })).toHaveAttribute('aria-pressed', 'true');

    // The Simulator window is there as well: a button brings it forward.
    await device.getByRole('button', { name: 'Bring the Simulator forward' }).click();
    await expect.poll(() => asked(`open -a Simulator --args -CurrentDeviceUDID ${PHONE}`)).toBe(1);

    // Reload opens the address again; another address is opened as soon as it is typed.
    expect(asked(`simctl openurl ${PHONE}`)).toBe(1);
    await preview.view.getByRole('button', { name: 'Open the address again' }).click();
    await expect.poll(() => asked(`simctl openurl ${PHONE}`)).toBe(2);
    await preview.show(`${shop.address}/cart`);
    await expect.poll(() => standInNote(core.toolsDir, 'xcrun', `${PHONE}.url`)).toBe(`http://${shop.address}/cart`);
    expect(asked('simctl bootstatus')).toBe(1);

    // Leaving the tab shuts down the simulator the core started, and no more pictures are taken.
    await preview.kind('Web').click();
    await expect(preview.pageIn('Phone 320').getByRole('heading', { name: 'Sign in' })).toBeVisible();
    await expect.poll(() => simulatorRuns(core.toolsDir, PHONE)).toBe(false);
    const taken = asked('simctl io');
    await app.page.waitForTimeout(2500);
    expect(asked('simctl io')).toBe(taken);
    // The one that ran before was never touched.
    expect(simulatorRuns(core.toolsDir, PAD)).toBe(true);
    expect(asked(`simctl shutdown ${PAD}`)).toBe(0);
  });

  test('PV-24 a simulator that was running already stays on, and one the core started goes off with the window', async ({
    app,
    core,
    project,
  }) => {
    setSimulators(core.toolsDir, [
      { udid: PHONE, name: 'iPhone 17 Pro' },
      { udid: PAD, name: 'iPad Air', running: true },
    ]);
    const preview = await openPreview(app, (await project()).dir, shop.address);
    const shutDown = () => core.calls('xcrun').filter((call) => call.startsWith('simctl shutdown')).length;

    // One that ran already: shown without being started, said to stay on, and left on.
    await preview.kind('iOS simulator').click();
    await preview.devices('iOS simulators').getByRole('button', { name: 'Show iPad Air' }).click();
    const pad = preview.device('iPad Air', 'simulator');
    await expect(pad.getByRole('img', { name: 'The screen of iPad Air' })).toBeVisible();
    await expect(pad).toContainText('It was running already, so it stays on when you leave this tab.');
    expect(core.calls('xcrun').some((call) => call.startsWith('simctl bootstatus'))).toBe(false);

    // "Another simulator" comes back to the list; the one that was shown is still running.
    await pad.getByRole('button', { name: 'Another simulator' }).click();
    await expect(preview.devices('iOS simulators').getByRole('listitem')).toHaveCount(2);
    expect(simulatorRuns(core.toolsDir, PAD)).toBe(true);

    // One the core starts, and then the window is loaded anew, as when it is closed: the simulator goes off.
    await preview.devices('iOS simulators').getByRole('button', { name: 'Start and show iPhone 17 Pro' }).click();
    await expect(preview.device('iPhone 17 Pro', 'simulator').getByRole('img')).toBeVisible();
    expect(simulatorRuns(core.toolsDir, PHONE)).toBe(true);
    await app.page.reload();
    await expect.poll(() => simulatorRuns(core.toolsDir, PHONE)).toBe(false);
    expect(shutDown()).toBe(1);
    expect(simulatorRuns(core.toolsDir, PAD)).toBe(true);
    // The window comes back on the web page, as on every visit.
    await expect(preview.pageIn('Phone 320').getByRole('heading', { name: 'Sign in' })).toBeVisible();
  });

  test('PV-27 a simulator that stops answering is said to, and its last picture is not left on show', async ({
    app,
    core,
    project,
  }) => {
    setSimulators(core.toolsDir, [{ udid: PAD, name: 'iPad Air', running: true }]);
    const preview = await openPreview(app, (await project()).dir, shop.address);
    await preview.kind('iOS simulator').click();
    await preview.devices('iOS simulators').getByRole('button', { name: 'Show iPad Air' }).click();
    const screen = preview.view.getByRole('img', { name: 'The screen of iPad Air' });
    await expect(screen).toBeVisible();

    // Somebody shuts the simulator down behind the tab's back.
    setSimulators(core.toolsDir, [{ udid: PAD, name: 'iPad Air' }]);
    const said = preview.view.getByRole('alert');
    await expect(said).toHaveText(
      'iPad Air gave no picture of its screen: Unable to lookup in current state: Shutdown. Is it still running?',
    );
    // The picture that was there is not its screen any more. After a few tries the tab is back at the list,
    // which says how things stand now; nothing was shut down or started by the tab.
    await expect(screen).toHaveCount(0);
    await expect(preview.devices('iOS simulators').getByRole('listitem')).toHaveText([
      /^iPad AiriOS 27\.0 · offStart and show$/,
    ]);
    await expect(said).toHaveText(/iPad Air gave no picture of its screen/);
    expect(core.calls('xcrun').filter((call) => /^simctl (shutdown|bootstatus)/.test(call))).toEqual([]);
  });

  test('PV-25 the Android emulator tab gives the emulator a way to the app, opens it there and shows its screen', async ({
    app,
    core,
    project,
  }) => {
    setEmulators(core.toolsDir, [{ serial: 'emulator-5554', name: 'Pixel_9_API_35' }]);
    const preview = await openPreview(app, (await project()).dir, shop.address, '/login?next=1&tab=2');
    const port = shop.address.split(':')[1]!;

    await preview.kind('Android emulator').click();
    // The emulator under the name it was given. The phone on a cable is not an emulator and is not offered.
    const rows = preview.devices('Android emulators').getByRole('listitem');
    await expect(rows).toHaveText([/^Pixel 9 API 35Android 15 · runningShow$/]);
    await expect(preview.view).toContainText('A press on the picture is a tap on the emulator.');

    await rows.getByRole('button', { name: 'Show Pixel 9 API 35' }).click();
    const device = preview.device('Pixel 9 API 35', 'emulator');
    const screen = device.getByRole('img', { name: 'The screen of Pixel 9 API 35' });
    await expect(screen).toBeVisible();
    await expect
      .poll(() => screen.evaluate((img: HTMLImageElement) => [img.naturalWidth, img.naturalHeight]))
      .toEqual([54, 120]);
    await expect(device).toContainText(
      "This is a picture of the emulator's screen, taken again every second. Press on it to tap, drag to swipe, and type in the field under it. It was running already, so it stays on when you leave this tab.",
    );
    await expect(device.getByRole('button', { name: 'Bring the Simulator forward' })).toHaveCount(0);

    // PV-31: a press on the picture lands on the emulator where it was made, in the pixels of its screen;
    // a drag is a swipe from where it began to where it ended.
    const did = (call: string) => expect.poll(() => core.calls('adb')).toContain(`-s emulator-5554 shell ${call}`);
    await screen.click({ position: { x: 27.5, y: 60.5 } });
    await did('input tap 27 60');
    const box = (await screen.boundingBox())!;
    // The picture stands inside a frame ten pixels wide; each place is the middle of one of its pixels.
    const [left, top] = [box.x + 10 + 27.5, box.y + 10];
    await app.page.mouse.move(left, top + 90.5);
    await app.page.mouse.down();
    await app.page.mouse.move(left, top + 60.5, { steps: 3 });
    await app.page.mouse.move(left, top + 30.5, { steps: 3 });
    await app.page.mouse.up();
    await did('input swipe 27 90 27 30 250');
    // Typed text goes with Enter in the field; the buttons are the emulator's own.
    const keys = device.getByRole('group', { name: 'The buttons of Pixel 9 API 35' });
    const field = keys.getByRole('textbox', { name: 'Type on Pixel 9 API 35' });
    await expect(keys.getByRole('button', { name: 'Type', exact: true })).toBeDisabled();
    await field.fill('shop at 5');
    await field.press('Enter');
    await did("input text 'shop%sat%s5'");
    await expect(field).toHaveValue('');
    for (const [name, code] of [
      ['Back', 'KEYCODE_BACK'],
      ['Home', 'KEYCODE_HOME'],
      ['Recent', 'KEYCODE_APP_SWITCH'],
      ['Enter', 'KEYCODE_ENTER'],
      ['Delete the letter before the cursor', 'KEYCODE_DEL'],
    ]) {
      await keys.getByRole('button', { name: name!, exact: true }).click();
      await did(`input keyevent ${code}`);
    }
    await keys.getByRole('button', { name: 'Dark' }).click();
    await did('cmd uimode night yes');
    // What an emulator cannot take is said, and the text stays in the field to be changed.
    await field.fill('привет');
    await field.press('Enter');
    await expect(device.getByRole('alert')).toContainText('An emulator takes only plain Latin letters');
    await expect(field).toHaveValue('привет');

    // "localhost" on the emulator is the emulator: the app's port is passed on to this computer, and the
    // address is opened there as one word.
    const calls = core.calls('adb');
    expect(calls).toContain(`-s emulator-5554 reverse tcp:${port} tcp:${port}`);
    expect(calls).toContain(
      `-s emulator-5554 shell am start -a android.intent.action.VIEW -d 'http://localhost:${port}/login?next=1&tab=2'`,
    );
    expect(standInNote(core.toolsDir, 'adb', 'emulator-5554.reverse')).toBe(`UsbFfs tcp:${port} tcp:${port}`);
    await expect
      .poll(() => core.calls('adb').filter((call) => call.endsWith('exec-out screencap -p')).length)
      .toBeGreaterThan(1);

    // Leaving the tab takes the way to the port back.
    await preview.kind('iOS simulator').click();
    await expect.poll(() => standInNote(core.toolsDir, 'adb', 'emulator-5554.reverse')).toBe('');
    expect(core.calls('adb')).toContain(`-s emulator-5554 reverse --remove tcp:${port}`);
  });

  test('PV-26 a missing tool, no device and no running app are each said in a sentence, and no screen is shown', async ({
    app,
    core,
    project,
  }) => {
    const preview = await openPreview(app, (await project()).dir, shop.address);
    const again = preview.view.getByRole('button', { name: 'Look again' });

    // Xcode is there and has no simulator.
    await preview.kind('iOS simulator').click();
    await expect(preview.view.getByRole('heading', { name: 'No simulator to show' })).toBeVisible();
    await expect(preview.view).toContainText(
      'Xcode has no iOS simulator yet: add one in Xcode under Window, Devices and Simulators.',
    );
    // "Look again" finds one that was added meanwhile.
    setSimulators(core.toolsDir, [{ udid: PHONE, name: 'iPhone 17 Pro' }]);
    await again.click();
    await expect(preview.devices('iOS simulators').getByRole('listitem')).toHaveCount(1);
    // With a simulator and no app running, the tab says what the web page says: nothing is running.
    await preview.show('');
    await expect(preview.view.getByRole('heading', { name: 'Nothing is running' })).toBeVisible();
    await expect(preview.devices('iOS simulators')).toHaveCount(0);
    await expect(preview.view.getByRole('button', { name: 'Open the address again' })).toBeDisabled();
    await preview.show(shop.address);
    await expect(preview.devices('iOS simulators').getByRole('listitem')).toHaveCount(1);
    // Xcode itself is not there.
    rmSync(path.join(core.toolsDir, 'xcrun'));
    await again.click();
    await expect(preview.view).toContainText(
      'The iOS simulator needs Xcode on this Mac: install it from the App Store and open it once.',
    );
    // PV-40: what to do about it stands under the sentence, step by step, with the command to copy.
    const forIos = preview.view.getByRole('list', { name: 'How to get a simulator' });
    await expect(forIos.getByRole('listitem')).toHaveCount(3);
    await expect(forIos).toContainText('sudo xcode-select -s /Applications/Xcode.app/Contents/Developer');

    // No emulator runs and the Device Manager has none to start; then adb itself is not there.
    await preview.kind('Android emulator').click();
    await expect(preview.view.getByRole('heading', { name: 'No emulator to show' })).toBeVisible();
    await expect(preview.view).toContainText(
      'Android has no emulator yet: make one in the Device Manager of Android Studio.',
    );
    rmSync(path.join(core.toolsDir, 'adb'));
    await again.click();
    await expect(preview.view).toContainText(
      'The Android emulator needs adb, which comes with Android Studio (SDK Platform-Tools in its SDK Manager).',
    );
    const forAndroid = preview.view.getByRole('list', { name: 'How to get an emulator' });
    await expect(forAndroid).toContainText('brew install --cask android-commandlinetools');
    await expect(forAndroid).toContainText('avdmanager create avd -n Pixel_7');
    await expect(
      forAndroid.getByRole('button', { name: 'Copy the command: sdkmanager "platform-tools" "emulator"' }),
    ).toBeVisible();
    await expect(preview.view.getByRole('img')).toHaveCount(0);
  });

  test('PV-29 an Android emulator that is off is started from the tab and shut down again when the tab is left', async ({
    app,
    core,
    project,
  }) => {
    setEmulators(core.toolsDir, [{ serial: 'emulator-5554', name: 'Pixel_9_API_35' }]);
    setAvds(core.toolsDir, ['Pixel_9_API_35', 'Tablet_API_34']);
    const preview = await openPreview(app, (await project()).dir, shop.address);
    const started = (call: string) => core.calls('emulator').filter((one) => one.startsWith(call)).length;

    await preview.kind('Android emulator').click();
    // The one that runs, then the one the Device Manager knows and nobody started. Looking starts nothing.
    const rows = preview.devices('Android emulators').getByRole('listitem');
    await expect(rows).toHaveText([
      /^Pixel 9 API 35Android 15 · runningShow$/,
      /^Tablet API 34Android · offStart and show$/,
    ]);
    expect(started('-avd')).toBe(0);

    await rows.getByRole('button', { name: 'Start and show Tablet API 34' }).click();
    const device = preview.device('Tablet API 34', 'emulator');
    await expect(device.getByRole('img', { name: 'The screen of Tablet API 34' })).toBeVisible();
    await expect(device).toContainText('Agent Editor started this emulator and shuts it down when you leave this tab.');
    expect(core.calls('emulator')).toContain('-avd Tablet_API_34');
    // Reload opens the address again on the emulator that runs by now: it is not started twice.
    await preview.view.getByRole('button', { name: 'Open the address again' }).click();
    await expect
      .poll(() => core.calls('adb').filter((call) => call.startsWith('-s emulator-5556 shell am start')).length)
      .toBe(2);
    expect(started('-avd')).toBe(1);

    // Leaving the tab shuts down what the core started, and only that.
    await preview.kind('Web').click();
    await expect.poll(() => core.calls('adb')).toContain('-s emulator-5556 emu kill');
    expect(core.calls('adb')).not.toContain('-s emulator-5554 emu kill');
  });

  test('PV-32 on a Mac without idb the simulator tab says what tapping needs, and its picture takes no press', async ({
    app,
    core,
    project,
  }) => {
    rmSync(path.join(core.toolsDir, 'idb'));
    setSimulators(core.toolsDir, [{ udid: PAD, name: 'iPad Air', running: true }]);
    const preview = await openPreview(app, (await project()).dir, shop.address);
    await preview.kind('iOS simulator').click();
    await preview.devices('iOS simulators').getByRole('button', { name: 'Show iPad Air' }).click();
    const device = preview.device('iPad Air', 'simulator');
    const screen = device.getByRole('img', { name: 'The screen of iPad Air' });
    await expect(screen).toBeVisible();
    await expect(device).toContainText('Tapping a simulator from here needs idb on this Mac');
    await expect(device).toContainText("Until then, tap and type in the Simulator's own window.");
    const keys = device.getByRole('group', { name: 'The buttons of iPad Air' });
    await expect(keys.getByRole('button', { name: 'Home' })).toHaveCount(0);
    await expect(keys.getByRole('textbox')).toHaveCount(0);
    await screen.click();
    await expect(device.getByRole('alert')).toHaveCount(0);
    // How it looks is Xcode's own to change, with or without idb.
    await keys.getByRole('button', { name: 'Light' }).click();
    await expect.poll(() => core.calls('xcrun')).toContain(`simctl ui ${PAD} appearance light`);
    await device.getByRole('button', { name: 'Bring the Simulator forward' }).click();
    await expect.poll(() => core.calls('xcrun')).toContain(`open -a Simulator --args -CurrentDeviceUDID ${PAD}`);
  });
});
