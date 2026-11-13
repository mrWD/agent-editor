import AxeBuilder from '@axe-core/playwright';
import { chmodSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { expect, test, type App } from './support/fixtures';

/**
 * The first run: what greets a user who opens the editor for the first time.
 * Three chapters, in this order: trackers, agents, code. Each is a dialog
 * that connects something, then hints on the interface itself, with "Next"
 * and "Back", that show where the connected thing now is. Each test here has
 * a core of its own that has not been through it; every other test starts
 * with it behind.
 */
test.use({ firstRun: true });

/** The dialog of a chapter. */
const dialog = (app: App) => app.page.locator('.first-run');
/** The card of the hint on screen. */
const hint = (app: App) => app.page.locator('.tour-card');
const ring = (app: App) => app.page.locator('.tour-ring');
const press = (app: App, name: string | RegExp) => dialog(app).getByRole('button', { name }).click();
const nextHint = (app: App) => hint(app).getByRole('button', { name: 'Next' }).click();

/** Opens the editor the way a new user does. */
async function open(app: App) {
  await app.page.goto(`${app.core.url}/`);
  await expect(dialog(app).getByRole('heading', { name: 'Connect your tracker' })).toBeVisible();
}

/** Whether the ring is drawn around this element: its box, a little wider. */
async function expectRingAround(app: App, selector: string) {
  await expect(ring(app)).toBeVisible();
  await expect
    .poll(async () => {
      const [around, thing] = await Promise.all([
        ring(app).boundingBox(),
        app.page.locator(selector).first().boundingBox(),
      ]);
      if (!around || !thing) return 'nothing to compare';
      const inside =
        around.x <= thing.x &&
        around.y <= thing.y &&
        around.x + around.width >= thing.x + thing.width &&
        around.y + around.height >= thing.y + thing.height;
      const close = around.width - thing.width <= 12 && around.height - thing.height <= 12;
      return inside && close ? 'around it' : `ring ${JSON.stringify(around)} thing ${JSON.stringify(thing)}`;
    })
    .toBe('around it');
}

test.describe('The first run', () => {
  test('FR-01 the editor greets a new user with the tracker first; a key Linear does not take is not kept; with nothing connected there is nothing to show', async ({
    app,
    core,
  }) => {
    await open(app);
    await expect(dialog(app).getByRole('listitem').filter({ hasText: 'Trackers' })).toHaveAttribute(
      'aria-current',
      'step',
    );
    const linear = dialog(app).getByRole('group', { name: 'Linear' });
    await expect(linear).toContainText('Not connected');

    // Linear cannot be reached from the tests: the key is refused, said so, and forgotten.
    await linear.getByLabel('Linear API key').fill('lin_api_not_a_real_key');
    await linear.getByRole('button', { name: 'Connect Linear' }).click();
    await expect(dialog(app).getByRole('alert')).toContainText('Linear did not take the key');
    await expect(linear).toContainText('Not connected');
    const status = await core.request<{ linear: { hasKey: boolean } }>('integrations.status');
    expect(status.linear.hasKey).toBe(false);

    // Nothing was connected, so there is nothing on the interface to point at: the next chapter opens.
    await expect(dialog(app).getByRole('button', { name: 'Show me where it is' })).toHaveCount(0);
    await press(app, 'Not now');
    await expect(dialog(app).getByRole('heading', { name: 'Connect your agents' })).toBeVisible();
    await expect(hint(app)).toHaveCount(0);
  });

  test('FR-02 with Linear connected, hints on the interface show where the issues are: Next, Back, and pressing the ringed thing itself', async ({
    app,
    core,
  }) => {
    // A key is kept already, as after a connection that worked.
    await core.request('integrations.setLinearKey', { key: 'lin_api_kept_for_the_test' });
    await open(app);
    await expect(dialog(app).getByRole('group', { name: 'Linear' })).toContainText('Connected');
    await press(app, 'Show me where it is');

    // The dialog is gone; the interface is there, dimmed, with one thing ringed and a card beside it.
    await expect(dialog(app)).toHaveCount(0);
    await expect(hint(app)).toContainText('Your Linear issues are in Issues');
    await expect(hint(app)).toContainText('1 of 3');
    await expectRingAround(app, '.header-nav a[href="#/issues"]');
    // Nothing else can be pressed meanwhile.
    await app.page
      .getByRole('textbox', { name: 'New task' })
      .click({ force: true, trial: false })
      .catch(() => {});
    await expect(app.page.getByRole('textbox', { name: 'New task' })).not.toBeFocused();

    // Pressing the ringed thing is the way on: it goes to its page, and the tour to its next hint.
    await app.page.locator('.header-nav a[href="#/issues"]').click();
    await app.page.waitForURL(/#\/issues$/);
    await expect(hint(app)).toContainText('The issues assigned to you');
    await expectRingAround(app, 'section.issues-linear');

    // Back and Next move between the hints, and between their pages.
    await hint(app).getByRole('button', { name: 'Back' }).click();
    await expect(hint(app)).toContainText('1 of 3');
    await app.page.waitForURL(/#\/$/);
    await nextHint(app);
    await expect(hint(app)).toContainText('2 of 3');
    await nextHint(app);
    await expect(hint(app)).toContainText('A task from an issue lives on Home');

    // The last hint closes the tour, and the next chapter opens.
    await hint(app).getByRole('button', { name: 'Got it' }).click();
    await expect(hint(app)).toHaveCount(0);
    await expect(dialog(app).getByRole('heading', { name: 'Connect your agents' })).toBeVisible();
  });

  test('FR-03 agents: the ones the editor knows, a program found on this computer, the long lists folded; then hints at where an agent is picked', async ({
    app,
    core,
  }) => {
    // A command-line agent the user already has. It is only looked for here, never started.
    const claude = path.join(core.toolsDir, 'claude');
    writeFileSync(claude, '#!/bin/sh\nexit 0\n');
    chmodSync(claude, 0o755);
    await open(app);
    await press(app, 'Not now');
    await expect(dialog(app).getByRole('heading', { name: 'Connect your agents' })).toBeVisible();

    await expect(dialog(app).getByRole('group', { name: 'Practice agent' })).toContainText('Installed');
    // The tests have no Node, so the agents the editor comes with say what they miss.
    const builtIn = dialog(app).getByRole('group', { name: 'Claude', exact: true });
    await expect(builtIn).toContainText('Not installed');
    await expect(builtIn).toContainText('npx command was not found');
    const withKey = dialog(app).getByRole('group', { name: 'Claude with an Anthropic API key' });
    await expect(withKey).toContainText('Needs Node.js');
    await expect(withKey.getByRole('button', { name: 'Set up' })).toBeDisabled();
    // Programs that are not here are not offered at all.
    await expect(dialog(app).getByRole('group', { name: 'aider in a terminal' })).toHaveCount(0);
    await expect(dialog(app).getByRole('group', { name: 'Cursor', exact: true })).toHaveCount(0);

    // The long lists show their first few; the rest is one press away. Agents from China are among them.
    const more = dialog(app).getByRole('group', { name: 'More agents that speak the protocol' });
    await expect(more.getByRole('group', { name: 'GitHub Copilot' })).toBeVisible();
    await expect(more.getByRole('group', { name: 'MiniMax Code' })).toHaveCount(0);
    await more.getByRole('button', { name: /^Show all \d+$/ }).click();
    await expect(more.getByRole('group', { name: 'MiniMax Code' })).toContainText('Needs Node.js');
    await expect(more.getByRole('group', { name: 'CodeBuddy Code' })).toBeVisible();
    const models = dialog(app).getByRole('group', { name: 'Models of other makers' });
    await expect(models.getByRole('group', { name: 'DeepSeek models' })).toBeVisible();
    await models.getByRole('button', { name: /^Show all \d+$/ }).click();
    await expect(models.getByRole('group', { name: 'OpenRouter models' })).toBeVisible();

    const found = dialog(app).getByRole('group', { name: 'Claude Code in a terminal' });
    await found.getByRole('button', { name: 'Add' }).click();
    await expect(found).toContainText('Added');
    await expect(dialog(app).getByRole('group', { name: 'Claude Code (terminal)' })).toContainText('In a terminal');
    // It is an agent like any other from now on.
    const listed = await core.request<{ id: string; speaks: string }[]>('agents.list');
    expect(listed.find((agent) => agent.id === 'own-claude-code-terminal')?.speaks).toBe('terminal');

    // The hints: where an agent is picked, what two of them allow, where they are watched and changed.
    await press(app, 'Show me where it is');
    await expect(hint(app)).toContainText('Every task goes to one agent');
    await expect(hint(app)).toContainText('Claude Code (terminal): it runs in a terminal inside the task');
    await expectRingAround(app, '.new-task select[aria-label="Agent"]');
    await nextHint(app);
    // There are two agents now, so there is a race to show.
    await expect(hint(app)).toContainText('Or to several at once');
    await expectRingAround(app, '.new-task .race-toggle');
    await nextHint(app);
    await expect(hint(app)).toContainText('Who is doing what');
    await expectRingAround(app, '.header-nav a[href="#/agents"]');
    await nextHint(app);
    await expect(hint(app)).toContainText('4 of 4');
    await expectRingAround(app, 'a[href="#/settings"]');
    await hint(app).getByRole('button', { name: 'Got it' }).click();
    await expect(dialog(app).getByRole('heading', { name: 'Connect your code' })).toBeVisible();
  });

  test('FR-04 FR-05 code: what is connected, a folder becomes a project, hints at where a task starts; then the first run is over and stays over', async ({
    app,
    project,
  }) => {
    const shop = await project();
    await open(app);
    await press(app, 'Not now');
    await press(app, 'Skip the hints');
    await expect(dialog(app).getByRole('heading', { name: 'Connect your code' })).toBeVisible();

    await expect(dialog(app).getByRole('group', { name: 'GitHub' })).toContainText('Connected');
    const gitlab = dialog(app).getByRole('group', { name: 'GitLab' });
    await expect(gitlab).toContainText('Not connected');
    await expect(gitlab).toContainText('glab auth login');

    // Nothing typed is said, not sent.
    await press(app, 'Add the project');
    await expect(dialog(app).getByRole('alert')).toContainText('Give the folder of a project');
    // Back keeps what was done, and comes back here.
    await press(app, 'Back');
    await expect(dialog(app).getByRole('heading', { name: 'Connect your agents' })).toBeVisible();
    await press(app, 'Skip the hints');
    await dialog(app).getByLabel('Folder of a project').fill(shop.dir);
    await press(app, 'Add the project');
    await expect(dialog(app).getByRole('group', { name: shop.name })).toContainText('Added');

    await press(app, 'Show me where it is');
    await expect(hint(app)).toContainText('A task starts in a project');
    await expect(hint(app)).toContainText(
      `${shop.name} is a git repository: every task gets a working copy and a branch`,
    );
    await expectRingAround(app, '.new-task select[aria-label="Project"]');
    await nextHint(app);
    await expectRingAround(app, '#task-what');
    await nextHint(app);
    await expect(hint(app)).toContainText('Every project and its tasks');
    await expectRingAround(app, 'section.rail');
    await nextHint(app);
    // GitHub is connected, so its issues are worth a hint.
    await expect(hint(app)).toContainText('Issues from GitHub');
    // The last hint has the longest button; nothing on its card is squeezed into a column of words.
    const lines = await hint(app).evaluate((card) =>
      [...card.querySelectorAll('.tour-count, .tour-foot .btn')].map((piece) => {
        const height = piece.getBoundingClientRect().height;
        return Math.round(height / Number.parseFloat(getComputedStyle(piece).lineHeight));
      }),
    );
    expect(lines.every((count) => count <= 2 && count >= 1)).toBe(true);
    expect(
      await hint(app)
        .locator('.tour-count')
        .evaluate((count) => count.getBoundingClientRect().height),
    ).toBeLessThan(24);
    // And the buttons stand in one row.
    const tops = await hint(app).evaluate((card) =>
      [...card.querySelectorAll('.tour-foot .btn')].map((button) => Math.round(button.getBoundingClientRect().top)),
    );
    expect(new Set(tops).size).toBe(1);

    // The last button of the last tour ends the first run: the editor is there to work in.
    await hint(app).getByRole('button', { name: 'Start using the editor' }).click();
    await expect(hint(app)).toHaveCount(0);
    await expect(dialog(app)).toHaveCount(0);
    await app.page.getByRole('textbox', { name: 'New task' }).fill('It can be typed into again');
    // A reload does not greet the user again.
    await app.page.reload();
    await expect(app.page.getByRole('textbox', { name: 'New task' })).toBeVisible();
    await expect(dialog(app)).toHaveCount(0);

    // Settings brings it back, from its first chapter.
    await app.page.goto(`${app.core.url}/#/settings`);
    await app.page.getByRole('button', { name: 'Open it again' }).click();
    await expect(dialog(app).getByRole('heading', { name: 'Connect your tracker' })).toBeVisible();
  });

  test('FR-06 hints can be skipped at any step, and Escape leaves them; both go on to the next chapter', async ({
    app,
  }) => {
    await open(app);
    await press(app, 'Not now');
    await press(app, 'Show me where it is');
    await expect(hint(app)).toContainText('1 of 3');
    await hint(app).getByRole('button', { name: 'Skip these hints' }).click();
    await expect(dialog(app).getByRole('heading', { name: 'Connect your code' })).toBeVisible();
    await press(app, 'Back');
    await press(app, 'Show me where it is');
    await nextHint(app);
    await app.page.keyboard.press('Escape');
    await expect(hint(app)).toHaveCount(0);
    await expect(dialog(app).getByRole('heading', { name: 'Connect your code' })).toBeVisible();
    // With no project there is nothing to show: the last chapter ends the first run.
    await press(app, 'Finish');
    await expect(dialog(app)).toHaveCount(0);
  });

  test('FR-07 FR-08 the dialogs and the hints keep the keyboard, Escape puts the first run aside, and all of it passes the accessibility rules', async ({
    app,
  }) => {
    const readable = async (what: string, selector: string) => {
      const found = await new AxeBuilder({ page: app.page })
        .include(selector)
        .withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa'])
        .analyze();
      expect(
        found.violations.flatMap((rule) => rule.nodes.map((node) => `${rule.id}: ${node.target.join(' ')}`)),
        what,
      ).toEqual([]);
    };

    await open(app);
    // A dialog starts at its heading. Going back from its first control lands on its last one,
    // not on what is behind it, and going on from the last one comes round to the first.
    await expect(dialog(app).getByRole('heading', { name: 'Connect your tracker' })).toBeFocused();
    await dialog(app).getByRole('button', { name: 'Skip the setup' }).focus();
    await app.page.keyboard.press('Shift+Tab');
    await expect(dialog(app).getByRole('button', { name: 'Not now' })).toBeFocused();
    await app.page.keyboard.press('Tab');
    await expect(dialog(app).getByRole('button', { name: 'Skip the setup' })).toBeFocused();
    await readable('Trackers', '.first-run');

    await press(app, 'Not now');
    await expect(dialog(app).getByRole('heading', { name: 'Connect your agents' })).toBeVisible();
    await readable('Agents', '.first-run');

    // A hint has the keyboard too: it starts on "Next", and Tab stays on its card.
    await press(app, 'Show me where it is');
    await expect(hint(app).getByRole('button', { name: 'Next' })).toBeFocused();
    await app.page.keyboard.press('Tab');
    await expect(hint(app).getByRole('button', { name: 'Skip these hints' })).toBeFocused();
    await app.page.keyboard.press('Shift+Tab');
    await expect(hint(app).getByRole('button', { name: 'Next' })).toBeFocused();
    await readable('A hint', '.tour-card');
    await app.page.keyboard.press('Enter');
    await expect(hint(app)).toContainText('2 of 3');
    await hint(app).getByRole('button', { name: 'Skip these hints' }).click();

    await expect(dialog(app).getByRole('heading', { name: 'Connect your code' })).toBeVisible();
    await readable('Code', '.first-run');

    await app.page.keyboard.press('Escape');
    await expect(dialog(app)).toHaveCount(0);
    await app.page.reload();
    await expect(app.page.getByRole('textbox', { name: 'New task' })).toBeVisible();
    await expect(dialog(app)).toHaveCount(0);
  });
});
