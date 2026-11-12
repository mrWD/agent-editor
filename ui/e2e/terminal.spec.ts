import path from 'node:path';
import { expect, test, unique } from './support/fixtures';

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
