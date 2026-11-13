import { DEMO_FILE, expect, test, unique, writeIn } from './support/fixtures';

test.describe('The search over everything', () => {
  test('K-01 the search opens with its key and with the button, and closes with Esc', async ({ app }) => {
    await app.openHome();
    await expect(app.palette).toBeHidden();
    await app.page.keyboard.press('ControlOrMeta+k');
    await expect(app.searchBox).toBeFocused();
    await expect(app.searchBox).toHaveAttribute('placeholder', 'Search tasks, chats and commands');
    await app.page.keyboard.press('Escape');
    await expect(app.palette).toBeHidden();

    await app.openSearch();
    // The same key closes it again, and so does a click beside it.
    await app.page.keyboard.press('ControlOrMeta+k');
    await expect(app.palette).toBeHidden();
    await app.openSearch();
    await app.page.mouse.click(5, 300);
    await expect(app.palette).toBeHidden();
  });

  test('K-02 K-07 tasks are found by title, with project and stage; arrows and Enter pick one', async ({
    app,
    project,
  }) => {
    const shop = await project();
    const word = unique('zebra').replace(' ', '');
    const older = `Describe the ${word} limiter`;
    const newer = `Look at the ${word} orders`;
    await app.openHome();
    const olderId = await app.createTask({ text: older, folder: shop.dir });
    await app.goHome();
    await app.createTask({ text: newer, project: shop.name });
    await app.goHome();

    await app.openSearch();
    await app.searchBox.fill(word);
    await expect(app.palette.getByRole('heading', { name: 'Tasks' })).toBeVisible();
    // The rows of tasks, not the rows of things said in them: those hold the word too and come a moment later.
    const hits = app.palette.getByRole('button').filter({ hasText: `${shop.name} · your turn` });
    // Newest first, each with its project and where it stands.
    await expect(hits).toHaveText([
      new RegExp(`^${newer}${shop.name} · your turn`),
      new RegExp(`^${older}${shop.name} · your turn`),
    ]);
    await expect(hits.first().locator('mark')).toHaveText(word);
    await expect(
      app.palette.getByRole('group', { name: 'Where to look' }).getByRole('button', { name: 'Tasks 2' }),
    ).toBeVisible();

    // The first row is picked; the arrows move the pick, Enter opens it.
    await expect(hits.first()).toHaveAttribute('aria-current', 'true');
    await app.page.keyboard.press('ArrowDown');
    await expect(hits.nth(1)).toHaveAttribute('aria-current', 'true');
    await app.page.keyboard.press('ArrowUp');
    await expect(hits.first()).toHaveAttribute('aria-current', 'true');
    await app.page.keyboard.press('ArrowDown');
    await app.page.keyboard.press('Enter');
    await expect(app.palette).toBeHidden();
    await expect(app.page).toHaveURL(new RegExp(`#/task/${olderId}/code$`));
    await expect(app.page.getByRole('heading', { level: 1, name: older })).toBeVisible();
  });

  test('K-03 files of the task are found: a match in the name comes first, ignored files are left out', async ({
    app,
    project,
  }) => {
    const shop = await project();
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    // `.gitignore` of the project names this file.
    const folder = await app.folderOf(taskId);
    writeIn(folder, 'ignored.log', 'nothing to see\n');
    writeIn(folder, 'listed.log', 'something to see\n');

    await app.openSearch();
    await expect(app.searchBox).toHaveAttribute('placeholder', 'Search tasks, files, chats and commands');
    await app.searchBox.fill('limit');
    await expect(app.palette.getByRole('heading', { name: 'Files' })).toBeVisible();
    const files = app.palette.getByRole('button').filter({ hasText: /\.ts/ });
    await expect(files).toHaveText([new RegExp(`^${DEMO_FILE}`), /^src\/limits\/config\.ts/]);

    await app.searchBox.fill('.log');
    await expect(app.palette.getByRole('button').filter({ hasText: 'listed.log' })).toBeVisible();
    await expect(app.palette.getByRole('button').filter({ hasText: 'ignored.log' })).toHaveCount(0);

    await app.searchBox.fill('rateLimit');
    // The row of the file itself: a conversation that mentions the file has a row with its path in it too.
    await app.palette
      .getByRole('button')
      .filter({ hasText: new RegExp(`^${DEMO_FILE}`) })
      .click();
    await expect(app.palette).toBeHidden();
    await expect(app.tab('rateLimit.ts')).toHaveAttribute('aria-selected', 'true');
    await expect(app.editor).toContainText('windowMs');
  });

  test('K-03 hits of an older query are gone the moment the query changes', async ({ app, project }) => {
    const shop = await project();
    await app.openHome();
    await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    await app.openSearch();
    await app.searchBox.fill('limit');
    const fileRows = app.palette.getByRole('button').filter({ hasText: /\.ts/ });
    await expect(fileRows).toHaveCount(2);
    const offer = app.palette.getByRole('button').filter({ hasText: 'inside the code of this task' });
    await expect(offer).toContainText('“limit”');

    // One look, without waiting: a row of the old query must not be there to be clicked,
    // and the row that searches the code must name the new query.
    await app.searchBox.fill('the limiter and more');
    expect(await fileRows.count()).toBe(0);
    await expect(offer).toContainText('“the limiter and more”');
  });

  test('K-04 things said in a conversation are found as a phrase, from any screen', async ({ app, project }) => {
    const shop = await project();
    const word = unique('walrus').replace(' ', '');
    const title = `Describe the ${word} limiter`;
    await app.openHome();
    const taskId = await app.createTask({ text: title, folder: shop.dir });

    // The answer arrived word by word; it is found as one sentence.
    await app.openSearch();
    await app.searchBox.fill('attempts a minute and answers');
    await expect(app.palette.getByRole('heading', { name: 'Chats' })).toBeVisible();
    const here = app.palette.getByRole('button').filter({ hasText: 'this task · just now' });
    await expect(here.first()).toContainText('The limiter allows five attempts a minute and answers 429 after that.');
    await expect(here.first().locator('mark')).toHaveText('attempts a minute and answers');
    await app.page.keyboard.press('Escape');

    // What the user said is found too, here from Home, and leads to the task.
    await app.goHome();
    await app.openSearch();
    await app.searchBox.fill(`the ${word} lim`);
    await app.palette
      .getByRole('group', { name: 'Where to look' })
      .getByRole('button', { name: /^Chats/ })
      .click();
    const said = app.palette.getByRole('button').filter({ hasText: `${title} · just now` });
    await expect(said.first()).toContainText(title);
    await said.first().click();
    await expect(app.page).toHaveURL(new RegExp(`#/task/${taskId}/code$`));
  });

  test('K-05 text is found inside the code, and opening a hit puts the caret there', async ({ app, project }) => {
    const shop = await project();
    await app.openHome();
    await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });

    await app.openSearch();
    await app.searchBox.fill('windowms');
    const offer = app.palette.getByRole('button', { name: /Look for “windowms” inside the code of this task/ });
    await offer.click();
    // The click leaves the keyboard in the search field.
    await expect(app.searchBox).toBeFocused();
    const places = app.palette.getByRole('group', { name: 'Where to look' });
    await expect(places.getByRole('button', { name: 'Code 1' })).toHaveAttribute('aria-pressed', 'true');
    const hit = app.palette.getByRole('button').filter({ hasText: `${DEMO_FILE}:2` });
    await expect(hit).toContainText('windowMs: 60 * 1000,');
    await expect(hit.locator('mark')).toHaveText('windowMs');

    await app.page.keyboard.press('Enter');
    await expect(app.palette).toBeHidden();
    await expect(app.tab('rateLimit.ts')).toHaveAttribute('aria-selected', 'true');
    // The found word is selected: the caret stands right after it.
    await expect(app.page.getByText('Line 2, column 11', { exact: true })).toBeVisible();
    await expect(app.editor).toBeFocused();
  });

  test('K-06 commands are listed, and >, @ and # narrow the search', async ({ app, project }) => {
    const shop = await project();
    const word = unique('limiter').replace(' ', '');
    const title = `Describe the ${word}`;
    await app.openHome();
    const taskId = await app.createTask({ text: title, folder: shop.dir });
    const places = app.palette.getByRole('group', { name: 'Where to look' });
    const titles = app.palette.getByRole('heading');

    // "limit" is in the task's title, in file names, in what was said and in the name of a command.
    await app.openSearch();
    await app.searchBox.fill('limit');
    await expect(titles).toHaveText(['Tasks', 'Files', 'Chats', 'Commands']);

    // "@" keeps the tasks, "#" the files, ">" the commands. The row of filters gives way to the prefix.
    await app.searchBox.fill('@limit');
    await expect(titles).toHaveText(['Tasks']);
    await expect(places).toBeHidden();
    await expect(app.palette.getByRole('button').filter({ hasText: title })).toBeVisible();

    await app.searchBox.fill('#limit');
    await expect(titles).toHaveText(['Files']);
    await expect(app.palette.getByRole('button').filter({ hasText: DEMO_FILE })).toBeVisible();

    await app.searchBox.fill('>');
    await expect(titles).toHaveText(['Commands']);
    for (const command of ['Start a new task', 'Open Issues', 'Show Changes', 'Open the terminal']) {
      await expect(app.palette.getByRole('button', { name: new RegExp(`^${command}`) })).toBeVisible();
    }

    // A command of this task: its Changes view.
    await app.searchBox.fill('>changes');
    await expect(app.palette.getByRole('button')).toHaveText([/^Show Changesthis task/]);
    await app.page.keyboard.press('Enter');
    await expect(app.page).toHaveURL(new RegExp(`#/task/${taskId}/changes$`));

    // The filters do the same as the prefixes.
    await app.openSearch();
    await app.searchBox.fill('open');
    await places.getByRole('button', { name: /^Commands/ }).click();
    await expect(titles).toHaveText(['Commands']);
    await app.palette.getByRole('button', { name: /^Open the terminal/ }).click();
    await expect(app.page).toHaveURL(new RegExp(`#/task/${taskId}/code$`));
    await expect(app.bottom.getByRole('button', { name: 'Terminal', exact: true })).toHaveAttribute(
      'aria-pressed',
      'true',
    );

    await app.openSearch();
    await app.searchBox.fill('>issues');
    await app.page.keyboard.press('Enter');
    await expect(app.page).toHaveURL(/#\/issues$/);
    await expect(app.page.getByRole('heading', { level: 1, name: 'Issues' })).toBeVisible();

    // Something nothing matches says so.
    await app.openSearch();
    await app.searchBox.fill('@no-such-task-anywhere');
    await expect(app.palette.getByText('Nothing matches.')).toBeVisible();
  });
});
