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

  test('LY-03 the box a message is typed in is as tall as its top edge was dragged, and the conversation gives it the room', async ({
    app,
    project,
  }) => {
    await app.openHome();
    await app.createTask({ text: unique('Describe the limiter'), folder: (await project()).dir });
    const page = app.page;
    const edge = page.getByRole('separator', { name: 'Drag to make the box you type in taller or shorter' });
    const height = async (what: 'box' | 'feed') =>
      Math.round((await (what === 'box' ? app.messageBox : app.conversation.locator('.feed')).boundingBox())!.height);
    expect(await height('box')).toBe(56);
    const feedBefore = await height('feed');

    // Dragged up, the box grows and the conversation above it shrinks by as much.
    const from = (await edge.boundingBox())!;
    await page.mouse.move(from.x + 120, from.y + 4);
    await page.mouse.down();
    await page.mouse.move(from.x + 120, from.y + 4 - 140, { steps: 5 });
    await page.mouse.up();
    expect(await height('box')).toBe(196);
    expect(await height('feed')).toBe(feedBefore - 140);
    await expect(edge).toHaveAttribute('aria-valuenow', '196');

    // The arrows move it too, the way they point; it is never shorter than a line and a half of text.
    await edge.focus();
    await page.keyboard.press('ArrowUp');
    expect(await height('box')).toBe(212);
    for (let press = 0; press < 14; press += 1) await page.keyboard.press('ArrowDown');
    expect(await height('box')).toBe(44);
    // And it never takes the conversation's last room.
    for (let press = 0; press < 60; press += 1) await page.keyboard.press('ArrowUp');
    expect(await height('feed')).toBeGreaterThanOrEqual(96);
    expect(await height('box')).toBeLessThanOrEqual(560);

    // Kept after a reload; a double click puts it back as it was at first.
    await edge.focus();
    for (let press = 0; press < 80; press += 1) await page.keyboard.press('ArrowDown');
    for (let press = 0; press < 5; press += 1) await page.keyboard.press('ArrowUp');
    expect(await height('box')).toBe(124);
    await page.reload();
    await expect(app.conversation).toBeVisible();
    expect(await height('box')).toBe(124);
    await edge.dblclick();
    expect(await height('box')).toBe(56);
  });

  test('LY-04 the column of Changes and the list of the Preview are as wide as they were dragged', async ({
    app,
    project,
  }) => {
    await app.openHome();
    await app.createTask({ text: unique('Describe the limiter'), folder: (await project()).dir });
    const page = app.page;
    await app.showView('Changes');
    const column = page.getByRole('region', { name: 'Git: what goes in, commit, push' });
    const columnEdge = page.getByRole('separator', { name: 'Drag to make the column of changes wider or narrower' });
    const width = async (of: typeof column) => Math.round((await of.boundingBox())!.width);
    expect(await width(column)).toBe(320);
    const from = (await columnEdge.boundingBox())!;
    await page.mouse.move(from.x, from.y + 200);
    await page.mouse.down();
    await page.mouse.move(from.x + 100, from.y + 200, { steps: 4 });
    await page.mouse.up();
    expect(await width(column)).toBe(420);
    await columnEdge.focus();
    await page.keyboard.press('ArrowLeft');
    expect(await width(column)).toBe(404);

    // The list of the API in the Preview has an edge of its own, with a size of its own.
    await app.showView('Preview');
    await page
      .getByRole('group', { name: 'What to preview on' })
      .getByRole('button', { name: 'API', exact: true })
      .click();
    const list = page.getByRole('navigation', { name: 'What the API offers' });
    const listEdge = page.getByRole('separator', { name: 'Drag to make the list of operations wider or narrower' });
    expect(await width(list)).toBe(300);
    await listEdge.focus();
    await page.keyboard.press('ArrowRight');
    await page.keyboard.press('ArrowRight');
    expect(await width(list)).toBe(332);

    // Both are kept; the column of Changes is as it was left.
    await page.reload();
    await page
      .getByRole('group', { name: 'What to preview on' })
      .getByRole('button', { name: 'API', exact: true })
      .click();
    await expect(list).toBeVisible();
    expect(await width(list)).toBe(332);
    await app.showView('Changes');
    expect(await width(column)).toBe(404);
  });
});
