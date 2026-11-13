import { expect, test, unique } from './support/fixtures';

test.describe('The panes at the sides of a task', () => {
  test('LY-01 the files and the conversation are as wide as they were dragged, also after a reload', async ({
    app,
    project,
  }) => {
    const shop = await project();
    await app.openHome();
    await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    const page = app.page;
    const width = async (name: string | RegExp, role: 'region' | 'complementary') =>
      Math.round((await page.getByRole(role, { name }).boundingBox())!.width);
    const filesEdge = page.getByRole('separator', { name: 'Drag to make the files wider or narrower' });
    const talkEdge = page.getByRole('separator', { name: 'Drag to make the conversation wider or narrower' });

    expect(await width('Files of this task', 'region')).toBe(230);
    expect(await width(/^Conversation with/, 'complementary')).toBe(380);

    // Dragged with the pointer: the edge follows it.
    const from = (await filesEdge.boundingBox())!;
    await page.mouse.move(from.x, from.y + 200);
    await page.mouse.down();
    await page.mouse.move(from.x + 90, from.y + 200, { steps: 4 });
    await page.mouse.up();
    expect(await width('Files of this task', 'region')).toBe(320);
    await expect(filesEdge).toHaveAttribute('aria-valuenow', '320');

    // Moved with the keyboard: an arrow moves the edge the way it points.
    await talkEdge.focus();
    await page.keyboard.press('ArrowLeft');
    await page.keyboard.press('ArrowLeft');
    expect(await width(/^Conversation with/, 'complementary')).toBe(412);

    // Never narrower than its least width.
    for (let press = 0; press < 12; press += 1) await page.keyboard.press('ArrowRight');
    expect(await width(/^Conversation with/, 'complementary')).toBe(280);

    await page.reload();
    await expect(app.conversation).toBeVisible();
    expect(await width('Files of this task', 'region')).toBe(320);
    expect(await width(/^Conversation with/, 'complementary')).toBe(280);

    // A double click puts a pane back to the width it had at first.
    await filesEdge.dblclick();
    expect(await width('Files of this task', 'region')).toBe(230);
  });

  test('LY-02 a folder shows a mark that turns when it opens, and every folder folds at once', async ({
    app,
    project,
  }) => {
    const shop = await project();
    await app.openHome();
    await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    const row = (name: string) => app.files.getByRole('button', { name, exact: true });
    const foldAll = app.files.getByRole('button', { name: 'Fold every folder' });

    await expect(row('src')).toHaveAttribute('aria-expanded', 'false');
    await expect(foldAll).toBeDisabled();
    await row('src').click();
    await row('middleware').click();
    await expect(row('middleware')).toHaveAttribute('aria-expanded', 'true');
    if (process.env.SHOT) await app.page.screenshot({ path: process.env.SHOT });

    await foldAll.click();
    await expect(row('src')).toHaveAttribute('aria-expanded', 'false');
    await expect(row('middleware')).toHaveCount(0);
    // Opened again, a folder inside is as it was left by the fold: closed.
    await row('src').click();
    await expect(row('middleware')).toHaveAttribute('aria-expanded', 'false');
  });
});
