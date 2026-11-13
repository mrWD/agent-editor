import { mkdirSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import type { Core } from './support/core';
import { expect, test, type App, type Project } from './support/fixtures';

const FIRST = '11111111-aaaa-4bbb-8ccc-000000000001';
const SECOND = '22222222-aaaa-4bbb-8ccc-000000000002';

/** One line of a conversation, as Claude Code writes them. */
function line(session: string, cwd: string, branch: string, at: string, kind: 'user' | 'assistant', content: unknown) {
  return JSON.stringify({
    type: kind,
    sessionId: session,
    cwd,
    gitBranch: branch,
    timestamp: at,
    message: { role: kind, content },
  });
}

/**
 * Writes two conversations where Claude Code would keep them for a project's folder. The core of a
 * test has a home folder of its own, so this is that home's `.claude`: the real one is never read.
 */
function keepConversations(core: Core, shop: Project) {
  const folder = path.join(core.root, 'home', '.claude', 'projects', '-code-shop');
  mkdirSync(folder, { recursive: true });
  const text = (words: string) => [{ type: 'text', text: words }];
  writeFileSync(
    path.join(folder, `${FIRST}.jsonl`),
    [
      line(FIRST, shop.dir, 'main', '2026-11-10T09:00:05.000Z', 'user', text('Take the login limit from config')),
      line(FIRST, shop.dir, 'fix/login-limit', '2026-11-10T09:00:09.000Z', 'assistant', [
        { type: 'text', text: 'I will run the tests first.' },
        { type: 'tool_use', id: 't1', name: 'Bash', input: { command: 'npm test' } },
      ]),
      line(FIRST, shop.dir, 'fix/login-limit', '2026-11-10T09:00:30.000Z', 'user', [
        { type: 'tool_result', tool_use_id: 't1', content: '12 passed' },
      ]),
      line(
        FIRST,
        shop.dir,
        'fix/login-limit',
        '2026-11-10T09:01:00.000Z',
        'assistant',
        text('Done: the limit comes from config now.'),
      ),
    ].join('\n'),
  );
  writeFileSync(
    path.join(folder, `${SECOND}.jsonl`),
    [
      line(SECOND, shop.dir, 'main', '2026-11-12T10:00:00.000Z', 'user', 'Why is the cart slow?'),
      line(SECOND, shop.dir, 'main', '2026-11-12T10:00:20.000Z', 'assistant', text('It asks for every price twice.')),
    ].join('\n'),
  );
}

const offer = (app: App) => app.page.getByRole('button', { name: 'Bring in a conversation from Claude Code' });
const card = (app: App) => app.page.getByRole('region', { name: 'Bring in a conversation from Claude Code' });

test.describe('A conversation of Claude Code', () => {
  test.use({ freshCore: true });

  test('BI-01 BI-02 BI-03 is listed for its project, found by its words, and brought in as a sleeping task', async ({
    app,
    core,
    project,
  }) => {
    const shop = await project();
    await app.addProject(shop.dir);
    keepConversations(core, shop);
    await app.openHome();

    // The offer is a button; the list is read when it is opened. The newest conversation comes first.
    await expect(card(app)).toHaveCount(0);
    await offer(app).click();
    const rows = card(app).getByRole('list', { name: 'Conversations of Claude Code' }).getByRole('listitem');
    await expect(rows).toHaveCount(2);
    await expect(rows.nth(0)).toContainText('Why is the cart slow?');
    await expect(rows.nth(1)).toContainText('Take the login limit from config');
    // What it ended on and where it worked stand under its first words.
    await expect(rows.nth(1)).toContainText("fix/login-limit · the project's folder");

    // Typing narrows the list, by the first words or the branch.
    const find = card(app).getByRole('searchbox', { name: 'Find a conversation' });
    await find.fill('LOGIN-limit');
    await expect(rows).toHaveCount(1);
    await find.fill('nothing like this');
    await expect(card(app)).toContainText('No conversation has those words.');
    await find.fill('');
    await expect(rows).toHaveCount(2);

    // Brought in, it is a task of the project: its words, its tool call and what the tool answered.
    await card(app).getByRole('button', { name: 'Bring in: Take the login limit from config' }).click();
    await expect(app.page).toHaveURL(/#\/task\/[^/]+\/code$/);
    const talk = app.page.getByRole('complementary', { name: /^Conversation with / });
    await expect(talk.locator('.bubble')).toHaveText(['Take the login limit from config']);
    await expect(talk).toContainText('I will run the tests first.');
    await expect(talk).toContainText('npm test');
    await expect(talk).toContainText('Done: the limit comes from config now.');
    await expect(talk).toContainText(
      'This conversation was brought in from Claude Code. Your next message wakes Claude in it.',
    );
    const task = await core.task(app.taskId());
    expect(task.title).toBe('Take the login limit from config');
    // It works where the conversation worked, not in a copy of its own, and it sleeps.
    expect(task.cwd).toBe(shop.dir);
    expect(task.state).toBe('asleep');

    // BI-03: listed again, it is the task it is now, and is opened, not brought in twice.
    await app.openHome();
    await offer(app).click();
    const again = card(app).getByRole('button', { name: 'Open the task of: Take the login limit from config' });
    await expect(again).toHaveText('Open its task');
    await again.click();
    await expect(app.page).toHaveURL(new RegExp(`#/task/${task.id}/code$`));
    const all = await core.request<{ title: string }[]>('tasks.list');
    expect(all.filter((one) => one.title === 'Take the login limit from config')).toHaveLength(1);
  });

  test('BI-04 a project Claude Code never worked in says so, and without a project the card says what to add', async ({
    app,
    project,
  }) => {
    await app.openHome();
    await offer(app).click();
    await expect(card(app)).toContainText('Add that folder as a project first');
    await card(app).getByRole('button', { name: 'Not now' }).click();
    await expect(card(app)).toHaveCount(0);

    const shop = await project();
    await app.addProject(shop.dir);
    await app.openHome();
    await offer(app).click();
    await expect(card(app)).toContainText(`Claude Code has no conversation in ${shop.dir}.`);
  });
});
