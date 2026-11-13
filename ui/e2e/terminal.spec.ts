import { execFileSync } from 'node:child_process';
import path from 'node:path';
import { expect, test, unique, type App } from './support/fixtures';

test.describe('The terminal of a task', () => {
  test('J-01 A-07 a shell runs commands in the folder of the task, without the settings of the core', async ({
    app,
    project,
  }) => {
    const shop = await project();
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    const folder = await app.folderOf(taskId);

    await app.openTerminal();
    await app.runInTerminal('echo e2e-$((6*7))');
    await expect(app.terminal).toContainText('e2e-42');
    await app.runInTerminal('echo "in-$(basename "$PWD")"');
    await expect(app.terminal).toContainText(`in-${path.basename(folder)}`);
    await app.runInTerminal('ls');
    await expect(app.terminal).toContainText('package.json');

    // The core's own port and settings are not handed to what the user runs.
    await app.runInTerminal('echo "[${AE_MOCK_DELAY_MS:-none}][${AE_LINEAR_URL:-none}][${PORT:-none}]"');
    await expect(app.terminal).toContainText('[none][none][none]');
  });

  test('J-02 coming back to the task shows the same terminal with what it printed', async ({ app, project }) => {
    const shop = await project();
    const title = unique('Describe the limiter');
    await app.openHome();
    await app.createTask({ text: title, folder: shop.dir });
    await app.openTerminal();
    await app.runInTerminal('echo first-$((6*7)); marker=kept');
    await expect(app.terminal).toContainText('first-42');

    await app.goHome();
    await app.page.getByRole('main').getByRole('link', { name: title }).click();
    await expect(app.terminal).toContainText('first-42');
    // It is the same shell, not a new one: it still knows what was set in it.
    await app.runInTerminal('echo "marker-is-$marker"');
    await expect(app.terminal).toContainText('marker-is-kept');

    // The same after the page is loaded again.
    await app.page.reload();
    await expect(app.terminal).toContainText('first-42');
    await app.runInTerminal('echo "still-$marker"');
    await expect(app.terminal).toContainText('still-kept');
  });

  test('J-03 the shell learns the new size when the panel is resized', async ({ app, project }) => {
    const shop = await project();
    await app.openHome();
    await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    await app.openTerminal();

    const rowsNow = async (label: string) => {
      await app.runInTerminal(`echo ${label}-$(stty size | cut -d' ' -f1)-rows`);
      const shown = new RegExp(`${label}-(\\d+)-rows`);
      await expect(app.terminal).toContainText(shown);
      return Number(shown.exec((await app.terminal.textContent()) ?? '')![1]);
    };
    const before = await rowsNow('before');

    const grip = app.bottom.getByRole('separator', { name: 'Drag to resize the panel' });
    const box = (await grip.boundingBox())!;
    await app.page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await app.page.mouse.down();
    await app.page.mouse.move(box.x + box.width / 2, box.y - 160, { steps: 8 });
    await app.page.mouse.up();

    await expect.poll(() => rowsNow(`after${Date.now() % 1000}`)).toBeGreaterThan(before);
  });

  test('J-04 after "exit" the terminal says the shell ended and starts a new one on request', async ({
    app,
    project,
  }) => {
    const shop = await project();
    await app.openHome();
    await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    await app.openTerminal();
    await app.runInTerminal('echo first-$((6*7))');
    await expect(app.terminal).toContainText('first-42');

    await app.runInTerminal('exit');
    await expect(app.bottom.getByText('The shell ended.')).toBeVisible();
    await app.bottom.getByRole('button', { name: 'Start a new one' }).click();
    await expect(app.bottom.getByText('The shell ended.')).toBeHidden();
    // The new shell starts on an empty screen.
    await expect(app.terminal).not.toContainText('first-42');
    await expect(app.terminal).toHaveText(/\S/);
    await app.runInTerminal('echo second-$((40+2))');
    await expect(app.terminal).toContainText('second-42');
  });
});

/** The tab of one of the task's shells, by its name. */
const shellTab = (app: App, name: string) => app.bottom.getByRole('button', { name, exact: true });
/** What the terminal on screen shows. A task can have several; the others are out of sight. */
const shown = (app: App) => app.bottom.locator('.terminal:visible .xterm-rows');

/** Opens one more shell with the plus and waits for its prompt. `name` is what its tab is to be called. */
async function addShell(app: App, name: string) {
  await app.bottom.getByRole('button', { name: 'New terminal' }).click();
  await expect(shellTab(app, name)).toHaveAttribute('aria-pressed', 'true');
  // What is typed before the shell is there goes nowhere.
  await expect(shown(app)).toHaveText(/\S/);
}

/** Types a line into the terminal on screen and presses Enter. */
async function runInShown(app: App, command: string) {
  await app.bottom.locator('.terminal:visible').getByRole('textbox', { name: 'Terminal input' }).focus();
  await app.page.keyboard.type(command);
  await app.page.keyboard.press('Enter');
}

type Listed = { terminals: { termId: number; purpose: string; number: number | null; running: string | null }[] };
/** The shells the core has for a task: their numbers, in the order they were started. */
async function shellsOf(app: App, taskId: string): Promise<(number | null)[]> {
  const listed = await app.core.request<Listed>('terminal.list', { taskId });
  return listed.terminals.filter((info) => info.purpose === 'shell').map((info) => info.number);
}

/** True while some program on this machine has exactly this command line. */
function isRunning(commandLine: string): boolean {
  const listed = execFileSync('ps', ['-ax', '-ww', '-o', 'args='], { encoding: 'utf8' });
  return listed.split('\n').some((line) => line.trim() === commandLine);
}

test.describe('Several terminals in one task', () => {
  test('TT-01 the plus opens another shell in the folder of the task, and each tab shows its own', async ({
    app,
    project,
  }) => {
    const shop = await project();
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    const folder = path.basename(await app.folderOf(taskId));

    // Closed, the row reads as it always did, with the plus next to the terminal.
    await expect(app.bottom.getByRole('button')).toHaveText(['Terminal', '', 'Run', 'Problems']);
    await app.openTerminal();
    await runInShown(app, 'mark=first; echo one-$((6*7))');
    await expect(shown(app)).toContainText('one-42');

    await addShell(app, 'Terminal 2');
    await expect(shellTab(app, 'Terminal')).toHaveAttribute('aria-pressed', 'false');
    // A new shell on an empty screen, in the same folder, that knows nothing of the first one.
    await expect(shown(app)).not.toContainText('one-42');
    await runInShown(app, 'echo "two-[$mark]-$(basename "$PWD")"');
    await expect(shown(app)).toContainText(`two-[]-${folder}`);
    expect(await shellsOf(app, taskId)).toEqual([1, 2]);

    // Each tab brings back its own terminal, with what it printed and what was set in it.
    await shellTab(app, 'Terminal').click();
    await expect(shellTab(app, 'Terminal')).toHaveAttribute('aria-pressed', 'true');
    await expect(shown(app)).toContainText('one-42');
    await expect(shown(app)).not.toContainText('two-[');
    await runInShown(app, 'echo "back-in-$mark"');
    await expect(shown(app)).toContainText('back-in-first');
    await shellTab(app, 'Terminal 2').click();
    await expect(shown(app)).toContainText(`two-[]-${folder}`);
    await expect(shown(app)).not.toContainText('back-in-first');

    // Run and Problems are where they were; coming back shows the terminal that was in front.
    await app.openBottom('Run');
    await expect(shellTab(app, 'Terminal 2')).toHaveAttribute('aria-pressed', 'false');
    await shellTab(app, 'Terminal 2').click();
    await expect(shown(app)).toContainText(`two-[]-${folder}`);
    // A press on the tab in front folds the panel away, as before.
    await shellTab(app, 'Terminal 2').click();
    await expect(shown(app)).toHaveCount(0);
    await expect(shellTab(app, 'Terminal 2')).toHaveAttribute('aria-pressed', 'false');
  });

  test('TT-02 every shell lives on in the core: after a reload each tab is back with what it printed', async ({
    app,
    project,
  }) => {
    const shop = await project();
    const title = unique('Describe the limiter');
    await app.openHome();
    const taskId = await app.createTask({ text: title, folder: shop.dir });
    await app.openTerminal();
    await runInShown(app, 'mark=first; echo one-$((6*7))');
    await expect(shown(app)).toContainText('one-42');
    await addShell(app, 'Terminal 2');
    await runInShown(app, 'mark=second; echo two-$((6*7))');
    await expect(shown(app)).toContainText('two-42');

    // Leaving the task and coming back: the same two shells, the first one in front.
    await app.goHome();
    await app.page.getByRole('main').getByRole('link', { name: title }).click();
    await expect(shellTab(app, 'Terminal')).toHaveAttribute('aria-pressed', 'true');
    await expect(shown(app)).toContainText('one-42');
    await shellTab(app, 'Terminal 2').click();
    await expect(shown(app)).toContainText('two-42');
    await runInShown(app, 'echo "it-is-$mark"');
    await expect(shown(app)).toContainText('it-is-second');

    // A shell started from another window shows up here as a tab of its own.
    await app.core.request('terminal.open', { taskId, cols: 80, rows: 24 });
    await expect(shellTab(app, 'Terminal 3')).toBeVisible();

    // The same after the page is loaded again: three tabs, each with its shell.
    await app.page.reload();
    await expect(app.bottom.getByRole('button', { name: /^Terminal/ })).toHaveText([
      'Terminal',
      'Terminal 2',
      'Terminal 3',
    ]);
    await expect(shown(app)).toContainText('one-42');
    await runInShown(app, 'echo "still-$mark"');
    await expect(shown(app)).toContainText('still-first');
    await shellTab(app, 'Terminal 2').click();
    await expect(shown(app)).toContainText('it-is-second');
    await shellTab(app, 'Terminal 3').click();
    await expect(shown(app)).toHaveText(/\S/);
    await runInShown(app, 'echo "third-[$mark]"');
    await expect(shown(app)).toContainText('third-[]');
    expect(await shellsOf(app, taskId)).toEqual([1, 2, 3]);
  });

  test('TT-03 a shell is named after the command that runs in it, and closing it asks first', async ({
    app,
    project,
  }) => {
    const shop = await project();
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    await app.openTerminal();
    await addShell(app, 'Terminal 2');

    // A command that stays. Its length is this test's own, so it can be found among the programs of the machine.
    const sleep = `sleep 3${process.pid % 1000}${String(Date.now() % 10000).padStart(4, '0')}`;
    await runInShown(app, sleep);
    await expect(shellTab(app, sleep)).toHaveAttribute('aria-pressed', 'true');
    await expect(shellTab(app, 'Terminal 2')).toHaveCount(0);
    // The other shell waits at its prompt and keeps its name.
    await expect(shellTab(app, 'Terminal')).toBeVisible();

    // Closing it asks first, and says what would be stopped.
    const question = app.bottom.getByRole('group', { name: 'Close this terminal?' });
    await app.bottom.getByRole('button', { name: `Close ${sleep}`, exact: true }).click();
    await expect(question).toContainText(`${sleep} is still running in this terminal. Closing the terminal stops it.`);
    await question.getByRole('button', { name: 'Keep it open' }).click();
    await expect(question).toBeHidden();
    await expect(shellTab(app, sleep)).toBeVisible();
    expect(isRunning(sleep)).toBe(true);
    expect(await shellsOf(app, taskId)).toEqual([1, 2]);

    // The second word closes it: the tab goes, its neighbour comes in front, and the command is stopped.
    await app.bottom.getByRole('button', { name: `Close ${sleep}`, exact: true }).click();
    await question.getByRole('button', { name: 'Close and stop it' }).click();
    await expect(question).toBeHidden();
    await expect(shellTab(app, sleep)).toHaveCount(0);
    await expect(shellTab(app, 'Terminal')).toHaveAttribute('aria-pressed', 'true');
    await expect.poll(() => isRunning(sleep)).toBe(false);
    await expect.poll(() => shellsOf(app, taskId)).toEqual([1]);

    // A shell with nothing running in it closes at once. The next new one takes the number that is free again.
    await addShell(app, 'Terminal 2');
    await app.bottom.getByRole('button', { name: 'Close Terminal 2', exact: true }).click();
    await expect(shellTab(app, 'Terminal 2')).toHaveCount(0);
    await expect(question).toBeHidden();
    await expect.poll(() => shellsOf(app, taskId)).toEqual([1]);
  });

  test('TT-04 a shell that ended keeps its tab until it is closed; closing the last one folds the panel', async ({
    app,
    project,
  }) => {
    const shop = await project();
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    await app.openTerminal();
    await runInShown(app, 'echo one-$((6*7))');
    await expect(shown(app)).toContainText('one-42');
    await addShell(app, 'Terminal 2');

    // "exit" ends the second shell. Its tab stays and says so; the first shell is untouched.
    await runInShown(app, 'exit');
    await expect(app.bottom.getByText('The shell ended.')).toBeVisible();
    await expect(shellTab(app, 'Terminal 2')).toHaveAttribute('aria-pressed', 'true');
    await expect.poll(() => shellsOf(app, taskId)).toEqual([1]);
    // There is nothing left to stop in it, so its tab closes without a question.
    await app.bottom.getByRole('button', { name: 'Close Terminal 2', exact: true }).click();
    await expect(shellTab(app, 'Terminal 2')).toHaveCount(0);
    await expect(app.bottom.getByText('The shell ended.')).toHaveCount(0);
    await expect(shellTab(app, 'Terminal')).toHaveAttribute('aria-pressed', 'true');
    await expect(shown(app)).toContainText('one-42');

    // The last one closed: the panel folds away and the core has no shell of this task left.
    await app.bottom.getByRole('button', { name: 'Close Terminal', exact: true }).click();
    await expect(shown(app)).toHaveCount(0);
    await expect(app.bottom.getByRole('button', { name: 'Hide' })).toHaveCount(0);
    await expect.poll(() => shellsOf(app, taskId)).toEqual([]);
    await expect(app.bottom.getByRole('button')).toHaveText(['Terminal', '', 'Run', 'Problems']);

    // Opening the terminal again starts a new shell on an empty screen.
    await app.openTerminal();
    await expect(shown(app)).not.toContainText('one-42');
    await runInShown(app, 'echo again-$((40+2))');
    await expect(shown(app)).toContainText('again-42');
    await expect.poll(() => shellsOf(app, taskId)).toEqual([1]);
  });
});
