import type { Page } from '@playwright/test';
import { AGENT, DEMO_FILE, expect, PLAIN_ANSWER, readIn, test, unique, type App } from './support/fixtures';
import type { Core } from './support/core';

const GREEN = 'rgb(91, 208, 138)';
const ACCENT = 'rgb(245, 181, 68)';

const escaped = (text: string) => text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');

/** The Plan view of the task on screen, in one page. */
const planIn = (page: Page) => {
  const view = page.getByRole('region', { name: 'Plan of this task' });
  const chart = view.getByRole('group', { name: 'The plan as a flow chart' });
  return {
    view,
    chart,
    /** A block of the chart: a button whose name starts with its kind and its text, as one sentence. */
    block: (text: string) =>
      chart.getByRole('button', {
        name: new RegExp(`^(Step|Question|Note): ${escaped(text)}${/[.?!]$/.test(text) ? '' : '\\.'}( |$)`),
      }),
    /** The panel of the chosen block. */
    detail: view.getByRole('region', { name: 'The selected block' }),
    tool: (name: string) => view.getByRole('button', { name, exact: true }),
    counts: view.locator('.plan-counts'),
    labels: chart.locator('.map-flow-label'),
  };
};
const plan = (app: App) => planIn(app.page);
const planLink = (app: App) =>
  app.page.getByRole('navigation', { name: 'Views of this task' }).getByRole('link', { name: /^Plan/ });
/** The card in the conversation for blocks the agent proposed while building. */
const proposals = (app: App) =>
  app.conversation.getByRole('region', { name: 'Blocks the agent wants to add to the plan' });

type Kept = {
  blocks: { id: string; kind: string; text: string; state: string; details: string }[];
  arrows: { from: string; to: string; label: string | null }[];
  only: boolean;
};

/** Adds a block through the core, for cases that are not about adding. Returns its id. */
async function add(core: Core, taskId: string, text: string, more: Record<string, unknown> = {}): Promise<string> {
  const answer = await core.request<{ blockId: string }>('plan.add', { taskId, kind: 'step', text, ...more });
  return answer.blockId;
}

/** A task whose first turn is over, with the Plan view not opened yet. */
async function newTask(app: App, folder: string): Promise<string> {
  await app.openHome();
  return app.createTask({ text: unique('Describe the limiter'), folder });
}

/** Types a name over the one in the name box, which is selected when a block is new or double-clicked. */
async function typeName(app: App, text: string) {
  const name = plan(app).detail.getByRole('textbox', { name: 'Name' });
  await expect(name).toBeFocused();
  await app.page.keyboard.type(text);
  await app.page.keyboard.press('Enter');
  await expect(plan(app).detail.getByRole('heading')).toHaveText(text);
}

test.describe('Plan: the flow chart a task is built from', () => {
  test('PL-01 a plan is drawn by hand: a block of each kind, renamed, joined and removed', async ({
    app,
    core,
    project,
  }) => {
    const shop = await project();
    const taskId = await newTask(app, shop.dir);
    await app.showView('Plan');
    const { view, chart, block, detail, tool, counts, labels } = plan(app);

    // An empty plan says what to do, and offers everything the design's toolbar has.
    await expect(view).toContainText(
      `Nothing is planned yet. Add a step, or let ${AGENT} draft the plan from the issue.`,
    );
    await expect(view.getByRole('group', { name: 'Add to the plan' }).getByRole('button')).toHaveText([
      'Step',
      'Question',
      'Note',
    ]);
    for (const name of ['Draft from the issue', 'Draw from the code', 'Tidy up'])
      await expect(tool(name)).toBeVisible();
    await expect(view.locator('.plan-legend > span')).toHaveText([
      'Built',
      'Being built',
      'Planned',
      'Proposed by an agent',
      'Was in the code before',
    ]);
    await expect(tool('Build the next block')).toBeDisabled();
    await expect(tool('Build the next block')).toHaveAttribute(
      'title',
      'The plan has no blocks yet. Add a step first.',
    );
    await expect(counts).toHaveText('Nothing to build yet');
    await expect(detail).toContainText('No block is picked');
    await expect(planLink(app)).toHaveText('Plan');

    // A step. It is chosen at once and its name is ready to be typed over.
    await tool('Step').click();
    await expect(detail.getByRole('textbox', { name: 'Name' })).toHaveValue('New step');
    await typeName(app, 'Request to POST /login');
    const request = block('Request to POST /login');
    await expect(request).toHaveAttribute('aria-pressed', 'true');
    await expect(request).toHaveAccessibleName('Step: Request to POST /login. Planned.');
    await expect(request).toHaveCSS('border-top-style', 'dashed');
    await expect(detail.locator('.plan-state')).toHaveText('Planned');
    await expect(detail).toContainText('Nothing yet. It stands on its own.');

    // A question, added while the step is chosen: it comes after the step.
    await tool('Question').click();
    await typeName(app, 'Over the limit?');
    const question = block('Over the limit?');
    await expect(request).toHaveAccessibleName('Step: Request to POST /login. Planned. Then Over the limit?.');
    await expect(question.locator('.plan-diamond')).toHaveCount(1);
    await expect(chart.locator('.map-lines path[marker-end]')).toHaveCount(1);

    // The button under the last block adds a step after it. The arrow out of a question gets its label.
    await chart.getByRole('button', { name: 'Add a step after Over the limit?' }).click();
    await typeName(app, 'Respond 429');
    const label = detail.getByRole('textbox', { name: 'Label on the arrow from Over the limit?' });
    await label.fill('yes');
    await label.press('Tab');
    await expect(labels).toHaveText(['yes']);

    // A step that stands on its own: nothing is chosen when it is added. It is joined in its panel.
    await block('Respond 429').click();
    await expect(detail).toContainText('No block is picked');
    await tool('Step').click();
    await typeName(app, 'Check the password');
    await expect(chart.locator('.map-lines path[marker-end]')).toHaveCount(2);
    await detail.getByRole('combobox', { name: 'Connect it after' }).selectOption({ label: 'Over the limit?' });
    await detail.getByRole('textbox', { name: 'Label on the new arrow' }).fill('no');
    await detail.getByRole('button', { name: 'Connect', exact: true }).click();
    await expect(labels).toHaveText(['yes', 'no']);
    await expect(question).toHaveAccessibleName(
      'Question: Over the limit? Planned. Then yes: Respond 429; no: Check the password.',
    );
    // Laid out as the design draws it: the question under the first step, one answer down, the other to the right.
    const [first, asked, yes, no] = await Promise.all(
      [request, question, block('Respond 429'), block('Check the password')].map(
        async (one) => (await one.boundingBox())!,
      ),
    );
    expect(asked.y).toBeGreaterThan(first.y + first.height);
    expect(yes.x).toBeGreaterThan(asked.x + asked.width);
    expect(no.y).toBeGreaterThan(asked.y + asked.height);

    // A note stands on its own and is not counted as something to build.
    await block('Check the password').click();
    await tool('Note').click();
    await typeName(app, 'Never hardcode the limit');
    await expect(block('Never hardcode the limit')).toHaveAccessibleName('Note: Never hardcode the limit.');
    await expect(detail.getByRole('group', { name: 'What it comes after' })).toHaveCount(0);
    await expect(counts).toHaveText('0 of 4 built · 4 planned');
    await expect(planLink(app)).toHaveText(/^Plan\s*0 of 4$/);

    // Double-click renames. "What this block should do" says more than the name.
    await block('Check the password').dblclick();
    await typeName(app, 'Check the password hash');
    const more = detail.getByRole('textbox', { name: 'What this block should do' });
    await more.fill('Compare it with the stored hash.');
    await more.press('Tab');
    await expect
      .poll(async () => (await core.request<Kept>('plan.get', { taskId })).blocks.map((one) => one.details))
      .toContain('Compare it with the stored hash.');

    // Disconnected, the block stands on its own again; removed, it is gone.
    await detail.getByRole('button', { name: 'Disconnect from Over the limit?' }).click();
    await expect(labels).toHaveText(['yes']);
    await expect(detail).toContainText('Nothing yet. It stands on its own.');
    await detail.getByRole('button', { name: 'Remove', exact: true }).click();
    await expect(block('Check the password hash')).toHaveCount(0);
    await expect(detail).toContainText('No block is picked');
    await expect(counts).toHaveText('0 of 3 built · 3 planned');

    // The core keeps the plan: it is there after a reload, as it was drawn.
    await app.page.reload();
    await expect(block('Respond 429')).toBeVisible();
    await expect(labels).toHaveText(['yes']);
    const kept = await core.request<Kept>('plan.get', { taskId });
    expect(kept.blocks.map((one) => [one.kind, one.text, one.state])).toEqual([
      ['step', 'Request to POST /login', 'planned'],
      ['ask', 'Over the limit?', 'planned'],
      ['step', 'Respond 429', 'planned'],
      ['note', 'Never hardcode the limit', 'planned'],
    ]);
    expect(kept.arrows).toEqual([
      { from: 'b1', to: 'b2', label: null },
      { from: 'b2', to: 'b3', label: 'yes' },
    ]);
  });

  test('PL-02 "Build the next block" sends the plan and one block; the block ends built, with its file as a link', async ({
    app,
    core,
    project,
  }) => {
    const shop = await project();
    const taskId = await newTask(app, shop.dir);
    const folder = await app.folderOf(taskId);
    const first = await add(core, taskId, 'Answer 429');
    await add(core, taskId, 'Tell the form how long to wait', { after: first });
    await app.showView('Plan');
    const { block, detail, tool, counts } = plan(app);
    const next = tool('Build the next block');
    await expect(block('Answer 429')).toBeVisible();
    await expect(next).toHaveAttribute('title', 'Builds “Answer 429”');

    // The agent gets one message: the whole plan, and the one block to build.
    await next.click();
    await expect(app.messages.last()).toContainText("Build one block of this task's plan, and nothing else.");
    await expect(app.messages.last()).toContainText('step b1: Answer 429');
    await expect(app.messages.last()).toContainText('Build only this block: `b1`, “Answer 429”.');

    // While it is being built the block says so, and nothing else can be started.
    await expect(block('Answer 429')).toContainText(`${AGENT} is building`);
    await expect(block('Answer 429')).toHaveAccessibleName(
      'Step: Answer 429. Being built. Then Tell the form how long to wait.',
    );
    await expect(next).toBeDisabled();
    await expect(next).toHaveAttribute('title', `${AGENT} is building “Answer 429”`);
    await expect(tool('Draft from the issue')).toBeDisabled();
    await expect(counts).toHaveText('0 of 2 built · 1 being built · 1 planned');
    await block('Answer 429').click();
    await expect(detail).toContainText(`${AGENT} is building this block. Stop its turn to change the block.`);
    await expect(detail.getByRole('button', { name: 'Remove' })).toHaveCount(0);

    // The turn ends well: built, by the agent, in the file it changed.
    await app.answer('Allow this edit');
    await app.turnEnds();
    await expect(block('Answer 429')).toHaveCSS('border-top-color', GREEN);
    await expect(block('Answer 429')).toHaveAccessibleName(
      'Step: Answer 429. Built. Then Tell the form how long to wait.',
    );
    await expect(detail.locator('.plan-state')).toHaveText('Built');
    await expect(detail.locator('.plan-facts')).toContainText('Built in');
    await expect(detail.locator('.plan-by')).toHaveText(new RegExp(`${AGENT}, just now$`));
    await expect(counts).toHaveText('1 of 2 built · 1 planned');
    await expect(planLink(app)).toHaveText(/^Plan\s*1 of 2$/);
    await expect(next).toHaveAttribute('title', 'Builds “Tell the form how long to wait”');
    expect(readIn(folder, DEMO_FILE)).toContain('limit: limits.login,');

    // The file is a link that opens the editor.
    await detail.getByRole('link', { name: DEMO_FILE }).click();
    await expect(app.page).toHaveURL(new RegExp(`#/task/${taskId}/code$`));
    await expect(app.tab('rateLimit.ts')).toHaveAttribute('aria-selected', 'true');

    // Back in the plan the block is still the chosen one. Its text changes: it is planned again.
    await app.showView('Plan');
    await expect(block('Answer 429')).toHaveAttribute('aria-pressed', 'true');
    const name = detail.getByRole('textbox', { name: 'Name' });
    await name.fill('Answer 429 with Retry-After');
    await name.press('Enter');
    await expect(detail.locator('.plan-state')).toHaveText('Planned');
    await expect(detail.locator('.plan-facts')).toHaveCount(0);
    await expect(counts).toHaveText('0 of 2 built · 2 planned');

    // A single block is built from its panel, and the agent is told how it read before.
    await detail.getByRole('button', { name: 'Build this block' }).click();
    await expect(app.messages.last()).toContainText('It was built before, when it read: Answer 429');
    await app.answer('Allow this edit');
    await app.turnEnds();
    await expect(detail.locator('.plan-state')).toHaveText('Built');
    await expect(detail.getByRole('button', { name: 'Build it again' })).toBeEnabled();

    // "I will do it myself": the block is the user's, and with it nothing is left for the agent.
    await block('Tell the form how long to wait').click();
    await detail.getByRole('button', { name: 'I will do it myself' }).click();
    await expect(block('Tell the form how long to wait')).toContainText('planned, yours');
    await expect(detail.locator('.plan-state')).toHaveText('Planned, yours');
    await expect(detail.getByRole('button', { name: /^Build/ })).toHaveCount(0);
    await expect(next).toBeDisabled();
    await expect(next).toHaveAttribute('title', 'Nothing is left to build: every block of the plan is built or yours.');
    await detail.getByRole('button', { name: 'Give it back to the agent' }).click();
    await expect(next).toBeEnabled();
  });

  test('PL-03 a block the agent proposes waits in the conversation for "Add it to the plan" or "Not needed"', async ({
    app,
    core,
    project,
  }) => {
    const shop = await project();
    const taskId = await newTask(app, shop.dir);
    await add(core, taskId, 'Count against the limit');
    await app.showView('Plan');
    const { chart, block, detail, tool, counts } = plan(app);
    const card = proposals(app);
    await expect(card).toHaveCount(0);

    // Building a block about a limit, the practice agent misses a block in the plan and says so.
    await tool('Build the next block').click();
    await app.answer('Allow this edit');
    await app.turnEnds();
    await expect(card.getByRole('heading')).toHaveText(`${AGENT} wants to add one block`);
    await expect(card).toContainText('“Reset the count after a correct password.”');
    await expect(card).toContainText(`It is not in the plan, so ${AGENT} will not build it until you say yes.`);
    await expect(card.getByRole('button')).toHaveText(['Add it to the plan', 'Not needed']);

    // In the chart it hangs after the block that was built, dashed and in the colour of what needs the user.
    const proposed = block('Reset the count after a correct password');
    await expect(proposed).toHaveCSS('border-top-style', 'dashed');
    await expect(proposed).toHaveCSS('border-top-color', ACCENT);
    await expect(proposed).toContainText('proposed, waits for you');
    await expect(chart.locator('.plan-lines-proposed path[marker-end]')).toHaveCount(1);
    await expect(counts).toHaveText('1 of 2 built · 1 proposed');
    await expect(tool('Build the next block')).toBeDisabled();
    await expect(tool('Build the next block')).toHaveAttribute(
      'title',
      'The blocks that are left wait for your yes. Add them to the plan first.',
    );

    // The card is in the conversation whichever view is open. "Not needed": the block leaves the plan.
    await app.showView('Code');
    await card.getByRole('button', { name: /^Not needed/ }).click();
    await expect(card).toHaveCount(0);
    await app.showView('Plan');
    await expect(proposed).toHaveCount(0);
    await expect(counts).toHaveText('1 of 1 built');

    // A new session of the agent proposes it again. "Add it to the plan": it is planned, and next.
    await core.request('tasks.stopAgent', { taskId });
    await expect(app.conversation.getByText('No agent is running')).toBeVisible();
    await block('Count against the limit').click();
    await detail.getByRole('button', { name: 'Build it again' }).click();
    await app.answer('Allow this edit');
    await app.turnEnds();
    await card.getByRole('button', { name: /^Add it to the plan/ }).click();
    await expect(card).toHaveCount(0);
    await expect(proposed).toHaveAccessibleName('Step: Reset the count after a correct password. Planned.');
    await expect(proposed).not.toHaveCSS('border-top-color', ACCENT);
    await expect(chart.locator('.plan-lines-proposed path[marker-end]')).toHaveCount(0);
    await expect(counts).toHaveText('1 of 2 built · 1 planned');
    await expect(tool('Build the next block')).toHaveAttribute(
      'title',
      'Builds “Reset the count after a correct password”',
    );
  });

  test('PL-04 "Draft from the issue" brings the agent’s plan as proposed blocks; "Draw from the code" what is there', async ({
    app,
    project,
  }) => {
    const shop = await project();
    const taskId = await newTask(app, shop.dir);
    const folder = await app.folderOf(taskId);
    const before = readIn(folder, DEMO_FILE);
    await app.showView('Plan');
    const { view, chart, block, detail, tool, counts, labels } = plan(app);

    // The agent is asked for a plan in the format the editor can draw.
    await tool('Draft from the issue').click();
    await expect(app.messages.last()).toContainText('Draft a plan for this task as a flow chart');
    await expect(app.messages.last()).toContainText('The task, as it was first described:');
    await expect(app.messages.last()).toContainText('```flow');
    await expect(view.getByRole('status')).toContainText(`Waiting for ${AGENT} to draft the plan.`);
    await app.turnEnds();

    // What it drew is in the plan, and every block of it waits for a yes.
    await expect(chart.locator('.map-step-text')).toHaveText([
      'Request to POST /login',
      'Over the limit?',
      'Respond 429',
      'Check the password',
    ]);
    await expect(chart.locator('.plan-block.proposed')).toHaveCount(4);
    await expect(labels).toHaveText(['yes', 'no']);
    await expect(chart.locator('.plan-lines-proposed path[marker-end]')).toHaveCount(3);
    await expect(view).toContainText('4 blocks wait for your yes.');
    await expect(counts).toHaveText('0 of 4 built · 4 proposed');
    await expect(planLink(app)).toHaveText(/^Plan\s*0 of 4$/);
    await expect(tool('Build the next block')).toBeDisabled();
    // A draft the user asked for is answered in the plan; the conversation has no card for it.
    await expect(proposals(app)).toHaveCount(0);
    await expect(app.question).toBeHidden();
    expect(readIn(folder, DEMO_FILE)).toBe(before);

    // One by one: yes to the first, no to the last.
    await block('Request to POST /login').click();
    await expect(detail.locator('.plan-state')).toHaveText('Proposed');
    await detail.getByRole('button', { name: 'Add it to the plan' }).click();
    await expect(detail.locator('.plan-state')).toHaveText('Planned');
    await expect(view).toContainText('3 blocks wait for your yes.');
    await block('Check the password').click();
    await detail.getByRole('button', { name: 'Not needed' }).click();
    await expect(block('Check the password')).toHaveCount(0);
    await expect(labels).toHaveText(['yes']);
    // And the rest at once.
    await view.getByRole('button', { name: 'Add all to the plan' }).click();
    await expect(chart.locator('.plan-block.proposed')).toHaveCount(0);
    await expect(view).not.toContainText('for your yes');
    await expect(counts).toHaveText('0 of 3 built · 3 planned');
    await expect(tool('Build the next block')).toHaveAttribute('title', 'Builds “Request to POST /login”');

    // "Draw from the code": what the code does now. Nothing of it is to be built, and each step names its place.
    await tool('Draw from the code').click();
    await expect(app.messages.last()).toContainText('Draw the code this task is about as a flow chart');
    await app.turnEnds();
    const drawn = chart.locator('.plan-block.before');
    await expect(drawn).toHaveCount(4);
    await expect(counts).toHaveText('0 of 3 built · 3 planned');
    await drawn.filter({ hasText: 'Over the limit?' }).click();
    await expect(detail.locator('.plan-state')).toHaveText('Was in the code before');
    await detail.getByRole('link', { name: `${DEMO_FILE}, line 3` }).click();
    await expect(app.tab('rateLimit.ts')).toHaveAttribute('aria-selected', 'true');
    await expect(app.page.getByText('Line 3, column 1', { exact: true })).toBeVisible();
  });

  test('PL-05 "Agents build only what is in the plan": on, every message carries the plan; off, none does', async ({
    app,
    core,
    project,
  }) => {
    const shop = await project();
    const taskId = await newTask(app, shop.dir);
    await add(core, taskId, 'Answer 429');
    await app.showView('Plan');
    const only = plan(app).view.getByRole('checkbox', { name: 'Agents build only what is in the plan' });
    await expect(only).toBeChecked();

    // On: the agent is shown the plan and told to keep to it. The user sees only what they typed.
    await app.say('What else is there?');
    await expect(app.thoughts.last()).toContainText('Note from the editor: this task is built from a plan');
    await expect(app.thoughts.last()).toContainText('Build nothing that is not a block of this plan');
    await expect(app.thoughts.last()).toContainText('step b1: Answer 429');
    await expect(app.messages.last()).toHaveText('What else is there?');
    // The plan in front of a message is not a request to draw one: the answer is the plain one.
    await expect(app.answers.last()).toHaveText(PLAIN_ANSWER);

    // Off: not a word about the plan.
    await only.uncheck();
    await expect(only).not.toBeChecked();
    await expect.poll(async () => (await core.request<Kept>('plan.get', { taskId })).only).toBe(false);
    await app.say('And now?');
    await expect(app.thoughts.last()).toHaveText('The user asked: And now?');

    // The switch is the task's, kept by the core.
    await app.page.reload();
    await expect(only).not.toBeChecked();
    // The conversation is read again after a reload; the next message is counted from all of it.
    await expect(app.answers).toHaveCount(3);
    await only.check();
    await app.say('And again?');
    await expect(app.thoughts.last()).toContainText('Note from the editor: this task is built from a plan');
  });

  test('PL-06 nothing scrolls sideways at 800 wide; a large plan is shrunk and dragged, and "Tidy up" puts it back', async ({
    app,
    core,
    project,
  }) => {
    const shop = await project();
    const taskId = await newTask(app, shop.dir);
    // Five questions, each off to the right of the one before, each with a line of steps under it:
    // wider than any card, and taller than a card lets a picture be.
    const questions = [
      'Is there a session?',
      'Over the limit?',
      'Is the password right?',
      'Is a code needed?',
      'Is the device known?',
    ];
    let question: string | null = null;
    for (const [index, text] of questions.entries()) {
      question = await add(core, taskId, text, { kind: 'ask', ...(question ? { after: question, label: 'yes' } : {}) });
      // The line under a question is longer than everything to its right, so it is what goes down.
      let last = question;
      for (let step = 1; step <= 10 - 2 * index; step++) {
        const label = step === 1 ? 'no' : undefined;
        last = await add(core, taskId, `Step ${index + 1}.${step}`, { after: last, label });
      }
    }
    await add(core, taskId, 'Let them in', { after: question!, label: 'yes' });
    await app.showView('Plan');
    const { page } = app;
    const { view, chart, block, detail, tool } = plan(app);
    await expect(block('Let them in')).toBeAttached();

    /** How far anything of the view reaches past its own right edge. Zero means nothing scrolls sideways. */
    const sideways = () =>
      page.evaluate(() => {
        const over = (el: Element) => el.scrollWidth - el.clientWidth;
        const parts = '.plan-view, .plan-tools, .plan-legend, .plan-body, .plan-chart, .plan-detail';
        return Math.max(...[document.documentElement, ...document.querySelectorAll(parts)].map(over));
      });
    const stage = async () => (await chart.boundingBox())!;
    const place = async (text: string) => (await block(text).boundingBox())!;

    // A wide window: the chart and the panel of the chosen block side by side.
    let chartBox = await stage();
    let panel = (await detail.boundingBox())!;
    expect(panel.x).toBeGreaterThan(chartBox.x + chartBox.width);
    expect(await sideways()).toBe(0);
    // The plan is shrunk, and still larger than its card: it says so, and it is dragged.
    await expect(view.getByText('The picture is larger than this card. Drag it to see the rest.')).toBeVisible();
    expect((await place('Is there a session?')).height).toBeLessThan(88);
    const far = await place('Let them in');
    expect(far.x).toBeGreaterThan(chartBox.x + chartBox.width);

    // 800 wide: the tools wrap, the panel goes under the chart, and still nothing scrolls sideways.
    await page.setViewportSize({ width: 800, height: 900 });
    await expect.poll(async () => (await detail.boundingBox())!.y).toBeGreaterThan(chartBox.y + 100);
    chartBox = await stage();
    panel = (await detail.boundingBox())!;
    expect(panel.y).toBeGreaterThanOrEqual(chartBox.y + chartBox.height);
    expect(await sideways()).toBe(0);
    await block('Is there a session?').click();
    await expect(detail.getByRole('heading')).toHaveText('Is there a session?');
    expect(await sideways()).toBe(0);
    await block('Is there a session?').click();

    // Back in a wide window. The keyboard reaches every block: one out of sight is brought into view by the focus.
    await page.setViewportSize({ width: 1440, height: 900 });
    await expect.poll(async () => (await detail.boundingBox())!.x).toBeGreaterThan(chartBox.x + chartBox.width);
    chartBox = await stage();
    const start = await place('Is there a session?');
    const inSight = async (text: string) => {
      const box = await place(text);
      return box.x >= chartBox.x && box.x + box.width <= chartBox.x + chartBox.width + 1;
    };
    expect(await inSight('Let them in')).toBe(false);
    await block('Let them in').focus();
    await expect.poll(() => inSight('Let them in')).toBe(true);
    expect((await place('Is there a session?')).x).toBeLessThan(start.x);

    // "Tidy up" puts the chart back where it started.
    await tool('Tidy up').click();
    await expect.poll(async () => Math.round((await place('Is there a session?')).x)).toBe(Math.round(start.x));

    // Dragging the chart moves it; a drag that starts on a block does not choose the block.
    const grip = await place('Is the password right?');
    await page.mouse.move(grip.x + 60, grip.y + 26);
    await page.mouse.down();
    await page.mouse.move(grip.x - 240, grip.y - 94, { steps: 6 });
    await page.mouse.up();
    await expect.poll(async () => (await place('Is there a session?')).x).toBeLessThan(start.x - 200);
    expect((await place('Is there a session?')).y).toBeLessThan(start.y - 50);
    await expect(detail).toContainText('No block is picked');
    expect(await sideways()).toBe(0);
    await tool('Tidy up').click();
    await expect.poll(async () => Math.round((await place('Is there a session?')).x)).toBe(Math.round(start.x));
    expect(Math.round((await place('Is there a session?')).y)).toBe(Math.round(start.y));
  });

  test('PL-07 a change made in one window shows in every other window at once', async ({ app, core, project }) => {
    const shop = await project();
    const taskId = await newTask(app, shop.dir);
    await add(core, taskId, 'Answer 429');
    await app.showView('Plan');
    const here = plan(app);
    await expect(here.block('Answer 429')).toBeVisible();

    const second = await app.page.context().newPage();
    await second.goto(`${core.url}/#/task/${taskId}/plan`);
    const there = planIn(second);
    await expect(there.block('Answer 429')).toBeVisible();

    // Drawn here, seen there.
    await here.block('Answer 429').click();
    await here.tool('Step').click();
    await typeName(app, 'Tell the form how long to wait');
    await expect(there.block('Tell the form how long to wait')).toBeVisible();
    await expect(there.block('Answer 429')).toHaveAccessibleName(
      'Step: Answer 429. Planned. Then Tell the form how long to wait.',
    );
    await expect(there.counts).toHaveText('0 of 2 built · 2 planned');

    // Changed there, seen here: the switch, and a block taken out.
    await there.view.getByRole('checkbox', { name: 'Agents build only what is in the plan' }).uncheck();
    await expect(here.view.getByRole('checkbox', { name: 'Agents build only what is in the plan' })).not.toBeChecked();
    await there.block('Answer 429').click();
    await there.detail.getByRole('button', { name: 'Remove', exact: true }).click();
    await expect(here.block('Answer 429')).toHaveCount(0);
    await expect(here.counts).toHaveText('0 of 1 built · 1 planned');
    await second.close();
  });
});
