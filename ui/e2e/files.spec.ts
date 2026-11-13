import { existsSync } from 'node:fs';
import path from 'node:path';
import { DEMO_FILE, DEMO_TEXT, expect, readIn, test, unique } from './support/fixtures';

test.describe('Making, renaming and deleting files in the tree', () => {
  test('FL-01 FL-02 a new file opens in the editor; a new folder takes files; the agent is told', async ({
    app,
    project,
  }) => {
    const shop = await project();
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    const folder = await app.folderOf(taskId);

    await app.files.getByRole('button', { name: 'New file', exact: true }).click();
    const box = app.files.getByRole('textbox', { name: 'Name of the new file' });
    await box.fill('src/limits/extra.ts');
    await box.press('Enter');
    // Made with the folders on the way, and opened at once.
    await expect(app.tab('extra.ts')).toHaveAttribute('aria-selected', 'true');
    expect(readIn(folder, 'src/limits/extra.ts')).toBe('');
    await expect(app.activities.filter({ hasText: 'created' })).toContainText('src/limits/extra.ts');

    await app.files.getByRole('button', { name: 'New folder', exact: true }).click();
    await app.files.getByRole('textbox', { name: 'Name of the new folder' }).fill('docs');
    await app.page.keyboard.press('Enter');
    await expect(app.files.getByRole('button', { name: 'docs/', exact: true })).toBeVisible();
    await app.files.getByRole('button', { name: 'New file in docs' }).click();
    await app.files.getByRole('textbox', { name: 'Name of the new file' }).fill('notes.md');
    await app.page.keyboard.press('Enter');
    await expect(app.tab('notes.md')).toHaveAttribute('aria-selected', 'true');
    expect(existsSync(path.join(folder, 'docs/notes.md'))).toBe(true);

    // A name that is taken is refused, and says why; Escape gives up.
    await app.files.getByRole('button', { name: 'New file', exact: true }).click();
    await app.files.getByRole('textbox', { name: 'Name of the new file' }).fill('README.md');
    await app.page.keyboard.press('Enter');
    await expect(app.files.getByRole('alert')).toContainText('There is already something called README.md');
    await app.page.keyboard.press('Escape');
    await expect(app.files.getByRole('alert')).toBeHidden();

    // The agent hears about the new files with the next message.
    await app.say('And who calls it?');
    await expect(app.thoughts.last()).toContainText('the user changed these files by hand');
    await expect(app.thoughts.last()).toContainText('`src/limits/extra.ts`');
  });

  test('FL-03 a renamed file keeps its tab, its text and the marks of who wrote it', async ({ app, project }) => {
    const shop = await project();
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    const folder = await app.folderOf(taskId);
    await app.letAgentEdit('Change the limit');
    await app.openFile(DEMO_FILE);
    // Unsaved typing goes with the file.
    await app.typeAtEndOf('windowMs: 60 * 1000,', ' // mine');

    await app.files.getByRole('button', { name: `Rename ${DEMO_FILE}` }).click();
    const box = app.files.getByRole('textbox', { name: `New name of ${DEMO_FILE}` });
    await expect(box).toHaveValue('rateLimit.ts');
    await box.fill('limiter.ts');
    await box.press('Enter');

    await expect(app.tab('limiter.ts')).toHaveAttribute('aria-selected', 'true');
    await expect(app.tab('rateLimit.ts')).toHaveCount(0);
    await expect(app.editor).toContainText('windowMs: 60 * 1000, // mine');
    const renamed = 'src/middleware/limiter.ts';
    expect(existsSync(path.join(folder, DEMO_FILE))).toBe(false);
    expect(readIn(folder, renamed)).toContain('// mine');
    expect(readIn(folder, renamed)).toContain('limit: limits.login,');
    await expect(app.activities.filter({ hasText: 'renamed' })).toContainText(`${DEMO_FILE}to${renamed}`);
    // The agent's line is still marked as the agent's under the new name.
    await expect(app.files.getByRole('button', { name: /^limiter\.ts/ })).toContainText('Pr');
  });

  test('FL-04 a deleted file asks twice, closes its tab, and can be put back from the conversation', async ({
    app,
    project,
  }) => {
    const shop = await project();
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    const folder = await app.folderOf(taskId);
    await app.openFile(DEMO_FILE);

    // The first press only asks, and is forgotten when no second press follows.
    await app.files.getByRole('button', { name: `Delete ${DEMO_FILE}` }).click();
    await expect(app.files.getByRole('button', { name: `Really delete ${DEMO_FILE}?` })).toHaveText('Delete?');
    expect(existsSync(path.join(folder, DEMO_FILE))).toBe(true);
    await expect(app.files.getByRole('button', { name: `Really delete ${DEMO_FILE}?` })).toHaveCount(0, {
      timeout: 8000,
    });
    expect(existsSync(path.join(folder, DEMO_FILE))).toBe(true);

    await app.files.getByRole('button', { name: `Delete ${DEMO_FILE}` }).click();
    await app.files.getByRole('button', { name: `Really delete ${DEMO_FILE}?` }).click();
    await expect(app.tab('rateLimit.ts')).toHaveCount(0);
    await expect(app.files.getByRole('button', { name: /^rateLimit\.ts/ })).toHaveCount(0);
    expect(existsSync(path.join(folder, DEMO_FILE))).toBe(false);

    const told = app.activities.filter({ hasText: 'deleted' });
    await expect(told).toContainText(DEMO_FILE);
    await told.getByRole('button', { name: 'Put it back' }).click();
    await expect(app.files.getByRole('button', { name: /^rateLimit\.ts/ })).toBeVisible();
    expect(readIn(folder, DEMO_FILE)).toBe(DEMO_TEXT);
    await expect(app.activities.filter({ hasText: 'put back' })).toContainText(DEMO_FILE);
    // What was put back cannot be put back twice.
    await expect(told.getByRole('button', { name: 'Put it back' })).toHaveCount(0);
  });

  test('FL-05 a whole folder is deleted and put back with everything in it', async ({ app, project }) => {
    const shop = await project();
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    const folder = await app.folderOf(taskId);
    await app.openFile(DEMO_FILE);

    await app.files.getByRole('button', { name: 'Delete src', exact: true }).click();
    await app.files.getByRole('button', { name: 'Really delete src?' }).click();
    await expect(app.files.getByRole('button', { name: 'src/', exact: true })).toHaveCount(0);
    await expect(app.tab('rateLimit.ts')).toHaveCount(0);
    expect(existsSync(path.join(folder, 'src'))).toBe(false);

    await app.activities.filter({ hasText: 'deleted' }).getByRole('button', { name: 'Put it back' }).click();
    await expect(app.files.getByRole('button', { name: 'src/', exact: true })).toBeVisible();
    expect(readIn(folder, DEMO_FILE)).toBe(DEMO_TEXT);
  });
});
