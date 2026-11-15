import { AGENT_LINE, DEMO_FILE, DEMO_TEXT, expect, readIn, test, unique } from './support/fixtures';

const EDITED_TEXT = DEMO_TEXT.replace('  limit: 5,', AGENT_LINE);

test.describe('Looking back and rewinding', () => {
  test('RW-01 RW-02 a point shows what rewinding would change; Rewind puts the code back; the rewind can be undone', async ({
    app,
    project,
  }) => {
    const shop = await project();
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    const folder = await app.folderOf(taskId);
    await app.letAgentEdit('Change the limit');

    await app.conversation.getByRole('button', { name: 'Look back' }).click();
    const points = app.conversation.getByLabel('Checkpoints of this task');
    await expect(points.locator('.point')).toHaveText([
      /^\d\d:\d\dStart of the taskView$/,
      /^\d\d:\d\dBefore your messageView$/,
      /^\d\d:\d\dNow$/,
    ]);
    await expect(points.locator('.point-said')).toHaveText([/^Describe the limiter/, 'Change the limit']);
    await expect(points.locator('.activity')).toContainText(`changed${DEMO_FILE}+1−1`);
    // Nothing is picked yet: the middle says what to do.
    await expect(app.page.getByRole('heading', { name: 'Looking back' })).toBeVisible();

    await points
      .getByRole('button', { name: /^View the code as it was at .* before your message “Change the limit”$/ })
      .click();
    await expect(points.locator('.point[aria-current="true"]')).toContainText('You are looking at this point');
    await expect(
      app.page.getByText(
        /You are looking at the code as it was at \d\d:\d\d, before your message “Change the limit”\. Nothing has been changed yet\./,
      ),
    ).toBeVisible();
    const then = app.page.getByRole('region', { name: 'Files that were different at the chosen point' });
    await expect(then).toContainText(DEMO_FILE);
    await expect(then).toContainText('differs');
    await expect(then).toContainText('One file was different then. The rest are the same as now.');
    await expect(app.page.getByRole('heading', { name: 'What rewinding would change' })).toBeVisible();
    await expect(app.diffRow('remove', 'limit: limits.login,')).toBeVisible();
    await expect(app.diffRow('add', 'limit: 5,')).toBeVisible();
    await expect(points.locator('.activity')).toContainText('would be undone');
    // Looking changes nothing.
    expect(readIn(folder, DEMO_FILE)).toBe(EDITED_TEXT);

    const card = app.conversation.getByRole('region', { name: 'Rewind' });
    await expect(card.getByRole('heading')).toHaveText(/^Rewind to \d\d:\d\d\?$/);
    await expect(card).toContainText('Puts 1 file back as it was then.');
    await expect(card).toContainText('Commits are not touched. A rewind can itself be undone from this list.');
    await card.getByRole('button', { name: 'Rewind', exact: true }).click();

    // Back in the conversation, with the code as it was.
    await expect(app.messageBox).toBeVisible();
    const told = app.conversation.locator('.rewound');
    await expect(told).toContainText(/You rewound the code to \d\d:\d\d \(Change the limit\): one file went back\./);
    expect(readIn(folder, DEMO_FILE)).toBe(DEMO_TEXT);
    await expect(app.messages).toHaveText([/Describe the limiter/, 'Change the limit']);

    await told.getByRole('button', { name: 'Undo this rewind' }).click();
    await expect(app.conversation.locator('.rewound')).toHaveCount(2);
    expect(readIn(folder, DEMO_FILE)).toBe(EDITED_TEXT);
  });

  test('RW-03 a rewind keeps the lines the user wrote by hand', async ({ app, project }) => {
    const shop = await project();
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    const folder = await app.folderOf(taskId);
    await app.letAgentEdit('Change the limit');
    await app.openFile(DEMO_FILE);
    await app.typeAtEndOf('windowMs: 60 * 1000,', ' // mine');
    await app.saveWithKeyboard();

    await app.conversation.getByRole('button', { name: 'Look back' }).click();
    const points = app.conversation.getByLabel('Checkpoints of this task');
    await expect(points.locator('.point')).toHaveText([
      /Start of the task/,
      /Before your message/,
      /Before your edit by hand/,
      /Now/,
    ]);
    await points.getByRole('button', { name: /before your message “Change the limit”$/ }).click();
    const card = app.conversation.getByRole('region', { name: 'Rewind' });
    const keep = card.getByRole('checkbox', { name: 'Keep my own edits to rateLimit.ts' });
    await expect(keep).toBeChecked();
    await expect(points.locator('.activity').last()).toContainText('stays');
    await card.getByRole('button', { name: 'Rewind', exact: true }).click();

    await expect(app.conversation.locator('.rewound')).toContainText('your own lines stayed');
    const text = readIn(folder, DEMO_FILE);
    expect(text).toContain('  limit: 5,');
    expect(text).toContain('windowMs: 60 * 1000, // mine');
  });

  test('RW-04 a rewind can take the later messages out of the chat', async ({ app, project }) => {
    const shop = await project();
    await app.openHome();
    const first = unique('Describe the limiter');
    const taskId = await app.createTask({ text: first, folder: shop.dir });
    const folder = await app.folderOf(taskId);
    await app.letAgentEdit('Change the limit');

    await app.conversation.getByRole('button', { name: 'Look back' }).click();
    await app.conversation
      .getByLabel('Checkpoints of this task')
      .getByRole('button', { name: /before your message “Change the limit”$/ })
      .click();
    const card = app.conversation.getByRole('region', { name: 'Rewind' });
    await card.getByRole('checkbox', { name: 'Also remove the later messages from the chat' }).check();
    await expect(card).toContainText('Practice agent is started again and told only what is left of the conversation.');
    await card.getByRole('button', { name: 'Rewind', exact: true }).click();

    await expect(app.conversation.locator('.rewound')).toContainText('and the later messages left the chat');
    await expect(app.messages).toHaveText([first]);
    await expect(app.conversation.getByRole('heading', { name: 'No agent is running' })).toBeVisible();
    expect(readIn(folder, DEMO_FILE)).toBe(DEMO_TEXT);

    // The next agent hears only what is left.
    await app.say('And who calls it?');
    // What the agent was sent is read from its thought, and a thought is shown only with every step.
    await app.allSteps();
    await expect(app.thoughts.last()).toContainText(`User: ${first}`);
    await expect(app.thoughts.last()).not.toContainText('Change the limit');
  });

  test('RW-05 "Back to now" leaves everything as it is; a folder without git has nothing to look back at', async ({
    app,
    project,
  }) => {
    const shop = await project();
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    const folder = await app.folderOf(taskId);
    await app.letAgentEdit('Change the limit');
    await app.conversation.getByRole('button', { name: 'Look back' }).click();
    await app.conversation
      .getByLabel('Checkpoints of this task')
      .getByRole('button', { name: /at the start of the task$/ })
      .click();
    await app.conversation.getByRole('region', { name: 'Rewind' }).getByRole('button', { name: 'Back to now' }).click();
    await expect(app.messageBox).toBeVisible();
    await expect(app.conversation.locator('.rewound')).toHaveCount(0);
    expect(readIn(folder, DEMO_FILE)).toBe(EDITED_TEXT);

    const plain = await project({ git: false });
    await app.goHome();
    await app.createTask({ text: unique('Describe the folder'), folder: plain.dir });
    await expect(app.conversation.getByRole('button', { name: 'Look back' })).toBeHidden();
  });
});
