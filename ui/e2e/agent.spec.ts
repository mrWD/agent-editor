import { DEMO_FILE, DEMO_TEXT, expect, readIn, test, unique } from './support/fixtures';

const AGENT_LINE_TEXT = 'limit: limits.login,';

test.describe("The agent's settings, its context and what it writes", () => {
  test('AG-01 the model is picked in the head and goes with the agent to the next task', async ({ app, project }) => {
    const shop = await project();
    await app.openHome();
    await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    const model = app.conversation.getByLabel('Model of Practice agent');
    await expect(model.getByRole('option')).toHaveText(['Practice', 'Practice fast']);

    await model.selectOption({ label: 'Practice fast' });
    await expect(model).toHaveValue('practice-fast');
    // The choice is the agent's now, not only this window's.
    await app.page.reload();
    await expect(app.conversation.getByLabel('Model of Practice agent')).toHaveValue('practice-fast');

    await app.goHome();
    await app.createTask({ text: unique('Describe the limiter again'), project: shop.name });
    await expect(app.conversation.getByLabel('Model of Practice agent')).toHaveValue('practice-fast');

    // Left as it was found, for the tests that share this core.
    await app.conversation.getByLabel('Model of Practice agent').selectOption({ label: 'Practice' });
    await expect(app.conversation.getByLabel('Model of Practice agent')).toHaveValue('practice');
  });

  test('AG-02 in the mode that edits freely nothing is asked; the next task asks again', async ({ app, project }) => {
    const shop = await project();
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    const folder = await app.folderOf(taskId);
    const mode = app.conversation.getByLabel('What Practice agent may do without asking');
    await expect(mode).toHaveValue('ask');

    await mode.selectOption({ label: 'Edit freely' });
    await expect(mode).toHaveValue('auto');
    await app.messageBox.fill('Change the limit');
    await app.sendButton.click();
    await expect(app.answers.last()).toHaveText(
      'Done. The limit now comes from config instead of a fixed 5. One file changed.',
    );
    await expect(app.question).toBeHidden();
    expect(readIn(folder, DEMO_FILE)).toContain(AGENT_LINE_TEXT);

    // Editing without a question is this task's choice only.
    await app.goHome();
    await app.createTask({ text: unique('Describe it once more'), project: shop.name });
    await expect(app.conversation.getByLabel('What Practice agent may do without asking')).toHaveValue('ask');
  });

  test('AG-03 the head says how full the context is', async ({ app, project }) => {
    const shop = await project();
    await app.openHome();
    await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    const context = app.conversation.getByText(/^Context \d+%$/);
    await expect(context).toBeVisible();
    await expect(context).toHaveAttribute('title', /^[\d,]+ of 200,000 tokens of context are in use\.$/);
    const before = Number(/\d+/.exec((await context.getAttribute('title'))!.replace(/,/g, ''))![0]);

    await app.say('And who calls it?');
    await expect
      .poll(async () => Number(/\d+/.exec((await context.getAttribute('title'))!.replace(/,/g, ''))![0]))
      .toBeGreaterThan(before);
  });

  test('AG-04 a sleeping task offers no settings; they come back with the agent, as they were', async ({
    app,
    core,
    project,
  }) => {
    const shop = await project();
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    const mode = app.conversation.getByLabel('What Practice agent may do without asking');
    await mode.selectOption({ label: 'Edit freely' });
    await expect(mode).toHaveValue('auto');

    await core.request('tasks.stopAgent', { taskId });
    await expect(app.conversation.getByRole('heading', { name: 'No agent is running' })).toBeVisible();
    await expect(mode).toBeHidden();
    await expect(app.conversation.getByLabel('Model of Practice agent')).toBeHidden();

    await app.say('And who calls it?');
    await expect(mode).toHaveValue('auto');
    await expect(app.conversation.getByLabel('Model of Practice agent')).toBeVisible();
  });

  test('AG-05 an answer in Markdown is shown as it is meant, opens the files it names and runs nothing', async ({
    app,
    project,
  }) => {
    const shop = await project();
    await app.openHome();
    await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    await app.messageBox.fill('Answer in markdown');
    await app.sendButton.click();
    await app.turnEnds();

    const answer = app.conversation.getByRole('article').last();
    await expect(answer.getByRole('heading', { name: 'What I found', level: 2 })).toBeVisible();
    await expect(answer.getByRole('listitem')).toHaveText([
      `The limiter is in ${DEMO_FILE}:3.`,
      'It allows five attempts a minute.',
    ]);
    await expect(answer.locator('strong')).toHaveText('five');
    await expect(answer.locator('pre')).toHaveText('limit: 5,\n');

    // A link out of the app opens in the browser, not in place of the app.
    const link = answer.getByRole('link', { name: 'the Express docs' });
    await expect(link).toHaveAttribute('href', 'https://expressjs.com/');
    await expect(link).toHaveAttribute('target', '_blank');
    await expect(link).toHaveAttribute('rel', /noreferrer/);

    // What the agent tried to slip in is not there and did not run.
    await expect(answer.locator('img, script')).toHaveCount(0);
    expect(await app.page.evaluate(() => (window as unknown as { hacked?: number }).hacked)).toBeUndefined();

    // The file it named opens in the editor, at the line it named.
    await answer.getByRole('link', { name: `${DEMO_FILE}:3` }).click();
    await expect(app.tab('rateLimit.ts')).toHaveAttribute('aria-selected', 'true');
    await expect(app.page.getByText('Line 3, column 1', { exact: false })).toBeVisible();
    await expect(app.editor).toContainText(DEMO_TEXT.split('\n')[2]!.trim());
  });
});

/** The smallest PNG there is: one transparent pixel, 70 bytes. */
const PIXEL = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==';

test.describe('Pictures in a message', () => {
  test('AG-10 AG-11 a picture is added, pasted or left out, goes with the message and is shown in the conversation', async ({
    app,
    project,
    browserName,
  }) => {
    const shop = await project();
    await app.openHome();
    await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    const png = (name: string) => ({ name, mimeType: 'image/png', buffer: Buffer.from(PIXEL, 'base64') });
    const picker = app.conversation.getByLabel('Pictures to add to your message');
    const waiting = app.conversation.getByRole('list', { name: 'Pictures that go with your message' });
    await expect(app.conversation.getByRole('button', { name: 'Picture', exact: true })).toBeEnabled();

    // Two pictures from the file dialog; one is left out again.
    await picker.setInputFiles([png('login errors.png'), png('second.png')]);
    await expect(waiting.getByRole('listitem')).toHaveText(['login errors.png70 bytes×', 'second.png70 bytes×']);
    await waiting.getByRole('button', { name: 'Leave second.png out' }).click();
    await expect(waiting.getByRole('listitem')).toHaveCount(1);
    // A file that is no picture is refused in words, and the picture that was there stays.
    await picker.setInputFiles({ name: 'notes.txt', mimeType: 'text/plain', buffer: Buffer.from('hello') });
    await expect(app.conversation.getByRole('alert')).toHaveText(
      'notes.txt is not a picture an agent can take. PNG, JPEG, GIF and WebP are.',
    );
    await expect(waiting.getByRole('listitem')).toHaveCount(1);

    // It goes with the message; the agent says what arrived; the conversation shows the picture itself.
    await app.messageBox.fill('What is wrong on this screen?');
    await app.sendButton.click();
    await expect(app.answers.last()).toHaveText('I see one picture: image/png, 70 bytes. Nothing was changed.');
    await expect(waiting).toBeHidden();
    await expect(app.conversation.getByRole('alert')).toBeHidden();
    const sent = app.conversation.getByRole('list', { name: 'Pictures sent with this message' });
    await expect(sent).toHaveCount(1);
    const shown = sent.getByRole('img', { name: 'login errors.png' });
    await expect(shown).toBeVisible();
    await expect.poll(() => shown.evaluate((img: HTMLImageElement) => img.complete && img.naturalWidth)).toBe(1);
    await expect(sent.getByRole('link')).toHaveAttribute('href', /^\/api\/picture\/[0-9a-f-]+\/[0-9a-f-]+\.png$/);

    // A picture pasted into the message box, or dropped onto it, goes into the message too, and needs no words
    // with it. Firefox gives a made-up paste no files, so there the same picture is dropped.
    await app.messageBox.evaluate(
      (box, [data, how]) => {
        const bytes = Uint8Array.from(atob(data!), (letter) => letter.charCodeAt(0));
        const carried = new DataTransfer();
        carried.items.add(new File([bytes], 'pasted.png', { type: 'image/png' }));
        const event =
          how === 'drop'
            ? new DragEvent('drop', { dataTransfer: carried, bubbles: true, cancelable: true })
            : new ClipboardEvent('paste', { clipboardData: carried, bubbles: true, cancelable: true });
        box.dispatchEvent(event);
      },
      [PIXEL, browserName === 'firefox' ? 'drop' : 'paste'],
    );
    await expect(waiting.getByRole('listitem')).toHaveText(['pasted.png70 bytes×']);
    await expect(app.messageBox).toHaveValue('');
    await app.sendButton.click();
    await expect(app.answers).toHaveCount(3);
    await expect(app.answers.last()).toHaveText('I see one picture: image/png, 70 bytes. Nothing was changed.');
    await expect(sent).toHaveCount(2);
    await expect(sent.last().getByRole('img', { name: 'pasted.png' })).toBeVisible();

    // The pictures are there after the page is loaded again: they are kept by the core, not by the window.
    await app.page.reload();
    await expect(sent).toHaveCount(2);
    await expect(sent.first().getByRole('img', { name: 'login errors.png' })).toBeVisible();
  });
});
