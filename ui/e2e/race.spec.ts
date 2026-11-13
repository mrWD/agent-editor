import { existsSync } from 'node:fs';
import type { Core } from './support/core';
import { AGENT, AGENT_LINE, DEMO_FILE, expect, test, unique, type App, type Project } from './support/fixtures';

/** The second practice agent, as the core with `twoAgents` names it. */
const SECOND = 'Second practice agent';
/** What the practice agent says after an edit it was allowed to make. */
const DONE = 'Done. The limit now comes from config instead of a fixed 5. One file changed.';
const NOT_ALLOWED = 'I left the file as it was, because the edit was not allowed.';

type Listed = { id: string; agentId: string; title: string; cwd: string; branch: string; race: unknown };

const pane = (app: App, agent: string) =>
  app.page.getByRole('complementary', { name: `Conversation with ${agent}`, exact: true });
const card = (app: App, agent: string) => app.page.getByRole('article', { name: `${agent}'s result`, exact: true });
const talkingTo = (app: App) => app.page.getByRole('group', { name: 'Talking to' });
const pickCard = (app: App) => app.page.getByRole('region', { name: 'Pick a result' });
const subbar = (app: App) => app.page.locator('.subbar');
const views = (app: App) => app.page.getByRole('navigation', { name: 'Views of this task' });
const raceButton = (app: App) => app.page.getByRole('button', { name: 'Race', exact: true });
/** The branches the core made for tasks of a project and has not removed. */
const taskBranches = (shop: Project) =>
  shop.git(['branch', '--list', 'task/*', '--format=%(refname:short)']).split('\n').filter(Boolean);

/** The entrants of the race with this task text, the first practice agent's first. */
async function entrants(core: Core, title: string): Promise<Listed[]> {
  const tasks = await core.request<Listed[]>('tasks.list');
  return tasks.filter((task) => task.title === title).sort((a, b) => a.agentId.localeCompare(b.agentId));
}

/**
 * Starts a race through the form on Home: both practice agents get the same
 * text. It ends on the Compare view of the first agent's task.
 */
async function startRace(app: App, options: { text: string; folder?: string; project?: string }) {
  const { page } = app;
  await page.getByRole('textbox', { name: 'New task' }).fill(options.text);
  const projects = page.getByLabel('Project', { exact: true });
  if (options.folder) {
    await projects.selectOption({ label: 'Another folder' });
    await page.getByLabel('Folder of the new project').fill(options.folder);
  } else if (options.project) {
    await projects.selectOption({ label: options.project });
  }
  await raceButton(app).click();
  const ticks = page.getByRole('group', { name: 'Agents in the race' });
  // The agent picked for one task is in already; the others are ticked by hand.
  await expect(ticks.getByRole('checkbox', { name: AGENT, exact: true })).toBeChecked();
  await expect(ticks.getByRole('checkbox', { name: SECOND })).not.toBeChecked();
  await ticks.getByRole('checkbox', { name: SECOND }).check();
  await page.getByRole('button', { name: 'Start the race' }).click();
}

/** Starts a race whose entrants only answer a question, and waits until both have. */
async function finishedRace(app: App, folder: string): Promise<{ title: string; first: Listed; second: Listed }> {
  const title = unique('Describe the limiter');
  await app.openHome();
  await startRace(app, { text: title, folder });
  await app.page.waitForURL(/#\/task\/[^/]+\/compare$/);
  await expect(subbar(app).getByRole('status')).toHaveText(/^Finished /);
  const [first, second] = await entrants(app.core, title);
  return { title, first: first!, second: second! };
}

test.describe('A race: one task, several agents', () => {
  test.use({ twoAgents: true });

  test('RC-01 a race is started from Home; both agents answer; Compare shows each with its numbers and the diff of one', async ({
    app,
    core,
    project,
  }) => {
    const shop = await project();
    await app.openHome();
    const title = unique('Fix the login limit');
    await startRace(app, { text: title, folder: shop.dir });

    // It lands on the Compare view of the first agent's task: first in the switcher, and the one shown.
    await app.page.waitForURL(/#\/task\/[^/]+\/compare$/);
    await expect(views(app).getByRole('link').first()).toHaveText('Compare');
    await expect(views(app).getByRole('link', { name: 'Compare' })).toHaveAttribute('aria-current', 'page');
    await expect(subbar(app)).toContainText('Same task, two agents, each in its own copy of the code');
    await expect(subbar(app).getByRole('status')).toHaveText('Two wait for your answer');
    await expect(card(app, AGENT)).toContainText('waits for you');
    await expect(card(app, SECOND)).toContainText('It asked a question and waits for your answer.');

    // Two ordinary tasks with the same text, each in its own working copy on its own branch.
    const [first, second] = await entrants(core, title);
    expect([first!.agentId, second!.agentId]).toEqual(['mock', 'mock-2']);
    expect(first!.cwd).not.toBe(second!.cwd);
    expect(first!.branch).not.toBe(second!.branch);
    expect(app.taskId()).toBe(first!.id);

    // The first agent may edit: its card gets what it said, how long it took and what it changed.
    await app.answer('Allow this edit');
    await expect(card(app, AGENT)).toContainText(DONE);
    await expect(card(app, AGENT)).toContainText('1 file +1 −1');
    await expect(card(app, AGENT).locator('.time')).toHaveText(/^\d+ (s|min)$/);
    await expect(card(app, AGENT)).toContainText('No checks');
    await expect(card(app, AGENT).getByRole('button', { name: 'Diff shown below' })).toHaveAttribute(
      'aria-pressed',
      'true',
    );
    await expect(subbar(app).getByRole('status')).toHaveText('One waits for your answer');

    // Below the cards: the change of the entrant on screen, line by line.
    const change = app.page.getByRole('region', { name: `${AGENT}'s change` });
    await expect(change.getByRole('heading', { name: `${AGENT}'s change` })).toBeVisible();
    await expect(change).toContainText(DEMO_FILE);
    await expect(app.diffRow('remove', 'limit: 5,')).toBeVisible();
    await expect(app.diffRow('add', AGENT_LINE.trim())).toBeVisible();
    await expect(app.diffRow('context', 'windowMs: 60 * 1000,')).toBeVisible();

    // The second agent is not allowed to: its card says so and shows no change.
    await talkingTo(app).getByRole('button', { name: SECOND }).click();
    await app.page.waitForURL(new RegExp(`#/task/${second!.id}/compare$`));
    await pane(app, SECOND).getByRole('button', { name: 'Do not edit', exact: true }).click();
    await expect(card(app, SECOND)).toContainText(NOT_ALLOWED);
    await expect(card(app, SECOND)).toContainText('No file changed');
    await expect(card(app, SECOND).locator('.time')).toHaveText(/^\d+ (s|min)$/);
    await expect(app.page.getByRole('region', { name: `${SECOND}'s change` })).toContainText(
      `${SECOND} changed no file.`,
    );
    await expect(subbar(app).getByRole('status')).toHaveText('Finished just now');
    // The first agent's numbers are still on its card, next to the second's.
    await expect(card(app, AGENT)).toContainText('1 file +1 −1');

    // The change is in the first agent's copy only.
    expect(shop.read(DEMO_FILE)).toContain('limit: 5,');
    expect(existsSync(first!.cwd) && existsSync(second!.cwd)).toBe(true);
  });

  test('RC-02 "Show diff" and "Talking to" go from one entrant to the other; "Talking to" keeps the view', async ({
    app,
    project,
  }) => {
    const shop = await project({ scripts: { lint: "echo 'no problems found'" } });
    const { first, second } = await finishedRace(app, shop.dir);

    // The first agent's task is on screen: its card is the one shown, and the conversation is with it.
    await expect(card(app, AGENT)).toHaveClass(/shown/);
    await expect(card(app, SECOND)).not.toHaveClass(/shown/);
    await expect(pane(app, AGENT)).toBeVisible();
    await expect(talkingTo(app).getByRole('button', { name: AGENT, exact: true })).toHaveAttribute(
      'aria-pressed',
      'true',
    );
    await expect(talkingTo(app).getByRole('button', { name: SECOND })).toHaveAttribute('aria-pressed', 'false');

    // "Show diff" on the other card shows that result, and the conversation follows it.
    await card(app, SECOND).getByRole('button', { name: 'Show diff' }).click();
    await app.page.waitForURL(new RegExp(`#/task/${second.id}/compare$`));
    await expect(card(app, SECOND).getByRole('button', { name: 'Diff shown below' })).toBeVisible();
    await expect(pane(app, SECOND)).toBeVisible();
    await expect(pickCard(app).getByRole('button', { name: `Use ${SECOND}'s result` })).toBeVisible();

    // "Talking to" keeps the view: from the code of one entrant to the code of the other.
    await views(app).getByRole('link', { name: /^Code/ }).click();
    await app.page.waitForURL(new RegExp(`#/task/${second.id}/code$`));
    await talkingTo(app).getByRole('button', { name: AGENT, exact: true }).click();
    await app.page.waitForURL(new RegExp(`#/task/${first.id}/code$`));
    await expect(pane(app, AGENT)).toBeVisible();
    await expect(app.files).toBeVisible();
    // The link "Race" at the head of the conversation leads back to the comparison.
    await pane(app, AGENT).getByRole('link', { name: 'Race', exact: true }).click();
    await app.page.waitForURL(new RegExp(`#/task/${first.id}/compare$`));

    // The checks of each entrant are its own: run for one, they show on its card only.
    await expect(card(app, AGENT)).toContainText('Checks not run yet');
    await card(app, AGENT).getByRole('link', { name: 'Checks not run yet' }).click();
    await app.page.waitForURL(new RegExp(`#/task/${first.id}/checks$`));
    await app.page
      .getByRole('region', { name: 'Done when' })
      .getByRole('button', { name: 'Run all', exact: true })
      .click();
    await views(app).getByRole('link', { name: 'Compare' }).click();
    await expect(card(app, AGENT)).toContainText('All checks passed');
    await expect(card(app, SECOND)).toContainText('Checks not run yet');
  });

  test('RC-03 Home lists the entrants as ordinary tasks and marks them as a race', async ({ app, project }) => {
    const shop = await project();
    const { title, first } = await finishedRace(app, shop.dir);
    await app.goHome();

    const rows = app.stage('Your turn').locator('.stage-row', { hasText: title });
    await expect(rows).toHaveCount(2);
    await expect(rows.filter({ hasText: `Race · ${AGENT}` }).first()).toBeVisible();
    await expect(rows.filter({ hasText: `Race · ${SECOND}` })).toHaveCount(1);
    // The mark opens the comparison; the name opens the task as any other.
    await rows.getByRole('link', { name: `Race · ${AGENT}`, exact: true }).click();
    await app.page.waitForURL(new RegExp(`#/task/${first.id}/compare$`));
    await expect(card(app, AGENT)).toBeVisible();
  });

  test('RC-04 picking a result asks twice, then the other entrant is gone from Home and from disk', async ({
    app,
    core,
    project,
  }) => {
    const shop = await project();
    const { title, first, second } = await finishedRace(app, shop.dir);
    expect(taskBranches(shop)).toHaveLength(2);

    const pick = pickCard(app);
    await expect(pick.getByRole('heading', { name: 'Pick a result' })).toBeVisible();
    await expect(pick).toContainText('Picking one removes the other copy of the code.');
    await expect(pick.getByRole('button', { name: `Ask ${SECOND} to review it first` })).toBeVisible();
    await expect(pick.getByRole('button', { name: 'Discard both' })).toBeVisible();

    // The first press only asks; nothing is removed by it.
    await pick.getByRole('button', { name: `Use ${AGENT}'s result` }).click();
    await expect(pick.getByRole('button', { name: 'Really remove the other copy?' })).toBeVisible();
    expect(await entrants(core, title)).toHaveLength(2);
    await pick.getByRole('button', { name: 'Really remove the other copy?' }).click();

    // The picked task is an ordinary one now: no Compare, no card, and its changes are on screen.
    await app.page.waitForURL(new RegExp(`#/task/${first.id}/changes$`));
    await expect(views(app).getByRole('link', { name: 'Compare' })).toHaveCount(0);
    await expect(pickCard(app)).toHaveCount(0);
    await expect(talkingTo(app)).toHaveCount(0);
    await expect(pane(app, AGENT)).toBeVisible();

    // The other one is archived, and its working copy and branch are removed.
    const left = await entrants(core, title);
    expect(left.map((task) => task.id)).toEqual([first.id]);
    expect(left[0]!.race).toBeNull();
    expect(existsSync(second.cwd)).toBe(false);
    expect(existsSync(first.cwd)).toBe(true);
    expect(taskBranches(shop)).toEqual([first.branch]);

    await app.goHome();
    const rows = app.page.locator('.stage-row', { hasText: title });
    await expect(rows).toHaveCount(1);
    await expect(rows.locator('.race-tag')).toHaveCount(0);
    // A link to the comparison that is no more shows the task's changes instead.
    await app.openTask(first.id, 'compare');
    await app.page.waitForURL(new RegExp(`#/task/${first.id}/changes$`));
  });

  test('RC-05 another entrant reviews a result first: it gets the change as text, in its own conversation', async ({
    app,
    project,
  }) => {
    const shop = await project();
    await app.openHome();
    const title = unique('Fix the login limit');
    await startRace(app, { text: title, folder: shop.dir });
    await app.page.waitForURL(/#\/task\/[^/]+\/compare$/);
    await app.answer('Allow this edit');
    await expect(card(app, AGENT)).toContainText('1 file +1 −1');

    // The other agent is still waiting for an answer, so it cannot review yet.
    const ask = pickCard(app).getByRole('button', { name: `Ask ${SECOND} to review it first` });
    await expect(ask).toBeDisabled();
    await expect(pickCard(app)).toContainText('Not everyone has finished.');
    await talkingTo(app).getByRole('button', { name: SECOND }).click();
    await pane(app, SECOND).getByRole('button', { name: 'Do not edit', exact: true }).click();
    await expect(card(app, SECOND)).toContainText(NOT_ALLOWED);
    // A result that changed nothing has nothing to review.
    await expect(pickCard(app).getByRole('button', { name: `Ask ${AGENT} to review it first` })).toBeDisabled();

    await talkingTo(app).getByRole('button', { name: AGENT, exact: true }).click();
    await expect(ask).toBeEnabled();
    await ask.click();
    await expect(pickCard(app).getByRole('status')).toHaveText(
      `${SECOND} is reviewing it. Its answer comes in its own conversation.`,
    );

    // The reviewer's conversation holds the request, with the change in it, and its answer.
    await talkingTo(app).getByRole('button', { name: SECOND }).click();
    const reviewer = pane(app, SECOND);
    const request = reviewer.locator('.bubble').last();
    await expect(request).toContainText(`Another agent, ${AGENT}, worked on the same task as you`);
    await expect(request).toContainText(title);
    await expect(request).toContainText(`--- ${DEMO_FILE} (modified, +1 -1)`);
    await expect(request).toContainText('-  limit: 5,');
    await expect(request).toContainText(`+${AGENT_LINE}`);
    await expect(reviewer).toContainText(`You asked for a review of ${AGENT}'s result.`);
    await expect(reviewer.getByRole('article').last()).toContainText('Nothing was changed.');

    // Nothing else changed: both are still in the race, and the reviewer's card still tells of its own work.
    await expect(card(app, SECOND)).toContainText(NOT_ALLOWED);
    await expect(card(app, AGENT)).toContainText(DONE);
    await expect(pickCard(app).getByRole('button', { name: 'Discard both' })).toBeVisible();
  });

  test('RC-06 discarding a race asks twice, then every entrant is gone from Home and from disk', async ({
    app,
    core,
    project,
  }) => {
    const shop = await project();
    const { title, first, second } = await finishedRace(app, shop.dir);

    await pickCard(app).getByRole('button', { name: 'Discard both' }).click();
    await expect(pickCard(app).getByRole('button', { name: 'Really discard?' })).toBeVisible();
    expect(await entrants(core, title)).toHaveLength(2);
    await pickCard(app).getByRole('button', { name: 'Really discard?' }).click();

    await app.page.waitForURL(/#\/$/);
    await expect(app.page.getByRole('textbox', { name: 'New task' })).toBeVisible();
    await expect(app.page.locator('.stage-row', { hasText: title })).toHaveCount(0);
    expect(await entrants(core, title)).toHaveLength(0);
    expect(existsSync(first.cwd) || existsSync(second.cwd)).toBe(false);
    expect(taskBranches(shop)).toEqual([]);
    // The project itself is as it was.
    expect(shop.git(['status', '--porcelain'])).toBe('');
  });

  test('RC-07 "Run the race again" starts a new race with the same text and agents; the old one stays', async ({
    app,
    core,
    project,
  }) => {
    const shop = await project();
    const { title, first } = await finishedRace(app, shop.dir);

    await subbar(app).getByRole('button', { name: 'Run the race again' }).click();
    await app.page.waitForURL((url) => /#\/task\/[^/]+\/compare$/.test(url.hash) && !url.hash.includes(first.id));
    await expect(subbar(app).getByRole('status')).toHaveText(/^Finished /);
    await expect(card(app, AGENT)).toBeVisible();
    await expect(card(app, SECOND)).toBeVisible();

    const all = await entrants(core, title);
    expect(all.map((task) => task.agentId)).toEqual(['mock', 'mock', 'mock-2', 'mock-2']);
    const races = new Set(all.map((task) => JSON.stringify(task.race)));
    expect(races.size).toBe(2);
    // "Talking to" of the new race offers its own two entrants only.
    await expect(talkingTo(app).getByRole('button')).toHaveCount(2);
    await app.goHome();
    await expect(app.page.locator('.stage-row', { hasText: title })).toHaveCount(4);
  });

  test('RC-08 a folder without git cannot hold a race, and the form says why', async ({ app, core, project }) => {
    const shop = await project({ git: false });
    await app.openHome();
    const title = unique('Describe the limiter');
    await startRace(app, { text: title, folder: shop.dir });

    await expect(app.page.getByRole('alert')).toContainText(
      `A race needs git: each agent works in its own copy of the code, and ${shop.name} is a folder without git`,
    );
    await expect(app.page).toHaveURL(/#\/$/);
    expect(await entrants(core, title)).toHaveLength(0);

    // With one agent ticked it is no race, and the form says so before anything starts.
    const ticks = app.page.getByRole('group', { name: 'Agents in the race' });
    await ticks.getByRole('checkbox', { name: SECOND }).uncheck();
    await app.page.getByRole('button', { name: 'Start the race' }).click();
    await expect(app.page.getByRole('alert')).toHaveText('Tick at least two agents for a race.');

    // Turning the race off gives the form for one agent back, and that works in such a folder.
    await raceButton(app).click();
    await expect(ticks).toHaveCount(0);
    await expect(app.page.getByLabel('Agent', { exact: true })).toBeVisible();
    await app.page.getByRole('button', { name: 'Start', exact: true }).click();
    await app.page.waitForURL(/#\/task\/[^/]+\/code$/);
    await app.turnEnds();
    await expect(views(app).getByRole('link', { name: 'Compare' })).toHaveCount(0);
  });
});

test.describe('With one agent there is nothing to race', () => {
  test('RC-09 the form on Home has no "Race" when only one agent is available', async ({ app, project }) => {
    const shop = await project();
    await app.openHome();
    await expect(app.page.getByLabel('Agent', { exact: true })).toBeVisible();
    await expect(raceButton(app)).toHaveCount(0);
    await expect(app.page.getByRole('group', { name: 'Agents in the race' })).toHaveCount(0);

    // The form is as it was: one task, one agent, no Compare view.
    await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    await expect(views(app).getByRole('link', { name: 'Compare' })).toHaveCount(0);
    await expect(talkingTo(app)).toHaveCount(0);
    await expect(pickCard(app)).toHaveCount(0);
  });
});
