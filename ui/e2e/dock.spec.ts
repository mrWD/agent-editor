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

test.describe('The projects rail with many projects', () => {
  // What fits on the line depends on every project there is, so these tests share their core with nobody.
  test.use({ freshCore: true });

  test('F-08 a project is put into a folder; the folder shows its totals in the rail and folds in the dock', async ({
    app,
    project,
  }) => {
    const shop = await project({ name: 'shop', git: false });
    const notes = await project({ name: 'notes', git: false });
    const shopId = await app.addProject(shop.dir);
    await app.addProject(notes.dir);
    const asks = unique('Change the limit');
    const quiet = unique('Describe the limiter');
    await app.core.request('tasks.create', { projectId: shopId, agentId: 'mock', text: quiet });
    await app.core.request('tasks.create', { projectId: shopId, agentId: 'mock', text: asks });
    await app.openHome();
    await expect(app.rail.getByRole('link', { name: `${asks}: Practice agent asks` })).toBeVisible();

    // From the open dock the project goes into a folder that is made by naming it.
    await app.rail.getByRole('button', { name: 'Expand' }).click();
    await app.dock.getByRole('button', { name: `Put ${shop.name} into a folder` }).click();
    const form = app.dock.locator('form');
    await expect(form).toContainText(`Folder for ${shop.name}`);
    await expect(form.getByRole('combobox')).toBeFocused();
    await form.getByRole('combobox').fill('Client work');
    await form.getByRole('button', { name: 'Save' }).click();
    await expect(form).toBeHidden();

    const folder = app.dock.getByRole('group', { name: 'Folder Client work' });
    await expect(folder).toContainText('1 project · 2 tasks');
    await expect(folder.getByRole('heading', { level: 4, name: shop.name })).toBeVisible();
    await expect(folder.getByRole('link', { name: new RegExp(quiet) })).toBeVisible();
    // The other project stands on its own, outside the folder.
    await expect(app.dock.getByRole('heading', { level: 3, name: notes.name })).toBeVisible();
    await expect(folder.getByText(notes.name)).toHaveCount(0);

    // Folded, the folder still names the task that needs the user, and nothing else of it.
    await folder.getByRole('button', { name: 'Client work', exact: true }).click();
    await expect(folder).toContainText('1 project · 2 tasks · folded');
    await expect(folder.getByRole('link')).toHaveCount(1);
    await expect(folder.getByRole('link')).toContainText(`${asks}${shop.name}Practice agent asks`);
    await app.page.reload();
    await expect(folder).toContainText('folded');
    await folder.getByRole('button', { name: 'Client work', exact: true }).click();
    await expect(folder.getByRole('link')).toHaveCount(2);

    // In the rail the folder is one item with its totals; its card names the projects and what needs the user.
    await app.dock.getByRole('button', { name: 'Collapse' }).click();
    const folded = app.rail.getByRole('button', { name: /^Folder Client work: 1 project, 1 needs you/ });
    await expect(folded).toHaveText('Client work111');
    await expect(app.rail.getByRole('group', { name: shop.name })).toHaveCount(0);
    await folded.hover();
    const card = app.railCard('Client work');
    await expect(card).toContainText('1 project · 2 tasks');
    await expect(card).toContainText(`${shop.name}112 tasks`);
    await expect(card).toContainText('Needs you in this folder');
    await expect(card.getByRole('link')).toHaveText([`${asks}${shop.name}Practice agent asks`]);

    // Opened, it shows its projects as the rail shows any project; the choice is kept.
    await folded.click();
    const opened = app.rail.getByRole('group', { name: 'Folder Client work' });
    await expect(opened.getByRole('group', { name: shop.name }).getByRole('link')).toHaveCount(2);
    await app.page.reload();
    await expect(opened.getByRole('link', { name: `${asks}: Practice agent asks` })).toBeVisible();
    await opened.getByRole('button', { name: 'Client work', exact: true }).click();
    await expect(folded).toBeVisible();

    // Taken out again, the folder is gone: it was only a name.
    await app.rail.getByRole('button', { name: 'Expand' }).click();
    await app.dock.getByRole('button', { name: `${shop.name} is in Client work. Move it.` }).click();
    await form.getByRole('button', { name: 'Take it out of Client work' }).click();
    await expect(app.dock.getByRole('group', { name: 'Folder Client work' })).toHaveCount(0);
    await expect(app.dock.getByRole('heading', { level: 3, name: shop.name })).toBeVisible();
  });

  test('F-09 projects that do not fit fold into one item; what needs the user and the newest stay', async ({
    app,
    project,
  }) => {
    // The first project asks a question and is the oldest of all; the second is the oldest of the quiet ones.
    const asking = await project({ name: 'asking-project', git: false });
    const askingId = await app.addProject(asking.dir);
    const title = unique('Change the limit');
    await app.core.request('tasks.create', { projectId: askingId, agentId: 'mock', text: title });
    const names: string[] = [];
    for (let n = 1; n <= 16; n++) {
      const one = await project({ name: `quiet-project-number-${n}`, git: false });
      await app.addProject(one.dir);
      names.push(one.name);
    }
    await app.openHome();

    const more = app.rail.getByRole('button', { name: /^\d+ more projects/ });
    await expect(more).toBeVisible();
    // One line, however many projects there are.
    expect((await app.rail.boundingBox())!.height).toBeLessThan(60);
    await expect(app.rail.getByRole('link', { name: `${title}: Practice agent asks` })).toBeVisible();
    await expect(app.rail.getByRole('group', { name: names[15]!, exact: true })).toBeVisible();
    await expect(app.rail.getByRole('group', { name: names[0]!, exact: true })).toHaveCount(0);
    const shownNow = await app.rail.getByRole('group').count();
    await expect(more).toContainText(`${17 - shownNow} more`);

    // With this many projects the switcher of Home is one list, and the header stays one row.
    const switcher = app.page.getByRole('group', { name: 'Show tasks of' });
    await expect(switcher.getByRole('button')).toHaveText(['All 1']);
    expect((await app.page.getByRole('banner').boundingBox())!.height).toBeLessThan(60);
    const list = switcher.getByRole('combobox', { name: 'One project' });
    await list.selectOption({ label: `${names[2]} (0)` });
    await expect(app.page.getByLabel('Project', { exact: true }).locator('option:checked')).toHaveText(names[2]!);
    await expect(app.page.getByRole('main').getByRole('link', { name: title })).toBeHidden();
    await switcher.getByRole('button', { name: /^All/ }).click();
    await expect(app.page.getByRole('main').getByRole('link', { name: title })).toBeVisible();
    await expect(list).toHaveValue('');

    // The card of the folded ones names them.
    await more.hover();
    const card = app.page.getByRole('region', { name: /^\d+ more, shown while/ });
    await expect(card).toContainText(names[0]!);
    await expect(card).not.toContainText(names[15]!);
    await app.page.getByRole('heading', { level: 1 }).hover();
    await expect(card).toBeHidden();

    // A narrower window folds more of them, a wider one fewer.
    await app.page.setViewportSize({ width: 900, height: 900 });
    await expect.poll(() => app.rail.getByRole('group').count()).toBeLessThan(shownNow);
    await expect(app.rail.getByRole('link', { name: `${title}: Practice agent asks` })).toBeVisible();
    await app.page.setViewportSize({ width: 1440, height: 900 });
    await expect.poll(() => app.rail.getByRole('group').count()).toBe(shownNow);

    // The item opens the dock, where every project is listed by name.
    await more.click();
    await expect(app.dock.getByRole('heading', { level: 3, name: names[0]!, exact: true })).toBeVisible();
    await expect(app.dock.getByRole('heading', { level: 3 })).toHaveCount(17);
  });

  test('F-10 F-11 the open dock finds a project or a task, shows one stage, and is dragged taller', async ({
    app,
    project,
  }) => {
    const shop = await project({ name: 'shop', git: false });
    const notes = await project({ name: 'notes', git: false });
    const shopId = await app.addProject(shop.dir);
    const notesId = await app.addProject(notes.dir);
    const asks = unique('Change the limit');
    const limiter = unique('Describe the limiter');
    const readme = unique('Describe the readme');
    await app.core.request('tasks.create', { projectId: shopId, agentId: 'mock', text: limiter });
    await app.core.request('tasks.create', { projectId: shopId, agentId: 'mock', text: asks });
    await app.core.request('tasks.create', { projectId: notesId, agentId: 'mock', text: readme });
    await app.openHome();
    await expect(app.rail.getByRole('link', { name: `${asks}: Practice agent asks` })).toBeVisible();
    await expect(app.rail.getByRole('link', { name: `${readme}: your turn` })).toBeVisible();
    await app.rail.getByRole('button', { name: 'Expand' }).click();
    await expect(app.dock).toContainText('1 needs you · 2 your turn · 3 tasks in 2 projects');

    // Words find tasks by title; a project's name finds the project with all its tasks.
    const find = app.dock.getByRole('searchbox', { name: 'Find' });
    await find.fill('readme describe');
    await expect(app.dock.getByRole('link')).toHaveText([`${readme}your turn`]);
    await expect(app.dock.getByRole('heading', { level: 3 })).toHaveText([notes.name]);
    await find.fill(shop.name);
    await expect(app.dock.getByRole('heading', { level: 3 })).toHaveText([shop.name]);
    await expect(app.dock.getByRole('link')).toHaveCount(2);
    await find.fill('nothing like this');
    await expect(app.dock).toContainText('Nothing matches.');
    await find.fill('');

    // One stage at a time, with how many tasks are in it.
    const filter = app.dock.getByRole('group', { name: 'Which tasks to show' });
    await expect(filter.getByRole('button')).toHaveText(['All3', 'Needs you1', 'Ready to merge0', 'Checks running0']);
    await filter.getByRole('button', { name: /^Needs you/ }).click();
    await expect(filter.getByRole('button', { name: /^Needs you/ })).toHaveAttribute('aria-pressed', 'true');
    await expect(app.dock.getByRole('link')).toHaveText([`${asks}Practice agent asks`]);
    await expect(app.dock.getByRole('heading', { level: 3 })).toHaveText([shop.name]);
    await filter.getByRole('button', { name: /^All/ }).click();
    await expect(app.dock.getByRole('link')).toHaveCount(3);

    // The grip makes the dock taller and shorter, from the keyboard too, and the height is kept.
    const grip = app.dock.getByRole('separator', { name: 'Drag to make the panel taller or shorter' });
    const before = (await app.dock.boundingBox())!.height;
    await grip.focus();
    await app.page.keyboard.press('ArrowUp');
    await app.page.keyboard.press('ArrowUp');
    await expect.poll(async () => (await app.dock.boundingBox())!.height).toBe(Math.round(before) + 48);
    const box = (await grip.boundingBox())!;
    await app.page.mouse.move(box.x + box.width / 2, box.y + 2);
    await app.page.mouse.down();
    await app.page.mouse.move(box.x + box.width / 2, box.y - 98, { steps: 4 });
    await app.page.mouse.up();
    const taller = (await app.dock.boundingBox())!.height;
    expect(taller).toBeGreaterThan(before + 120);
    await app.page.reload();
    await expect.poll(async () => (await app.dock.boundingBox())!.height).toBe(taller);
  });
});
