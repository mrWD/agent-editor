import { DEMO_FILE, DEMO_TEXT, expect, test, unique, writeIn, type App } from './support/fixtures';

/** A lint that passes and tests that fail, naming a line of the demo file. */
const SCRIPTS = {
  lint: "echo 'no problems found'",
  test: `printf '${DEMO_FILE}(3,10): error TS2322: Type mismatch.\\n'; echo 'Tests: 1 failed, 2 passed'; exit 1`,
};

const doneWhen = (app: App) => app.page.getByRole('region', { name: 'Done when' });
const row = (app: App, title: string) => doneWhen(app).locator('.check-row', { hasText: title });
const checksLink = (app: App) =>
  app.page.getByRole('navigation', { name: 'Views of this task' }).getByRole('link', { name: /^Checks/ });

test.describe('The checks of a task', () => {
  test("CK-01 CK-02 the project's own checks are listed, run, and what they name opens at the line", async ({
    app,
    project,
  }) => {
    const shop = await project({ scripts: SCRIPTS });
    await app.openHome();
    await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    await app.showView('Checks');

    await expect(doneWhen(app)).toContainText('0 of 2');
    await expect(row(app, 'Lint is clean')).toContainText('not run yet');
    await expect(row(app, 'Unit tests pass')).toContainText('not run yet');
    await expect(app.page.locator('.check-command code')).toHaveText('npm run lint');
    await expect(app.page.getByText('from package.json')).toBeVisible();

    await doneWhen(app).getByRole('button', { name: 'Run all', exact: true }).click();
    await expect(row(app, 'Lint is clean')).toContainText(/passed in \d/);
    await expect(row(app, 'Unit tests pass')).toContainText('failed, 1 problem');
    await expect(doneWhen(app)).toContainText('1 of 2');
    await expect(checksLink(app)).toContainText('1 of 2');
    await expect(doneWhen(app).getByRole('button', { name: 'Run all again' })).toBeVisible();

    // The failed one is what the middle shows: what it printed, and what it names.
    await row(app, 'Unit tests pass').click();
    await expect(app.page.getByLabel('Output of Unit tests pass')).toContainText('Tests: 1 failed, 2 passed');
    const named = app.page.locator('.center .problem');
    await expect(named).toContainText('error TS2322: Type mismatch.');
    await expect(named).toContainText(`${DEMO_FILE}:3`);

    // The same problem is on the Problems tab under the code, and opens the file at the line.
    await named.click();
    await expect(app.tab('rateLimit.ts')).toHaveAttribute('aria-selected', 'true');
    await expect(app.page.getByText('Line 3, column 10')).toBeVisible();
    await app.bottom.getByRole('button', { name: 'Problems 1' }).click();
    await expect(app.bottom.getByLabel('Problems').getByRole('button')).toContainText('Unit tests pass');
  });

  test('CK-03 CK-04 a failed check goes to the agent with what it printed; a change to the files makes verdicts old', async ({
    app,
    core,
    project,
  }) => {
    const shop = await project({ scripts: SCRIPTS });
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    await app.showView('Checks');
    await doneWhen(app).getByRole('button', { name: 'Run all', exact: true }).click();
    await expect(row(app, 'Unit tests pass')).toContainText('failed, 1 problem');

    await row(app, 'Unit tests pass').click();
    await app.page.getByRole('button', { name: 'Send to Practice agent' }).click();
    await expect(app.messages.last()).toContainText(
      'The check “Unit tests pass” (`npm run test`) failed with exit code 1.',
    );
    await expect(app.messages.last()).toContainText('Tests: 1 failed, 2 passed');
    await expect(app.question).toBeVisible();
    await app.answer('Allow this edit');
    await app.turnEnds();

    // The agent changed a file: what was found out before says nothing about the files now.
    await expect(row(app, 'Lint is clean')).toContainText('passed before the last change');
    await expect(row(app, 'Unit tests pass')).toContainText('failed before the last change');
    await expect(doneWhen(app)).toContainText('0 of 2');
    await expect(app.page.getByText('The files changed after this run.')).toBeVisible();
    // And the Problems tab no longer claims the old problem.
    await app.showView('Code');
    await expect(app.bottom.getByRole('button', { name: 'Problems', exact: true })).toBeVisible();

    await app.showView('Checks');
    await row(app, 'Lint is clean').click();
    await app.page.getByRole('button', { name: 'Run again' }).click();
    await expect(row(app, 'Lint is clean')).toContainText(/passed in \d/);
    expect((await core.request<{ done: number }>('checks.state', { taskId })).done).toBe(1);
  });

  test("CK-05 CK-06 lines for the user to confirm, commands of the user's own, and turning a check off", async ({
    app,
    project,
  }) => {
    const shop = await project({ scripts: SCRIPTS });
    await app.openHome();
    await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    await app.showView('Checks');
    const box = doneWhen(app).getByRole('textbox', { name: 'Add a line' });

    // A line only a person can confirm.
    await box.fill('The sixth attempt in a minute gets 429');
    await doneWhen(app).getByRole('button', { name: 'Add as a line I confirm' }).click();
    await expect(row(app, 'The sixth attempt')).toContainText('waiting for your OK');
    await expect(doneWhen(app)).toContainText('0 of 3');
    await row(app, 'The sixth attempt').click();
    await app.page.getByRole('button', { name: 'Not yet' }).click();
    await app.page.getByRole('textbox', { name: /^What is missing\?/ }).fill('The answer has no Retry-After header.');
    await app.page.getByRole('button', { name: 'Send to Practice agent' }).click();
    await expect(app.messages.last()).toContainText('This is not done yet: “The sixth attempt in a minute gets 429”.');
    await expect(app.messages.last()).toContainText('The answer has no Retry-After header.');
    await app.turnEnds();
    await app.page.getByRole('button', { name: 'Yes, this is done' }).click();
    await expect(row(app, 'The sixth attempt')).toContainText('you said it is done');
    await expect(doneWhen(app)).toContainText('1 of 3');
    await app.page.getByRole('button', { name: 'Not done after all' }).click();
    await expect(doneWhen(app)).toContainText('0 of 3');

    // A command of the user's own runs like the project's.
    await box.fill('test -f README.md');
    await doneWhen(app).getByRole('button', { name: 'Add as a command to run' }).click();
    await row(app, 'test -f README.md').click();
    await expect(app.page.getByText('added by you')).toBeVisible();
    await app.page.getByRole('button', { name: 'Run', exact: true }).click();
    await expect(row(app, 'test -f README.md')).toContainText(/passed in \d/);
    await expect(doneWhen(app)).toContainText('1 of 4');
    await app.page.locator('.check-command').getByRole('button', { name: 'Remove' }).click();
    await expect(row(app, 'test -f README.md')).toHaveCount(0);

    // A check that is off is not run and does not count.
    await row(app, 'Unit tests pass').click();
    await app.page.getByRole('button', { name: 'Turn off for this project' }).click();
    await expect(row(app, 'Unit tests pass')).toContainText('off for this project');
    await expect(doneWhen(app)).toContainText('0 of 2');
    await doneWhen(app)
      .getByRole('button', { name: /^Run all/ })
      .click();
    await expect(row(app, 'Lint is clean')).toContainText(/passed in \d/);
    await expect(row(app, 'Unit tests pass')).toContainText('off for this project');
    await expect(doneWhen(app)).toContainText('1 of 2');
  });

  test('CK-07 a failed check stands in the way of "Merge now" until it passes or is turned off', async ({
    app,
    project,
  }) => {
    const shop = await project({ remote: 'github', scripts: SCRIPTS });
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    const task = await app.core.task(taskId);
    writeIn(task.cwd, DEMO_FILE, DEMO_TEXT.replace('limit: 5,', 'limit: 6,'));
    await app.showView('Changes');
    await app.git.getByRole('radio', { name: 'Push and open a pull request' }).check();
    // The choice holds only once the core has said where the project lives; until then it falls back.
    await expect(app.git.getByRole('button', { name: /^Commit 1 file/ })).toHaveText(
      'Commit 1 file, push and open a pull request',
    );
    await app.git.getByRole('button', { name: /^Commit 1 file/ }).click();
    await expect(app.request).toContainText(/Number \d+ · open/);
    const merge = app.request.getByRole('button', { name: 'Merge now' });
    await expect(merge).toBeEnabled();

    await app.showView('Checks');
    await doneWhen(app).getByRole('button', { name: 'Run all', exact: true }).click();
    await expect(row(app, 'Unit tests pass')).toContainText('failed, 1 problem');
    await expect(checksLink(app).locator('.count')).toHaveClass(/bad/);
    await app.showView('Changes');
    await expect(app.request.locator('.gate', { hasText: 'Checks here' })).toContainText(
      '1 failed on this computer. See the Checks view.',
    );
    await expect(merge).toBeDisabled();

    await app.showView('Checks');
    await row(app, 'Unit tests pass').click();
    await app.page.getByRole('button', { name: 'Turn off for this project' }).click();
    await app.showView('Changes');
    await expect(app.request.locator('.gate', { hasText: 'Checks here' })).toContainText('1 of 1 lines');
    await expect(merge).toBeEnabled();
  });

  test('CK-08 a project without checks says so, and a running check can be stopped', async ({ app, project }) => {
    const plain = await project({});
    await app.openHome();
    await app.createTask({ text: unique('Describe the limiter'), folder: plain.dir });
    await app.showView('Checks');
    await expect(doneWhen(app)).toContainText('No checks were found in this project');
    await expect(doneWhen(app).getByRole('button', { name: 'Run all', exact: true })).toBeDisabled();

    const box = doneWhen(app).getByRole('textbox', { name: 'Add a line' });
    await box.fill('sleep 60');
    await doneWhen(app).getByRole('button', { name: 'Add as a command to run' }).click();
    await row(app, 'sleep 60').click();
    await app.page.getByRole('button', { name: 'Run', exact: true }).click();
    await expect(row(app, 'sleep 60')).toContainText('running');
    await app.page.getByRole('button', { name: 'Stop', exact: true }).click();
    await expect(row(app, 'sleep 60')).toContainText('not run yet');
  });
});
