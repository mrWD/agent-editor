import { expect, test, unique, type App } from './support/fixtures';

const open = async (app: App, section: 'Agents' | 'Automations') => {
  await app.page.getByRole('navigation', { name: 'Sections' }).getByRole('link', { name: section }).click();
  await expect(app.page.getByRole('heading', { level: 1 })).toBeVisible();
};

test.describe('Agents: who is doing what', () => {
  test('W-01 every agent is listed with what it is doing; one that is not installed says so', async ({
    app,
    project,
  }) => {
    const shop = await project();
    await app.openHome();
    await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    await open(app, 'Agents');

    const practice = app.page.getByRole('region', { name: 'Practice agent', exact: true });
    await expect(practice).toContainText(/Idle, (one open task|\d+ open tasks)/);
    await expect(practice).toContainText('costs nothing');
    const claude = app.page.getByRole('region', { name: 'Claude', exact: true });
    await expect(claude).toContainText('Not installed');
    await expect(app.page.getByText('No task has cost anything today.')).toBeVisible();
    await expect(app.page.getByRole('heading', { level: 1 })).toContainText('nothing spent today');
  });
});

test.describe('Agents: cost and the daily budget', () => {
  test.use({ practiceCost: 0.25 });

  test('W-02 W-03 cost is shown by agent and by task; over the budget a new task asks first', async ({
    app,
    project,
  }) => {
    const shop = await project();
    await app.openHome();
    const title = unique('Describe the limiter');
    await app.createTask({ text: title, folder: shop.dir });
    // Inside the task the cost sits next to the agent's name.
    await expect(app.conversation.getByText(/^Context \d+% · \$0\.25$/)).toBeVisible();

    await open(app, 'Agents');
    await expect(app.page.getByRole('heading', { level: 1 })).toContainText('$0.25 spent today');
    const practice = app.page.getByRole('region', { name: 'Practice agent', exact: true });
    await expect(practice).toContainText('Today$0.25');
    await expect(practice).toContainText('This week$0.25');
    const row = app.page.locator('.cost-row', { hasText: title });
    await expect(row).toContainText('$0.25');
    await expect(app.page.locator('.cost-row.total')).toContainText('$0.25');

    // A budget below what is spent: the next task asks before it starts.
    const budget = app.page.getByRole('textbox', { name: 'Daily budget, in dollars' });
    await budget.fill('0.2');
    await budget.press('Enter');
    await app.page.reload();
    await expect(app.page.getByRole('textbox', { name: 'Daily budget, in dollars' })).toHaveValue('0.2');

    await app.goHome();
    await app.page.getByRole('textbox', { name: 'New task' }).fill(unique('One more look'));
    await app.page.getByLabel('Project', { exact: true }).selectOption({ label: shop.name });
    await app.page.getByRole('button', { name: 'Start', exact: true }).click();
    await expect(app.page.getByRole('status')).toContainText(
      'You have spent $0.25 today; your daily budget is $0.20. The budget is set under Agents.',
    );
    await expect(app.page).toHaveURL(/#\/$/);
    await app.page.getByRole('button', { name: 'Start anyway' }).click();
    await app.page.waitForURL(/#\/task\/[^/]+\/code$/);
    await app.turnEnds();

    // With the question turned off, a task starts at once.
    await open(app, 'Agents');
    await app.page.getByRole('checkbox', { name: 'Ask before starting a new task once it is reached' }).uncheck();
    await app.goHome();
    await app.createTask({ text: unique('And another'), project: shop.name });
  });
});

test.describe('Handing a task to another agent', () => {
  test.use({ twoAgents: true });

  test('W-04 the agent of a task is changed in the head of the conversation; the new one is told what was said', async ({
    app,
    core,
    project,
  }) => {
    const shop = await project();
    await app.openHome();
    const title = unique('Describe the limiter');
    await app.createTask({ text: title, folder: shop.dir });

    await app.conversation.getByLabel('The agent of this task').selectOption({ label: 'Second practice agent' });
    const second = app.page.getByRole('complementary', { name: 'Conversation with Second practice agent' });
    await expect(second).toBeVisible();
    await expect(second).toContainText(
      'The task was handed from Practice agent to Second practice agent. With your next message Second practice agent is told what was said so far.',
    );
    await expect(second.getByRole('heading', { name: 'No agent is running' })).toBeVisible();

    await second.getByRole('textbox', { name: 'Message Second practice agent' }).fill('And who calls it?');
    await second.getByRole('button', { name: 'Send', exact: true }).click();
    await expect(second.getByText('Second practice agent finished this turn.')).toBeVisible();
    // The practice agent repeats what it was sent: the earlier conversation is in it.
    await expect(second.locator('.thought').last()).toContainText(`User: ${title}`);
    // The core knows who has the task now, and so does every other window.
    expect((await core.request<{ id: string; agentId: string }[]>('tasks.list')).at(-1)?.agentId).toBe('mock-2');
  });
});

test.describe('Automations', () => {
  test("W-05 W-06 the built-in ones are listed and off; a switch is kept; one of the user's own is added and removed", async ({
    app,
    project,
  }) => {
    const shop = await project();
    await app.addProject(shop.dir);
    await app.openHome();
    await open(app, 'Automations');

    const merged = app.page.locator('.auto-item', { hasText: 'When a merge request is merged' });
    await expect(merged).toContainText('Archive the task and delete its working copy and branch.');
    await expect(merged).toContainText('Built in');
    await expect(merged).toContainText('Never ran');
    const opens = app.page.locator('.auto-item', { hasText: 'When a merge request opens' });
    await expect(opens).toContainText('Start the end-to-end tests of the task');

    const turn = opens.getByRole('button', { name: /^When a merge request opens: o(n|ff)$/ });
    const before = await turn.getAttribute('aria-pressed');
    await turn.click();
    await expect(turn).toHaveAttribute('aria-pressed', before === 'true' ? 'false' : 'true');
    await app.page.reload();
    await expect(
      app.page
        .locator('.auto-item', { hasText: 'When a merge request opens' })
        .getByRole('button', { name: /: o(n|ff)$/ }),
    ).toHaveAttribute('aria-pressed', before === 'true' ? 'false' : 'true');

    // One of the user's own. A form with nothing in it says what is missing.
    await app.page.getByRole('button', { name: 'New automation' }).click();
    const form = app.page.getByRole('form', { name: 'New automation' });
    await form.getByRole('button', { name: 'Add the automation' }).click();
    await expect(app.page.getByRole('alert')).toContainText('Give the automation a name');
    const name = unique('Bugs get a test');
    await form.getByRole('textbox', { name: 'Name' }).fill(name);
    await form.getByLabel('When', { exact: true }).selectOption({ label: 'A new issue has a label' });
    await form.getByRole('textbox', { name: 'Label' }).fill('bug');
    await form
      .getByRole('textbox', { name: 'What to tell it after the issue itself' })
      .fill('Write a failing test first');
    await form.getByRole('checkbox', { name: shop.name }).check();
    await form.getByRole('button', { name: 'Add the automation' }).click();

    const own = app.page.locator('.auto-item', { hasText: name });
    await expect(own).toContainText('When a new issue has the label bug');
    await expect(own).toContainText('Write a failing test first');
    await expect(own).toContainText('Practice agent');
    await expect(own).toContainText(shop.name);
    await own.getByRole('button', { name: 'Remove' }).click();
    await expect(own).toHaveCount(0);
  });

  test('W-07 "every day at a time" starts a task at that time and writes the run down', async ({ app, project }) => {
    const shop = await project();
    await app.addProject(shop.dir);
    await app.openHome();
    await open(app, 'Automations');
    await app.page.getByRole('button', { name: 'New automation' }).click();
    const form = app.page.getByRole('form', { name: 'New automation' });
    const name = unique('Morning look');
    const what = unique('Describe the limiter by yourself');
    await form.getByRole('textbox', { name: 'Name' }).fill(name);
    // The time is now, on the clock of this browser: it is due at once.
    const now = await app.page.evaluate(() => {
      const date = new Date();
      return `${String(date.getHours()).padStart(2, '0')}:${String(date.getMinutes()).padStart(2, '0')}`;
    });
    await form.getByLabel('Time of day').fill(now);
    await form.getByRole('textbox', { name: 'What to tell it' }).fill(what);
    await form.getByRole('checkbox', { name: shop.name }).check();
    await form.getByRole('button', { name: 'Add the automation' }).click();

    const own = app.page.locator('.auto-item', { hasText: name });
    await expect(own).toContainText(`Every day at ${now}`);
    await expect(own).toContainText(`Ran just now: Started “${what}” in ${shop.name}`, { timeout: 20_000 });
    // What it started is a task like any other.
    await app.goHome();
    await expect(app.page.locator('.stage-row', { hasText: what })).toBeVisible();
    // It does not start a second one the same day.
    await app.page.waitForTimeout(4000);
    await expect(app.page.locator('.stage-row', { hasText: what })).toHaveCount(1);
  });
});
