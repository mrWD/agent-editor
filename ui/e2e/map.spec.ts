import type { Locator } from '@playwright/test';
import {
  AGENT,
  APP_FILE,
  APP_TEXT,
  DEMO_FILE,
  expect,
  PLAIN_ANSWER,
  readIn,
  test,
  unique,
  writeIn,
  type App,
} from './support/fixtures';

/** One card of the Map view, by its title. */
const card = (app: App, title: 'Flow' | 'Tree' | 'Dependencies' | 'Calls') =>
  app.page.getByRole('region', { name: title, exact: true });
/** A box of a picture, a file or a definition: all are buttons, found by the start of their name. */
const button = (within: Locator, name: string) =>
  within.getByRole('button', { name: new RegExp(`^${name.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}`) });
const center = (app: App) => app.page.getByRole('combobox', { name: 'Centered on' });
/** The line under the editor that says where the caret is. */
const caret = (app: App, line: number) => app.page.getByText(`Line ${line}, column 1`, { exact: true });

const GREEN = 'rgb(91, 208, 138)';

/** A task in which the practice agent changed the limiter, as most cases here start. */
async function taskWithAnEdit(app: App, folder: string): Promise<{ taskId: string; folder: string }> {
  await app.openHome();
  const taskId = await app.createTask({ text: unique('Describe the limiter'), folder });
  await app.letAgentEdit('Change the limit');
  return { taskId, folder: await app.folderOf(taskId) };
}

async function backToMap(app: App) {
  await app.showView('Map');
  await expect(card(app, 'Tree')).toBeVisible();
}

test.describe('Map: pictures of what a task changed', () => {
  test('V-01 the cards show the file the agent changed, what it defines, who needs it and what it calls', async ({
    app,
    project,
  }) => {
    const shop = await project();
    const { folder } = await taskWithAnEdit(app, shop.dir);
    // The limiter comes from a package, and a file the map cannot read changed too.
    writeIn(folder, DEMO_FILE, `import rateLimit from 'express-rate-limit';\n\n${readIn(folder, DEMO_FILE)}`);
    writeIn(folder, 'README.md', '# Shop\n\nNow with a limit on sign-ins.\n');
    await backToMap(app);

    // The head line: the centre, what the marks mean, and the way to the agent.
    const map = app.page.getByRole('region', { name: 'Map of this task' });
    await expect(map.locator('.map-center')).toHaveText(/Centered on\s*loginLimiter/);
    await expect(center(app)).toHaveValue('1');
    await expect(map.getByText('Changed in this task', { exact: true })).toBeVisible();
    await expect(map.getByText('External package', { exact: true })).toBeVisible();
    await expect(map.getByRole('button', { name: `Ask ${AGENT} to explain this` })).toBeEnabled();
    await expect(map.getByRole('heading', { level: 2 })).toHaveText(['Flow', 'Tree', 'Dependencies', 'Calls']);

    // Tree: the changed file and the file that imports it, by folder, each with what it defines.
    const tree = card(app, 'Tree');
    await expect(tree.locator('.map-folder-name')).toHaveText(['src/', 'middleware/']);
    await expect(tree.locator('.map-file')).toHaveText(['rateLimit.ts', 'app.ts', 'README.md']);
    const changed = button(tree, 'rateLimit.ts, changed in this task');
    await expect(changed).toHaveCSS('border-top-color', GREEN);
    await expect(button(tree, 'app.ts')).toHaveAccessibleName('app.ts');
    await expect(button(tree, 'app.ts')).not.toHaveCSS('border-top-color', GREEN);
    await expect(tree.locator('.map-file-row', { hasText: 'rateLimit.ts' })).toContainText('const loginLimiter');
    await expect(tree.locator('.map-file-row', { hasText: 'app.ts' }).locator('.map-defs')).toHaveText(
      'type Order, const orders, function addOrder, function listOrders, function totalOf, function findOrder, function describe, const app',
    );
    // Markdown is not read: the file is listed with nothing after its name, and the card says why.
    await expect(tree.locator('.map-file-row', { hasText: 'README.md' }).locator('.map-defs')).toHaveCount(0);
    await expect(
      tree.getByText('A file with nothing after its name is of a kind the map does not read yet.'),
    ).toBeVisible();

    // Dependencies: the file that needs the changed one, and the package the changed one needs.
    const deps = card(app, 'Dependencies');
    await expect(deps.getByRole('group', { name: 'Dependencies of rateLimit.ts' })).toBeVisible();
    await expect(deps.locator('.map-node')).toHaveCount(3);
    await expect(button(deps, 'app.ts')).toHaveAccessibleName('app.ts, needs rateLimit.ts');
    await expect(button(deps, 'rateLimit.ts')).toHaveAccessibleName('rateLimit.ts, changed in this task');
    await expect(button(deps, 'rateLimit.ts')).toHaveCSS('border-top-color', GREEN);
    const packageBox = button(deps, 'express-rate-limit');
    await expect(packageBox).toHaveAccessibleName('express-rate-limit, external package, needed by rateLimit.ts');
    await expect(packageBox).toHaveCSS('border-top-style', 'dashed');
    await expect(deps.locator('.map-lines path[marker-end]')).toHaveCount(2);

    // Calls: who uses the changed constant, and what it calls.
    const calls = card(app, 'Calls');
    await expect(calls.locator('.map-node')).toHaveCount(3);
    await expect(button(calls, 'app,')).toHaveAccessibleName('app, uses loginLimiter');
    await expect(button(calls, 'loginLimiter')).toHaveAccessibleName('loginLimiter, changed in this task');
    await expect(button(calls, 'rateLimit()')).toHaveAccessibleName(
      'rateLimit(), external package, used by loginLimiter',
    );
    await expect(button(calls, 'rateLimit()')).toHaveCSS('border-top-style', 'dashed');

    // The flow is not drawn until it is asked for.
    await expect(card(app, 'Flow').getByRole('button', { name: `Ask ${AGENT} to draw the flow` })).toBeEnabled();
  });

  test('V-02 a file, a definition and a box each open the editor at their line; the centre can be moved and is kept', async ({
    app,
    project,
  }) => {
    const shop = await project();
    const { taskId, folder } = await taskWithAnEdit(app, shop.dir);
    writeIn(folder, DEMO_FILE, `import rateLimit from 'express-rate-limit';\n\n${readIn(folder, DEMO_FILE)}`);
    // A second file changes, so there is another place to centre the map on.
    writeIn(folder, APP_FILE, APP_TEXT.replace('`order ${order.id}: ${order.total}`', 'String(totalOf([order]))'));
    await backToMap(app);
    // The map starts on the first changed function, which is now in app.ts. Back to the limiter first.
    await expect(app.page.locator('.map-center')).toHaveText(/Centered on\s*describe/);
    await center(app).selectOption({ label: 'loginLimiter in rateLimit.ts' });
    await expect(app.page.locator('.map-center')).toHaveText(/Centered on\s*loginLimiter/);

    // A definition in the tree: its line in its file.
    await button(card(app, 'Tree'), 'const loginLimiter').click();
    await expect(app.page).toHaveURL(new RegExp(`#/task/${taskId}/code$`));
    await expect(app.tab('rateLimit.ts')).toHaveAttribute('aria-selected', 'true');
    await expect(caret(app, 3)).toBeVisible();

    // A box of someone who uses the centre: the line where it does.
    await backToMap(app);
    await button(card(app, 'Calls'), 'app,').click();
    await expect(app.tab('app.ts')).toHaveAttribute('aria-selected', 'true');
    await expect(caret(app, 32)).toBeVisible();

    // A package has no file here: its box leads to the line that imports it.
    await backToMap(app);
    await button(card(app, 'Dependencies'), 'express-rate-limit').click();
    await expect(app.tab('rateLimit.ts')).toHaveAttribute('aria-selected', 'true');
    await expect(caret(app, 1)).toBeVisible();

    // A file in the tree: the file itself.
    await backToMap(app);
    await button(card(app, 'Tree'), 'app.ts').click();
    await expect(app.tab('app.ts')).toHaveAttribute('aria-selected', 'true');

    // The centre moves to another changed function, and both pictures are drawn around it.
    await backToMap(app);
    await expect(center(app).locator('option')).toHaveText([
      `${APP_FILE}, the whole file`,
      'describe in app.ts',
      `${DEMO_FILE}, the whole file`,
      'loginLimiter in rateLimit.ts',
    ]);
    await center(app).selectOption({ label: 'describe in app.ts' });
    await expect(app.page.locator('.map-center')).toHaveText(/Centered on\s*describe/);
    const calls = card(app, 'Calls');
    await expect(button(calls, 'describe()')).toHaveAccessibleName('describe(), changed in this task');
    await expect(button(calls, 'totalOf()')).toHaveAccessibleName('totalOf(), used by describe()');
    const deps = card(app, 'Dependencies');
    await expect(button(deps, 'app.ts')).toHaveAccessibleName('app.ts, changed in this task');
    await expect(button(deps, 'rateLimit.ts')).toHaveAccessibleName(
      'rateLimit.ts, changed in this task, needed by app.ts',
    );
    await expect(button(deps, 'limits/config.ts')).toHaveAccessibleName('limits/config.ts, needed by app.ts');
    // Two steps out: what the limiter's file needs.
    await expect(button(deps, 'express-rate-limit')).toHaveAccessibleName(
      'express-rate-limit, external package, needed by rateLimit.ts',
    );

    // A function the project defines: its own line.
    await button(calls, 'totalOf()').click();
    await expect(caret(app, 18)).toBeVisible();

    // The centre is still there after a look at the code, and after a reload.
    await backToMap(app);
    await expect(app.page.locator('.map-center')).toHaveText(/Centered on\s*describe/);
    await app.page.reload();
    await expect(app.page.locator('.map-center')).toHaveText(/Centered on\s*describe/);
    await expect(button(card(app, 'Calls'), 'totalOf()')).toBeVisible();
  });

  test('V-03 the flow is drawn from the practice agent’s answer, and an answer in another shape is called that', async ({
    app,
    project,
  }) => {
    const shop = await project();
    const { taskId, folder } = await taskWithAnEdit(app, shop.dir);
    const before = readIn(folder, DEMO_FILE);
    await backToMap(app);
    const flow = card(app, 'Flow');
    await expect(flow).toContainText(`A flow takes understanding the code, so ${AGENT} draws it.`);

    // Asking sends the agent one message that shows the format to answer in.
    await flow.getByRole('button', { name: `Ask ${AGENT} to draw the flow` }).click();
    await expect(app.messages.last()).toContainText(`Draw the flow of \`loginLimiter\` in \`${DEMO_FILE}\``);
    await expect(app.messages.last()).toContainText('```flow');

    // The chart: three steps and a question, with the answers written on the arrows.
    const chart = flow.getByRole('group', { name: 'Flow of loginLimiter' });
    await expect(chart.locator('.map-step-text')).toHaveText([
      'Request to POST /login',
      'Over the limit?',
      'Respond 429',
      'Check the password',
    ]);
    await expect(chart.locator('.map-flow-label')).toHaveText(['yes', 'no']);
    await expect(chart.locator('.map-lines path[marker-end]')).toHaveCount(3);
    await expect(chart.locator('.map-step.ask .map-diamond')).toHaveCount(1);
    const changedStep = button(chart, 'Respond 429');
    await expect(changedStep).toHaveCSS('border-top-color', GREEN);
    await expect(changedStep).toHaveAccessibleName('Respond 429, changed in this task.');
    await expect(button(chart, 'Over the limit?')).toHaveAccessibleName(
      'Over the limit? Then yes: Respond 429; no: Check the password.',
    );
    await expect(flow).toContainText(`Drawn by ${AGENT}, just now.`);
    // The chart is laid out as the design draws it: the question under the first step, "yes" off to the right.
    const first = (await chart.locator('.map-step', { hasText: 'Request to POST /login' }).boundingBox())!;
    const question = (await button(chart, 'Over the limit?').boundingBox())!;
    const yes = (await changedStep.boundingBox())!;
    expect(question.y).toBeGreaterThan(first.y + first.height);
    expect(yes.x).toBeGreaterThan(question.x + question.width);
    expect(Math.abs(yes.y + yes.height / 2 - (question.y + question.height / 2))).toBeLessThan(1);
    // Drawing a flow asks nothing and changes no file.
    await app.turnEnds();
    await expect(app.question).toBeHidden();
    expect(readIn(folder, DEMO_FILE)).toBe(before);

    // A step that says where it happens opens that place.
    await changedStep.click();
    await expect(app.page).toHaveURL(new RegExp(`#/task/${taskId}/code$`));
    await expect(app.tab('rateLimit.ts')).toHaveAttribute('aria-selected', 'true');
    await expect(caret(app, 3)).toBeVisible();

    // The same request written by hand, without the format: the agent answers in plain words.
    await backToMap(app);
    await expect(chart).toBeVisible();
    await app.say(
      `Draw the flow of \`loginLimiter\` in \`${DEMO_FILE}\`: what happens, step by step, when it is used. In your own words.`,
    );
    await expect(app.answers.last()).toHaveText(PLAIN_ANSWER);
    await expect(flow.getByRole('alert')).toHaveText(
      `${AGENT} answered, but not in the flow format, so there is nothing to draw. The answer has no flow block.`,
    );
    await expect(chart).toHaveCount(0);

    // Asking again draws it again.
    await flow.getByRole('button', { name: `Ask ${AGENT} to draw it again` }).click();
    await expect(chart.locator('.map-step-text')).toHaveCount(4);
    await expect(flow.getByRole('alert')).toHaveCount(0);
    await app.turnEnds();

    // "Explain this" is a plain message about the centre of the map. It asks for no change.
    const answers = await app.answers.count();
    await app.page.getByRole('button', { name: `Ask ${AGENT} to explain this` }).click();
    await expect(app.messages.last()).toHaveText(
      `Map, \`loginLimiter\` in \`${DEMO_FILE}\`\n\nExplain what changed here.`,
    );
    await expect(app.answers).toHaveCount(answers + 1);
    await app.turnEnds();
    await expect(app.answers.last()).toHaveText(PLAIN_ANSWER);
    await expect(app.question).toBeHidden();
    expect(readIn(folder, DEMO_FILE)).toBe(before);
  });

  test('V-04 no window is too narrow: cards stack, a picture turns top to bottom, and a large one is dragged', async ({
    app,
    project,
  }) => {
    const shop = await project();
    const { taskId, folder } = await taskWithAnEdit(app, shop.dir);
    writeIn(folder, DEMO_FILE, `import rateLimit from 'express-rate-limit';\n\n${readIn(folder, DEMO_FILE)}`);
    // A function that calls ten others: its picture is taller than a card lets it be.
    const steps = ['One', 'Two', 'Three', 'Four', 'Five', 'Six', 'Seven', 'Eight', 'Nine', 'Ten'].map(
      (n) => `step${n}`,
    );
    writeIn(folder, 'src/wide.ts', `export function wide() {\n${steps.map((step) => `  ${step}();`).join('\n')}\n}\n`);
    await backToMap(app);
    const { page } = app;
    const calls = card(app, 'Calls');

    /** How far anything of the view reaches past its own right edge. Zero means nothing scrolls sideways. */
    const sideways = () =>
      page.evaluate(() => {
        const over = (el: Element) => el.scrollWidth - el.clientWidth;
        const parts = [document.documentElement, ...document.querySelectorAll('.map-body, .map-head, .map-card')];
        return Math.max(...parts.map(over));
      });
    const place = async (name: string) => (await button(calls, name).boundingBox())!;

    // A wide window: two cards in a row, and the picture runs left to right as the design draws it.
    const flowBox = (await card(app, 'Flow').boundingBox())!;
    const treeBox = (await card(app, 'Tree').boundingBox())!;
    expect(treeBox.x).toBeGreaterThan(flowBox.x + flowBox.width);
    expect(Math.abs(treeBox.y - flowBox.y)).toBeLessThan(1);
    let user = await place('app,');
    let limiter = await place('loginLimiter');
    expect(limiter.x).toBeGreaterThan(user.x + user.width);
    expect(Math.abs(limiter.y - user.y)).toBeLessThan(1);
    expect(await sideways()).toBe(0);

    // A narrow window: the cards stack, and the same picture runs top to bottom at its full size.
    await page.setViewportSize({ width: 800, height: 800 });
    await expect.poll(async () => (await place('loginLimiter')).y - (await place('app,')).y).toBeGreaterThan(40);
    user = await place('app,');
    limiter = await place('loginLimiter');
    expect(Math.abs(limiter.x + limiter.width / 2 - (user.x + user.width / 2))).toBeLessThan(1);
    expect(limiter.height).toBe(40);
    const flowNarrow = (await card(app, 'Flow').boundingBox())!;
    const treeNarrow = (await card(app, 'Tree').boundingBox())!;
    expect(treeNarrow.y).toBeGreaterThan(flowNarrow.y + flowNarrow.height);
    expect(await sideways()).toBe(0);

    // Narrower still, and a picture too wide either way: it is shrunk to fit its card. Still nothing scrolls sideways.
    await page.setViewportSize({ width: 640, height: 800 });
    await center(app).selectOption({ label: 'wide in wide.ts' });
    await expect.poll(async () => (await place('wide()')).height).toBeLessThan(40);
    const stage = (await calls.locator('.map-stage').boundingBox())!;
    const farthest = await place('stepThree()');
    expect(farthest.x + farthest.width).toBeLessThanOrEqual(stage.x + stage.width + 1);
    expect(await sideways()).toBe(0);

    // A large picture: it is cut at the card's height and dragged to see the rest.
    await page.setViewportSize({ width: 1440, height: 900 });
    await expect.poll(async () => (await place('wide()')).height).toBe(40);
    await expect(calls.locator('.map-node')).toHaveCount(11);
    await expect(calls.getByText('The picture is larger than this card. Drag it to see the rest.')).toBeVisible();
    await calls.scrollIntoViewIfNeeded();
    const frame = calls.locator('.map-stage');
    const frameBox = async () => (await frame.boundingBox())!;
    const last = button(calls, 'stepTen()');
    expect((await last.boundingBox())!.y).toBeGreaterThan((await frameBox()).y + (await frameBox()).height);

    // Dragging the empty part of the picture moves it.
    let box = await frameBox();
    await page.mouse.move(box.x + box.width - 60, box.y + 300);
    await page.mouse.down();
    await page.mouse.move(box.x + box.width - 60, box.y + 100, { steps: 5 });
    await page.mouse.up();
    await expect.poll(async () => (await last.boundingBox())!.y + 40).toBeLessThanOrEqual(box.y + box.height);
    expect(await sideways()).toBe(0);

    // A drag that starts on a box moves the picture too, and does not open the box.
    const middleBox = (await button(calls, 'stepSix()').boundingBox())!;
    await page.mouse.move(middleBox.x + 20, middleBox.y + 20);
    await page.mouse.down();
    await page.mouse.move(middleBox.x + 20, middleBox.y + 120, { steps: 5 });
    await page.mouse.up();
    await expect(page).toHaveURL(new RegExp(`#/task/${taskId}/map$`));
    expect((await button(calls, 'stepSix()').boundingBox())!.y).toBeGreaterThan(middleBox.y + 50);

    // The keyboard reaches every box: one that is out of sight is brought into view when it gets the focus.
    box = await frameBox();
    await last.focus();
    await expect(last).toBeFocused();
    await expect.poll(async () => (await last.boundingBox())!.y + 40).toBeLessThanOrEqual(box.y + box.height);
    await button(calls, 'wide()').focus();
    const top = (await button(calls, 'wide()').boundingBox())!;
    expect(top.y).toBeGreaterThanOrEqual(box.y);
    expect(top.y + top.height).toBeLessThanOrEqual(box.y + box.height);

    // And a plain click still opens the line of the call.
    await button(calls, 'stepTwo()').focus();
    await button(calls, 'stepTwo()').click();
    await expect(app.tab('wide.ts')).toHaveAttribute('aria-selected', 'true');
    await expect(caret(app, 3)).toBeVisible();
  });

  test('V-05 a task that changed nothing says so, and a folder without git says what the map can know there', async ({
    app,
    project,
  }) => {
    const shop = await project();
    await app.openHome();
    await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    await app.showView('Map');
    const map = app.page.getByRole('region', { name: 'Map of this task' });
    await expect(map.getByRole('heading', { name: 'Nothing to map yet' })).toBeVisible();
    await expect(map).toContainText(`When ${AGENT} or you change a file, it appears here.`);

    // The map draws itself when a file changes; nobody has to ask.
    await app.letAgentEdit('Change the limit');
    await expect(card(app, 'Tree').locator('.map-file')).toHaveText(['rateLimit.ts', 'app.ts']);
    await expect(button(card(app, 'Calls'), 'loginLimiter')).toBeVisible();

    const plain = await project({ git: false });
    await app.goHome();
    await app.createTask({ text: unique('Describe the limiter'), folder: plain.dir });
    await app.showView('Map');
    await expect(map.getByRole('heading', { name: 'Nothing to map yet' })).toBeVisible();
    await expect(map).toContainText(
      'This folder has no git, so the map only knows about files you save in the editor.',
    );
  });
});
