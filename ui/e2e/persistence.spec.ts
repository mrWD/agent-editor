import { readdirSync } from 'node:fs';
import path from 'node:path';
import { AGENT_LINE, DEMO_FILE, DEMO_TEXT, expect, PLAIN_ANSWER, readIn, test, unique } from './support/fixtures';

// Each of these tests stops or restarts the core, so each gets a core of its own.
test.use({ freshCore: true });

test.describe('What survives the core', () => {
  test('A-04 the page says when the core is gone, reconnects by itself and reads everything anew', async ({
    app,
    project,
    core,
  }) => {
    const shop = await project();
    const title = unique('Describe the limiter');
    await app.openHome();
    await app.createTask({ text: title, folder: shop.dir });
    await expect(app.conversation.getByText('Paused', { exact: true })).toBeVisible();
    const lost = app.page.getByRole('status').filter({ hasText: 'The core is not answering.' });
    await expect(lost).toBeHidden();

    await core.stop();
    await expect(lost).toContainText('this page reconnects by itself');

    await core.startAgain();
    await expect(lost).toBeHidden({ timeout: 20_000 });
    // Nothing was reloaded by hand: the page asked the core again and shows the task as it is now.
    await expect(app.conversation.getByText('Asleep', { exact: true })).toBeVisible();
    await expect(app.conversation.getByRole('heading', { name: 'No agent is running' })).toBeVisible();
    await expect(app.answers).toHaveText([PLAIN_ANSWER]);
    await app.goHome();
    await expect(app.stageRow('Your turn', title)).toBeVisible();
  });

  test('P-01 P-02 P-04 E-03 G-07 J-05 after a restart tasks, history, open files and line marks are there; a message wakes the task', async ({
    app,
    project,
    core,
  }) => {
    const shop = await project();
    const title = unique('Describe the limiter');
    await app.openHome();
    const taskId = await app.createTask({ text: title, folder: shop.dir });
    await app.letAgentEdit('Change the limit');
    await app.openFile('README.md');
    await app.openFile(DEMO_FILE);
    const banner = app.page.locator('.banner.review');
    await expect(banner).toBeVisible();
    await app.openTerminal();
    await app.runInTerminal('echo first-$((6*7))');
    await expect(app.terminal).toContainText('first-42');

    await core.restart();
    await app.page.reload();

    // The conversation is there as it was; no agent runs.
    await expect(app.page.getByRole('heading', { level: 1, name: title })).toBeVisible();
    await expect(app.messages).toHaveText([title, 'Change the limit']);
    await expect(app.answers).toHaveText([
      PLAIN_ANSWER,
      'Done. The limit now comes from config instead of a fixed 5. One file changed.',
    ]);
    await expect(app.conversation.getByText(`You answered “Allow this edit” to: Edit ${DEMO_FILE}.`)).toBeVisible();
    await expect(app.conversation.getByText('Asleep', { exact: true })).toBeVisible();
    await expect(app.conversation.getByRole('heading', { name: 'No agent is running' })).toBeVisible();

    // The same files are open, and the lines of the agent are still marked.
    await expect(app.page.getByRole('group', { name: 'Open files' }).getByRole('tab')).toHaveText([
      'README.md',
      'rateLimit.ts',
    ]);
    await expect(app.tab('rateLimit.ts')).toHaveAttribute('aria-selected', 'true');
    await expect(app.editor.locator('.cm-line-agent')).toHaveText(AGENT_LINE);
    await expect(banner).toContainText('Practice agent wrote the highlighted lines.');
    await expect(app.files.getByRole('button', { name: /^Changed/ })).toHaveText('Changed 1');

    // The terminal is a new shell, without an error.
    await expect(app.terminal).toHaveText(/\S/);
    await expect(app.terminal).not.toContainText('first-42');
    await expect(app.bottom.getByRole('alert')).toHaveCount(0);
    await app.runInTerminal('echo second-$((40+2))');
    await expect(app.terminal).toContainText('second-42');

    // On Home the task waits for the next word like any other; a stopped agent is not a stage of its own.
    await app.goHome();
    await expect(app.page.getByRole('heading', { level: 1 })).toHaveText(
      'Nothing needs you, one thing waits for your next word',
    );
    await expect(app.stageRow('Your turn', title)).toContainText(shop.name);
    await expect(app.rail.getByRole('link', { name: `${title}: your turn` })).toBeVisible();

    // The next message starts a new agent, which is told what was said before.
    await app.stageRow('Your turn', title).getByRole('link', { name: title }).click();
    await expect(app.answers).toHaveCount(2);
    await app.say('Describe the limiter once more');
    await expect(
      app.conversation.getByText('The agent was started again and told what was said before.'),
    ).toBeVisible();
    const told = app.thoughts.last();
    await expect(told).toContainText('This task continues an earlier conversation. What was said before:');
    await expect(told).toContainText(`User: ${title}`);
    await expect(told).toContainText(`Agent: ${PLAIN_ANSWER}`);
    await expect(told).toContainText('Now the user says: Describe the limiter once more');
    await expect(app.answers.last()).toHaveText(PLAIN_ANSWER);
    await expect(app.conversation.getByText('Paused', { exact: true })).toBeVisible();
    expect((await core.task(taskId)).state).toBe('idle');
  });

  test('P-03 a core that is killed in the middle of a question leaves the question closed and the task asleep', async ({
    app,
    project,
    core,
  }) => {
    const shop = await project();
    const title = unique('Change the limit');
    await app.openHome();
    const taskId = await app.createTask({ text: title, folder: shop.dir, until: 'question' });
    const folder = await app.folderOf(taskId);

    await core.kill();
    await core.startAgain();
    await app.page.reload();

    await expect(app.conversation.getByText(`Cancelled: Edit ${DEMO_FILE}.`)).toBeVisible();
    await expect(app.question).toBeHidden();
    await expect(app.conversation.getByText('Asleep', { exact: true })).toBeVisible();
    await expect(app.conversation.getByRole('button', { name: 'Stop this turn' })).toHaveCount(0);
    expect(readIn(folder, DEMO_FILE)).toBe(DEMO_TEXT);
    await app.goHome();
    await expect(app.page.getByRole('heading', { level: 1 })).toHaveText(
      'Nothing needs you, one thing waits for your next word',
    );

    // The task can go on.
    await app.stageRow('Your turn', title).getByRole('link', { name: title }).click();
    await expect(app.conversation.getByRole('heading', { name: 'No agent is running' })).toBeVisible();
    await app.say('Describe the limiter');
    await expect(app.answers.last()).toHaveText(PLAIN_ANSWER);
  });

  test('O-09 containers that run while the core restarts are shown as running afterwards', async ({
    app,
    project,
    core,
  }) => {
    const shop = await project();
    await app.openHome();
    await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    await app.openBottom('Run');
    await app.bottom.getByRole('button', { name: 'Start all' }).click();
    const db = app.service('db').getByText(/^localhost:\d+$/);
    await expect(db).toBeVisible();
    await expect(app.service('dev').getByRole('link', { name: 'localhost:4567' })).toBeVisible();
    const address = await db.textContent();

    await core.restart();
    await app.page.reload();
    await app.openBottom('Run');
    // The container outlived the core; the dev script was the core's own child and went with it.
    await expect(db).toHaveText(address!);
    await expect(app.service('dev')).toContainText('stopped');
    await expect(app.bottom.getByRole('button', { name: /^Run / })).toHaveText('Run 1 of 2');
  });
});

test.describe('An agent that does not start', () => {
  test.use({ brokenAgent: true });

  test('D-05 when the agent does not answer at its start no task is made and no working copy is left', async ({
    app,
    project,
    core,
  }) => {
    const shop = await project();
    await app.openHome();
    await app.page.getByRole('textbox', { name: 'New task' }).fill(unique('Describe the limiter'));
    await app.page.getByLabel('Project', { exact: true }).selectOption({ label: 'Another folder' });
    await app.page.getByLabel('Folder of the new project').fill(shop.dir);
    await app.page.getByRole('button', { name: 'Start', exact: true }).click();

    await expect(app.page.getByRole('alert')).toContainText('Practice agent did not start');
    await expect(app.page).toHaveURL(/#\/$/);
    await expect(app.page.getByRole('heading', { level: 1 })).toHaveText('No tasks yet');
    expect(await core.request<unknown[]>('tasks.list')).toEqual([]);
    // The working copy and the branch that were made for the task are gone again.
    const copies = readdirSync(path.join(core.dataDir, 'worktrees'), { recursive: true, withFileTypes: true }).filter(
      (entry) => entry.isFile(),
    );
    expect(copies).toEqual([]);
    expect(shop.git(['branch', '--list', 'task/*'])).toBe('');
    expect(shop.git(['worktree', 'list']).split('\n')).toHaveLength(1);
  });
});
