import {
  APP_FILE,
  APP_TEXT,
  DEMO_FILE,
  DEMO_TEXT,
  expect,
  pullTemplate,
  test,
  unique,
  writeIn,
  type App,
} from './support/fixtures';

const tabs = (app: App) => app.page.getByRole('group', { name: 'Which changes to show' });
/** The part of the Changes view that is on show. */
const shown = (app: App) => tabs(app).locator('[aria-pressed="true"]');
const commitButton = (app: App) => app.git.getByRole('button', { name: /^Commit \d+ files?/ });
const fileBox = (app: App, file: string) => app.git.getByRole('checkbox', { name: `Put ${file} into the commit` });
/** The middle of the view: what the part on show changes. */
const middle = (app: App) => app.page.locator('section.center:visible');

async function commit(app: App, message: string) {
  await app.git.getByRole('textbox', { name: 'Commit message' }).fill(message);
  await commitButton(app).click();
}

test.describe('Changes: three parts along the path of a change', () => {
  test('CT-01 CT-02 the view opens on what is not committed, follows a commit to the commits, and an undo back to the files', async ({
    app,
    project,
  }) => {
    const shop = await project();
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    const folder = await app.folderOf(taskId);
    writeIn(folder, DEMO_FILE, DEMO_TEXT.replace('limit: 5,', 'limit: 6,'));
    writeIn(folder, APP_FILE, `${APP_TEXT}\n// a note\n`);
    await app.showView('Changes');

    // Three parts, each with how much it holds; the first that holds something is on show.
    await expect(tabs(app).getByRole('button')).toHaveText(['Not committed2', 'Committed', 'Pushed']);
    await expect(shown(app)).toHaveText('Not committed2');
    // What is new since the last visit is apart from them.
    await expect(app.git.getByRole('button', { name: 'Since last visit' })).toHaveAttribute('aria-pressed', 'false');

    // A commit that leaves a file behind: the file is still to be looked at, and the commit is counted.
    await fileBox(app, APP_FILE).uncheck();
    await commit(app, 'Allow six attempts');
    await expect(tabs(app).getByRole('button')).toHaveText(['Not committed1', 'Committed1', 'Pushed']);
    await expect(shown(app)).toHaveText('Not committed1');

    // With everything committed the view goes on to the commits.
    await fileBox(app, APP_FILE).check();
    await commit(app, 'Describe the app');
    await expect(shown(app)).toHaveText('Committed2');
    await expect(app.git.locator('.git-commit:visible')).toHaveText([/^Describe the app/, /^Allow six attempts/]);
    await expect(app.git).toContainText('There is no remote to push them to.');
    await expect(app.git.getByRole('button', { name: 'Push', exact: true })).toHaveCount(0);
    // The middle shows what those commits change, and nothing else.
    await expect(middle(app).getByRole('heading', { level: 2 })).toHaveText('Committed, not pushed');
    await expect(middle(app)).toContainText('2 commits');
    await expect(middle(app).locator('.span-file')).toHaveText([new RegExp(APP_FILE), new RegExp(DEMO_FILE)]);
    await middle(app)
      .getByRole('button', { name: `Show ${DEMO_FILE}` })
      .click();
    await expect(app.diffRow('add', 'limit: 6,')).toBeVisible();
    await expect(middle(app)).toContainText('What the commits that are not pushed change. 2 files');

    // An undo puts the newest commit back into the files; while a commit is left, the view stays with the commits.
    const newest = app.git.locator('.git-commit:visible').first();
    await newest.getByRole('button', { name: 'Undo' }).click();
    await expect(shown(app)).toHaveText('Committed1');
    await expect(tabs(app).getByRole('button').first()).toHaveText('Not committed1');
    // With the last one undone there is nothing left to push: back to the files.
    await app.git.locator('.git-commit:visible').getByRole('button', { name: 'Undo' }).click();
    await expect(shown(app)).toHaveText('Not committed2');
    await expect(commitButton(app)).toHaveText('Commit 2 files');

    // A part can be opened while it holds nothing, and says so.
    await tabs(app).getByRole('button', { name: 'Pushed' }).click();
    await expect(app.git).toContainText('There is no remote, so nothing can be pushed.');
    await expect(middle(app)).toContainText('Nothing is pushed yet.');
    await tabs(app)
      .getByRole('button', { name: /^Committed/ })
      .click();
    await expect(app.git).toContainText('No commits yet since main.');
  });

  test('CT-03 CT-04 a push with a request leads to the third part: the request under its name, the pipeline and what is pushed', async ({
    app,
    project,
  }) => {
    const shop = await project({ remote: 'github' });
    shop.setPullTemplate(pullTemplate({ checks: 'passed' }));
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    const task = await app.core.task(taskId);
    shop.setPullWhole(128, {
      number: 128,
      title: 'Allow six attempts',
      body: 'Closes #12.',
      state: 'OPEN',
      isDraft: false,
      author: { login: 'ann' },
      assignees: [],
      reviewRequests: [],
      labels: [],
      createdAt: '2026-11-10T08:00:00Z',
      updatedAt: '2026-11-12T19:05:30Z',
      url: `https://github.com/${shop.slug}/pull/128`,
      baseRefName: 'main',
      headRefName: task.branch,
      comments: [{ author: { login: 'ann' }, body: 'Ready for a look.', createdAt: '2026-11-11T10:00:00Z' }],
      reviews: [
        {
          author: { login: 'cy' },
          state: 'COMMENTED',
          body: 'Six is fine by me.',
          submittedAt: '2026-11-12T09:00:00Z',
        },
      ],
    });
    writeIn(task.cwd, DEMO_FILE, DEMO_TEXT.replace('limit: 5,', 'limit: 6,'));
    await app.showView('Changes');

    // Committed and not pushed: the second part has the push, alone or with a request.
    await commit(app, 'Allow six attempts');
    await expect(shown(app)).toHaveText('Committed1');
    await expect(app.git.getByText('1 not pushed')).toBeVisible();
    await expect(app.git.getByRole('button', { name: 'Push', exact: true })).toBeVisible();
    expect(shop.remoteBranches()).toEqual([]);
    await app.git.getByRole('button', { name: 'Push and open a pull request' }).click();

    // Pushed, with its request: the third part is called by the request's name and is on show.
    await expect(shown(app)).toHaveText('PR #128');
    await expect(tabs(app).getByRole('button')).toHaveText(['Not committed', 'Committed', 'PR #128']);
    expect(shop.remoteBranches()).toEqual([task.branch]);
    await expect(app.request).toContainText('Number 128 · open');
    await expect(app.request.getByRole('heading', { name: 'Before it can merge' })).toBeVisible();
    // What was written under the request is in its panel, and is not said here a second time.
    await expect(app.git.getByRole('region', { name: 'Comments', exact: true })).toHaveCount(0);
    // What is pushed, commit by commit: it can only be reverted.
    const pushed = app.git.locator('.git-commit:visible');
    await expect(pushed).toHaveText([/^Allow six attempts/]);
    await expect(pushed.getByRole('button', { name: 'Revert' })).toBeVisible();
    await expect(pushed.getByRole('button', { name: 'Undo' })).toHaveCount(0);
    // The middle shows what a reviewer sees.
    await expect(middle(app).getByRole('heading', { level: 2 })).toHaveText('In PR #128');
    await expect(middle(app)).toContainText('compared with where the branch left main');
    await expect(app.diffRow('add', 'limit: 6,')).toBeVisible();

    // The other two parts hold nothing now, and say so.
    await tabs(app)
      .getByRole('button', { name: /^Committed/ })
      .click();
    await expect(app.git).toContainText('Every commit is pushed.');
    await expect(middle(app)).toContainText('Nothing waits for a push.');
    await tabs(app).getByRole('button', { name: 'Not committed' }).click();
    await expect(app.git.getByText('Everything is committed.')).toBeVisible();

    // A change made while another part is read does not pull the view away: it is counted on its part.
    await tabs(app).getByRole('button', { name: 'PR #128' }).click();
    writeIn(task.cwd, DEMO_FILE, DEMO_TEXT.replace('limit: 5,', 'limit: 7,'));
    // Another program made it: the view learns of it when the window is come back to.
    await app.page.evaluate(() => window.dispatchEvent(new Event('focus')));
    await expect(tabs(app).getByRole('button').first()).toHaveText('Not committed1');
    await expect(shown(app)).toHaveText('PR #128');
    // Only what is pushed is in the middle: the new line is not.
    await expect(app.diffRow('add', 'limit: 6,')).toBeVisible();
    await expect(app.diffRow('add', 'limit: 7,')).toHaveCount(0);
  });
});
