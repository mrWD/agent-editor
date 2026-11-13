import { createServer } from 'node:net';
import { APP_FILE, expect, test, unique } from './support/fixtures';
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

  test('PV-15 the iOS simulator and the Android emulator say that they are not available yet', async ({
    app,
    project,
  }) => {
    const preview = await openPreview(app, (await project()).dir, shop.address);
    const kinds = preview.view.getByRole('group', { name: 'What to preview on' });

    await kinds.getByRole('button', { name: 'iOS simulator' }).click();
    await expect(preview.view.getByRole('heading', { name: 'The iOS simulator is not available yet' })).toBeVisible();
    await expect(preview.view.locator('iframe')).toHaveCount(0);
    await kinds.getByRole('button', { name: 'Android emulator' }).click();
    await expect(
      preview.view.getByRole('heading', { name: 'The Android emulator is not available yet' }),
    ).toBeVisible();

    await kinds.getByRole('button', { name: 'Web' }).click();
    await expect(preview.pageIn('Phone 320').getByRole('heading', { name: 'Sign in' })).toBeVisible();
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
