import { AGENT_BINARY } from './support/core';
import { expect, test, unique, type App } from './support/fixtures';

/**
 * An agent that does not speak the Agent Client Protocol: a program in a
 * terminal of the task. The practice agent started with `--terminal` is such
 * a program: it reads a line and answers "You said: …".
 */

/** Adds the practice agent in its terminal form as an agent of the user's own. */
async function addTerminalAgent(app: App): Promise<string> {
  // Every test has an agent of its own name: the tests of one worker share a core, and so its settings.
  const name = unique('Liner');
  await app.page.goto(`${app.core.url}/#/settings`);
  await app.page.getByRole('button', { name: 'Add an agent' }).click();
  const form = app.page.getByRole('form', { name: 'A new agent' });
  await form.getByRole('textbox', { name: 'Name', exact: true }).fill(name);
  await form.getByRole('textbox', { name: 'Program' }).fill(AGENT_BINARY);
  await form.getByRole('textbox', { name: 'Arguments, as on a command line' }).fill('--terminal');
  // The form says what the protocol is for until the box is ticked, then what a terminal means.
  await expect(form).toContainText('The command has to speak the Agent Client Protocol');
  await form.getByRole('checkbox', { name: /It runs in a terminal/ }).check();
  await expect(form).toContainText('your messages are typed into it');
  await form.getByRole('button', { name: 'Add the agent' }).click();
  await expect(form).toHaveCount(0);
  return name;
}

/** The conversation pane of the task on screen, whoever its agent is. */
const conversation = (app: App) => app.page.getByRole('complementary', { name: /^Conversation with / });
/** What the agent's terminal shows. */
const screen = (app: App) => conversation(app).locator('.agent-terminal .xterm-rows');

/** Types a line into the agent's terminal and presses Enter, as the user would. */
async function typeInto(app: App, line: string) {
  await conversation(app).locator('.agent-terminal').getByRole('textbox', { name: 'Terminal input' }).focus();
  await app.page.keyboard.type(line);
  await app.page.keyboard.press('Enter');
}

/** Starts a task with the agent in a terminal and waits until the program has gone quiet. */
async function startTask(app: App, name: string, folder: string, text: string) {
  await app.goHome();
  await app.page.getByRole('textbox', { name: 'New task' }).fill(text);
  await app.page.getByLabel('Project', { exact: true }).selectOption({ label: 'Another folder' });
  await app.page.getByLabel('Folder of the new project').fill(folder);
  await app.page.getByLabel('Agent', { exact: true }).selectOption({ label: `${name} (in a terminal)` });
  await app.page.getByRole('button', { name: 'Start', exact: true }).click();
  await app.page.waitForURL(/#\/task\/[^/]+\/code$/);
  await expect(conversation(app).getByText(`${name} has gone quiet.`)).toBeVisible();
  return app.taskId();
}

test.describe('An agent that runs in a terminal', () => {
  test('TA-01 TA-02 it is added in Settings with a tick, and "Try it" says where its program is', async ({ app }) => {
    const name = await addTerminalAgent(app);
    const row = app.page.getByRole('group', { name, exact: true });
    await expect(row).toContainText('Yours');
    await expect(row).toContainText('In a terminal');
    await row.getByRole('button', { name: 'Try it' }).click();
    await expect(row.getByRole('status')).toContainText(`${name} runs in a terminal: its program was found at`);

    // The tick is kept: the form opens with it, and it can be taken away again.
    await row.getByRole('button', { name: 'Change' }).click();
    const form = app.page.getByRole('form', { name: `The command of ${name}` });
    await expect(form.getByRole('checkbox', { name: /It runs in a terminal/ })).toBeChecked();
  });

  test('TA-03 TA-04 a task shows its terminal in place of the conversation; a message is typed into it and the quiet ends the turn', async ({
    app,
    project,
  }) => {
    const shop = await project();
    const name = await addTerminalAgent(app);
    const title = unique('Describe the limiter');
    await startTask(app, name, shop.dir, title);

    // The program got the message as if typed, and answered in its terminal.
    await expect(screen(app)).toContainText(`You said: ${title}`);
    // What the editor itself knows of the task is still said beside it.
    await expect(conversation(app).locator('.feed')).toContainText(title);

    const box = conversation(app).getByRole('textbox', { name: `Message ${name}` });
    await box.fill('WRITE notes/from-the-terminal.md written by the program');
    await conversation(app).getByRole('button', { name: 'Send', exact: true }).click();
    await expect(screen(app)).toContainText('Wrote notes/from-the-terminal.md.');
    await expect(conversation(app).getByText(`${name} has gone quiet.`)).toBeVisible();
    // What it wrote is the agent's: the file is in the list of changed files with its mark.
    const changed = app.files.getByRole('button', { name: /^Changed/ });
    await expect(changed).toHaveText('Changed 1');
    await changed.click();
    const listed = app.files.getByRole('button', { name: /^notes\/from-the-terminal\.md/ });
    await expect(listed.getByRole('img', { name: `Changed by ${name}` })).toBeVisible();
  });

  test('TA-05 TA-06 the user types into the terminal themselves; a program that ends leaves the task asleep, and the next message starts it again', async ({
    app,
    project,
  }) => {
    const shop = await project();
    const name = await addTerminalAgent(app);
    await startTask(app, name, shop.dir, unique('Say hello'));

    // Typed by hand, with Enter: the agent has the turn, then goes quiet again.
    await typeInto(app, 'typed by hand');
    await expect(screen(app)).toContainText('You said: typed by hand');
    await expect(conversation(app).getByText(`${name} has gone quiet.`)).toBeVisible();

    // Its own way out ends the program: the task is asleep and says what the next message does.
    await typeInto(app, '/exit');
    await expect(conversation(app).getByRole('heading', { name: `${name} is not running` })).toBeVisible();
    await expect(conversation(app).locator('.agent-terminal')).toHaveCount(0);

    const box = conversation(app).getByRole('textbox', { name: `Message ${name}` });
    await box.fill('Are you back?');
    await conversation(app).getByRole('button', { name: 'Send', exact: true }).click();
    // A new program in a new terminal, told what was said before and then the new message.
    await expect(screen(app)).toContainText('Now the user says: Are you back?');
    await expect(conversation(app).getByText(`${name} has gone quiet.`)).toBeVisible();
  });

  test('TA-07 "Stop this turn" presses the key that stops a program', async ({ app, project }) => {
    const shop = await project();
    const name = await addTerminalAgent(app);
    const taskId = await startTask(app, name, shop.dir, unique('Say hello'));

    // A message is on its way; the turn is the agent's until its quiet.
    await app.core.request('tasks.prompt', { taskId, text: 'One more' });
    await conversation(app).getByRole('button', { name: 'Stop this turn' }).click();
    // The practice program ends at that key, as programs that read lines do.
    await expect(conversation(app).getByRole('heading', { name: `${name} is not running` })).toBeVisible();
  });
});
