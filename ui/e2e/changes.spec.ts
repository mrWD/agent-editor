import { existsSync, mkdirSync, rmSync } from 'node:fs';
import path from 'node:path';
import {
  AGENT_LINE,
  APP_FILE,
  APP_TEXT,
  DEMO_FILE,
  DEMO_TEXT,
  expect,
  readIn,
  test,
  unique,
  writeIn,
  type App,
} from './support/fixtures';

/** The long file with two changes far apart: one near its top, one near its end. */
const TOP_OLD = 'export type Order = { id: number; total: number };';
const TOP_NEW = 'export type Order = { id: number; total: number; note?: string };';
const END_OLD = '  return `order ${order.id}: ${order.total}`;';
const END_NEW = '  return `order ${order.id} costs ${order.total}`;';
const APP_TWO_CHANGES = APP_TEXT.replace(TOP_OLD, TOP_NEW).replace(END_OLD, END_NEW);

const fileBox = (app: App, file: string) => app.git.getByRole('checkbox', { name: `Put ${file} into the commit` });
const fileRow = (app: App, file: string) => app.git.locator('.git-file', { hasText: file });
const commitButton = (app: App) => app.git.getByRole('button', { name: /^(Commit|Add) \d+ files?/ });
const commits = (app: App) => app.git.locator('.git-commit');

test.describe('Changes: what goes into a commit', () => {
  test('L-01 L-02 I-07 changed files are listed with counts; the diff has line numbers, and a number attaches its line', async ({
    app,
    project,
  }) => {
    const shop = await project();
    const title = unique('Describe the limiter');
    await app.openHome();
    const taskId = await app.createTask({ text: title, folder: shop.dir });
    const folder = await app.folderOf(taskId);
    await app.letAgentEdit('Change the limit');
    // One more file is new and one is gone, as another program would leave them.
    mkdirSync(path.join(folder, 'notes'));
    writeIn(folder, 'notes/todo.md', 'one\ntwo\n');
    rmSync(path.join(folder, 'README.md'));

    await app.showView('Changes');
    await expect(app.git.locator('.git-file')).toHaveCount(3);
    await expect(fileRow(app, 'README.md')).toContainText('−3');
    await expect(fileRow(app, 'README.md')).toContainText('deleted');
    await expect(fileRow(app, 'notes/todo.md')).toContainText('+2');
    await expect(fileRow(app, 'notes/todo.md')).toContainText('new');
    await expect(fileRow(app, DEMO_FILE)).toContainText('+1−1');
    await expect(fileRow(app, DEMO_FILE).getByRole('img', { name: 'Changed by Practice agent' })).toBeVisible();
    const views = app.page.getByRole('navigation', { name: 'Views of this task' });
    await expect(views.getByRole('link', { name: /^Changes/ }).getByTitle('files that are not committed')).toHaveText(
      '3',
    );
    await expect(app.git.getByText('Ticked files go into the commit')).toBeVisible();
    await expect(app.git.getByText('No commits yet since main.')).toBeVisible();

    // The diff of one file: what was taken out and put in, with the numbers of the lines and who wrote them.
    await app.git.getByRole('button', { name: new RegExp(`^${DEMO_FILE}`) }).click();
    await expect(app.page.getByRole('heading', { level: 2, name: DEMO_FILE })).toBeVisible();
    await expect(app.page.locator('.diff-head')).toContainText('1 change, in the commit');
    await expect(app.page.locator('.hunk-bar')).toContainText('Line 3, written by Practice agent');
    await expect(app.diffRow('remove', 'limit: 5,')).toHaveText('3−  limit: 5,');
    await expect(app.diffRow('add', 'limits.login')).toHaveText(`3+${AGENT_LINE}`);
    await expect(app.diffRow('context', 'windowMs')).toHaveText('2   windowMs: 60 * 1000,');

    // A line number asks the agent about that line.
    await app.diffRow('add', 'limits.login').getByRole('button', { name: '3' }).click();
    await expect(app.conversation.locator('.quote')).toContainText('rateLimit.ts line 3');
    await expect(app.messageBox).toBeFocused();
    await app.say('Why this line?');
    await expect(app.messages.last()).toContainText(`About \`${DEMO_FILE}\`, line 3:`);
    await expect(app.messages.last()).toContainText(AGENT_LINE.trim());
    await expect(app.messages.last()).toContainText('Why this line?');

    // From the diff straight to the same place in the editor.
    await app.page.getByRole('button', { name: 'Open in the editor' }).click();
    await expect(app.page).toHaveURL(new RegExp(`#/task/${taskId}/code$`));
    await expect(app.tab('rateLimit.ts')).toHaveAttribute('aria-selected', 'true');
    await expect(app.page.getByText('Line 3, column 1', { exact: true })).toBeVisible();
  });

  test('L-03 L-06 only ticked files are committed; without a message or a tick there is nothing to press', async ({
    app,
    project,
  }) => {
    const shop = await project();
    const title = unique('Describe the limiter');
    await app.openHome();
    const taskId = await app.createTask({ text: title, folder: shop.dir });
    const task = await app.core.task(taskId);
    await app.letAgentEdit('Change the limit');
    writeIn(task.cwd, APP_FILE, APP_TWO_CHANGES);

    await app.showView('Changes');
    const message = app.git.getByRole('textbox', { name: 'Commit message' });
    await expect(message).toHaveValue(title);
    await expect(app.git.getByText("Taken from the task's title. Edit it freely.")).toBeVisible();
    await expect(commitButton(app)).toHaveText('Commit 2 files');

    // No message: nothing to press.
    await message.fill('  ');
    await expect(commitButton(app)).toBeDisabled();
    await message.fill('Take the limit from config');
    await expect(commitButton(app)).toBeEnabled();

    // No tick: nothing to press either.
    await fileBox(app, APP_FILE).uncheck();
    await expect(commitButton(app)).toHaveText('Commit 1 file');
    await fileBox(app, DEMO_FILE).uncheck();
    await expect(commitButton(app)).toHaveText('Commit 0 files');
    await expect(commitButton(app)).toBeDisabled();
    await fileBox(app, DEMO_FILE).check();

    await commitButton(app).click();
    await expect(commits(app)).toHaveText([/^Take the limit from config.*Test, just now.*not pushed/]);
    await expect(app.git.locator('.git-file')).toHaveCount(1);
    await expect(fileRow(app, APP_FILE)).toBeVisible();
    await expect(app.activities.filter({ hasText: 'committed' })).toContainText('Take the limit from config');
    // On disk: one commit with the one file; the other file is still changed.
    expect(shop.git(['log', '--format=%s', `main..${task.branch}`])).toBe('Take the limit from config');
    expect(shop.git(['show', '--format=', '--name-only', task.branch!])).toBe(DEMO_FILE);
    expect(shop.git(['status', '--porcelain'], task.cwd)).toBe(`M ${APP_FILE}`);
  });

  test('L-04 one change of a file is committed and the other stays', async ({ app, project }) => {
    const shop = await project();
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    const task = await app.core.task(taskId);
    writeIn(task.cwd, APP_FILE, APP_TWO_CHANGES);

    await app.showView('Changes');
    const bars = app.page.locator('.hunk-bar');
    await expect(bars).toHaveCount(2);
    await expect(bars.nth(0)).toContainText('Line 4');
    await expect(bars.nth(1)).toContainText('Line 29');
    await expect(app.page.locator('.diff-head')).toContainText('2 changes, all in the commit');

    await bars.nth(1).getByRole('checkbox', { name: 'In the commit' }).uncheck();
    await expect(app.page.locator('.diff-head')).toContainText('2 changes, 1 in the commit');
    await expect(fileRow(app, APP_FILE)).toContainText('1 of 2 changes ticked');
    expect(await fileBox(app, APP_FILE).evaluate((box: HTMLInputElement) => box.indeterminate)).toBe(true);

    await app.commit('Give an order a note');
    // The file is still listed, now with the one change that was left out.
    await expect(bars).toHaveCount(1);
    await expect(bars).toContainText('Line 29');
    await expect(app.diffRow('add', 'costs')).toBeVisible();
    await expect(app.page.locator('.diff-row', { hasText: 'note?: string' })).toHaveCount(0);
    const committed = shop.git(['show', '--format=', task.branch!]);
    expect(committed).toContain(`+${TOP_NEW}`);
    expect(committed).not.toContain('costs');
    expect(readIn(task.cwd, APP_FILE)).toBe(APP_TWO_CHANGES);
  });

  test('L-05 "Add to the last commit" keeps one commit on the branch', async ({ app, project }) => {
    const shop = await project();
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    const task = await app.core.task(taskId);
    writeIn(task.cwd, DEMO_FILE, DEMO_TEXT.replace('limit: 5,', 'limit: 6,'));
    await app.showView('Changes');
    const amend = app.git.getByRole('checkbox', { name: 'Add to the last commit, not a new one' });
    // There is no commit to add to yet.
    await expect(amend).toBeDisabled();
    await app.commit('Allow six attempts');

    // The view reads the files when it opens; a change by another program shows the next time.
    writeIn(task.cwd, APP_FILE, APP_TWO_CHANGES);
    await app.showView('Code');
    await app.showView('Changes');
    await expect(fileRow(app, APP_FILE)).toBeVisible();
    await amend.check();
    await expect(commitButton(app)).toHaveText('Add 1 file to the last commit');
    await app.commit('Allow six attempts and note orders');

    await expect(commits(app)).toHaveCount(1);
    await expect(app.git.getByText('Everything is committed.')).toBeVisible();
    expect(shop.git(['log', '--format=%s', `main..${task.branch}`])).toBe('Allow six attempts and note orders');
    expect(shop.git(['show', '--format=', '--name-only', task.branch!]).split('\n')).toEqual([APP_FILE, DEMO_FILE]);
  });

  test('L-09 Undo takes a commit that is not pushed off the branch and keeps its changes', async ({ app, project }) => {
    const shop = await project();
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    const task = await app.core.task(taskId);
    const changed = DEMO_TEXT.replace('limit: 5,', 'limit: 6,');
    writeIn(task.cwd, DEMO_FILE, changed);
    await app.showView('Changes');
    await app.commit('Allow six attempts');
    await expect(app.git.getByText('Everything is committed.')).toBeVisible();

    await app.commitRow('Allow six attempts').getByRole('button', { name: 'Undo' }).click();
    await expect(commits(app)).toHaveCount(0);
    await expect(app.git.getByText('No commits yet since main.')).toBeVisible();
    await expect(fileRow(app, DEMO_FILE)).toBeVisible();
    await expect(app.activities.filter({ hasText: 'undid the commit' })).toContainText('Allow six attempts');
    expect(shop.git(['log', '--format=%s', `main..${task.branch}`])).toBe('');
    expect(readIn(task.cwd, DEMO_FILE)).toBe(changed);
  });

  test('L-12 Revert of one change and of a file wants a second press', async ({ app, project }) => {
    const shop = await project();
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    const folder = await app.folderOf(taskId);
    writeIn(folder, APP_FILE, APP_TWO_CHANGES);
    writeIn(folder, DEMO_FILE, DEMO_TEXT.replace('limit: 5,', 'limit: 6,'));
    await app.showView('Changes');
    await app.git.getByRole('button', { name: new RegExp(`^${APP_FILE}`) }).click();
    const bars = app.page.locator('.hunk-bar');
    await expect(bars).toHaveCount(2);

    // One change: the first press only asks.
    await bars.nth(1).getByRole('button', { name: 'Revert this change' }).click();
    const sure = bars.nth(1).getByRole('button', { name: 'Really revert?' });
    await expect(sure).toBeVisible();
    expect(readIn(folder, APP_FILE)).toBe(APP_TWO_CHANGES);
    await sure.click();
    await expect(bars).toHaveCount(1);
    await expect(bars).toContainText('Line 4');
    expect(readIn(folder, APP_FILE)).toBe(APP_TEXT.replace(TOP_OLD, TOP_NEW));

    // A whole file, from its row in the list.
    const revert = app.git.getByTitle(`Put ${DEMO_FILE} back as it is in the last commit`);
    await expect(revert).toHaveText('Revert');
    await revert.click();
    await expect(revert).toHaveText('Really revert?');
    expect(readIn(folder, DEMO_FILE)).toContain('limit: 6,');
    await revert.click();
    await expect(fileRow(app, DEMO_FILE)).toBeHidden();
    expect(readIn(folder, DEMO_FILE)).toBe(DEMO_TEXT);

    // And the file on screen, from the head of its diff.
    await app.page.getByRole('button', { name: 'Revert this file' }).click();
    await app.page.getByRole('button', { name: 'Really revert?' }).click();
    await expect(app.git.getByText('Everything is committed.')).toBeVisible();
    await expect(app.page.getByRole('heading', { name: 'Nothing to show' })).toBeVisible();
    expect(readIn(folder, APP_FILE)).toBe(APP_TEXT);
  });

  test('L-12 the first press is remembered while the list is read again, and is not spent on another file', async ({
    app,
    project,
  }) => {
    const shop = await project();
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    const folder = await app.folderOf(taskId);
    const changed = DEMO_TEXT.replace('limit: 5,', 'limit: 6,');
    writeIn(folder, DEMO_FILE, changed);
    writeIn(folder, APP_FILE, APP_TWO_CHANGES);
    await app.showView('Changes');
    await expect(app.git.locator('.git-file')).toHaveCount(2);

    // A file's row: the first press survives a new reading of the list, as when the agent changes a file meanwhile.
    const revert = app.git.getByTitle(`Put ${DEMO_FILE} back as it is in the last commit`);
    await revert.click();
    await expect(revert).toHaveText('Really revert?');
    await app.core.request('files.write', { taskId, path: 'README.md', text: '# Changed\n', version: null });
    await expect(app.git.locator('.git-file')).toHaveCount(3);
    await expect(revert).toHaveText('Really revert?');
    await revert.click();
    await expect(fileRow(app, DEMO_FILE)).toBeHidden();
    expect(readIn(folder, DEMO_FILE)).toBe(DEMO_TEXT);

    // The button over the diff: a first press for one file does not count for the file picked next.
    await app.git.getByRole('button', { name: new RegExp(`^${APP_FILE}`) }).click();
    await expect(app.page.getByRole('heading', { level: 2, name: APP_FILE })).toBeVisible();
    await app.page.getByRole('button', { name: 'Revert this file' }).click();
    await expect(app.page.getByRole('button', { name: 'Really revert?' })).toBeVisible();
    await app.git.getByRole('button', { name: /^README\.md/ }).click();
    await expect(app.page.getByRole('heading', { level: 2, name: 'README.md' })).toBeVisible();
    await expect(app.page.getByRole('button', { name: 'Really revert?' })).toHaveCount(0);
    await app.page.getByRole('button', { name: 'Revert this file' }).click();
    await expect(app.page.getByRole('button', { name: 'Really revert?' })).toBeVisible();
    expect(readIn(folder, 'README.md')).toBe('# Changed\n');
    expect(readIn(folder, APP_FILE)).toBe(APP_TWO_CHANGES);
  });
});

test.describe('Changes: the branch and its remote', () => {
  test('L-07 L-10 L-11 a commit is pushed to the remote; a pushed commit is taken back with Revert', async ({
    app,
    project,
  }) => {
    const shop = await project({ remote: 'bare' });
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    const task = await app.core.task(taskId);
    writeIn(task.cwd, DEMO_FILE, DEMO_TEXT.replace('limit: 5,', 'limit: 6,'));
    await app.showView('Changes');
    expect(shop.remoteBranches()).toEqual([]);

    await app.git.getByRole('radio', { name: 'Push', exact: true }).check();
    await expect(commitButton(app)).toHaveText('Commit 1 file and push');
    await app.commit('Allow six attempts');
    const first = app.commitRow('Allow six attempts');
    await expect(first).toContainText(/Test, just now\s*pushed/);
    expect(shop.remoteBranches()).toEqual([task.branch]);
    expect(shop.git(['log', '--format=%s', '-1', task.branch!], shop.bare!)).toBe('Allow six attempts');
    await expect(app.activities.filter({ hasText: 'pushed' }).last()).toBeVisible();
    // The remote is a plain folder, not GitHub or GitLab: no request can be opened.
    await expect(app.request).toContainText("This project's remote is not on GitHub or GitLab");

    // What is pushed cannot be undone, only reverted, and that wants a second press.
    await expect(first.getByRole('button', { name: 'Undo' })).toHaveCount(0);
    await first.getByRole('button', { name: 'Revert' }).click();
    await first.getByRole('button', { name: 'Really revert?' }).click();
    const revert = app.commitRow('Revert "Allow six attempts"');
    await expect(revert).toContainText('not pushed');
    await expect(commits(app)).toHaveCount(2);
    expect(readIn(task.cwd, DEMO_FILE)).toBe(DEMO_TEXT);

    // The branch says one commit is not pushed, and pushes it.
    await expect(app.git.getByText('1 not pushed')).toBeVisible();
    await app.git.getByRole('button', { name: 'Push', exact: true }).click();
    await expect(app.git.getByText('1 not pushed')).toBeHidden();
    await expect(revert).not.toContainText('not pushed');
    expect(shop.git(['log', '--format=%s', '-1', task.branch!], shop.bare!)).toBe('Revert "Allow six attempts"');
  });

  test('L-08 a project without a remote cannot push, and says why', async ({ app, project }) => {
    const shop = await project();
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    writeIn(await app.folderOf(taskId), DEMO_FILE, DEMO_TEXT.replace('limit: 5,', 'limit: 6,'));
    await app.showView('Changes');

    await expect(app.git.getByRole('radio', { name: /^Push/ })).toBeDisabled();
    await expect(app.git.getByText('this project has no remote')).toBeVisible();
    await expect(app.git.getByRole('radio', { name: 'Stop there' })).toBeChecked();
    await app.commit('Allow six attempts');
    await expect(app.git.getByText('1 not pushed')).toBeVisible();
    await expect(app.git.getByRole('button', { name: 'Push', exact: true })).toHaveCount(0);
  });

  test('L-13 "Update from main" brings the new commits of the base into the branch', async ({ app, project }) => {
    const shop = await project();
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    const folder = await app.folderOf(taskId);
    await app.showView('Changes');
    await expect(app.git.getByText('up to date with main')).toBeVisible();

    shop.write('NEWS.md', 'Something new on main.\n');
    shop.git(['add', 'NEWS.md']);
    shop.git(['commit', '-q', '-m', 'Add the news']);
    await app.showView('Code');
    await app.showView('Changes');
    await expect(app.git.getByText('main has 1 new commit')).toBeVisible();
    expect(existsSync(path.join(folder, 'NEWS.md'))).toBe(false);

    await app.git.getByRole('button', { name: 'Update from main' }).click();
    await expect(app.git.getByText('up to date with main')).toBeVisible();
    expect(readIn(folder, 'NEWS.md')).toBe('Something new on main.\n');
    await expect(app.activities.filter({ hasText: 'updated from' })).toContainText('main');
  });

  test('L-14 an update that would conflict is called off and names the file', async ({ app, project }) => {
    const shop = await project();
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    const folder = await app.folderOf(taskId);
    const mine = DEMO_TEXT.replace('limit: 5,', 'limit: 6,');
    writeIn(folder, DEMO_FILE, mine);
    await app.showView('Changes');
    await app.commit('Allow six attempts');

    shop.write(DEMO_FILE, DEMO_TEXT.replace('limit: 5,', 'limit: 7,'));
    shop.git(['commit', '-q', '-am', 'Allow seven attempts']);
    await app.showView('Code');
    await app.showView('Changes');
    await app.git.getByRole('button', { name: 'Update from main' }).click();

    await expect(app.git.getByRole('alert')).toContainText(
      `Updating from main would conflict in ${DEMO_FILE}. Nothing was changed.`,
    );
    await expect(app.git.getByText('main has 1 new commit')).toBeVisible();
    expect(readIn(folder, DEMO_FILE)).toBe(mine);
    expect(shop.git(['status', '--porcelain'], folder)).toBe('');
    await app.git.getByRole('alert').getByRole('button', { name: 'Dismiss' }).click();
    await expect(app.git.getByRole('alert')).toBeHidden();
  });
});
