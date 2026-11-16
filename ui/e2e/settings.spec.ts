import type { Locator, Page } from '@playwright/test';
import { chmodSync, existsSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { AGENT_BINARY, type Core } from './support/core';
import { DEMO_FILE, DEMO_TEXT, expect, pullTemplate, test, unique, writeIn, type App } from './support/fixtures';

// Settings hold for the whole core: every test here has a core of its own, so no other test feels them.
test.use({ freshCore: true });

const openSettings = async (app: App) => {
  await app.page.goto(`${app.core.url}/#/settings`);
  await expect(app.page.getByRole('heading', { level: 1, name: 'Settings' })).toBeVisible();
  await expect(app.page.getByRole('region', { name: 'Agents' })).toBeVisible();
};

/** The row of one agent in Settings. */
const agentRow = (app: App, name: string): Locator =>
  app.page.getByRole('region', { name: 'Agents' }).getByRole('group', { name, exact: true });

/** A file of the practice language, which the practice language server serves. */
const NOTES = 'notes.practice';

/** An on-off button of Settings, by what it switches. */
const switchOf = (app: App, label: string): Locator =>
  app.page.getByRole('button', { name: new RegExp(`^${label}: o(n|ff)$`) });

// ----- stand-ins -----

type Shown = { title: string; body: string; tag: string };

type StandIn = {
  __shown: (Shown & { click: () => void; closed: boolean })[];
  __asked: number;
  __focused: number;
  __away: boolean;
};

/**
 * Stands in for the browser's notifications and for "is this window in front".
 * A test browser shows no real notifications and its window is always in front,
 * so both are played here: what would be shown is noted in `__shown`, and
 * `leave` and `comeBack` say where the window is. What the "browser" was asked
 * and answered survives a reload, as it does in a real one.
 */
async function standIn(page: Page, answer: 'granted' | 'denied' | 'default' = 'granted') {
  await page.addInitScript((given) => {
    const state = window as unknown as StandIn;
    state.__shown = [];
    state.__asked = 0;
    state.__focused = 0;
    state.__away = sessionStorage.getItem('e2e.away') === '1';

    class Notice {
      static get permission() {
        return localStorage.getItem('e2e.permission') ?? 'default';
      }
      static requestPermission() {
        state.__asked += 1;
        const said = localStorage.getItem('e2e.answer') ?? given;
        if (said !== 'default') localStorage.setItem('e2e.permission', said);
        return Promise.resolve(said);
      }
      onclick: (() => void) | null = null;
      private entry: StandIn['__shown'][number];
      constructor(title: string, options?: { body?: string; tag?: string }) {
        this.entry = {
          title,
          body: options?.body ?? '',
          tag: options?.tag ?? '',
          closed: false,
          click: () => this.onclick?.(),
        };
        state.__shown.push(this.entry);
      }
      close() {
        this.entry.closed = true;
      }
    }
    Object.defineProperty(window, 'Notification', { value: Notice, configurable: true, writable: true });

    Object.defineProperty(document, 'visibilityState', {
      get: () => (state.__away ? 'hidden' : 'visible'),
      configurable: true,
    });
    Object.defineProperty(document, 'hidden', { get: () => state.__away, configurable: true });
    document.hasFocus = () => !state.__away;
    const focus = window.focus.bind(window);
    window.focus = () => {
      state.__focused += 1;
      focus();
    };
  }, answer);
}

/** Notes what the page tells the core about being in front. Call before the page is opened. */
function attentionOf(page: Page): () => boolean | null {
  let told: boolean | null = null;
  page.on('websocket', (socket) => {
    socket.on('framesent', (frame) => {
      const text = typeof frame.payload === 'string' ? frame.payload : '';
      if (text.includes('"notifications.attention"')) told = text.includes('"front":true');
    });
  });
  return () => told;
}

/** The window goes behind another one, or its tab is hidden. Waits until the core was told. */
async function leave(page: Page, told: () => boolean | null) {
  await page.evaluate(() => {
    (window as unknown as StandIn).__away = true;
    sessionStorage.setItem('e2e.away', '1');
    document.dispatchEvent(new Event('visibilitychange'));
    window.dispatchEvent(new Event('blur'));
  });
  // Generous: on a busy machine the page may need a while to get its word out.
  await expect.poll(told, { timeout: 30_000 }).toBe(false);
}

async function comeBack(page: Page, told: () => boolean | null) {
  await page.evaluate(() => {
    (window as unknown as StandIn).__away = false;
    sessionStorage.removeItem('e2e.away');
    document.dispatchEvent(new Event('visibilitychange'));
    window.dispatchEvent(new Event('focus'));
  });
  await expect.poll(told, { timeout: 30_000 }).toBe(true);
}

/** What the page has shown as notifications so far. */
const shown = (page: Page): Promise<Shown[]> =>
  page.evaluate(() => (window as unknown as StandIn).__shown.map(({ title, body, tag }) => ({ title, body, tag })));

/** Turns notifications on in Settings; the stand-in browser says yes. */
async function turnNotificationsOn(app: App) {
  await openSettings(app);
  await switchOf(app, 'Notifications').click();
  await expect(switchOf(app, 'Notifications')).toHaveAttribute('aria-pressed', 'true');
}

type Event = { type: string; requestId?: string };

/** Sends a message to a task's agent the way another window would, and comes back once the agent has it. */
const say = (core: Core, taskId: string, text: string) => core.request('tasks.prompt', { taskId, text });

/** Answers the agent's open question with "allow". */
async function allow(core: Core, taskId: string) {
  let requestId = '';
  await expect
    .poll(async () => {
      const { events } = await core.request<{ events: Event[] }>('tasks.history', { taskId });
      const answered = new Set(events.filter((event) => event.type === 'permission_resolved').map((e) => e.requestId));
      const open = events.findLast((event) => event.type === 'permission_request' && !answered.has(event.requestId));
      requestId = open?.requestId ?? '';
      return requestId;
    })
    .not.toBe('');
  await core.request('tasks.permission', { taskId, requestId, optionId: 'allow-once' });
}

const turnIsOver = (core: Core, taskId: string) =>
  expect.poll(async () => (await core.task(taskId)).state, { timeout: 20_000 }).toBe('idle');

// ----- Settings -----

test.describe('Settings: the section', () => {
  test('ST-01 the Settings button opens the section: the agents with their commands, where the data lives, and the way to the budget', async ({
    app,
    core,
  }) => {
    await app.openHome();
    const button = app.page.getByRole('link', { name: 'Settings', exact: true });
    await button.click();
    await expect(app.page).toHaveURL(/#\/settings$/);
    await expect(app.page.getByRole('heading', { level: 1, name: 'Settings' })).toBeVisible();
    await expect(button).toHaveAttribute('aria-current', 'page');

    // Every agent the editor knows, each with the command that starts it.
    const practice = agentRow(app, 'Practice agent');
    await expect(practice).toContainText('Preset');
    await expect(practice.locator('code')).toHaveText(AGENT_BINARY);
    // "Claude" alone does not say what is started: the row does.
    await expect(agentRow(app, 'Claude')).toContainText(
      'Claude Code, behind an adapter that speaks the Agent Client Protocol.',
    );
    await expect(agentRow(app, 'Claude').locator('code')).toContainText('-y @agentclientprotocol/claude-agent-acp');
    await expect(agentRow(app, 'Codex').locator('code')).toContainText('-y @agentclientprotocol/codex-acp');
    await expect(agentRow(app, 'Gemini').locator('code')).toContainText('-y @google/gemini-cli --acp');
    await expect(app.page.getByLabel('A new task starts with')).toHaveValue('');

    // The other parts are there, each with what it has at first.
    for (const part of ['Notifications', 'The computer', 'The editor', 'About']) {
      await expect(app.page.getByRole('region', { name: part, exact: true })).toBeVisible();
    }
    await expect(switchOf(app, 'Notifications')).toHaveAttribute('aria-pressed', 'false');
    await expect(switchOf(app, 'Keep the computer awake while an agent works')).toHaveAttribute(
      'aria-pressed',
      'false',
    );
    await expect(switchOf(app, 'Format files on save in new projects')).toHaveAttribute('aria-pressed', 'true');
    await expect(switchOf(app, 'Offer a rule after a correction')).toHaveAttribute('aria-pressed', 'true');

    // About: where the data lives, the version, and the daily budget's place.
    const about = app.page.getByRole('region', { name: 'About' });
    const health = (await (await fetch(`${core.url}/api/health`)).json()) as { version: string };
    await expect(about).toContainText(core.dataDir);
    await expect(about).toContainText(`version ${health.version}`);
    await about.getByRole('link', { name: 'Open Agents' }).click();
    await expect(app.page).toHaveURL(/#\/agents$/);
    await expect(app.page.getByRole('textbox', { name: 'Daily budget, in dollars' })).toBeVisible();

    // The section has an address of its own: a reload stays in it.
    await openSettings(app);
    await app.page.reload();
    await expect(app.page.getByRole('heading', { level: 1, name: 'Settings' })).toBeVisible();
  });

  test('ST-02 "Try it" says how an agent introduces itself, and what went wrong for a program that does not exist', async ({
    app,
    core,
  }) => {
    await openSettings(app);
    const practice = agentRow(app, 'Practice agent');
    await practice.getByRole('button', { name: 'Try it' }).click();
    await expect(practice.getByRole('status')).toHaveText(
      /^It started and calls itself Practice agent, version \d+\.\d+\.\d+\. It is told the conversation again after a restart, takes pictures and can use the editor's own tools\.$/,
    );

    // An agent whose program is not on this machine.
    await app.page.getByRole('button', { name: 'Add an agent' }).click();
    const form = app.page.getByRole('form', { name: 'A new agent' });
    await form.getByRole('textbox', { name: 'Name', exact: true }).fill('Ghost');
    await form.getByRole('textbox', { name: 'Program' }).fill('/nonexistent/ae-e2e/ghost');
    await form.getByRole('button', { name: 'Add the agent' }).click();
    const ghost = agentRow(app, 'Ghost');
    await expect(ghost).toContainText('Not installed');
    await expect(ghost).toContainText('Its command was not found: /nonexistent/ae-e2e/ghost');
    await ghost.getByRole('button', { name: 'Try it' }).click();
    await expect(ghost.getByRole('status')).toHaveText(
      'The program "/nonexistent/ae-e2e/ghost" was not found. Check how it is spelled, or give its full path.',
    );
    // Trying is not working: no task was made, and what the first try found is still there.
    expect(await core.request('tasks.list')).toEqual([]);
    await expect(practice.getByRole('status')).toContainText('It started and calls itself Practice agent');
  });

  test('ST-03 the command of an agent is changed, kept, and put back to the built-in one', async ({ app, project }) => {
    const shop = await project();
    await openSettings(app);
    const practice = agentRow(app, 'Practice agent');
    await practice.getByRole('button', { name: 'Change' }).click();
    const form = app.page.getByRole('form', { name: 'The command of Practice agent' });
    // The form starts with the command as it is. Only the command of a built-in agent can be changed.
    await expect(form.getByRole('textbox', { name: 'Program' })).toHaveValue(AGENT_BINARY);
    await expect(form.getByRole('textbox', { name: 'Name', exact: true })).toHaveCount(0);
    await form.getByRole('textbox', { name: 'Program' }).fill('/nonexistent/ae-e2e/agent');
    await form.getByRole('textbox', { name: 'Arguments, as on a command line' }).fill('--acp "two words"');
    await form.getByRole('textbox', { name: 'Environment: one NAME=value on a line' }).fill('just words');
    await form.getByRole('button', { name: 'Save the command' }).click();
    await expect(app.page.getByRole('alert')).toContainText('"just words" is not NAME=value');
    await form.getByRole('textbox', { name: 'Environment: one NAME=value on a line' }).fill('KEY=value');
    await form.getByRole('button', { name: 'Save the command' }).click();
    await expect(form).toHaveCount(0);

    // It holds at once: the agent is "not installed" everywhere one is picked.
    await expect(practice).toContainText('Command changed');
    await expect(practice.locator('code')).toHaveText('/nonexistent/ae-e2e/agent --acp "two words"');
    await expect(practice).toContainText('With KEY set.');
    await expect(practice).toContainText('Its command was not found: /nonexistent/ae-e2e/agent');
    await app.page.reload();
    await expect(agentRow(app, 'Practice agent')).toContainText('Command changed');
    await app.goHome();
    await expect(
      app.page.getByLabel('Agent', { exact: true }).getByRole('option', { name: 'Practice agent (not installed)' }),
    ).toBeDisabled();

    // Put back, it is the built-in command again and a task starts with it. What a try
    // found for the changed command goes with that command.
    await openSettings(app);
    await agentRow(app, 'Practice agent').getByRole('button', { name: 'Try it' }).click();
    await expect(agentRow(app, 'Practice agent').getByRole('status')).toContainText(
      'The program "/nonexistent/ae-e2e/agent" was not found.',
    );
    await agentRow(app, 'Practice agent').getByRole('button', { name: 'Put back' }).click();
    await expect(agentRow(app, 'Practice agent')).toContainText('Preset');
    await expect(agentRow(app, 'Practice agent').getByRole('status')).toHaveCount(0);
    await expect(agentRow(app, 'Practice agent').locator('code')).toHaveText(AGENT_BINARY);
    await expect(agentRow(app, 'Practice agent').getByRole('button', { name: 'Put back' })).toHaveCount(0);
    await app.goHome();
    await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
  });

  test('ST-04 ST-05 an agent of your own is added, becomes the one a new task starts with, and is removed', async ({
    app,
    core,
    project,
  }) => {
    const shop = await project();
    await openSettings(app);
    await app.page.getByRole('button', { name: 'Add an agent' }).click();
    const form = app.page.getByRole('form', { name: 'A new agent' });
    // A form with nothing in it says what is missing, one thing at a time.
    await form.getByRole('button', { name: 'Add the agent' }).click();
    await expect(app.page.getByRole('alert')).toContainText('Give the program that starts the agent');
    await form.getByRole('textbox', { name: 'Program' }).fill(AGENT_BINARY);
    await form.getByRole('button', { name: 'Add the agent' }).click();
    await expect(app.page.getByRole('alert')).toContainText('Give the agent a name');
    await form.getByRole('textbox', { name: 'Name', exact: true }).fill('Second look');
    await form.getByRole('textbox', { name: 'Environment: one NAME=value on a line' }).fill('AE_MOCK_DELAY_MS=0');
    await form.getByRole('button', { name: 'Add the agent' }).click();
    await expect(form).toHaveCount(0);

    const mine = agentRow(app, 'Second look');
    await expect(mine).toContainText('Yours');
    // The mark was left empty: it is made from the name.
    await expect(mine.locator('.chip')).toHaveText('Se');
    await expect(mine).toContainText('With AE_MOCK_DELAY_MS set.');
    await mine.getByRole('button', { name: 'Try it' }).click();
    await expect(mine.getByRole('status')).toContainText('It started and calls itself Practice agent');

    // It becomes the agent a new task starts with: Home has it picked.
    await app.page.getByLabel('A new task starts with').selectOption({ label: 'Second look' });
    await app.page.reload();
    await expect(app.page.getByLabel('A new task starts with')).toHaveValue('own-second-look');
    await app.goHome();
    await expect(app.page.getByLabel('Agent', { exact: true })).toHaveValue('own-second-look');
    const title = unique('Describe the limiter');
    await app.page.getByRole('textbox', { name: 'New task' }).fill(title);
    await app.page.getByLabel('Project', { exact: true }).selectOption({ label: 'Another folder' });
    await app.page.getByLabel('Folder of the new project').fill(shop.dir);
    await app.page.getByRole('button', { name: 'Start', exact: true }).click();
    await app.page.waitForURL(/#\/task\/[^/]+\/code$/);
    const conversation = app.page.getByRole('complementary', { name: 'Conversation with Second look' });
    await expect(conversation.getByText('Second look finished this turn.')).toBeVisible();
    const taskId = app.taskId();

    // While a task has it, it cannot be removed, and the reason is said.
    await openSettings(app);
    await agentRow(app, 'Second look').getByRole('button', { name: 'Remove' }).click();
    await expect(app.page.getByRole('alert')).toContainText(
      'Second look has one open task. Hand it to another agent or archive it first.',
    );
    await core.request('tasks.archive', { taskId });
    await agentRow(app, 'Second look').getByRole('button', { name: 'Remove' }).click();
    await expect(agentRow(app, 'Second look')).toHaveCount(0);
    // With it gone, a new task starts with the first agent that is installed again.
    await expect(app.page.getByLabel('A new task starts with')).toHaveValue('');
    await app.goHome();
    await expect(app.page.getByLabel('Agent', { exact: true })).toHaveValue('mock');
    await expect(
      app.page.getByLabel('Agent', { exact: true }).getByRole('option', { name: 'Second look' }),
    ).toHaveCount(0);
  });

  test('ST-06 formatting on save is what a new project starts with; the offer of a rule is stopped everywhere at once', async ({
    app,
    core,
    project,
  }) => {
    // A file of the practice language: its server can format, so the project's own switch shows under the code.
    const withNotes = async (name: string) => {
      const shop = await project({ name });
      shop.write(NOTES, 'alpha\nbeta\n');
      shop.git(['add', '.']);
      shop.git(['commit', '-q', '-m', 'Practice file']);
      return shop;
    };
    const formatting = (state: 'on' | 'off') => app.page.getByRole('button', { name: `Format on save: ${state}` });

    const older = await withNotes('older');
    await app.openHome();
    const olderTask = await app.createTask({ text: unique('Describe the limiter'), folder: older.dir });
    await app.letAgentEdit('Change the limit');
    const olderId = (await core.task(olderTask)).projectId;

    await openSettings(app);
    const format = switchOf(app, 'Format files on save in new projects');
    const offers = switchOf(app, 'Offer a rule after a correction');
    await format.click();
    await expect(format).toHaveAttribute('aria-pressed', 'false');
    await expect(format).toHaveText('Off');
    await offers.click();
    await expect(offers).toHaveAttribute('aria-pressed', 'false');

    // The offer is gone at once, in the project that was there too: rewriting the agent's line asks nothing.
    await app.openTask(olderTask);
    await app.openFile(DEMO_FILE);
    await app.typeAtEndOf('limit: limits.login,', ' // from config');
    await app.saveWithKeyboard();
    await expect(app.activities.filter({ has: app.page.locator('.chip.you') })).toContainText(DEMO_FILE);
    // The question would have come with the save; give it the moment it would need.
    await app.page.waitForTimeout(500);
    await expect(app.conversation.getByRole('region', { name: 'Make this a rule?' })).toBeHidden();
    expect(await core.request('rules.offers', { projectId: olderId })).toEqual({ on: false });
    // Formatting is another matter: the project that was there keeps what it had.
    await app.openFile(NOTES);
    await expect(formatting('on')).toBeVisible();

    // A project added now starts without formatting on save, and has its own switch to turn it on.
    const newer = await withNotes('newer');
    await app.goHome();
    await app.createTask({ text: unique('Describe the limiter'), folder: newer.dir });
    await app.openFile(NOTES);
    await expect(formatting('off')).toBeVisible();

    // Kept by the core: a reload shows both as they were left, and so does the core itself.
    await openSettings(app);
    await app.page.reload();
    await expect(switchOf(app, 'Format files on save in new projects')).toHaveAttribute('aria-pressed', 'false');
    await expect(switchOf(app, 'Offer a rule after a correction')).toHaveAttribute('aria-pressed', 'false');
    const kept = await core.request<{ formatOnSave: boolean; ruleOffers: boolean }>('settings.get');
    expect([kept.formatOnSave, kept.ruleOffers]).toEqual([false, false]);

    // On again, every project is asked again.
    await switchOf(app, 'Offer a rule after a correction').click();
    await expect(switchOf(app, 'Offer a rule after a correction')).toHaveAttribute('aria-pressed', 'true');
    expect(await core.request('rules.offers', { projectId: olderId })).toEqual({ on: true });
  });
});

test.describe('Settings: an agent that does not start', () => {
  test.use({ brokenAgent: true });

  test('ST-07 "Try it" on an agent that ends at once says so in words', async ({ app }) => {
    await openSettings(app);
    const practice = agentRow(app, 'Practice agent');
    await practice.getByRole('button', { name: 'Try it' }).click();
    await expect(practice.getByRole('status')).toHaveText(
      'Practice agent started and stopped again before it said anything.',
    );
  });
});

test.describe('Settings: keeping the computer awake', () => {
  // Slow enough to be caught working.
  test.use({ mockDelayMs: 400 });

  test('ST-08 the helper runs while an agent works and ends with the turn', async ({ app, core, project }) => {
    // A stand-in for the system's helper, first on the core's PATH: it notes its start and waits, as the real one does.
    const log = path.join(core.toolsDir, 'awake.log');
    for (const name of ['caffeinate', 'systemd-inhibit']) {
      const file = path.join(core.toolsDir, name);
      writeFileSync(file, '#!/bin/sh\necho "$$ $*" >> "$(dirname "$0")/awake.log"\nexec sleep 600\n');
      chmodSync(file, 0o755);
    }
    const starts = () => (existsSync(log) ? readFileSync(log, 'utf8').trim().split('\n').filter(Boolean) : []);
    const runs = (pid: number) => {
      try {
        process.kill(pid, 0);
        return true;
      } catch {
        return false;
      }
    };

    const shop = await project();
    await openSettings(app);
    const computer = app.page.getByRole('region', { name: 'The computer' });
    await expect(computer).toContainText(/Runs (caffeinate -i|systemd-inhibit) for as long as an agent is working/);
    const awake = switchOf(app, 'Keep the computer awake while an agent works');
    await awake.click();
    await expect(awake).toHaveAttribute('aria-pressed', 'true');
    // On, with nobody working: no helper yet.
    await expect(computer.getByRole('status')).toHaveCount(0);
    expect(starts()).toEqual([]);

    await app.goHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir, until: 'started' });
    await openSettings(app);
    await expect(app.page.getByRole('region', { name: 'The computer' }).getByRole('status')).toHaveText(
      'An agent is working: the computer is being kept awake right now.',
    );
    // The helper needs a moment between being started and noting itself.
    await expect.poll(starts).toHaveLength(1);
    const pid = Number(starts()[0]!.split(' ')[0]);
    expect(runs(pid)).toBe(true);

    // The turn ends: the helper is gone, and Settings says nothing about it any more.
    await turnIsOver(core, taskId);
    await expect(app.page.getByRole('region', { name: 'The computer' }).getByRole('status')).toHaveCount(0);
    await expect.poll(() => runs(pid)).toBe(false);
    expect(starts()).toHaveLength(1);
  });
});

// ----- notifications -----

test.describe('Notifications', () => {
  test('NT-01 they are off until turned on; turning them on asks the browser, and a "no" keeps them off and says why', async ({
    app,
    core,
  }) => {
    await standIn(app.page, 'denied');
    await openSettings(app);
    const all = switchOf(app, 'Notifications');
    await expect(all).toHaveAttribute('aria-pressed', 'false');
    // The four kinds cannot be on while the whole is off.
    const kinds = [
      'An agent asks a question',
      'An agent finished its turn',
      'The checks of a merge request failed',
      'A merge request became ready to merge',
    ];
    for (const kind of kinds) {
      await expect(switchOf(app, kind)).toHaveAttribute('aria-pressed', 'false');
      await expect(switchOf(app, kind)).toBeDisabled();
    }

    // The browser says no.
    await all.click();
    await expect(app.page.getByRole('region', { name: 'Notifications' }).getByRole('alert')).toContainText(
      'This browser does not allow notifications from this address.',
    );
    await expect(all).toHaveAttribute('aria-pressed', 'false');
    expect(await app.page.evaluate(() => (window as unknown as StandIn).__asked)).toBe(1);
    expect((await core.request<{ notifications: { on: boolean } }>('settings.get')).notifications.on).toBe(false);

    // The user allows them in the browser and presses again: now they are on, each kind with them.
    await app.page.evaluate(() => {
      localStorage.removeItem('e2e.permission');
      localStorage.setItem('e2e.answer', 'granted');
    });
    await all.click();
    await expect(all).toHaveAttribute('aria-pressed', 'true');
    await expect(app.page.getByRole('region', { name: 'Notifications' }).getByRole('alert')).toHaveCount(0);
    for (const kind of kinds) await expect(switchOf(app, kind)).toHaveAttribute('aria-pressed', 'true');

    // Kept by the core, not by this page.
    await app.page.reload();
    await expect(switchOf(app, 'Notifications')).toHaveAttribute('aria-pressed', 'true');
    // Off again, and the kinds go quiet with it.
    await switchOf(app, 'Notifications').click();
    await expect(switchOf(app, 'Notifications')).toHaveAttribute('aria-pressed', 'false');
    await expect(switchOf(app, kinds[0]!)).toBeDisabled();
  });

  test('NT-02 NT-03 a window that is not in front is told once that the agent asks and once that it finished; a click opens the task; a reload tells nothing again', async ({
    app,
    core,
    project,
  }) => {
    const shop = await project();
    await standIn(app.page);
    const told = attentionOf(app.page);
    await turnNotificationsOn(app);
    await app.goHome();
    const title = unique('Describe the limiter');
    const taskId = await app.createTask({ text: title, folder: shop.dir });
    // The window was in front all the time: the user saw the turn end, nothing was told.
    expect(await shown(app.page)).toEqual([]);

    // The user goes elsewhere. The agent asks a question.
    await app.goHome();
    await leave(app.page, told);
    await say(core, taskId, 'Please edit the limit');
    await expect.poll(() => shown(app.page)).toHaveLength(1);
    const [question] = await shown(app.page);
    expect(question).toMatchObject({
      title: 'Practice agent asks a question',
      body: `${title} · ${shop.name}\nEdit ${DEMO_FILE}`,
    });

    // A click on it brings the window forward and opens the task, where the question waits.
    await app.page.evaluate(() => (window as unknown as StandIn).__shown[0]!.click());
    await expect(app.page).toHaveURL(new RegExp(`#/task/${taskId}/code$`));
    await expect(app.question).toBeVisible();
    expect(await app.page.evaluate(() => (window as unknown as StandIn).__focused)).toBe(1);
    expect(await app.page.evaluate(() => (window as unknown as StandIn).__shown[0]!.closed)).toBe(true);

    // Still not in front (the stand-in says so): the end of the turn is told too, once.
    await allow(core, taskId);
    await turnIsOver(core, taskId);
    await expect.poll(() => shown(app.page)).toHaveLength(2);
    const [, ended] = await shown(app.page);
    expect(ended).toMatchObject({ title: 'Practice agent finished its turn', body: `${title} · ${shop.name}` });
    expect(ended!.tag).not.toBe(question!.tag);

    // A reload, which is also a new connection to the core, tells nothing again.
    await app.page.reload();
    await expect(app.conversation).toBeVisible();
    await expect.poll(told).toBe(false);
    await app.page.waitForTimeout(1500);
    expect(await shown(app.page)).toEqual([]);
    // What happens next is told as before.
    await say(core, taskId, 'And who calls it?');
    await turnIsOver(core, taskId);
    await expect.poll(() => shown(app.page)).toHaveLength(1);
    await app.page.waitForTimeout(500);
    expect((await shown(app.page)).map((notice) => notice.title)).toEqual(['Practice agent finished its turn']);
  });

  test('NT-04 each kind has its own switch: with "finished its turn" off, only the question is told', async ({
    app,
    core,
    project,
  }) => {
    const shop = await project();
    await standIn(app.page);
    const told = attentionOf(app.page);
    await turnNotificationsOn(app);
    const ends = switchOf(app, 'An agent finished its turn');
    await ends.click();
    await expect(ends).toHaveAttribute('aria-pressed', 'false');
    await expect(switchOf(app, 'An agent asks a question')).toHaveAttribute('aria-pressed', 'true');

    await app.goHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    await app.goHome();
    await leave(app.page, told);
    await say(core, taskId, 'Please edit the limit');
    await allow(core, taskId);
    await turnIsOver(core, taskId);
    await app.page.waitForTimeout(500);
    expect((await shown(app.page)).map((notice) => notice.title)).toEqual(['Practice agent asks a question']);
  });

  test('NT-05 nothing is told while a window is in front, not even by another window that is hidden; with none in front, one event is one notification', async ({
    app,
    core,
    project,
  }) => {
    const shop = await project();
    await standIn(app.page);
    const told = attentionOf(app.page);
    await turnNotificationsOn(app);
    await app.goHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });

    // A second window of the same browser, hidden behind the first.
    const second = await app.page.context().newPage();
    await standIn(second);
    const secondTold = attentionOf(second);
    await second.goto(`${core.url}/#/`);
    await expect(second.getByRole('textbox', { name: 'New task' })).toBeVisible();
    await leave(second, secondTold);

    // The user is looking at the task in the first window: neither window tells anything.
    await say(core, taskId, 'Please edit the limit');
    await expect(app.question).toBeVisible();
    await allow(core, taskId);
    await turnIsOver(core, taskId);
    await app.page.waitForTimeout(500);
    expect(await shown(app.page)).toEqual([]);
    expect(await shown(second)).toEqual([]);

    // Now no window is in front. Both hear of the end of the turn, under one tag: the browser shows it once.
    await leave(app.page, told);
    await say(core, taskId, 'And who calls it?');
    await turnIsOver(core, taskId);
    await expect.poll(() => shown(app.page)).toHaveLength(1);
    await expect.poll(() => shown(second)).toHaveLength(1);
    expect((await shown(second))[0]!.tag).toBe((await shown(app.page))[0]!.tag);

    // Back in front: quiet again.
    await comeBack(app.page, told);
    await say(core, taskId, 'And since when?');
    await turnIsOver(core, taskId);
    await app.page.waitForTimeout(500);
    expect(await shown(app.page)).toHaveLength(1);
    expect(await shown(second)).toHaveLength(1);
    await second.close();
  });

  test('NT-06 NT-07 failed checks and a pull request that became ready to merge are each told once; a click opens the request', async ({
    app,
    core,
    project,
  }) => {
    const shop = await project({ remote: 'github' });
    // The checks of the pull request are still running when it is opened.
    shop.setPullTemplate(pullTemplate({ checks: 'pending' }));
    await standIn(app.page);
    const told = attentionOf(app.page);
    await turnNotificationsOn(app);
    await app.goHome();
    const title = unique('Describe the limiter');
    const taskId = await app.createTask({ text: title, folder: shop.dir });
    const task = await core.task(taskId);
    writeIn(task.cwd, DEMO_FILE, DEMO_TEXT.replace('limit: 5,', 'limit: 6,'));
    await core.request('git.commit', {
      taskId,
      message: 'Raise the limit',
      picks: [{ path: DEMO_FILE }],
      amend: false,
      push: false,
    });
    await core.request('mr.create', { taskId, title: 'Raise the limit', body: '' });

    const checks = (conclusion: 'FAILURE' | 'SUCCESS' | null) => (pull: string) => {
      const request = JSON.parse(pull) as { statusCheckRollup: unknown[] };
      request.statusCheckRollup = [
        conclusion
          ? { __typename: 'CheckRun', status: 'COMPLETED', conclusion }
          : { __typename: 'CheckRun', status: 'IN_PROGRESS', conclusion: '' },
      ];
      return JSON.stringify(request);
    };

    // The user goes elsewhere. On GitHub the checks fail; nobody in the editor does anything.
    await app.goHome();
    await leave(app.page, told);
    await app.page.waitForTimeout(2000);
    expect(await shown(app.page)).toEqual([]);
    shop.changePull(task.branch!, checks('FAILURE'));
    await expect.poll(() => shown(app.page), { timeout: 15_000 }).toHaveLength(1);
    expect((await shown(app.page))[0]).toMatchObject({
      title: 'The checks of pull request 128 failed',
      body: `${title} · ${shop.name}`,
    });
    // The core looks again and again; the same failure is told once.
    await app.page.waitForTimeout(4000);
    expect(await shown(app.page)).toHaveLength(1);
    // Home, though nobody looks at it, has the task where it stands now.
    await expect(app.stageRow('Checks failed', title)).toBeVisible();

    // The agent's fix is pushed, the checks run again and pass: ready to merge, told once.
    shop.changePull(task.branch!, checks(null));
    await expect(app.stageRow('Checks running', title)).toBeVisible({ timeout: 15_000 });
    shop.changePull(task.branch!, checks('SUCCESS'));
    await expect.poll(() => shown(app.page), { timeout: 15_000 }).toHaveLength(2);
    expect((await shown(app.page))[1]).toMatchObject({
      title: 'Pull request 128 is ready to merge',
      body: `${title} · ${shop.name}`,
    });
    await app.page.waitForTimeout(4000);
    expect(await shown(app.page)).toHaveLength(2);

    // A click opens the task where the request is.
    await app.page.evaluate(() => (window as unknown as StandIn).__shown[1]!.click());
    await expect(app.page).toHaveURL(new RegExp(`#/task/${taskId}/changes$`));
    await expect(app.request).toContainText('Number 128 · open');
  });

  test('NT-08 inside the desktop app the page asks no browser and shows nothing itself: the app does', async ({
    app,
    core,
    project,
  }) => {
    const shop = await project();
    // The stand-in browser would say "no"; inside the app nobody asks it.
    await standIn(app.page, 'denied');
    const told = attentionOf(app.page);
    // The core under test runs on its own. Inside the app it says that the app shows the notices
    // ("byApp"), in the settings and with every notice: that is played here, on the way to the page.
    const fromCore: { title: string }[] = [];
    await app.page.addInitScript(() => {
      (window as unknown as { __AE_SHELL__: unknown }).__AE_SHELL__ = Object.freeze({ key: 'the-window' });
    });
    await app.page.routeWebSocket(/\/ws$/, (socket) => {
      const server = socket.connectToServer();
      socket.onMessage((message) => server.send(message));
      server.onMessage((message) => {
        if (typeof message !== 'string') {
          socket.send(message);
          return;
        }
        const parsed = JSON.parse(message) as { event?: string; data?: { title: string } };
        if (parsed.event === 'notification' && parsed.data) fromCore.push(parsed.data);
        socket.send(message.replaceAll('"byApp":false', '"byApp":true'));
      });
    });

    await openSettings(app);
    const part = app.page.getByRole('region', { name: 'Notifications' });
    await expect(part).toContainText('The app shows them as notifications of the system, with its window closed too.');
    await switchOf(app, 'Notifications').click();
    await expect(switchOf(app, 'Notifications')).toHaveAttribute('aria-pressed', 'true');
    expect(await app.page.evaluate(() => (window as unknown as StandIn).__asked)).toBe(0);
    await expect(part.getByRole('alert')).toHaveCount(0);
    await expect(part.getByRole('button', { name: 'Allow in this browser' })).toHaveCount(0);

    await app.goHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    await app.goHome();
    await leave(app.page, told);
    await say(core, taskId, 'And who calls it?');
    await turnIsOver(core, taskId);
    // The core did tell; the page left the showing to the app.
    await expect.poll(() => fromCore.map((notice) => notice.title)).toEqual(['Practice agent finished its turn']);
    await app.page.waitForTimeout(300);
    expect(await shown(app.page)).toEqual([]);
  });
});
