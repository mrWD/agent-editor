import { existsSync } from 'node:fs';
import path from 'node:path';
import {
  AGENT_LINE,
  APP_FILE,
  DEMO_FILE,
  DEMO_TEXT,
  expect,
  readIn,
  test,
  unique,
  writeIn,
  type App,
} from './support/fixtures';

const YOUR_BADGE = 'Lines you wrote start here';
const AGENT_BADGE = 'Lines the agent wrote start here';
const REVIEW_BANNER = 'Practice agent wrote the highlighted lines. You have not reviewed them yet.';

// The editor hides its gutter from screen readers, so the badges in it are found among the hidden elements.
// One more badge is always there and never seen: the editor measures the gutter's width with it.
const badge = (app: App, name: string) =>
  app.page.getByRole('img', { name, includeHidden: true }).filter({ visible: true });
const yourBadge = (app: App) => badge(app, YOUR_BADGE);
const agentBadge = (app: App) => badge(app, AGENT_BADGE);

test.describe('Files and the editor', () => {
  test('G-01 G-02 G-11 the tree lists folders first and hides .git; a file opens in a tab with its language', async ({
    app,
    project,
  }) => {
    const shop = await project();
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    // A working copy has a `.git` of its own; it is not shown.
    expect(existsSync(path.join(await app.folderOf(taskId), '.git'))).toBe(true);

    const rows = app.files.locator('.tree-row');
    await expect(rows).toHaveText(['src', '.gitignore', 'compose.yaml', 'logo.bin', 'package.json', 'README.md']);
    await app.files.getByRole('button', { name: 'src', exact: true }).click();
    await expect(rows).toHaveText([
      'src',
      'limits',
      'middleware',
      'app.ts',
      '.gitignore',
      'compose.yaml',
      'logo.bin',
      'package.json',
      'README.md',
    ]);
    await expect(app.page.getByRole('heading', { name: 'Pick a file on the left' })).toBeVisible();

    await app.openFile('README.md');
    await expect(app.editor).toContainText('A small shop API for the end-to-end tests.');
    const openFiles = app.page.getByRole('group', { name: 'Open files' });
    await expect(openFiles).toContainText('Markdown');
    await app.openFile(APP_FILE);
    await expect(openFiles).toContainText('TypeScript');
    await expect(openFiles.getByRole('tab')).toHaveText(['README.md', 'app.ts']);
    await expect(app.files.getByRole('button', { name: /^app\.ts/ })).toHaveAttribute('aria-current', 'true');
    // The words of the language are coloured: each stands in an element of its own.
    await expect(app.editor.locator('span', { hasText: /^export$/ }).first()).toBeVisible();
    await expect(app.page.getByText('Line 1, column 1')).toBeVisible();

    // A tab brings its file back.
    await app.tab('README.md').click();
    await expect(app.editor).toContainText('# Shop');
    await expect(openFiles).toContainText('Markdown');

    await app.files.getByRole('button', { name: /^logo\.bin/ }).click();
    await expect(app.page.getByRole('heading', { name: 'This file cannot be shown' })).toBeVisible();
  });

  test('G-03 G-04 H-02 typing marks the line as yours and the file as not saved; the keyboard saves', async ({
    app,
    project,
  }) => {
    const shop = await project();
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    const folder = await app.folderOf(taskId);
    await app.openFile(DEMO_FILE);
    await expect(app.saved).toBeVisible();
    await expect(yourBadge(app)).toHaveCount(0);

    await app.typeAtEndOf('windowMs', ' // one minute');
    const views = app.page.getByRole('navigation', { name: 'Views of this task' });
    await expect(yourBadge(app)).toHaveText('You');
    await expect(app.editor.locator('.cm-line-you')).toHaveText('  windowMs: 60 * 1000, // one minute');
    await expect(app.tab('rateLimit.ts').getByRole('img', { name: 'not saved' })).toBeVisible();
    await expect(views.getByRole('link', { name: /^Code/ }).getByTitle('files with unsaved edits')).toHaveText('1');
    await expect(app.notSaved).toBeVisible();
    await expect(app.page.getByText('Line 2, column 37')).toBeVisible();
    expect(readIn(folder, DEMO_FILE)).toBe(DEMO_TEXT);

    await app.saveWithKeyboard();
    await expect(app.tab('rateLimit.ts').getByRole('img', { name: 'not saved' })).toBeHidden();
    await expect(views.getByRole('link', { name: /^Code/ }).getByTitle('files with unsaved edits')).toBeHidden();
    expect(readIn(folder, DEMO_FILE)).toBe(DEMO_TEXT.replace('1000,', '1000, // one minute'));
    // The line stays marked as yours, in the editor and in the tree, and the conversation notes the edit.
    await expect(yourBadge(app)).toBeVisible();
    await expect(
      app.files.getByRole('button', { name: /^rateLimit\.ts/ }).getByRole('img', { name: 'Changed by you' }),
    ).toBeVisible();
    await expect(app.activities.filter({ hasText: 'edited' })).toContainText(DEMO_FILE);
  });

  test('G-05 closing a tab saves what was typed', async ({ app, project }) => {
    const shop = await project();
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    const folder = await app.folderOf(taskId);
    await app.openFile(DEMO_FILE);
    await app.typeAtEndOf('windowMs', ' // kept on close');
    await expect(app.notSaved).toBeVisible();

    await app.page.getByRole('button', { name: 'Close rateLimit.ts' }).click();
    await expect(app.tab('rateLimit.ts')).toBeHidden();
    await expect(app.page.getByRole('heading', { name: 'Pick a file on the left' })).toBeVisible();
    await expect.poll(() => readIn(folder, DEMO_FILE)).toContain('1000, // kept on close');
  });

  test('G-06 leaving the task saves what was typed', async ({ app, project }) => {
    const shop = await project();
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    const folder = await app.folderOf(taskId);
    await app.openFile(DEMO_FILE);
    await app.typeAtEndOf('windowMs', ' // kept on leaving');
    await expect(app.notSaved).toBeVisible();

    await app.goHome();
    await expect.poll(() => readIn(folder, DEMO_FILE)).toContain('1000, // kept on leaving');
  });

  test('G-07 the same files are open after the page is loaded again', async ({ app, project }) => {
    const shop = await project();
    await app.openHome();
    await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    await app.openFile('README.md');
    await app.openFile(DEMO_FILE);
    await app.openFile(APP_FILE);
    await app.tab('rateLimit.ts').click();
    await expect(app.editor).toContainText('windowMs');

    await app.page.reload();
    const tabs = app.page.getByRole('group', { name: 'Open files' }).getByRole('tab');
    await expect(tabs).toHaveText(['README.md', 'rateLimit.ts', 'app.ts']);
    await expect(app.tab('rateLimit.ts')).toHaveAttribute('aria-selected', 'true');
    await expect(app.editor).toContainText('windowMs');
  });

  test('G-08 a file that changes on disk is shown anew when it has no edits here', async ({ app, project }) => {
    const shop = await project();
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    const folder = await app.folderOf(taskId);
    await app.openFile(DEMO_FILE);
    await expect(app.editor).toContainText('limit: 5,');

    writeIn(folder, DEMO_FILE, DEMO_TEXT.replace('limit: 5,', 'limit: 50,'));
    await app.comeBackToWindow();
    await expect(app.editor).toContainText('limit: 50,');
    await expect(app.saved).toBeVisible();
    await expect(app.page.getByRole('alert')).toBeHidden();
  });

  test('G-09 a file that changes on disk under unsaved edits asks whose version to keep: the one on disk', async ({
    app,
    project,
  }) => {
    const shop = await project();
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    const folder = await app.folderOf(taskId);
    await app.openFile(DEMO_FILE);
    await app.typeAtEndOf('windowMs', ' // mine');
    const outside = DEMO_TEXT.replace('limit: 5,', 'limit: 50,');
    writeIn(folder, DEMO_FILE, outside);
    await app.comeBackToWindow();

    const banner = app.page.getByRole('alert').filter({ hasText: 'changed on disk' });
    await expect(banner).toContainText('rateLimit.ts changed on disk while you were editing it.');
    await expect(app.editor).toContainText('1000, // mine');
    await expect(app.editor).toContainText('limit: 5,');

    await banner.getByRole('button', { name: 'Load the version on disk' }).click();
    await expect(banner).toBeHidden();
    await expect(app.editor).toContainText('limit: 50,');
    await expect(app.editor).not.toContainText('// mine');
    await expect(app.saved).toBeVisible();
    expect(readIn(folder, DEMO_FILE)).toBe(outside);
  });

  test('G-09 a save over a file that changed on disk is refused until "Keep mine" is pressed', async ({
    app,
    project,
  }) => {
    const shop = await project();
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    const folder = await app.folderOf(taskId);
    await app.openFile(DEMO_FILE);
    await app.typeAtEndOf('windowMs', ' // mine');
    const outside = DEMO_TEXT.replace('limit: 5,', 'limit: 50,');
    writeIn(folder, DEMO_FILE, outside);

    // The save is refused: the other program's text stays on disk, and the question is asked.
    await app.page.keyboard.press('ControlOrMeta+s');
    const banner = app.page.getByRole('alert').filter({ hasText: 'changed on disk' });
    await expect(banner).toBeVisible();
    await expect(app.notSaved).toBeVisible();
    expect(readIn(folder, DEMO_FILE)).toBe(outside);

    await banner.getByRole('button', { name: 'Keep mine' }).click();
    await expect(banner).toBeHidden();
    // The button took the keyboard away from the code; a click gives it back.
    await app.editor.getByText('loginLimiter').click();
    await app.saveWithKeyboard();
    expect(readIn(folder, DEMO_FILE)).toBe(DEMO_TEXT.replace('1000,', '1000, // mine'));
  });

  test('G-12 the search key of the editor opens a search inside the file', async ({ app, project }) => {
    const shop = await project();
    await app.openHome();
    await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    await app.openFile(DEMO_FILE);
    await app.editor.getByText('loginLimiter').click();
    await app.page.keyboard.press('ControlOrMeta+f');

    const find = app.page.getByRole('textbox', { name: 'Find' });
    await expect(find).toBeFocused();
    await find.pressSequentially('limit:');
    await find.press('Enter');
    await expect(app.page.getByText(/^Line 3, column \d+$/)).toBeVisible();
    await expect(app.editor.locator('.cm-searchMatch')).toHaveText('limit:');
    await find.press('Escape');
    await expect(find).toBeHidden();
  });
});

test.describe('Who wrote which line, Keep and Undo', () => {
  test('H-01 H-04 H-09 the lines of the agent are marked until Keep; the tree and "Changed" show the file', async ({
    app,
    project,
  }) => {
    const shop = await project();
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    const folder = await app.folderOf(taskId);
    await app.letAgentEdit('Change the limit');

    // The tree and the list of changed files name who changed the file.
    await app.files.getByRole('button', { name: 'src', exact: true }).click();
    await app.files.getByRole('button', { name: 'middleware', exact: true }).click();
    const inTree = app.files.getByRole('button', { name: /^rateLimit\.ts/ });
    await expect(inTree.getByRole('img', { name: 'Changed by Practice agent' })).toBeVisible();
    const changed = app.files.getByRole('button', { name: /^Changed/ });
    await expect(changed).toHaveText('Changed 1');
    await changed.click();
    const listed = app.files.getByRole('button', { name: new RegExp(`^${DEMO_FILE}`) });
    await expect(listed.getByRole('img', { name: 'Changed by Practice agent' })).toBeVisible();

    await listed.click();
    const banner = app.page.locator('.banner.review');
    await expect(banner).toContainText(REVIEW_BANNER);
    await expect(agentBadge(app)).toHaveText('Pr');
    await expect(app.editor.locator('.cm-line-agent')).toHaveText(AGENT_LINE);

    await banner.getByRole('button', { name: 'Keep' }).click();
    await expect(banner).toBeHidden();
    await expect(agentBadge(app)).toHaveCount(0);
    await expect(app.editor.locator('.cm-line-agent')).toHaveCount(0);
    await expect(changed).toHaveText('Changed');
    await expect(app.files.getByText('Nothing has changed since your last review.')).toBeVisible();
    // Keeping accepts the lines; it does not change the file.
    expect(readIn(folder, DEMO_FILE)).toContain(AGENT_LINE);

    // The marks are gone for good.
    await app.page.reload();
    await expect(app.editor).toContainText(AGENT_LINE);
    await expect(banner).toBeHidden();
    await expect(agentBadge(app)).toHaveCount(0);
  });

  test('H-05 H-06 Undo takes back the lines of the agent and leaves your own line next to them', async ({
    app,
    project,
  }) => {
    const shop = await project();
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    const folder = await app.folderOf(taskId);
    await app.letAgentEdit('Change the limit');
    await app.openFile(DEMO_FILE);
    const banner = app.page.locator('.banner.review');
    await expect(banner).toBeVisible();

    // A line of your own right above the agent's line.
    await app.typeAtEndOf('windowMs', ' // mine');
    const undo = banner.getByRole('button', { name: 'Undo' });
    await expect(undo).toBeDisabled();
    await expect(undo).toHaveAttribute('title', 'Save your own edits first');
    await app.saveWithKeyboard();
    await expect(app.editor.locator('.cm-line-you')).toHaveText('  windowMs: 60 * 1000, // mine');
    await expect(app.editor.locator('.cm-line-agent')).toHaveText(AGENT_LINE);
    await expect(yourBadge(app)).toBeVisible();
    await expect(agentBadge(app)).toBeVisible();

    await undo.click();
    await expect(banner).toBeHidden();
    await expect(app.editor).toContainText('limit: 5,');
    await expect(app.editor).toContainText('1000, // mine');
    await expect(app.editor.locator('.cm-line-agent')).toHaveCount(0);
    await expect(app.editor.locator('.cm-line-you')).toHaveText('  windowMs: 60 * 1000, // mine');
    expect(readIn(folder, DEMO_FILE)).toBe(DEMO_TEXT.replace('1000,', '1000, // mine'));
  });

  test('H-07 Undo of a file the agent made removes the file', async ({ app, project }) => {
    const shop = await project({ withoutDemoFile: true });
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the project'), folder: shop.dir });
    const folder = await app.folderOf(taskId);
    await app.letAgentEdit('Write the notes');
    await expect(app.answers.last()).toHaveText('Done. I changed NOTES.md. One file changed.');
    expect(readIn(folder, 'NOTES.md')).toBe('Notes from the practice agent.\n');

    // The new file shows up in the tree by itself.
    await app.files.getByRole('button', { name: /^NOTES\.md/ }).click();
    await expect(app.editor).toContainText('Notes from the practice agent.');
    const banner = app.page.locator('.banner.review');
    await banner.getByRole('button', { name: 'Undo' }).click();
    await expect(app.tab('NOTES.md')).toBeHidden();
    await expect(app.files.getByRole('button', { name: /^NOTES\.md/ })).toBeHidden();
    expect(existsSync(path.join(folder, 'NOTES.md'))).toBe(false);
    await expect(app.files.getByRole('button', { name: /^Changed/ })).toHaveText('Changed');
  });

  test('H-10 committing the whole file takes its marks away', async ({ app, project }) => {
    const shop = await project();
    await app.openHome();
    await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    await app.letAgentEdit('Change the limit');
    await app.openFile(DEMO_FILE);
    const banner = app.page.locator('.banner.review');
    await expect(banner).toBeVisible();

    await app.showView('Changes');
    await app.commit('Take the limit from config');
    await app.showView('Code');
    await expect(banner).toBeHidden();
    await expect(app.editor.locator('.cm-line-agent')).toHaveCount(0);
    await expect(app.files.getByRole('button', { name: /^Changed/ })).toHaveText('Changed');
  });

  test('H-11 in a folder without git the first edit of the agent is not marked', async ({ app, project }) => {
    const plain = await project({ name: 'plain', git: false });
    await app.openHome();
    await app.createTask({ text: unique('Describe the limiter'), folder: plain.dir });
    await app.letAgentEdit('Change the limit');
    await app.openFile(DEMO_FILE);
    await expect(app.editor).toContainText(AGENT_LINE);
    // A known limit: without git there is nothing to compare the first edit with.
    await expect(app.page.locator('.banner.review')).toBeHidden();
    await expect(app.files.getByRole('button', { name: /^Changed/ })).toHaveText('Changed');
  });
});
