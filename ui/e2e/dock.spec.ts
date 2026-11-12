import { expect, test, unique } from './support/fixtures';

test.describe('The projects rail', () => {
  test('F-01 F-02 F-03 the rail has a mark for each task; the card under the pointer names them and opens one', async ({
    app,
    project,
  }) => {
    const shop = await project();
    const waiting = unique('Change the limit');
    const done = unique('Describe the limiter');
    await app.openHome();
    const doneId = await app.createTask({ text: done, folder: shop.dir });
    await app.goHome();
    const waitingId = await app.createTask({ text: waiting, project: shop.name, until: 'question' });

    // One mark per task, named by where the task stands. The task that asks comes first; the one on screen is marked.
    await expect(app.rail.getByText(shop.name, { exact: true })).toBeVisible();
    const marks = app.rail.locator('.rail-project', { hasText: shop.name }).getByRole('link');
    await expect(marks).toHaveCount(2);
    await expect(marks.nth(0)).toHaveAccessibleName(`${waiting}: Practice agent asks`);
    await expect(marks.nth(1)).toHaveAccessibleName(`${done}: your turn`);
    await expect(marks.nth(0)).toHaveAttribute('aria-current', 'page');
    await expect(marks.nth(1)).not.toHaveAttribute('aria-current', 'page');
    await expect(marks.nth(1)).toHaveAttribute('href', `#/task/${doneId}/code`);

    // The card of the project under the pointer.
    const card = app.railCard(shop.name);
    await expect(card).toBeHidden();
    await app.rail.getByText(shop.name, { exact: true }).hover();
    await expect(card.getByRole('heading', { name: shop.name })).toBeVisible();
    await expect(card).toContainText('1 needs you · 1 your turn · 2 tasks');
    const rows = card.getByRole('link');
    await expect(rows).toHaveText([`${waiting}Practice agent asks`, `${done}your turn`]);
    await expect(rows.nth(0)).toHaveAttribute('aria-current', 'page');

    // The pointer can travel from the rail up into the card: the card stays, also after its own delay to hide.
    await rows.nth(1).hover();
    await app.page.waitForTimeout(400);
    await expect(card).toBeVisible();
    // Away from both, it goes.
    await app.page.getByRole('heading', { level: 1 }).hover();
    await expect(card).toBeHidden();

    // A row of the card opens its task.
    await app.rail.getByText(shop.name, { exact: true }).hover();
    await rows.nth(1).click();
    await expect(app.page).toHaveURL(new RegExp(`#/task/${doneId}/code$`));
    await expect(app.page.getByRole('heading', { level: 1, name: done })).toBeVisible();
    await expect(card).toBeHidden();
    await expect(marks.nth(1)).toHaveAttribute('aria-current', 'page');
    // A mark of the rail does the same.
    await marks.nth(0).click();
    await expect(app.page).toHaveURL(new RegExp(`#/task/${waitingId}/code$`));
  });

  test('F-04 Expand lists every task by name, Collapse brings the rail back, and both are remembered', async ({
    app,
    project,
  }) => {
    const shop = await project();
    const title = unique('Describe the limiter');
    await app.openHome();
    const taskId = await app.createTask({ text: title, folder: shop.dir });
    await app.goHome();

    await expect(app.rail).toBeVisible();
    await expect(app.rail.getByRole('button', { name: 'Expand' })).toHaveAttribute('aria-expanded', 'false');
    await app.rail.getByRole('button', { name: 'Expand' }).click();
    await expect(app.rail).toBeHidden();
    await expect(app.dock.getByRole('heading', { level: 2, name: 'Projects' })).toBeVisible();
    const section = app.dock.locator('section', { has: app.page.getByRole('heading', { level: 3, name: shop.name }) });
    await expect(section).toContainText('1 your turn · 1 task');
    await expect(section.getByRole('link', { name: title })).toHaveAttribute('href', `#/task/${taskId}/code`);

    // Open after the page is loaded again.
    await app.page.reload();
    await expect(app.dock).toBeVisible();
    await expect(section.getByRole('link', { name: title })).toBeVisible();
    await section.getByRole('link', { name: title }).click();
    await expect(app.page).toHaveURL(new RegExp(`#/task/${taskId}/code$`));
    await expect(section.getByRole('link', { name: title })).toHaveAttribute('aria-current', 'page');

    await app.dock.getByRole('button', { name: 'Collapse' }).click();
    await expect(app.dock).toBeHidden();
    await expect(app.rail).toBeVisible();
    await app.page.reload();
    await expect(app.rail.getByRole('button', { name: 'Expand' })).toBeVisible();
    await expect(app.dock).toBeHidden();
  });

  test('F-07 the keyboard opens the card of a project by focus and closes it with Esc', async ({ app, project }) => {
    const shop = await project();
    const title = unique('Describe the limiter');
    await app.openHome();
    await app.createTask({ text: title, folder: shop.dir });
    await app.goHome();

    const mark = app.rail.getByRole('link', { name: `${title}: your turn` });
    const card = app.railCard(shop.name);
    await mark.focus();
    await expect(card).toBeVisible();
    await expect(card.getByRole('link', { name: new RegExp(`^${title}`) })).toBeVisible();
    await app.page.keyboard.press('Escape');
    await expect(card).toBeHidden();
    await expect(mark).toBeFocused();

    // Enter on the focused mark opens the task.
    await app.page.keyboard.press('Enter');
    await expect(app.page.getByRole('heading', { level: 1, name: title })).toBeVisible();
  });

  test('F-05 F-06 a project with many tasks shows a count for each stage, and its card sums up the rest', async ({
    app,
    project,
  }) => {
    const shop = await project({ name: 'busy', git: false });
    const projectId = await app.addProject(shop.dir);
    const word = unique('part').replace(' ', '');
    // Thirteen tasks are one more than the rail draws one by one. They are made without the form, to save time.
    for (let n = 1; n <= 13; n++) {
      await app.core.request('tasks.create', { projectId, agentId: 'mock', text: `Describe ${word} number ${n}` });
    }
    await app.openHome();

    const counts = app.rail.getByLabel(`${shop.name}: 13 your turn · 13 tasks`);
    await expect(counts).toBeVisible({ timeout: 30_000 });
    await expect(counts).toHaveText('13');
    await expect(app.rail.locator('.rail-project', { hasText: shop.name }).getByRole('link')).toHaveCount(0);

    // The card names six of them and sums up the other seven.
    await app.rail.getByText(shop.name, { exact: true }).hover();
    const card = app.railCard(shop.name);
    await expect(card.getByRole('link')).toHaveCount(6);
    await expect(card).toContainText('7 more: your turn');
    await expect(card.getByRole('link').first()).toContainText(`Describe ${word} number 13`);

    // The open dock names five, and always the task on screen.
    await app.page.getByRole('heading', { level: 1 }).hover();
    await app.rail.getByRole('button', { name: 'Expand' }).click();
    const section = app.dock.locator('section', { has: app.page.getByRole('heading', { level: 3, name: shop.name }) });
    await expect(section.getByRole('link')).toHaveCount(5);
    await expect(section).toContainText('8 more: your turn');
    const oldest = (await app.core.request<{ id: string; title: string }[]>('tasks.list')).find(
      (task) => task.title === `Describe ${word} number 1`,
    )!;
    await app.openTask(oldest.id);
    await expect(section.getByRole('link')).toHaveCount(5);
    await expect(section.getByRole('link').filter({ hasText: /number 1your turn$/ })).toHaveAttribute(
      'aria-current',
      'page',
    );
  });
});
