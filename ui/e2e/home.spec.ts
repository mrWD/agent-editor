import { existsSync } from 'node:fs';
import path from 'node:path';
import { AGENT_LINE, DEMO_FILE, DEMO_TEXT, expect, PLAIN_ANSWER, readIn, test, unique } from './support/fixtures';

test.describe('Home on a core of its own', () => {
  test.use({ freshCore: true });

  test('A-01 the core serves the interface and Home opens', async ({ app }) => {
    await app.openHome();
    await expect(app.page).toHaveTitle(/Agent Editor/);
    await expect(app.page.getByRole('heading', { level: 1, name: 'No tasks yet' })).toBeVisible();
    await expect(app.page.getByText('The practice agent is free and calls no model.')).toBeVisible();
    await expect(app.rail.getByText('No projects yet')).toBeVisible();
    await expect(app.page.getByRole('link', { name: 'Home' })).toHaveAttribute('aria-current', 'page');
  });

  test('E-01 the headline counts what waits for you', async ({ app, project }) => {
    const shop = await project();
    const headline = app.page.getByRole('heading', { level: 1 });
    await app.openHome();
    await expect(headline).toHaveText('No tasks yet');

    await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    await app.goHome();
    await expect(headline).toHaveText('Nothing needs you, one thing waits for your next word');
    await expect(app.page.getByText('1 task in 1 project.')).toBeVisible();

    await app.createTask({ text: unique('Change the limit'), project: shop.name, until: 'question' });
    await app.goHome();
    await expect(headline).toHaveText('One thing needs you, one thing waits for your next word');
    await expect(app.page.getByText('2 tasks in 1 project.')).toBeVisible();
    // The same count sits next to "Home" on every screen.
    await expect(app.page.getByRole('link', { name: /^Home/ }).getByLabel('1 need you')).toHaveText('1');
  });

  test('B-05 the project filter in the header shows the tasks of one project', async ({ app, project }) => {
    const first = await project({ name: 'shop' });
    const second = await project({ name: 'blog' });
    const firstTitle = unique('Describe the shop limiter');
    const secondTitle = unique('Describe the blog limiter');
    await app.openHome();
    await app.createTask({ text: firstTitle, folder: first.dir });
    await app.goHome();
    await app.createTask({ text: secondTitle, folder: second.dir });
    await app.goHome();

    const filter = app.page.getByRole('group', { name: 'Show tasks of' });
    const main = app.page.getByRole('main');
    await expect(main.getByRole('link', { name: firstTitle })).toBeVisible();
    await expect(main.getByRole('link', { name: secondTitle })).toBeVisible();

    await filter.getByRole('button', { name: new RegExp(`^${first.name} 1$`) }).click();
    await expect(main.getByRole('link', { name: firstTitle })).toBeVisible();
    await expect(main.getByRole('link', { name: secondTitle })).toBeHidden();
    await expect(app.page.getByText('1 task in 1 project.')).toBeVisible();
    // The project picked at the top is also the one a new task goes to.
    await expect(app.page.getByLabel('Project', { exact: true }).locator('option:checked')).toHaveText(first.name);

    await filter.getByRole('button', { name: new RegExp(`^${second.name} 1$`) }).click();
    await expect(main.getByRole('link', { name: firstTitle })).toBeHidden();
    await expect(main.getByRole('link', { name: secondTitle })).toBeVisible();

    await filter.getByRole('button', { name: /^All \d+$/ }).click();
    await expect(main.getByRole('link', { name: firstTitle })).toBeVisible();
    await expect(main.getByRole('link', { name: secondTitle })).toBeVisible();
  });
});

test.describe('Home', () => {
  test('B-01 C-01 a task from another folder: the folder becomes a project and the answer comes in pieces', async ({
    app,
    project,
  }) => {
    const shop = await project();
    const title = unique('Describe the limiter');
    await app.openHome();
    await app.createTask({ text: title, folder: shop.dir });

    // The folder is a project now, named after the folder; the task is named after its first line.
    await expect(app.page.getByRole('heading', { level: 1, name: title })).toBeVisible();
    await expect(app.page.locator('.subbar').getByText(shop.name, { exact: true })).toBeVisible();
    await expect(app.conversation.getByText(title, { exact: true })).toBeVisible();
    await expect(app.answers).toHaveText([PLAIN_ANSWER]);
    await expect(app.conversation.getByText('Paused', { exact: true })).toBeVisible();

    // The next answer is watched as it grows: it arrives word by word, not in one piece.
    await app.conversation.evaluate((pane) => {
      const lengths: number[] = [];
      (window as unknown as { answerLengths: number[] }).answerLengths = lengths;
      new MutationObserver(() => {
        const said = pane.querySelectorAll('article p');
        if (said.length < 2) return;
        const length = said[said.length - 1]!.textContent.length;
        if (lengths[lengths.length - 1] !== length) lengths.push(length);
      }).observe(pane, { subtree: true, childList: true, characterData: true });
    });
    await app.say('Say it once more');
    await expect(app.answers).toHaveText([PLAIN_ANSWER, PLAIN_ANSWER]);
    const lengths = await app.page.evaluate(() => (window as unknown as { answerLengths: number[] }).answerLengths);
    expect(lengths.length).toBeGreaterThan(5);
    expect(lengths).toEqual([...lengths].sort((a, b) => a - b));
    expect(lengths[lengths.length - 1]).toBe(PLAIN_ANSWER.length);

    // The project can be picked for the next task, and stands in the rail.
    await app.goHome();
    await expect(app.page.getByLabel('Project', { exact: true }).getByRole('option', { name: shop.name })).toHaveCount(
      1,
    );
    await expect(app.rail.getByText(shop.name, { exact: true })).toBeVisible();
  });

  test('B-02 B-03 the same folder twice is one project; a folder that is not there is refused', async ({
    app,
    project,
  }) => {
    const shop = await project();
    await app.openHome();
    await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    await app.goHome();
    await app.createTask({ text: unique('Describe it again'), folder: shop.dir });
    await app.goHome();
    await expect(app.page.getByLabel('Project', { exact: true }).getByRole('option', { name: shop.name })).toHaveCount(
      1,
    );

    const missing = path.join(shop.dir, 'no-such-folder');
    await app.page.getByRole('textbox', { name: 'New task' }).fill(unique('Describe nothing'));
    await app.page.getByLabel('Project', { exact: true }).selectOption({ label: 'Another folder' });
    await app.page.getByLabel('Folder of the new project').fill(missing);
    await app.page.getByRole('button', { name: 'Start', exact: true }).click();
    await expect(app.page.getByRole('alert')).toContainText(`No such folder: ${missing}`);
    await expect(app.page).toHaveURL(/#\/$/);

    // Nothing to describe is refused before the core is asked.
    await app.page.getByRole('textbox', { name: 'New task' }).fill('   ');
    await app.page.getByRole('button', { name: 'Start', exact: true }).click();
    await expect(app.page.getByRole('alert')).toHaveText('Describe the task first.');
  });

  test('E-02 the question of an agent is answered from Home: allowed and refused', async ({ app, project }) => {
    const shop = await project();
    const allowed = unique('Change the limit');
    const refused = unique('Change the limit back');
    await app.openHome();
    const allowedId = await app.createTask({ text: allowed, folder: shop.dir, until: 'question' });
    await app.goHome();
    const refusedId = await app.createTask({ text: refused, project: shop.name, until: 'question' });
    await app.goHome();

    const card = (title: string) => app.page.getByRole('article').filter({ hasText: title });
    await expect(card(allowed)).toContainText('Practice agent asks before it acts');
    await expect(card(allowed)).toContainText(`Edit ${DEMO_FILE}`);
    await expect(card(allowed).getByRole('link', { name: 'Open' })).toBeVisible();

    await card(allowed).getByRole('button', { name: 'Allow this edit' }).click();
    await expect(card(allowed)).toBeHidden();
    await expect(app.stageRow('Your turn', allowed)).toContainText('Done. The limit now comes from config');
    expect(readIn(await app.folderOf(allowedId), DEMO_FILE)).toContain(AGENT_LINE);

    await card(refused).getByRole('button', { name: 'Do not edit' }).click();
    await expect(card(refused)).toBeHidden();
    await expect(app.stageRow('Your turn', refused)).toContainText('I left the file as it was');
    expect(readIn(await app.folderOf(refusedId), DEMO_FILE)).toBe(DEMO_TEXT);

    // The task itself tells the same story as if the answer had been given there.
    await app.stageRow('Your turn', allowed).getByRole('link', { name: allowed }).click();
    await expect(app.conversation.getByText(`You answered “Allow this edit” to: Edit ${DEMO_FILE}.`)).toBeVisible();
  });

  test('E-08 C-11 a finished task says where it stopped, and Archive takes it off every list', async ({
    app,
    project,
  }) => {
    const shop = await project();
    const title = unique('Describe the limiter');
    await app.openHome();
    const taskId = await app.createTask({ text: title, folder: shop.dir });
    const folder = await app.folderOf(taskId);
    await app.goHome();

    const row = app.stageRow('Your turn', title);
    await expect(row).toContainText(shop.name);
    await expect(row).toContainText('just now: The limiter allows five attempts a minute');
    await expect(row.getByRole('link', { name: 'Code' })).toHaveAttribute('href', `#/task/${taskId}/code`);
    await expect(app.rail.getByRole('link', { name: `${title}: your turn` })).toBeVisible();

    await row.getByRole('button', { name: 'Archive' }).click();
    await expect(app.page.getByRole('main').getByRole('link', { name: title })).toBeHidden();
    await expect(app.rail.getByRole('link', { name: `${title}: your turn` })).toBeHidden();
    const left = await app.core.request<{ id: string }[]>('tasks.list');
    expect(left.map((task) => task.id)).not.toContain(taskId);
    // Archiving keeps the working copy; only finishing a merge removes it.
    expect(existsSync(folder)).toBe(true);

    await app.page.goto(`${app.core.url}/#/task/${taskId}/code`);
    await expect(app.page.getByRole('heading', { name: 'This task is not here' })).toBeVisible();
    await app.page.getByRole('link', { name: 'Go to Home' }).click();
    await expect(app.page.getByRole('textbox', { name: 'New task' })).toBeVisible();
  });
});
