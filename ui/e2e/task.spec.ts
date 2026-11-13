import path from 'node:path';
import { AGENT_BINARY } from './support/core';
import { AGENT_LINE, DEMO_FILE, DEMO_TEXT, expect, readIn, test, unique } from './support/fixtures';

const EDITED_TEXT = DEMO_TEXT.replace('  limit: 5,', AGENT_LINE);

test.describe('A task and its conversation', () => {
  test('C-04 the agent asks, the user allows, the edit is made', async ({ app, project }) => {
    const shop = await project();
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    const folder = await app.folderOf(taskId);

    await app.askForEdit('Change the limit');
    await expect(app.question).toHaveText(`Practice agent asks: Edit ${DEMO_FILE}`);
    await expect(
      app.conversation.getByRole('heading', { name: 'Practice agent is waiting for your answer' }),
    ).toBeVisible();
    await expect(app.conversation.getByText('Waiting for you', { exact: true })).toBeVisible();
    // The question shows what would change, and nothing is changed before the answer.
    const change = app.conversation.getByLabel(/^Change to .*rateLimit\.ts$/);
    await expect(change).toContainText('limit: 5,');
    await expect(change).toContainText('limit: limits.login,');
    await expect(app.conversation.getByText('Nothing is changed until you answer.')).toBeVisible();
    for (const option of ['Allow this edit', 'Always allow edits', 'Do not edit']) {
      await expect(app.conversation.getByRole('button', { name: option, exact: true })).toBeVisible();
    }
    expect(readIn(folder, DEMO_FILE)).toBe(DEMO_TEXT);

    await app.answer('Allow this edit');
    await app.turnEnds();
    await expect(app.conversation.getByText(`You answered “Allow this edit” to: Edit ${DEMO_FILE}.`)).toBeVisible();
    await expect(app.answers.last()).toHaveText(
      'Done. The limit now comes from config instead of a fixed 5. One file changed.',
    );
    await expect(app.activities.filter({ hasText: 'edited' })).toContainText(DEMO_FILE);
    expect(readIn(folder, DEMO_FILE)).toBe(EDITED_TEXT);
    // The project's own folder is left alone: the task works in a copy.
    expect(shop.read(DEMO_FILE)).toBe(DEMO_TEXT);
  });

  test('C-05 the user refuses: the file is not touched and the agent says so', async ({ app, project }) => {
    const shop = await project();
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Change the limit'), folder: shop.dir, until: 'question' });
    await app.answer('Do not edit');
    await app.turnEnds();
    await expect(app.conversation.getByText(`You answered “Do not edit” to: Edit ${DEMO_FILE}.`)).toBeVisible();
    await expect(app.answers.last()).toHaveText('I left the file as it was, because the edit was not allowed.');
    await expect(app.activities.filter({ hasText: 'not done' })).toContainText(DEMO_FILE);
    expect(readIn(await app.folderOf(taskId), DEMO_FILE)).toBe(DEMO_TEXT);
  });

  test('C-06 C-07 "Stop this turn" ends the turn; while the agent has the turn nothing can be sent', async ({
    app,
    project,
  }) => {
    const shop = await project();
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Change the limit'), folder: shop.dir, until: 'question' });

    // The agent has the turn: a message can be written but not sent.
    await app.messageBox.fill('One more thing');
    await expect(app.sendButton).toBeDisabled();
    await app.messageBox.press('Enter');
    await expect(app.messages).toHaveCount(1);

    await app.conversation.getByRole('button', { name: 'Stop this turn' }).click();
    await expect(app.conversation.getByText(`Cancelled: Edit ${DEMO_FILE}.`)).toBeVisible();
    await expect(app.conversation.getByText('You stopped this turn.')).toBeVisible();
    await app.turnEnds();
    await expect(app.conversation.getByRole('button', { name: 'Stop this turn' })).toBeHidden();
    expect(readIn(await app.folderOf(taskId), DEMO_FILE)).toBe(DEMO_TEXT);

    // Now it is the user's turn, and the message goes out.
    await expect(app.sendButton).toBeEnabled();
    await app.messageBox.press('Enter');
    await expect(app.messages).toHaveText([/^Change the limit/, 'One more thing']);
    await app.turnEnds();
  });

  test('C-08 an empty message is not sent', async ({ app, project }) => {
    const shop = await project();
    await app.openHome();
    await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    await expect(app.sendButton).toBeDisabled();
    await app.messageBox.press('Enter');
    await app.messageBox.fill('   ');
    await expect(app.sendButton).toBeDisabled();
    await app.messageBox.press('Enter');
    await expect(app.messages).toHaveCount(1);
    await expect(app.answers).toHaveCount(1);
  });

  test('C-09 C-10 the plan, the thought and the tool calls are shown in order; a file path opens the file', async ({
    app,
    project,
  }) => {
    const shop = await project();
    const title = unique('Describe the limiter');
    await app.openHome();
    const taskId = await app.createTask({ text: title, folder: shop.dir });

    const feed = (await app.conversation.locator('.feed > *').allInnerTexts()).map((text) => text.replace(/\s+/g, ' '));
    const at = (pattern: RegExp) => feed.findIndex((entry) => pattern.test(entry));
    const order = [
      at(new RegExp(`^${title}$`)),
      at(/^Plan for this turn Read the file the task is about Say what was done$/i),
      at(new RegExp(`^The user asked: ${title}$`)),
      at(/read ?src\/middleware\/rateLimit\.ts$/),
      at(/The limiter allows five attempts a minute/),
    ];
    expect(order, feed.join(' | ')).toEqual([0, 1, 2, 3, 4]);
    // The agent names the file by its full path; the conversation shows it from the task's folder.
    const folder = await app.folderOf(taskId);
    expect(path.isAbsolute(folder)).toBe(true);
    await expect(app.conversation).not.toContainText(folder);

    // The path is a link, also from another view of the task.
    await app.showView('Changes');
    await app.activities.getByRole('button', { name: DEMO_FILE }).click();
    await expect(app.page).toHaveURL(new RegExp(`#/task/${taskId}/code$`));
    await expect(app.tab('rateLimit.ts')).toHaveAttribute('aria-selected', 'true');
    await expect(app.editor).toContainText('windowMs: 60 * 1000,');
  });

  test('C-02 two tasks of a git project have a working copy and a branch each', async ({ app, project }) => {
    const shop = await project();
    await app.openHome();
    const firstId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    await app.letAgentEdit('Change the limit');
    await app.goHome();
    const secondId = await app.createTask({ text: unique('Describe the limiter'), project: shop.name });

    const first = await app.core.task(firstId);
    const second = await app.core.task(secondId);
    expect(first.branch).toMatch(/^task\/describe-the-limiter-\d+-[0-9a-f]{4}$/);
    expect(second.branch).toMatch(/^task\/describe-the-limiter-\d+-[0-9a-f]{4}$/);
    expect(first.branch).not.toBe(second.branch);
    expect(first.cwd).not.toBe(second.cwd);
    for (const task of [first, second]) {
      expect(task.cwd.startsWith(path.join(app.core.dataDir, 'worktrees') + path.sep)).toBe(true);
    }
    // The branch is shown next to the task's name.
    await expect(app.page.locator('.subbar')).toContainText(second.branch!);
    expect(shop.git(['branch', '--list', 'task/*']).split('\n')).toHaveLength(2);

    // What the agent changed in the first task is in neither the second task nor the project itself.
    expect(readIn(first.cwd, DEMO_FILE)).toBe(EDITED_TEXT);
    expect(readIn(second.cwd, DEMO_FILE)).toBe(DEMO_TEXT);
    expect(shop.read(DEMO_FILE)).toBe(DEMO_TEXT);
  });

  test('C-03 L-16 in a folder without git the task works in the folder itself and has nothing to commit', async ({
    app,
    project,
  }) => {
    const plain = await project({ name: 'plain', git: false });
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: plain.dir });
    const task = await app.core.task(taskId);
    expect(task.branch).toBeNull();
    expect(task.cwd).toBe(plain.dir);
    await expect(app.page.locator('.subbar')).not.toContainText('task/');

    await app.letAgentEdit('Change the limit');
    expect(plain.read(DEMO_FILE)).toBe(EDITED_TEXT);

    await app.showView('Changes');
    await expect(app.page.getByRole('heading', { name: 'Nothing to commit here' })).toBeVisible();
    await expect(
      app.page.getByText('This task works in a folder without git, so there is nothing to commit here'),
    ).toBeVisible();
  });

  test('I-01 I-02 I-05 unsaved edits are saved when a message is sent, and the agent is told once', async ({
    app,
    project,
  }) => {
    const shop = await project();
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    const folder = await app.folderOf(taskId);
    await app.openFile(DEMO_FILE);
    await app.typeAtEndOf('windowMs', ' // one minute');
    await expect(app.notSaved).toBeVisible();
    await expect(app.conversation.getByText('Your unsaved edits are saved when you send')).toBeVisible();
    // While the file is being edited the conversation says so.
    await expect(app.activities.filter({ hasText: 'are editing' })).toContainText(`${DEMO_FILE}line 2`);
    expect(readIn(folder, DEMO_FILE)).toBe(DEMO_TEXT);

    await app.say('Describe the limiter again');
    expect(readIn(folder, DEMO_FILE)).toContain('windowMs: 60 * 1000, // one minute');
    await expect(app.saved).toBeVisible();
    await expect(app.activities.filter({ hasText: 'edited' })).toContainText(DEMO_FILE);
    await expect(app.conversation.getByText('Sent with a note about your edits to rateLimit.ts.')).toBeVisible();
    // The practice agent repeats what it was sent: the note of the editor came first.
    await expect(app.thoughts.last()).toContainText(
      'Note from the editor: since your last turn the user changed these files',
    );
    await expect(app.thoughts.last()).toContainText(DEMO_FILE);

    // The next message carries no note: the agent was told already.
    await app.say('And once more');
    await expect(app.thoughts.last()).toHaveText('The user asked: And once more');
    await expect(app.conversation.getByText('Sent with a note about your edits')).toHaveCount(1);
  });

  test('I-06 the search key with code selected attaches the code to the next message', async ({ app, project }) => {
    const shop = await project();
    await app.openHome();
    await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    await app.openFile(DEMO_FILE);

    // Lines 1 and 2, from the start of one line to the start of the line after.
    await app.editor.getByText('loginLimiter').click();
    await app.page.keyboard.press('Home');
    await app.page.keyboard.press('Shift+ArrowDown');
    await app.page.keyboard.press('Shift+ArrowDown');
    await app.page.keyboard.press('ControlOrMeta+k');

    const quote = app.conversation.locator('.quote');
    await expect(quote).toContainText('rateLimit.ts lines 1–2');
    await expect(app.palette).toBeHidden();
    await expect(app.messageBox).toBeFocused();
    await expect(app.messageBox).toHaveAttribute('placeholder', 'Ask Practice agent about the code you picked');

    await app.say('What does this do?');
    await expect(quote).toBeHidden();
    await expect(app.messages.last()).toHaveText(
      `About \`${DEMO_FILE}\`, lines 1–2:\n\n\`\`\`\nexport const loginLimiter = rateLimit({\n  windowMs: 60 * 1000,\n\`\`\`\n\nWhat does this do?`,
      { useInnerText: true },
    );

    // Picked code can be left out again; without a selection the key opens the search.
    await app.editor.getByText('windowMs').click();
    await app.page.keyboard.press('Home');
    await app.page.keyboard.press('Shift+End');
    await app.page.keyboard.press('ControlOrMeta+k');
    await expect(quote).toContainText('rateLimit.ts line 2');
    await quote.getByRole('button', { name: 'Leave it out' }).click();
    await expect(quote).toBeHidden();
    await app.editor.getByText('windowMs').click();
    await app.page.keyboard.press('ControlOrMeta+k');
    await expect(app.palette).toBeVisible();
    await app.page.keyboard.press('Escape');
    await expect(app.palette).toBeHidden();
  });
});

test.describe('Typing while the agent works', () => {
  // A slow practice agent, so its turn is still under way when the typing starts.
  test.use({ mockDelayMs: 250 });

  test('I-03 I-04 typing in the code stops the turn; "Let … continue" saves and goes on', async ({ app, project }) => {
    const shop = await project();
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    const folder = await app.folderOf(taskId);
    await app.openFile(DEMO_FILE);

    await app.messageBox.fill('Describe the limiter again');
    await app.sendButton.click();
    await expect(app.conversation.getByText('Working', { exact: true })).toBeVisible();
    await app.typeAtEndOf('windowMs', ' // mine');

    await expect(app.conversation.getByText('You stopped this turn.')).toBeVisible();
    await expect(app.conversation.getByText('Practice agent paused when you started typing.')).toBeVisible();
    const goOn = app.conversation.getByRole('button', { name: 'Let Practice agent continue' });
    await expect(goOn).toBeVisible();
    expect(readIn(folder, DEMO_FILE)).toBe(DEMO_TEXT);

    await goOn.click();
    await expect(app.messages.last()).toHaveText('Continue.');
    await expect(app.conversation.getByText('Sent with a note about your edits to rateLimit.ts.')).toBeVisible();
    expect(readIn(folder, DEMO_FILE)).toContain('windowMs: 60 * 1000, // mine');
    await expect(goOn).toBeHidden();
    await expect(app.thoughts.last()).toContainText('Note from the editor');
    await app.turnEnds();
    await expect(app.answers.last()).toContainText('Nothing was changed.');
  });
});

test.describe('The practice agent says what it is', () => {
  // An agent with a model behind it is used here; a core of its own keeps that from the other tests.
  test.use({ freshCore: true });

  test('C-12 C-13 a task of the practice agent says no model worked on it; one press hands it to a real agent, and the next task starts with that one', async ({
    app,
    core,
    project,
  }) => {
    const shop = await project();
    await app.openHome();
    await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    const note = app.page.getByRole('region', { name: 'About the practice agent' });
    await expect(note).toContainText('It calls no model');
    // No agent with a model is installed in the tests: the note says where one is connected.
    await expect(note).toContainText('Connect an agent with a model behind it in Settings');
    await expect(note.getByRole('button')).toHaveCount(0);

    // One is connected. An agent of the user's own counts as real: nobody knows what it costs.
    await core.request('agents.save', { name: 'Real one', short: 'Ro', program: AGENT_BINARY });
    const hand = note.getByRole('button', { name: 'Hand the task to Real one' });
    await hand.click();
    const conversation = app.page.getByRole('complementary', { name: 'Conversation with Real one' });
    await expect(conversation).toBeVisible();
    // The task is in real hands now: the note is gone.
    await expect(note).toHaveCount(0);

    // It answers, so it has been used: the next task starts with it, not with the practice agent.
    await conversation.getByRole('textbox', { name: 'Message Real one' }).fill('And now?');
    await conversation.getByRole('button', { name: 'Send', exact: true }).click();
    await expect(conversation.getByText('Real one finished this turn.')).toBeVisible();
    await app.goHome();
    await expect(app.page.getByLabel('Agent', { exact: true })).toHaveValue('own-real-one');
  });
});
