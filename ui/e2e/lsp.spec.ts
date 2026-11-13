import { chmodSync, mkdirSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { AGENT_BINARY } from './support/core';
import { APP_FILE, expect, readIn, test, unique, type App, type Project } from './support/fixtures';

/**
 * Language servers in the editor, on the practice language server: the
 * practice agent started with `--lsp`. It serves files that end in
 * `.practice`; it marks every line with FIXME, completes three fixed words,
 * knows the word `limits`, and formats by taking spaces off the ends of lines.
 * No real language server is started, and none is installed.
 */

const NOTES = 'notes.practice';
const SERVER_ON = 'Language server on';
const FIXME = 'FIXME: finish this before it ships';
const STOPPED = 'The Practice language server stopped. It starts again with the next file you open.';
const OUTSIDE = 'That is defined outside this task’s folder, so it is not opened here.';

/** A project with files of the practice language in its first commit. */
async function practiceProject(project: () => Promise<Project>, files: Record<string, string>): Promise<Project> {
  const shop = await project();
  for (const [file, text] of Object.entries(files)) shop.write(file, text);
  shop.git(['add', '.']);
  shop.git(['commit', '-q', '-m', 'Practice files']);
  return shop;
}

/** Starts a task in the project, opens a practice file and waits for its server. */
async function openWithServer(app: App, shop: Project, file = NOTES): Promise<string> {
  await app.openHome();
  const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
  await app.openFile(file);
  await expect(serverNote(app, SERVER_ON)).toBeVisible();
  return taskId;
}

/** The few words next to the file's language that say where it stands with its server. */
const serverNote = (app: App, text: string) => app.page.getByText(text, { exact: true });
const underlines = (app: App) => app.editor.locator('.cm-lintRange-error');
const gutterMarks = (app: App) => app.page.locator('.cm-gutter-lint .cm-lint-marker-error');
const problemsTab = (app: App) => app.bottom.getByRole('button', { name: /^Problems/ });
const problems = (app: App) => app.bottom.locator('.problem');
const completions = (app: App) => app.page.locator('.cm-tooltip-autocomplete').getByRole('option');
const openTabs = (app: App) => app.page.getByRole('group', { name: 'Open files' }).getByRole('tab');
/** A line of the code, by its number from 1. */
const line = (app: App, number: number) => app.editor.locator('.cm-line').nth(number - 1);
/** A point inside the first word of a line: lines are as wide as the editor, so their middle is empty space. */
const firstWord = { position: { x: 24, y: 9 } };

/** Presses Enter on the first completion. The editor ignores a key that comes in the instant the list appears. */
async function takeFirstCompletion(app: App) {
  await expect(completions(app).first()).toBeVisible();
  await app.page.waitForTimeout(200);
  await app.page.keyboard.press('Enter');
}

test.describe('Language servers', () => {
  test('LS-01 a problem is underlined, marked next to its line and listed in Problems; fixing it takes all three away', async ({
    app,
    project,
  }) => {
    const shop = await practiceProject(project, { [NOTES]: 'alpha\nbeta FIXME gamma\nlast\n' });
    await openWithServer(app, shop);

    await expect(underlines(app)).toHaveText('FIXME');
    await expect(gutterMarks(app)).toHaveCount(1);
    await expect(problemsTab(app)).toHaveText('Problems 1');
    await problemsTab(app).click();
    await expect(problems(app)).toHaveCount(1);
    await expect(problems(app)).toContainText(FIXME);
    await expect(problems(app)).toContainText(`${NOTES}:2`);
    await expect(problems(app)).toContainText('practice');

    // A second one appears while typing, without a save.
    await app.typeAtEndOf('last', ' FIXME too');
    await expect(underlines(app)).toHaveCount(2);
    await expect(problemsTab(app)).toHaveText('Problems 2');
    await expect(problems(app).nth(1)).toContainText(`${NOTES}:3`);
    // A press on a problem puts the caret on it.
    await problems(app).first().click();
    await expect(app.page.getByText('Line 2, column 6', { exact: true })).toBeVisible();

    // Fixing them takes the underline, the mark and the row away.
    await underlines(app).first().dblclick();
    await app.page.keyboard.type('done');
    await expect(underlines(app)).toHaveCount(1);
    await underlines(app).first().dblclick();
    await app.page.keyboard.type('done');
    await expect(underlines(app)).toHaveCount(0);
    await expect(gutterMarks(app)).toHaveCount(0);
    await expect(problemsTab(app)).toHaveText('Problems');
    await expect(problems(app)).toHaveCount(0);
    await expect(app.bottom.getByText(/^No problems are known\./)).toBeVisible();
  });

  test('LS-02 the problems of a file go away when its tab is closed, and come back with it', async ({
    app,
    project,
  }) => {
    const shop = await practiceProject(project, {
      [NOTES]: 'one FIXME\n',
      'more.practice': 'two FIXME\nthree FIXME\n',
    });
    await openWithServer(app, shop);
    await app.openFile('more.practice');
    await expect(problemsTab(app)).toHaveText('Problems 3');
    await problemsTab(app).click();
    await expect(problems(app)).toHaveCount(3);

    // A file behind another tab keeps its problems; a press on one brings the file forward.
    await problems(app)
      .filter({ hasText: `${NOTES}:1` })
      .click();
    await expect(app.tab(NOTES)).toHaveAttribute('aria-selected', 'true');
    await expect(underlines(app)).toHaveText('FIXME');

    await app.page.getByRole('button', { name: 'Close more.practice' }).click();
    await expect(problemsTab(app)).toHaveText('Problems 1');
    await expect(problems(app)).toContainText(`${NOTES}:1`);
    await app.page.getByRole('button', { name: `Close ${NOTES}` }).click();
    await expect(problemsTab(app)).toHaveText('Problems');

    await app.openFile('more.practice');
    await expect(problemsTab(app)).toHaveText('Problems 2');
    await expect(underlines(app)).toHaveCount(2);
  });

  test('LS-03 a completion of the server is offered and taken', async ({ app, project }) => {
    const shop = await practiceProject(project, { [NOTES]: 'alpha\nbeta\n' });
    await openWithServer(app, shop);

    await app.typeAtEndOf('alpha', ' pra');
    await expect(completions(app)).toHaveCount(3);
    await expect(completions(app)).toContainText(['practiceAlpha', 'practiceBeta', 'practiceGamma']);
    await expect(completions(app).first()).toContainText('practice word');
    // More letters narrow the list down without asking again.
    await app.page.keyboard.type('cticeG');
    await expect(completions(app)).toHaveCount(1);
    await takeFirstCompletion(app);
    await expect(line(app, 1)).toHaveText('alpha practiceGamma');
    await expect(completions(app)).toHaveCount(0);

    // Typing is never held up: letters that fit no completion simply stay as typed.
    await app.typeAtEndOf('beta', ' zzz');
    await expect(line(app, 2)).toHaveText('beta zzz');
    await expect(completions(app)).toHaveCount(0);
  });

  test('LS-04 the pointer over a word shows what the server knows about it', async ({ app, project }) => {
    const shop = await practiceProject(project, { [NOTES]: 'limits are set\nnothing here\n' });
    await openWithServer(app, shop);
    const hover = app.page.locator('.cm-lsp-hover');

    await line(app, 1).hover(firstWord);
    await expect(hover).toContainText('How many requests each route allows.');
    // The text is Markdown, shown as it is meant.
    await expect(hover.locator('strong')).toHaveText('limits');

    // A word the server knows nothing about shows nothing.
    await line(app, 2).hover(firstWord);
    await expect(hover).toBeHidden();
  });

  test('LS-05 "go to definition" opens the other file at the line; a place outside the task is not opened', async ({
    app,
    project,
  }) => {
    const shop = await practiceProject(project, { [NOTES]: 'limits\noutside\nplain\n' });
    await openWithServer(app, shop);

    // With the keyboard: the caret in the word, then F12.
    await line(app, 1).click(firstWord);
    await app.page.keyboard.press('F12');
    await expect(app.tab('config.ts')).toHaveAttribute('aria-selected', 'true');
    await expect(app.page.getByText('Line 1, column 14', { exact: true })).toBeVisible();
    await expect(app.editor).toContainText('export const limits');

    // With the pointer: a press on the word with the command key held.
    await app.page.getByRole('button', { name: 'Close config.ts' }).click();
    await expect(openTabs(app)).toHaveText([NOTES]);
    await line(app, 1).click({ ...firstWord, modifiers: ['ControlOrMeta'] });
    await expect(app.tab('config.ts')).toHaveAttribute('aria-selected', 'true');
    await expect(app.page.getByText('Line 1, column 14', { exact: true })).toBeVisible();

    // A definition outside the task's folder is not opened; a line in the status line says why.
    await app.tab(NOTES).click();
    await line(app, 2).click(firstWord);
    await app.page.keyboard.press('F12');
    await expect(app.page.getByText(OUTSIDE)).toBeVisible();
    await expect(openTabs(app)).toHaveText([NOTES, 'config.ts']);
    await expect(app.tab(NOTES)).toHaveAttribute('aria-selected', 'true');

    // A word with no definition: nothing happens, and the editor is as it was.
    await line(app, 3).click(firstWord);
    await app.page.keyboard.press('F12');
    await app.page.keyboard.type('x');
    await expect(line(app, 3)).toContainText('x');
    await expect(openTabs(app)).toHaveCount(2);
  });

  test('LS-06 saving formats the file; with formatting off for the project the text is left alone', async ({
    app,
    project,
  }) => {
    const shop = await practiceProject(project, { [NOTES]: 'alpha\nbeta\n' });
    const taskId = await openWithServer(app, shop);
    const folder = await app.folderOf(taskId);
    const formatting = (state: 'on' | 'off') => app.page.getByRole('button', { name: `Format on save: ${state}` });

    // The practice server formats by taking the spaces off the ends of lines.
    await app.typeAtEndOf('alpha', ' one   ');
    await app.saveWithKeyboard();
    expect(readIn(folder, NOTES)).toBe('alpha one\nbeta\n');
    await expect.poll(() => app.editor.evaluate((el) => (el as HTMLElement).innerText)).toContain('alpha one\n');
    // The caret is still where the typing ended, and the tidied line is the user's own.
    await app.page.keyboard.type('!');
    await expect(line(app, 1)).toHaveText('alpha one!');
    await app.saveWithKeyboard();
    expect(readIn(folder, NOTES)).toBe('alpha one!\nbeta\n');

    // Off, for this project: the file is saved exactly as typed.
    await expect(formatting('on')).toHaveAttribute('aria-pressed', 'true');
    await formatting('on').click();
    await expect(formatting('off')).toHaveAttribute('aria-pressed', 'false');
    await app.typeAtEndOf('beta', ' two   ');
    await app.saveWithKeyboard();
    expect(readIn(folder, NOTES)).toBe('alpha one!\nbeta two   \n');

    // The choice is the core's to keep: it is still there after the page is loaded again.
    await app.page.reload();
    await expect(app.tab(NOTES)).toHaveAttribute('aria-selected', 'true');
    await expect(formatting('off')).toBeVisible();
    await formatting('off').click();
    await expect(formatting('on')).toBeVisible();
    await app.typeAtEndOf('alpha', '  ');
    await app.saveWithKeyboard();
    expect(readIn(folder, NOTES)).toBe('alpha one!\nbeta two\n');
  });

  test('LS-07 a formatter that fails or is slow does not hold a save back: the file is saved as typed', async ({
    app,
    project,
  }) => {
    const shop = await practiceProject(project, {
      [NOTES]: 'BAD-FORMAT\nkeep\n',
      'slow.practice': 'SLOW-FORMAT\nhold\n',
    });
    const taskId = await openWithServer(app, shop);
    const folder = await app.folderOf(taskId);

    // The practice server refuses to format a file with the word BAD-FORMAT in it.
    await app.typeAtEndOf('keep', ' this   ');
    await app.saveWithKeyboard();
    expect(readIn(folder, NOTES)).toBe('BAD-FORMAT\nkeep this   \n');

    // And it takes three seconds over a file with SLOW-FORMAT. The save waits about one, then goes ahead.
    await app.openFile('slow.practice');
    await app.typeAtEndOf('hold', ' on   ');
    await app.saveWithKeyboard();
    // Had the save waited for the answer, the spaces would be gone.
    expect(readIn(folder, 'slow.practice')).toBe('SLOW-FORMAT\nhold on   \n');
    // The answer that comes late changes nothing: the text on screen stays as it was saved.
    await app.page.waitForTimeout(2500);
    await expect(app.saved).toBeVisible();
    expect(await line(app, 2).evaluate((el) => el.textContent)).toBe('hold on   ');
  });

  test('LS-08 a server that crashes is said once, the editor goes on working, and the next opened file starts it again', async ({
    app,
    project,
  }) => {
    const shop = await practiceProject(project, {
      [NOTES]: 'alpha FIXME\nbeta\n',
      'more.practice': 'gamma\n',
    });
    const taskId = await openWithServer(app, shop);
    const folder = await app.folderOf(taskId);
    await expect(problemsTab(app)).toHaveText('Problems 1');

    // The practice server ends at once when it reads this word.
    await app.typeAtEndOf('beta', ' CRASH-NOW');
    await expect(app.page.getByText(STOPPED)).toBeVisible();
    await expect(serverNote(app, SERVER_ON)).toBeHidden();
    await expect(serverNote(app, 'Language server stopped')).toBeVisible();
    // What it had found is no longer shown: nobody stands behind it.
    await expect(underlines(app)).toHaveCount(0);
    await expect(problemsTab(app)).toHaveText('Problems');

    // The editor works as it does for a file without a server: typing, saving, the marks of who wrote what.
    await app.page.keyboard.type(' still typing   ');
    await app.saveWithKeyboard();
    expect(readIn(folder, NOTES)).toBe('alpha FIXME\nbeta CRASH-NOW still typing   \n');
    await expect(app.editor.locator('.cm-line-you')).toHaveCount(1);
    // Nothing starts it again by itself, and nothing is said a second time.
    await app.page.getByRole('button', { name: 'Hide this note' }).click();
    await app.page.keyboard.type('more');
    await app.page.waitForTimeout(600);
    await expect(serverNote(app, SERVER_ON)).toBeHidden();
    await expect(app.page.getByText(STOPPED)).toBeHidden();

    // The word goes, and the next opened file starts the server again, for every open file.
    await line(app, 2).click();
    await app.page.keyboard.press('ControlOrMeta+a');
    await app.page.keyboard.type('alpha FIXME\nbeta\n');
    await app.saveWithKeyboard();
    await app.openFile('more.practice');
    await expect(serverNote(app, SERVER_ON)).toBeVisible();
    await expect(problemsTab(app)).toHaveText('Problems 1');
    await app.tab(NOTES).click();
    await expect(serverNote(app, SERVER_ON)).toBeVisible();
    await expect(underlines(app)).toHaveText('FIXME');
  });

  test('LS-09 a language whose server is not installed says so once; a file of no known language says nothing', async ({
    app,
    project,
  }) => {
    const shop = await project();
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    const folder = await app.folderOf(taskId);
    const missing = app.page.getByRole('status').filter({ hasText: 'is not installed' });

    // Markdown has no server in the table: nothing is said, nothing is added.
    await app.openFile('README.md');
    await expect(app.saved).toBeVisible();
    await expect(missing).toBeHidden();
    await expect(app.page.locator('.lsp-state')).toHaveCount(0);

    // The tests have no TypeScript server. The status line says so, with the command that would install it.
    await app.openFile(APP_FILE);
    await expect(missing).toContainText(
      'Errors and completions for TypeScript and JavaScript are off: typescript-language-server is not installed.',
    );
    await expect(missing.locator('code')).toHaveText('npm install -g typescript-language-server typescript');
    await expect(serverNote(app, 'No language server')).toBeVisible();
    // The editor is as it was before language servers: colours, typing, saving, its own completions.
    await expect(app.editor.locator('span', { hasText: /^export$/ }).first()).toBeVisible();
    await app.typeAtEndOf('const orders: Order[] = [];', '\nord');
    await expect(completions(app).filter({ hasText: /^orders$/ })).toBeVisible();
    await app.page.keyboard.press('Escape');
    await app.saveWithKeyboard();
    expect(readIn(folder, APP_FILE)).toContain('const orders: Order[] = [];\nord\n');
    await expect(app.page.locator('.cm-gutter-lint')).toHaveCount(0);

    // Said once: not for the next file of the language, and not after the page is loaded again.
    await missing.getByRole('button', { name: 'Hide this note' }).click();
    await expect(missing).toBeHidden();
    await app.openFile('src/limits/config.ts');
    await expect(app.editor).toContainText('export const limits');
    await app.page.reload();
    await expect(app.tab('config.ts')).toHaveAttribute('aria-selected', 'true');
    await expect(app.editor).toContainText('export const limits');
    await expect(missing).toBeHidden();
    // What stays is a few words next to the language, with the command for whoever points at them.
    await expect(serverNote(app, 'No language server')).toHaveAttribute(
      'title',
      /To install it: npm install -g typescript-language-server typescript$/,
    );
  });

  test('LS-15 a server that is installed while its note is on screen is found by itself, and the note goes away', async ({
    app,
    project,
  }) => {
    const shop = await project();
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    const folder = await app.folderOf(taskId);
    const missing = app.page.getByRole('status').filter({ hasText: 'is not installed' });
    await app.openFile(APP_FILE);
    await expect(missing).toBeVisible();
    await expect(serverNote(app, 'No language server')).toBeVisible();

    // The user installs it, as the note says, somewhere the editor does not see: here, into the task's own
    // packages. The practice server stands in for the real one.
    const bin = path.join(folder, 'node_modules', '.bin');
    mkdirSync(bin, { recursive: true });
    const server = path.join(bin, 'typescript-language-server');
    writeFileSync(server, `#!/bin/sh\nexec "${AGENT_BINARY}" --lsp\n`);
    chmodSync(server, 0o755);

    // Nothing is pressed: the note is gone, and the file has its server.
    await expect(missing).toBeHidden();
    await expect(serverNote(app, 'No language server')).toHaveCount(0);
    await expect(serverNote(app, SERVER_ON)).toBeVisible();
  });

  test('LS-10 leaving the task closes its files in the server, and coming back shows the problems again', async ({
    app,
    project,
  }) => {
    const shop = await practiceProject(project, { [NOTES]: 'alpha FIXME\n' });
    const taskId = await openWithServer(app, shop);
    await expect(problemsTab(app)).toHaveText('Problems 1');

    await app.goHome();
    await app.openTask(taskId);
    await expect(app.tab(NOTES)).toHaveAttribute('aria-selected', 'true');
    await expect(serverNote(app, SERVER_ON)).toBeVisible();
    await expect(underlines(app)).toHaveText('FIXME');
    await expect(problemsTab(app)).toHaveText('Problems 1');

    // Archiving the task stops its server; the core then has no server left for it.
    await app.core.request('tasks.archive', { taskId });
    await expect(
      app.core.request('lsp.send', { taskId, language: 'practice', message: { method: 'initialized', params: {} } }),
    ).rejects.toThrow('The language server is not running');
  });
});

/**
 * Asks the practice server of a task about the first word of a file, from a
 * connection of the test's own: another window, as far as the core can tell.
 * The answer is `null` when the server does not have the file open.
 */
async function hoverFromAnotherWindow(app: App, taskId: string, uri: string): Promise<unknown> {
  const socket = new WebSocket(`ws://127.0.0.1:${app.core.port}/ws`);
  const id = `test:${Date.now()}:${Math.random()}`;
  const message = {
    id,
    method: 'textDocument/hover',
    params: { textDocument: { uri }, position: { line: 0, character: 2 } },
  };
  try {
    return await new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error('The practice server did not answer')), 5000);
      socket.addEventListener('open', () =>
        socket.send(JSON.stringify({ id: 1, method: 'lsp.send', params: { taskId, language: 'practice', message } })),
      );
      socket.addEventListener('error', () => reject(new Error('The core cannot be reached')));
      socket.addEventListener('message', (received) => {
        if (typeof received.data !== 'string') return;
        const parsed = JSON.parse(received.data) as {
          event?: string;
          data?: { message?: { id?: unknown; result?: unknown } };
        };
        if (parsed.event !== 'lsp.message' || parsed.data?.message?.id !== id) return;
        clearTimeout(timer);
        resolve(parsed.data.message.result ?? null);
      });
    });
  } finally {
    socket.close();
  }
}

test.describe('Language servers and windows', () => {
  test('LS-12 a window that goes away without a word lets go of the files it had open', async ({ app, project }) => {
    const shop = await practiceProject(project, { [NOTES]: 'limits\n' });
    const taskId = await openWithServer(app, shop);
    const started = await app.core.request<{ root: string }>('lsp.start', { taskId, path: NOTES });
    const uri = `${started.root}/${NOTES}`;

    // The server is one for every window: another connection gets an answer about the file this page has open.
    expect(await hoverFromAnotherWindow(app, taskId, uri)).toMatchObject({ contents: { kind: 'markdown' } });

    // The page goes somewhere else, as a closed tab does. Nothing in it gets to say "closed".
    await app.page.goto('about:blank');
    // The core closes the file for it: the server no longer knows the file, and has nothing to say about it.
    await expect.poll(() => hoverFromAnotherWindow(app, taskId, uri)).toBeNull();
  });
});

test.describe('Language servers: the practice server in place of the TypeScript one', () => {
  test.use({ practiceLspForTs: true });

  test('LS-11 in a TypeScript file the server answers first, and the editor’s own completions are what is left', async ({
    app,
    project,
  }) => {
    const shop = await project();
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    const folder = await app.folderOf(taskId);
    await app.openFile(APP_FILE);
    await expect(serverNote(app, SERVER_ON)).toHaveAttribute(
      'title',
      'Errors and completions come from the TypeScript and JavaScript language server.',
    );
    // The colours of the language are still there: a server adds to the editor, it replaces nothing.
    await expect(app.editor.locator('span', { hasText: /^export$/ }).first()).toBeVisible();

    // What the server offers comes first.
    await app.typeAtEndOf('const orders: Order[] = [];', '\npra');
    await expect(completions(app)).toHaveCount(3);
    await expect(completions(app).first()).toContainText('practiceAlpha');
    await takeFirstCompletion(app);
    await expect(line(app, 7)).toHaveText('practiceAlpha');

    // For what the server has nothing, the completions the editor always had are offered: the words of the file.
    await app.page.keyboard.type(';\nord');
    await expect(completions(app).filter({ hasText: /^orders$/ })).toBeVisible();
    await expect(completions(app).filter({ hasText: 'practice' })).toHaveCount(0);
    await app.page.keyboard.press('Escape');

    // A problem in a real kind of file, and the way to a definition in another one.
    await app.page.keyboard.type(' // FIXME');
    await expect(underlines(app)).toHaveText('FIXME');
    await expect(problemsTab(app)).toHaveText('Problems 1');
    await app.saveWithKeyboard();
    expect(readIn(folder, APP_FILE)).toContain('practiceAlpha;\nord // FIXME\n');

    // The caret goes into the word `limits` of the second line: `import { limits } from …`.
    await line(app, 2).click();
    await app.page.keyboard.press('Home');
    for (let step = 0; step < 10; step++) await app.page.keyboard.press('ArrowRight');
    await app.page.keyboard.press('F12');
    await expect(app.tab('config.ts')).toHaveAttribute('aria-selected', 'true');
    await expect(app.page.getByText('Line 1, column 14', { exact: true })).toBeVisible();
  });
});
