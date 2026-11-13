import AxeBuilder from '@axe-core/playwright';
import { chmodSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { expect, test, type App } from './support/fixtures';

/**
 * The first run: the dialog that greets a user who opens the editor for the
 * first time. Each test here has a core of its own that has not been through
 * it; every other test starts with it behind.
 */
test.use({ firstRun: true });

const dialog = (app: App) => app.page.getByRole('dialog');
const have = (app: App, label: string) => dialog(app).getByRole('region', { name: label });
const next = (app: App) => dialog(app).getByRole('button', { name: 'Continue' }).click();

/** Opens the editor the way a new user does. */
async function open(app: App) {
  await app.page.goto(`${app.core.url}/`);
  await expect(dialog(app).getByRole('heading', { name: 'Connect your agents' })).toBeVisible();
}

test.describe('The first run', () => {
  test('FR-01 FR-02 the editor greets a new user by itself, lists the agents, and adds a program it found on this computer', async ({
    app,
    core,
  }) => {
    // A command-line agent the user already has. It is only looked for here, never started.
    const claude = path.join(core.toolsDir, 'claude');
    writeFileSync(claude, '#!/bin/sh\nexit 0\n');
    chmodSync(claude, 0o755);
    await open(app);

    const practice = dialog(app).getByRole('group', { name: 'Practice agent' });
    await expect(practice).toContainText('Installed');
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

    // Before anything is connected, the user is told what the practice agent is good for.
    await expect(have(app, 'What your agents give you')).toContainText('The practice agent');

    const found = dialog(app).getByRole('group', { name: 'Claude Code in a terminal' });
    await found.getByRole('button', { name: 'Add' }).click();
    await expect(found).toContainText('Added');
    await expect(dialog(app).getByRole('group', { name: 'Claude Code (terminal)' })).toContainText('In a terminal');
    await expect(have(app, 'What your agents give you')).toContainText(
      'Claude Code (terminal): it runs in a terminal inside the task',
    );
    // It is an agent like any other from now on.
    const listed = await core.request<{ id: string; speaks: string }[]>('agents.list');
    expect(listed.find((agent) => agent.id === 'own-claude-code-terminal')?.speaks).toBe('terminal');
  });

  test('FR-03 the services say which are connected and how to connect the rest; a key Linear does not take is not kept', async ({
    app,
    core,
  }) => {
    await open(app);
    await next(app);
    await expect(dialog(app).getByRole('heading', { name: 'Connect your services' })).toBeVisible();

    await expect(dialog(app).getByRole('group', { name: 'GitHub' })).toContainText('Connected');
    const gitlab = dialog(app).getByRole('group', { name: 'GitLab' });
    await expect(gitlab).toContainText('Not connected');
    await expect(gitlab).toContainText('glab auth login');
    await expect(have(app, 'What your services give you')).toContainText('GitHub: the issues of your projects');

    // Linear cannot be reached from the tests: the key is refused, said so, and forgotten.
    const linear = dialog(app).getByRole('group', { name: 'Linear' });
    await linear.getByLabel('Linear API key').fill('lin_api_not_a_real_key');
    await linear.getByRole('button', { name: 'Connect Linear' }).click();
    await expect(dialog(app).getByRole('alert')).toContainText('Linear did not take the key');
    await expect(linear).toContainText('Not connected');
    const status = await core.request<{ linear: { hasKey: boolean } }>('integrations.status');
    expect(status.linear.hasKey).toBe(false);
  });

  test('FR-04 FR-05 a folder becomes a project and is described; the last step sums up, and the dialog does not come back', async ({
    app,
    project,
  }) => {
    const shop = await project();
    await open(app);
    await next(app);
    await next(app);
    await expect(dialog(app).getByRole('heading', { name: 'Add a project' })).toBeVisible();

    // Nothing typed is said, not sent.
    await dialog(app).getByRole('button', { name: 'Add the project' }).click();
    await expect(dialog(app).getByRole('alert')).toContainText('Give the folder of a project');
    await dialog(app).getByLabel('Folder of the project').fill(shop.dir);
    await dialog(app).getByRole('button', { name: 'Add the project' }).click();
    await expect(dialog(app).getByRole('group', { name: shop.name })).toContainText('Added');
    await expect(have(app, 'What the project gives you')).toContainText(
      `${shop.name} is a git repository: every task gets a working copy and a branch of its own`,
    );

    await next(app);
    await expect(dialog(app).getByRole('heading', { name: 'You are set up' })).toBeVisible();
    const all = have(app, 'Everything you have now');
    await expect(all).toContainText('The practice agent');
    await expect(all).toContainText('GitHub');
    await expect(all).toContainText(`${shop.name} is a git repository`);
    // Back keeps what was done.
    await dialog(app).getByRole('button', { name: 'Back' }).click();
    await expect(dialog(app).getByRole('group', { name: shop.name })).toContainText('Added');
    await next(app);

    await dialog(app).getByRole('button', { name: 'Start using the editor' }).click();
    await expect(dialog(app)).toHaveCount(0);
    // The project is in the editor, and a reload does not greet the user again.
    await app.page.reload();
    await expect(app.page.getByRole('textbox', { name: 'New task' })).toBeVisible();
    await expect(dialog(app)).toHaveCount(0);

    // Settings brings it back.
    await app.page.goto(`${app.core.url}/#/settings`);
    await app.page.getByRole('button', { name: 'Open it again' }).click();
    await expect(dialog(app).getByRole('heading', { name: 'Connect your agents' })).toBeVisible();
  });

  test('FR-06 the last step starts a first task with the practice agent', async ({ app, project }) => {
    const shop = await project();
    await open(app);
    await next(app);
    await next(app);
    await dialog(app).getByLabel('Folder of the project').fill(shop.dir);
    await dialog(app).getByRole('button', { name: 'Add the project' }).click();
    await expect(dialog(app).getByRole('group', { name: shop.name })).toBeVisible();
    await next(app);
    await dialog(app).getByRole('button', { name: 'Try a first task with the practice agent' }).click();
    await app.page.waitForURL(/#\/task\/[^/]+\/code$/);
    await expect(dialog(app)).toHaveCount(0);
    await expect(app.page.getByRole('complementary', { name: 'Conversation with Practice agent' })).toBeVisible();
  });

  test('FR-07 FR-08 the dialog keeps the keyboard, is put aside with Escape, and passes the accessibility rules', async ({
    app,
  }) => {
    await open(app);
    // It starts at its heading. Going back from the first control lands on the last one of the dialog,
    // not on what is behind it, and going on from the last one comes round to the first.
    await expect(dialog(app).getByRole('heading', { name: 'Connect your agents' })).toBeFocused();
    await dialog(app).getByRole('button', { name: 'Skip the setup' }).focus();
    await app.page.keyboard.press('Shift+Tab');
    await expect(dialog(app).getByRole('button', { name: 'Continue' })).toBeFocused();
    await app.page.keyboard.press('Tab');
    await expect(dialog(app).getByRole('button', { name: 'Skip the setup' })).toBeFocused();

    for (const heading of ['Connect your agents', 'Connect your services', 'Add a project', 'You are set up']) {
      await expect(dialog(app).getByRole('heading', { name: heading })).toBeVisible();
      const found = await new AxeBuilder({ page: app.page })
        .include('.first-run')
        .withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa'])
        .analyze();
      expect(
        found.violations.flatMap((rule) => rule.nodes.map((node) => `${rule.id}: ${node.target.join(' ')}`)),
        heading,
      ).toEqual([]);
      if (heading !== 'You are set up') await next(app);
    }

    await app.page.keyboard.press('Escape');
    await expect(dialog(app)).toHaveCount(0);
    await app.page.reload();
    await expect(app.page.getByRole('textbox', { name: 'New task' })).toBeVisible();
    await expect(dialog(app)).toHaveCount(0);
  });
});
