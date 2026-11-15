import { existsSync } from 'node:fs';
import {
  DEMO_FILE,
  DEMO_TEXT,
  expect,
  pullTemplate,
  test,
  unique,
  writeIn,
  type App,
  type Project,
} from './support/fixtures';

const commitButton = (app: App) => app.git.getByRole('button', { name: /^Commit \d+ files?/ });
const gate = (app: App, title: string) => app.request.locator('.gate', { hasText: title });

/** A task with one change that is not committed yet, on its Changes view. */
async function taskWithAChange(app: App, shop: Project, title: string) {
  if (!app.page.url().endsWith('#/')) await app.openHome();
  const known = await app.page.getByLabel('Project', { exact: true }).getByRole('option', { name: shop.name }).count();
  const taskId = await app.createTask(known ? { text: title, project: shop.name } : { text: title, folder: shop.dir });
  const task = await app.core.task(taskId);
  writeIn(task.cwd, DEMO_FILE, DEMO_TEXT.replace('limit: 5,', 'limit: 6,'));
  await app.showView('Changes');
  await expect(commitButton(app)).toBeVisible();
  return task;
}

/** Commits the change, pushes and opens a pull request in one go. */
async function openRequest(app: App, message: string) {
  await app.git.getByRole('radio', { name: 'Push and open a pull request' }).check();
  await app.git.getByRole('textbox', { name: 'Commit message' }).fill(message);
  await expect(commitButton(app)).toHaveText('Commit 1 file, push and open a pull request');
  await commitButton(app).click();
  await expect(app.request).toContainText(/Number \d+ · open/);
}

test.describe('The merge request of a task', () => {
  test('M-02 M-03 M-05 M-09 a pull request is opened from its card: the branch is pushed first, three gates show how it stands', async ({
    app,
    project,
  }) => {
    const shop = await project({
      remote: 'github',
      issues: [{ number: 12, title: 'Login allows unlimited attempts', body: 'Five a minute is enough.' }],
    });
    await app.addProject(shop.dir);
    await app.page.goto(`${app.core.url}/#/issues`);
    const section = app.page.locator('section', { has: app.page.getByRole('heading', { level: 2, name: shop.name }) });
    await section.getByRole('button', { name: 'Start in editor' }).click();
    await expect(app.page).toHaveURL(/#\/task\/[^/]+\/code$/);
    await app.turnEnds();
    const task = await app.core.task(app.taskId());
    writeIn(task.cwd, DEMO_FILE, DEMO_TEXT.replace('limit: 5,', 'limit: 6,'));
    await app.showView('Changes');

    // Nothing is committed: there is nothing to ask a merge for.
    await expect(app.request.getByRole('heading', { name: 'Pull request' })).toBeVisible();
    await expect(app.request).toContainText('none yet');
    const create = app.request.getByRole('button', { name: 'Create' });
    await expect(create).toBeDisabled();
    await expect(create).toHaveAttribute('title', 'Commit something first');

    await app.commit('Take the login limit from config');
    // The title comes from the one commit, the description from the issue. The card reads them
    // again after a commit; a press within that moment still gets the older words, so look twice.
    const title = app.request.getByRole('textbox', { name: 'Title' });
    await expect(async () => {
      await create.click();
      try {
        await expect(title).toHaveValue('Take the login limit from config', { timeout: 500 });
      } catch (stale) {
        await app.request.getByRole('button', { name: 'Cancel' }).click();
        throw stale;
      }
    }).toPass();
    await expect(app.request.getByRole('textbox', { name: 'Description' })).toHaveValue('Closes #12.\n');
    await expect(app.request).toContainText(
      'Written from the commits and the issue. It goes into main; the branch is pushed first.',
    );
    const open = app.request.getByRole('button', { name: 'Push and open the pull request' });
    await title.fill(' ');
    await expect(open).toBeDisabled();
    await title.fill('Take the login limit from config');
    expect(shop.remoteBranches()).toEqual([]);
    await open.click();

    await expect(app.request).toContainText('Number 128 · open');
    await expect(app.request.getByRole('link', { name: 'Open on GitHub' })).toHaveAttribute(
      'href',
      `https://github.com/${shop.slug}/pull/128`,
    );
    // The branch went to the remote first, then the stand-in for gh was asked to open the request.
    expect(shop.remoteBranches()).toEqual([task.branch]);
    expect(app.core.calls('gh')).toContainEqual(
      `pr create -R ${shop.slug} --head ${task.branch} --base main --title Take the login limit from config --body Closes #12.`,
    );
    await expect(app.commitRow('Take the login limit from config')).not.toContainText('not pushed');
    await expect(app.activities.filter({ hasText: 'opened' })).toContainText('pull request 128');

    await expect(app.request.getByRole('heading', { name: 'Before it can merge' })).toBeVisible();
    await expect(gate(app, 'Conflicts')).toContainText('None');
    await expect(gate(app, 'Checks')).toContainText('1 passed');
    await expect(gate(app, 'Review')).toContainText('Not required');
    for (const what of ['Squash the commits into one', 'Delete the branch and its working copy', 'Archive this task']) {
      await expect(app.request.getByRole('checkbox', { name: what })).toBeChecked();
    }
    await expect(app.request).toContainText('#12 closes by itself if the description says “Closes #12”.');
    await expect(app.request.getByRole('button', { name: 'Merge now' })).toBeEnabled();
  });

  test('M-04 M-06 "push and open a pull request" right after the commit; "Merge now" merges and cleans up', async ({
    app,
    project,
  }) => {
    const shop = await project({ remote: 'github' });
    const title = unique('Describe the limiter');
    const task = await taskWithAChange(app, shop, title);
    await openRequest(app, 'Allow six attempts');
    expect(shop.remoteBranches()).toEqual([task.branch]);
    expect(app.core.calls('gh')).toContainEqual(
      expect.stringContaining(`pr create -R ${shop.slug} --head ${task.branch}`),
    );
    expect(existsSync(task.cwd)).toBe(true);

    await app.request.getByRole('button', { name: 'Merge now' }).click();
    // The task is finished: Home is shown, and the task is on none of its lists.
    await expect(app.page.getByRole('textbox', { name: 'New task' })).toBeVisible();
    await expect(app.page.getByRole('main').getByRole('link', { name: title })).toHaveCount(0);
    await expect(app.rail.getByRole('link', { name: new RegExp(`^${title}`) })).toHaveCount(0);
    expect(app.core.calls('gh')).toContainEqual(`pr merge 128 -R ${shop.slug} --squash --delete-branch`);
    // The working copy and the local branch are gone; the project folder itself is untouched.
    expect(existsSync(task.cwd)).toBe(false);
    expect(shop.git(['branch', '--list', task.branch!])).toBe('');
    expect(shop.read(DEMO_FILE)).toBe(DEMO_TEXT);
    const left = await app.core.request<{ id: string }[]>('tasks.list');
    expect(left.map((entry) => entry.id)).not.toContain(task.id);
  });

  test('M-08 E-06 a request with failed checks, running checks or a conflict cannot be merged; Home sorts them', async ({
    app,
    project,
  }) => {
    const shop = await project({ remote: 'github' });
    const merge = app.request.getByRole('button', { name: 'Merge now' });
    const locked = app.request.getByText('Merge now unlocks when nothing above stands in the way.');

    const failed = unique('Describe the failed checks');
    shop.setPullTemplate(pullTemplate({ checks: 'failed' }));
    await taskWithAChange(app, shop, failed);
    await openRequest(app, 'Checks will fail');
    await expect(gate(app, 'Checks')).toContainText('0 passed, 1 failed');
    await expect(merge).toBeDisabled();
    await expect(locked).toBeVisible();

    const running = unique('Describe the running checks');
    shop.setPullTemplate(pullTemplate({ checks: 'pending', review: 'REVIEW_REQUIRED' }));
    await app.goHome();
    await taskWithAChange(app, shop, running);
    await openRequest(app, 'Checks will run');
    await expect(gate(app, 'Checks')).toContainText('1 passed, 1 running');
    await expect(gate(app, 'Review')).toContainText('Waiting for a review');
    await expect(merge).toBeDisabled();

    const conflict = unique('Describe the conflict');
    shop.setPullTemplate(pullTemplate({ mergeable: 'CONFLICTING' }));
    await app.goHome();
    await taskWithAChange(app, shop, conflict);
    await openRequest(app, 'This will conflict');
    await expect(gate(app, 'Conflicts')).toContainText('It conflicts with its target. Update the branch.');
    await expect(gate(app, 'Checks')).toContainText('1 passed');
    await expect(merge).toBeDisabled();
    await expect(locked).toBeVisible();

    // Home puts each of them where it belongs.
    await app.goHome();
    await expect(app.stageRow('Checks failed', failed)).toContainText('Request 128: 1 failed');
    await expect(app.stageRow('Checks running', running)).toContainText('1 of 2 checks done.');
    await expect(app.stageRow('Your turn', conflict)).toBeVisible();
    await expect(app.rail.getByRole('link', { name: `${failed}: checks failed` })).toBeVisible();
    await expect(app.rail.getByRole('link', { name: `${running}: checks 1 of 2` })).toBeVisible();
  });
});

test.describe('Finishing merge requests from Home', () => {
  // Home counts every task of the core, so these tests get a core of their own.
  test.use({ freshCore: true });

  test('E-04 E-05 "Ready to merge": one press merges and finishes every request, and says which it could not', async ({
    app,
    project,
  }) => {
    const shop = await project({ remote: 'github' });
    const first = unique('Describe the first limiter');
    const second = unique('Describe the second limiter');
    const third = unique('Describe the third limiter');
    const firstTask = await taskWithAChange(app, shop, first);
    await openRequest(app, 'First change');
    await app.goHome();

    const ready = app.stage('Ready to merge');
    await expect(ready.getByRole('heading')).toHaveText('Ready to merge 1');
    await expect(app.stageRow('Ready to merge', first)).toContainText(
      'Request 128 is open and nothing stands in its way.',
    );
    await expect(ready.getByRole('button', { name: 'Merge and finish', exact: true })).toBeVisible();
    await expect(app.page.getByRole('heading', { level: 1 })).toHaveText(
      'Nothing needs you, one thing is ready to merge',
    );
    await expect(app.rail.getByRole('link', { name: `${first}: ready` })).toBeVisible();

    const secondTask = await taskWithAChange(app, shop, second);
    await openRequest(app, 'Second change');
    await app.goHome();
    const thirdTask = await taskWithAChange(app, shop, third);
    await openRequest(app, 'Third change');
    await app.goHome();
    await expect(ready.getByRole('heading')).toHaveText('Ready to merge 3');
    await expect(app.page.getByRole('heading', { level: 1 })).toHaveText(
      'Nothing needs you, three things are ready to merge',
    );
    // Meanwhile, on the host, the third request ran into a conflict. Home does not know yet.
    shop.changePull(thirdTask.branch!, (pull) => pull.replace('MERGEABLE', 'CONFLICTING'));

    await ready.getByRole('button', { name: 'Merge and finish all 3' }).click();
    const problem = app.page.getByRole('alert');
    await expect(problem).toContainText(
      `${third}: This pull request conflicts with its target. Update the branch first.`,
    );
    const main = app.page.getByRole('main');
    await expect(main.getByRole('link', { name: first })).toHaveCount(0);
    await expect(main.getByRole('link', { name: second })).toHaveCount(0);
    await expect(main.getByRole('link', { name: third })).toBeVisible();

    const calls = app.core.calls('gh');
    expect(calls).toContain(`pr merge 128 -R ${shop.slug} --squash --delete-branch`);
    expect(calls).toContain(`pr merge 129 -R ${shop.slug} --squash --delete-branch`);
    expect(calls).not.toContainEqual(expect.stringContaining('pr merge 130'));
    expect(existsSync(firstTask.cwd)).toBe(false);
    expect(existsSync(secondTask.cwd)).toBe(false);
    expect(existsSync(thirdTask.cwd)).toBe(true);
    expect(shop.git(['branch', '--list', '--format=%(refname:short)', 'task/*'])).toBe(thirdTask.branch);
    await problem.getByRole('button', { name: 'Dismiss' }).click();
    await expect(problem).toBeHidden();
  });

  test('E-07 "Merged, not cleaned up": "Clean up" removes the working copy and the branch and archives the task', async ({
    app,
    project,
  }) => {
    const shop = await project({ remote: 'github' });
    const title = unique('Describe the limiter');
    const task = await taskWithAChange(app, shop, title);
    await openRequest(app, 'Allow six attempts');

    // Merge, but keep the branch, the working copy and the task.
    await app.request.getByRole('checkbox', { name: 'Delete the branch and its working copy' }).uncheck();
    await app.request.getByRole('checkbox', { name: 'Archive this task' }).uncheck();
    await app.request.getByRole('button', { name: 'Merge now' }).click();
    await expect(app.request).toContainText('Number 128 · merged');
    await expect(app.request).toContainText('It is merged. This task can be archived from Home.');
    expect(app.core.calls('gh')).toContain(`pr merge 128 -R ${shop.slug} --squash`);
    expect(existsSync(task.cwd)).toBe(true);
    await expect(app.activities.filter({ hasText: 'merged' })).toContainText('pull request 128');

    await app.goHome();
    const merged = app.stage('Merged, not cleaned up');
    await expect(app.stageRow('Merged, not cleaned up', title)).toContainText('Request 128 is merged.');
    await expect(app.rail.getByRole('link', { name: `${title}: merged` })).toBeVisible();
    await merged.getByRole('button', { name: 'Clean up', exact: true }).click();

    await expect(app.page.getByRole('heading', { level: 1 })).toHaveText('No tasks yet');
    expect(existsSync(task.cwd)).toBe(false);
    expect(shop.git(['branch', '--list', 'task/*'])).toBe('');
    expect(await app.core.request<unknown[]>('tasks.list')).toEqual([]);
  });
});
