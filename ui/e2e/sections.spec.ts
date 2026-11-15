import { expect, test, unique, type App } from './support/fixtures';

const open = async (app: App, section: 'Agents' | 'Automations' | 'Health') => {
  await app.page.getByRole('navigation', { name: 'Sections' }).getByRole('link', { name: section }).click();
  await expect(app.page.getByRole('heading', { level: 1 })).toBeVisible();
};

test.describe('Agents: who is doing what', () => {
  // What an agent is doing depends on every task there is, so this test shares its core with nobody.
  test.use({ freshCore: true });

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
    // What the agent was sent is read from its thought, and a thought is shown only with every step.
    await app.allSteps();
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

test.describe('Health: what the projects are missing', () => {
  test('W-08 W-09 findings are read from the files; Fix starts a task with what is missing; Ignore hides one', async ({
    app,
    project,
  }) => {
    // A test command but no lint: one thing is missing. (No remote, so no pipeline is expected.)
    const shop = await project({ scripts: { test: 'echo ok' } });
    await app.addProject(shop.dir);
    await app.openHome();
    await app.page.getByRole('navigation', { name: 'Sections' }).getByRole('link', { name: 'Health' }).click();

    const missing = app.page.getByRole('region', { name: 'What is missing' });
    const row = missing.locator('.finding', { hasText: shop.name });
    await expect(row).toContainText('There is no lint command');
    await expect(row).toContainText('Tests');
    await expect(app.page.getByRole('heading', { level: 1 })).toContainText(/your projects (is|are) missing$/);

    // The checks of the project, with their switches.
    const checks = app.page.getByRole('region', { name: 'Checks on every task' });
    const unit = checks.getByRole('button', { name: `Unit tests pass in ${shop.name}: on` });
    await expect(unit).toHaveAttribute('aria-pressed', 'true');
    await unit.click();
    await expect(checks.getByRole('button', { name: `Unit tests pass in ${shop.name}: off` })).toBeVisible();
    // With no test command that is on... the command is still there, so nothing new is missing.
    await expect(row).toHaveCount(1);

    // Ignore takes the finding off the list and says how many are hidden; they can be shown again.
    await row.getByRole('button', { name: 'Ignore' }).click();
    await expect(row).toHaveCount(0);
    await expect(missing).toContainText(/(One finding is|\d+ findings are) ignored\./);
    await missing.getByRole('button', { name: 'Show them again' }).click();
    await expect(row).toHaveCount(1);

    // Fix starts a task in that project and tells the agent what is missing.
    await row.getByRole('button', { name: 'Fix' }).click();
    await app.page.waitForURL(/#\/task\/[^/]+\/code$/);
    await expect(app.messages.first()).toContainText('This project has no lint command.');
    await expect(app.page.locator('.subbar .project')).toHaveText(shop.name);
  });
});

test.describe("The editor's own tools for agents", () => {
  test('W-10 W-11 a note an agent leaves is in the conversation; a problem it reports is on Health', async ({
    app,
    project,
  }) => {
    const shop = await project({ scripts: { lint: 'echo ok', test: 'echo ok' } });
    await app.openHome();
    const title = unique('Describe the limiter');
    await app.createTask({ text: title, folder: shop.dir });

    await app.say('Please leave a note for whoever comes next');
    const note = app.conversation.locator('.team-note');
    await expect(note).toContainText('Note · Practice agent, for whoever works on this next');
    await expect(note).toContainText('The limit lives in config/limits.ts. Do not put the number back.');
    await expect(app.answers.last()).toHaveText('The note is kept with the task. Whoever works on it next is told.');

    await app.say('Please report a problem you noticed');
    await app.page.getByRole('navigation', { name: 'Sections' }).getByRole('link', { name: 'Health' }).click();
    const row = app.page
      .getByRole('region', { name: 'What is missing' })
      .locator('.finding', { hasText: 'POST /register has no rate limit' })
      .filter({ hasText: shop.name });
    await expect(row).toContainText('Noticed');
    await expect(row).toContainText(`Noticed by Practice agent while it worked on “${title}”.`);
  });
});

test.describe('Rows are as tall as what they say', () => {
  test('W-14 a finding of Health and an automation take no more room than their words', async ({ app, project }) => {
    // A project with a test command and no pipeline: Health finds something to say about it.
    const shop = await project({ scripts: { test: 'node --test' } });
    await app.addProject(shop.dir);
    await app.openHome();

    await open(app, 'Health');
    const finding = app.page.locator('.finding').first();
    await expect(finding).toBeVisible();
    // A title over one line of detail, with its buttons beside them: well under a hundred pixels.
    expect((await finding.boundingBox())!.height).toBeLessThan(100);

    await open(app, 'Automations');
    const automation = app.page.locator('.auto-item').first();
    await expect(automation).toBeVisible();
    expect((await automation.boundingBox())!.height).toBeLessThan(100);
  });
});

test.describe('Coming back to a screen', () => {
  /** The words a screen shows while it has nothing yet. */
  const EMPTY = [
    'Reading the issues',
    'Looking at the projects',
    'Reading the automations',
    'Reading what the agents cost',
    'Reading the settings',
    'Reading the changes',
    'Reading the code',
    'Reading what this project can run',
  ];

  /** From now on, notes every one of those words that is ever drawn, however briefly. */
  const watch = (app: App) =>
    app.page.evaluate((words) => {
      const seen = new Set<string>();
      const look = () => {
        const text = document.body.innerText;
        for (const word of words) if (text.includes(word)) seen.add(word);
      };
      new MutationObserver(look).observe(document.body, { childList: true, subtree: true, characterData: true });
      (window as unknown as { emptySeen: Set<string> }).emptySeen = seen;
    }, EMPTY);
  const seenEmpty = (app: App) =>
    app.page.evaluate(() => [...(window as unknown as { emptySeen: Set<string> }).emptySeen]);

  test('W-18 a section opens on what it showed last time, never on an empty screen', async ({ app, project }) => {
    const shop = await project();
    await app.openHome();
    await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    const sections = app.page.getByRole('navigation', { name: 'Sections' });
    const visit = async (name: string, shows: string | RegExp) => {
      await sections.getByRole('link', { name }).click();
      await expect(app.page.getByRole('main')).toContainText(shows);
    };
    const all = async () => {
      await visit('Issues', 'Synced');
      await visit('Health', /missing|Nothing is missing/i);
      await visit('Automations', 'When a merge request is merged');
      await visit('Agents', 'Practice agent');
      await app.page.getByRole('link', { name: 'Settings' }).click();
      await expect(app.page.getByRole('main')).toContainText('Where the data lives');
    };

    // The first time each one has to read: that is where the words of an empty screen belong.
    await all();
    await watch(app);
    // The second time each is there at once.
    await all();
    expect(await seenEmpty(app)).toEqual([]);
  });

  test('W-19 the views of a task open on what they showed last time too', async ({ app, project }) => {
    const shop = await project();
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Change the limit'), folder: shop.dir, until: 'question' });
    await app.answer('Allow this edit');
    await app.turnEnds();
    const all = async () => {
      await app.showView('Changes');
      await expect(app.page.getByText('Not committed').first()).toBeVisible();
      await app.showView('Map');
      await expect(app.page.getByRole('region', { name: 'Map of this task' })).toContainText('rateLimit.ts');
      await app.showView('Code');
      await app.openBottom('Run');
      await expect(app.bottom).toContainText(/dev|compose/i);
    };
    await all();
    // Away from the task altogether, and back: the task's screen is made anew.
    await app.goHome();
    await app.openTask(taskId);
    await watch(app);
    await all();
    expect(await seenEmpty(app)).toEqual([]);
  });
});

test.describe('Setting the editor up with an agent', () => {
  test.use({ freshCore: true });

  test('W-17 the offer is a button on the home screen and never a form that opens by itself', async ({ app }) => {
    await app.openHome();
    // An editor with nothing in it yet: the first run has its own dialogs, so this one waits to be asked for.
    const offer = app.page.getByRole('button', { name: 'Set up with an agent' });
    const form = app.page.getByRole('form', { name: 'Set up the editor with an agent' });
    await expect(offer).toBeVisible();
    await expect(form).toHaveCount(0);

    await offer.click();
    await expect(form.getByRole('textbox', { name: 'What does your team use?' })).toBeVisible();
    // Without a project there is only a folder to give, and starting without one says so.
    await expect(form.getByRole('combobox', { name: 'Project the agent works from' })).toHaveValue('__new__');
    await form.getByRole('button', { name: 'Start setting up' }).click();
    await expect(form.getByRole('alert')).toHaveText(
      'Give the folder of one of your repositories, for example /Users/you/code/app.',
    );
    // It can always be put away again.
    await form.getByRole('button', { name: 'Not now' }).click();
    await expect(form).toHaveCount(0);
    await expect(offer).toBeVisible();
  });
});
