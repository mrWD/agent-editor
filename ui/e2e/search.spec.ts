import { DEMO_FILE, expect, test, unique, writeIn, type App, type Project } from './support/fixtures';

test.describe('The search over everything', () => {
  // The search looks through every task the core has, so each test gets a core with only its own tasks.
  test.use({ freshCore: true });

  test('K-01 the search opens with its key and with the button, and closes with Esc', async ({ app }) => {
    await app.openHome();
    await expect(app.palette).toBeHidden();
    await app.page.keyboard.press('ControlOrMeta+k');
    await expect(app.searchBox).toBeFocused();
    await expect(app.searchBox).toHaveAttribute('placeholder', 'Search tasks, files, chats and commands');
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
    await expect(app.searchBox).toHaveAttribute('placeholder', 'Search tasks, files, chats, commits and commands');
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

/** Matches a row that ends with `note`. The picked row has the word "Enter" after it. */
const ends = (note: string) => new RegExp(`${note.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}(Enter)?$`);

/** The rows of the search that are files: their text starts with a path. */
const fileRows = (app: App, path: string) =>
  app.palette.getByRole('button').filter({ hasText: new RegExp(`^${path.replaceAll('.', '\\.')}`) });

/**
 * Three tasks in two projects, the way the search over every project meets
 * them. "Here" is on screen at the end. Its sibling works on the same project
 * and differs from it in one file; the far task belongs to another project,
 * which has a file no other has.
 */
async function threeTasks(app: App, project: (options?: { name?: string }) => Promise<Project>) {
  const shop = await project();
  const blog = await project({ name: 'blog' });
  blog.write('src/posts/feed.ts', 'export const feed = { perPage: 20 };\n');
  blog.git(['add', '.']);
  blog.git(['commit', '-q', '-m', 'Add the feed']);

  const titles = {
    sibling: unique('Describe the search'),
    far: unique('Describe the feed'),
    here: unique('Describe it'),
  };
  await app.openHome();
  const sibling = await app.createTask({ text: titles.sibling, folder: shop.dir });
  await app.goHome();
  const far = await app.createTask({ text: titles.far, folder: blog.dir });
  await app.goHome();
  const here = await app.createTask({ text: titles.here, project: shop.name });
  writeIn(await app.folderOf(sibling), 'src/limits/config.ts', 'export const limits = { login: 5, search: 60 };\n');
  return { shop, blog, titles, ids: { sibling, far, here } };
}

test.describe('The search over everything: commits and every project', () => {
  // What is found depends on every task the core has.
  test.use({ freshCore: true });

  test('SR-01 commits of the branch and of the project are found, and picking one shows it in Changes', async ({
    app,
    project,
  }) => {
    const shop = await project();
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    const folder = await app.folderOf(taskId);
    writeIn(folder, 'README.md', '# Shop\n\nby hand\n');
    await app.showView('Changes');
    const subject = 'Say who wrote the README';
    await app.commit(subject);
    const short = shop.git(['rev-parse', '--short', 'HEAD'], folder);
    await app.showView('Code');

    // The task's own commit: its id, its first line with the found words marked, who made it and when.
    await app.openSearch();
    await expect(app.searchBox).toHaveAccessibleName('Search tasks, files, chats, commits and commands');
    await app.searchBox.fill('WROTE the');
    await expect(app.palette.getByRole('heading', { name: 'Commits' })).toBeVisible();
    const mine = app.palette.getByRole('button').filter({ hasText: subject });
    await expect(mine).toHaveText(new RegExp(`^${short}${subject}this task · Test, just now`));
    await expect(mine.locator('mark')).toHaveText('wrote the');
    const places = app.palette.getByRole('group', { name: 'Where to look' });
    await expect(places.getByRole('button', { name: 'Commits 1' })).toBeVisible();

    // The project's commit, the one the task's branch was started from, is marked with the project.
    await app.searchBox.fill('start');
    const first = app.palette.getByRole('button').filter({ hasText: new RegExp(`${shop.name} · Test, `) });
    await expect(first).toHaveText(/^[0-9a-f]{7,}Start/);
    // The first letters of an id find the commit too. The prefixes for tasks and files leave commits out.
    await app.searchBox.fill(short);
    await expect(mine).toBeVisible();
    await app.searchBox.fill('@wrote');
    await expect(app.palette.getByText('Nothing matches.')).toBeVisible();
    await expect(mine).toHaveCount(0);

    // Picking the commit opens it in the Changes view: what it says, its files, its lines.
    await app.searchBox.fill('wrote the');
    await places.getByRole('button', { name: 'Commits 1' }).click();
    await expect(app.palette.getByRole('heading')).toHaveText(['Commits']);
    await app.page.keyboard.press('Enter');
    await expect(app.palette).toBeHidden();
    await expect(app.page).toHaveURL(new RegExp(`#/task/${taskId}/changes$`));
    const commit = app.page.getByRole('region', { name: 'Commit', exact: true });
    await expect(commit.getByRole('heading', { name: subject })).toBeVisible();
    await expect(commit).toContainText(`${short}Test, just now`);
    await expect(commit).toContainText('On the branch of this task');
    await expect(commit.getByRole('button', { name: /^README\.md/ })).toHaveAttribute('aria-expanded', 'true');
    await expect(app.diffRow('add', 'by hand')).toBeVisible();
    await expect(
      app.page.getByText('What this commit changed, compared with the commit before it. 1 file'),
    ).toBeVisible();

    // Another commit can be picked while one is shown; the first commit of a project adds all it holds.
    await app.openSearch();
    await app.searchBox.fill('start');
    await first.click();
    await expect(commit.getByRole('heading', { name: 'Start' })).toBeVisible();
    await expect(commit).toContainText(`A commit of ${shop.name}, made outside this task`);
    await expect(commit.getByRole('button', { name: new RegExp(`^${DEMO_FILE}`) })).toContainText('new');
    await expect(app.diffRow('add', 'by hand')).toHaveCount(0);

    // Back to what is not committed, which is nothing now.
    await commit.getByRole('button', { name: 'Back to the changes' }).click();
    await expect(app.git.getByText('Everything is committed.')).toBeAttached();
    await expect(app.commitRow(subject)).toBeVisible();

    // Leaving the view puts a commit away too: coming back to Changes shows the changes.
    await app.openSearch();
    await app.searchBox.fill('start');
    await first.click();
    await expect(commit.getByRole('heading', { name: 'Start' })).toBeVisible();
    await app.showView('Code');
    await app.showView('Changes');
    await expect(app.git.getByText('Everything is committed.')).toBeAttached();
    await expect(commit).toHaveCount(0);
  });

  test('SR-02 files of other tasks and projects are found, each with its project and task; picking one opens it there', async ({
    app,
    project,
  }) => {
    const { shop, blog, titles, ids } = await threeTasks(app, project);

    // The task on screen first, marked with its project alone. Then the other tasks, newest first.
    await app.openSearch();
    await app.searchBox.fill('config');
    await expect(fileRows(app, 'src/limits/config.ts')).toHaveText([
      ends(`config.ts${shop.name}`),
      ends(`config.ts${blog.name} · ${titles.far}`),
      ends(`config.ts${shop.name} · ${titles.sibling}`),
    ]);
    await expect(
      app.palette.getByRole('group', { name: 'Where to look' }).getByRole('button', { name: 'Files 3' }),
    ).toBeVisible();

    // A file the sibling task has not changed is the same file as here: it is listed once for the project.
    await app.searchBox.fill('rateLimit');
    await expect(fileRows(app, DEMO_FILE)).toHaveText([
      ends(`rateLimit.ts${shop.name}`),
      ends(`rateLimit.ts${blog.name} · ${titles.far}`),
    ]);

    // "#" keeps the files, of every project; "@" and ">" leave them out.
    await app.searchBox.fill('#feed');
    const feed = fileRows(app, 'src/posts/feed.ts');
    await expect(feed).toHaveText(ends(`feed.ts${blog.name} · ${titles.far}`));
    await expect(feed.locator('mark')).toHaveText('feed');
    await app.searchBox.fill('>feed');
    await expect(app.palette.getByText('Nothing matches.')).toBeVisible();
    await expect(feed).toHaveCount(0);

    // Picking the file opens the task it belongs to, with the file in the editor.
    await app.searchBox.fill('feed.ts');
    await expect(feed).toHaveAttribute('aria-current', 'true');
    await app.page.keyboard.press('Enter');
    await expect(app.palette).toBeHidden();
    await expect(app.page).toHaveURL(new RegExp(`#/task/${ids.far}/code$`));
    await expect(app.page.getByRole('heading', { level: 1, name: titles.far })).toBeVisible();
    await expect(app.tab('feed.ts')).toHaveAttribute('aria-selected', 'true');
    await expect(app.editor).toContainText('perPage: 20');

    // From Home, where no task is on screen, every task is looked through. There are no commits to look for.
    await app.goHome();
    await app.openSearch();
    await expect(app.searchBox).toHaveAttribute('placeholder', 'Search tasks, files, chats and commands');
    await expect(app.palette.getByRole('button', { name: /^Commits/ })).toHaveCount(0);
    await app.searchBox.fill('config.ts');
    // The task that was on screen is one of the others now: its copy is the first of its project, the newest.
    await expect(fileRows(app, 'src/limits/config.ts')).toHaveText([
      ends(`config.ts${shop.name} · ${titles.here}`),
      ends(`config.ts${blog.name} · ${titles.far}`),
      ends(`config.ts${shop.name} · ${titles.sibling}`),
    ]);
    await fileRows(app, 'src/limits/config.ts').last().click();
    await expect(app.page).toHaveURL(new RegExp(`#/task/${ids.sibling}/code$`));
    await expect(app.tab('config.ts')).toHaveAttribute('aria-selected', 'true');
    await expect(app.editor).toContainText('search: 60');

    // A task that had a file open gets its tab back, and still shows the file that was asked for.
    await app.openSearch();
    await app.searchBox.fill('rateLimit');
    await fileRows(app, DEMO_FILE).filter({ hasText: titles.far }).click();
    await expect(app.page).toHaveURL(new RegExp(`#/task/${ids.far}/code$`));
    await expect(app.tab('rateLimit.ts')).toHaveAttribute('aria-selected', 'true');
    await expect(app.tab('feed.ts')).toHaveAttribute('aria-selected', 'false');
    await expect(app.editor).toContainText('windowMs');
  });

  test('SR-03 text is found in the code of every project, and a hit opens its task at the line', async ({
    app,
    project,
  }) => {
    const { shop, blog, titles, ids } = await threeTasks(app, project);
    const places = app.palette.getByRole('group', { name: 'Where to look' });
    const everywhere = (text: string) =>
      app.palette.getByRole('button', { name: new RegExp(`Look for “${text}” inside the code of every project`) });

    // The task on screen comes first; a line of a file the sibling has not changed is not listed twice.
    await app.openSearch();
    await app.searchBox.fill('windowms');
    await everywhere('windowms').click();
    await expect(app.searchBox).toBeFocused();
    await expect(places.getByRole('button', { name: 'Code 2' })).toHaveAttribute('aria-pressed', 'true');
    const lines = app.palette.getByRole('button').filter({ hasText: 'windowMs: 60 * 1000,' });
    await expect(lines).toHaveText([
      ends(`1000,${DEMO_FILE}:2`),
      ends(`1000,${DEMO_FILE}:2 · ${blog.name} · ${titles.far}`),
    ]);
    await expect(lines.first()).toHaveAttribute('aria-current', 'true');
    // Once every project was looked through, there is nothing wider left to offer.
    await expect(app.palette.getByRole('button', { name: /inside the code of/ })).toHaveCount(0);

    // A line only the sibling task has: found there, marked with its project and task, opened at its place.
    await app.searchBox.fill('search: 60');
    await expect(app.palette.getByRole('button', { name: /inside the code of this task/ })).toBeVisible();
    await everywhere('search: 60').click();
    await expect(places.getByRole('button', { name: 'Code 1' })).toHaveAttribute('aria-pressed', 'true');
    const line = app.palette.getByRole('button').filter({ hasText: `${shop.name} · ${titles.sibling}` });
    await expect(line).toContainText(`src/limits/config.ts:1 · ${shop.name} · ${titles.sibling}`);
    // The line is long and the row is narrow: it is shown from a little before the found words.
    await expect(line.locator('mark')).toHaveText('search: 60');
    await expect(line).toContainText('…');
    await app.page.keyboard.press('Enter');
    await expect(app.palette).toBeHidden();
    await expect(app.page).toHaveURL(new RegExp(`#/task/${ids.sibling}/code$`));
    await expect(app.tab('config.ts')).toHaveAttribute('aria-selected', 'true');
    await expect(app.page.getByText('Line 1, column 45', { exact: true })).toBeVisible();
    await expect(app.editor).toBeFocused();

    // From Home the code of every project is all there is to look through.
    await app.goHome();
    await app.openSearch();
    await app.searchBox.fill('perPage');
    await expect(app.palette.getByRole('button', { name: /inside the code of this task/ })).toHaveCount(0);
    await everywhere('perPage').click();
    await expect(
      app.palette.getByRole('button').filter({ hasText: `src/posts/feed.ts:1 · ${blog.name}` }),
    ).toBeVisible();
    await app.searchBox.fill('no such words anywhere');
    await everywhere('no such words anywhere').click();
    await expect(app.palette.getByText('Nothing in the code matches.')).toBeVisible();
  });

  test('SR-04 a late answer about other projects is never shown for a newer query, and never moves the pick', async ({
    app,
    project,
  }) => {
    // The page talks to the core through this test, which holds back every answer about the files
    // of other tasks until it is let through, as a slow look through many projects would.
    const asked = new Map<number, string>();
    const held: { query: string; release: () => void }[] = [];
    await app.page.routeWebSocket(/\/ws$/, (socket) => {
      const toCore = socket.connectToServer();
      socket.onMessage((message) => {
        if (typeof message === 'string') {
          const sent = JSON.parse(message) as { id: number; method?: string; params?: { query?: string } };
          if (sent.method === 'everywhere.files') asked.set(sent.id, sent.params?.query ?? '');
        }
        toCore.send(message);
      });
      toCore.onMessage((message) => {
        const query =
          typeof message === 'string' ? asked.get((JSON.parse(message) as { id?: number }).id ?? -1) : undefined;
        if (query === undefined) socket.send(message);
        else held.push({ query, release: () => socket.send(message) });
      });
    });
    const { titles } = await threeTasks(app, project);
    const waiting = () => held.map((answer) => answer.query);
    const tsRows = app.palette.getByRole('button').filter({ hasText: /\.ts/ });

    // The answer to "feed" is on its way when the query changes. The quick search of the task is not held up by it.
    await app.openSearch();
    await app.searchBox.fill('feed');
    await expect.poll(waiting).toEqual(['feed']);
    await app.searchBox.fill('limit');
    await expect(tsRows).toHaveCount(2);
    // It arrives now, too late: nothing of it is shown. Only then is the newer question asked, once.
    held.shift()!.release();
    await expect.poll(waiting).toEqual(['limit']);
    await expect(tsRows).toHaveCount(2);
    await expect(app.palette.getByRole('button').filter({ hasText: 'feed.ts' })).toHaveCount(0);

    // The user picks a row below the files. Rows of other projects that arrive later are added
    // above it, and the pick stays on the row it was on.
    const command = app.palette.getByRole('button', { name: /^Open limits and cost/ });
    for (let presses = 0; presses < 4 && (await command.getAttribute('aria-current')) !== 'true'; presses++) {
      await app.page.keyboard.press('ArrowUp');
    }
    await expect(command).toHaveAttribute('aria-current', 'true');
    held.shift()!.release();
    await expect(tsRows).toHaveCount(5);
    await expect(tsRows.filter({ hasText: titles.far })).toHaveCount(2);
    await expect(command).toHaveAttribute('aria-current', 'true');
    await app.page.keyboard.press('Enter');
    await expect(app.page).toHaveURL(/#\/agents$/);
  });
});
