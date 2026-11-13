import type { Page } from '@playwright/test';
import { expect, PLAIN_ANSWER, test, unique } from './support/fixtures';

/** What the core says when something that is not the desktop app asks for a folder dialog. */
const REFUSAL = 'Only the desktop app can show a folder dialog. Type the path instead.';

/**
 * Makes the page look like the window of the desktop app: the app leaves this
 * mark on its window before the page loads.
 */
async function insideTheApp(page: Page, key: string) {
  await page.addInitScript((given) => {
    (window as unknown as { __AE_SHELL__: unknown }).__AE_SHELL__ = Object.freeze({ key: given });
  }, key);
}

/**
 * Stands in for the app's folder dialog. The core under test runs on its own
 * and has no dialog, so the one question about a folder is answered here, with
 * `answer`; everything else goes to the real core untouched. Returns what the
 * dialog was asked with.
 */
async function dialogAnswers(page: Page, answer: () => string | null): Promise<Record<string, unknown>[]> {
  const asked: Record<string, unknown>[] = [];
  await page.routeWebSocket(/\/ws$/, (socket) => {
    const core = socket.connectToServer();
    socket.onMessage((message) => {
      if (typeof message === 'string') {
        const command = JSON.parse(message) as { id: number; method: string; params: Record<string, unknown> };
        if (command.method === 'dialogs.pickFolder') {
          asked.push(command.params);
          socket.send(JSON.stringify({ id: command.id, result: { path: answer() } }));
          return;
        }
      }
      core.send(message);
    });
  });
  return asked;
}

test.describe('The folder of a new project', () => {
  test('V-01 in a browser there is no folder dialog: the path is typed', async ({ app }) => {
    await app.openHome();
    await app.page.getByLabel('Project', { exact: true }).selectOption({ label: 'Another folder' });
    await expect(app.page.getByLabel('Folder of the new project')).toBeVisible();
    await expect(app.page.getByRole('button', { name: 'Choose folder' })).toHaveCount(0);
  });

  test('V-02 in the desktop app the chosen folder lands in the field and becomes a project', async ({
    app,
    project,
  }) => {
    const shop = await project();
    await insideTheApp(app.page, 'the-window');
    const asked = await dialogAnswers(app.page, () => shop.dir);
    await app.openHome();
    await app.page.getByLabel('Project', { exact: true }).selectOption({ label: 'Another folder' });

    const field = app.page.getByLabel('Folder of the new project');
    await app.page.getByRole('button', { name: 'Choose folder' }).click();
    await expect(field).toHaveValue(shop.dir);
    // The dialog was asked with the key of this window, which is how the core tells the app from a browser.
    expect(asked).toEqual([{ key: 'the-window' }]);

    const title = unique('Describe the limiter');
    await app.page.getByRole('textbox', { name: 'New task' }).fill(title);
    await app.page.getByRole('button', { name: 'Start', exact: true }).click();
    await expect(app.page.getByRole('heading', { level: 1, name: title })).toBeVisible();
    await expect(app.page.locator('.subbar').getByText(shop.name, { exact: true })).toBeVisible();
    await expect(app.answers).toHaveText([PLAIN_ANSWER]);
  });

  test('V-03 closing the dialog without choosing keeps what was typed', async ({ app }) => {
    await insideTheApp(app.page, 'the-window');
    const asked = await dialogAnswers(app.page, () => null);
    await app.openHome();
    await app.page.getByLabel('Project', { exact: true }).selectOption({ label: 'Another folder' });

    const field = app.page.getByLabel('Folder of the new project');
    await field.fill('/typed/by/hand');
    await app.page.getByRole('button', { name: 'Choose folder' }).click();
    await expect.poll(() => asked.length).toBe(1);
    await expect(app.page.getByRole('button', { name: 'Choose folder' })).toBeEnabled();
    await expect(field).toHaveValue('/typed/by/hand');
    await expect(app.page.getByRole('alert')).toHaveCount(0);
  });

  test('V-04 a page that only looks like the app gets no dialog, and the typed path still works', async ({
    app,
    project,
  }) => {
    const shop = await project();
    // The mark is there, but the core runs on its own: this is a browser tab that claims to be the app.
    await insideTheApp(app.page, 'a-guess');
    await app.openHome();
    await app.page.getByLabel('Project', { exact: true }).selectOption({ label: 'Another folder' });
    await app.page.getByRole('button', { name: 'Choose folder' }).click();
    await expect(app.page.getByRole('alert')).toHaveText(REFUSAL);

    const title = unique('Describe the limiter');
    await app.createTask({ text: title, folder: shop.dir });
    await expect(app.page.getByRole('heading', { level: 1, name: title })).toBeVisible();
    await expect(app.answers).toHaveText([PLAIN_ANSWER]);
  });
});
