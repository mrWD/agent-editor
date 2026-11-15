import { expect, test, unique } from './support/fixtures';

/**
 * How a conversation is read. The practice agent, asked to "look around", takes a row of steps
 * the way a real agent does: it reads a file, runs a command of three lines, searches, says a
 * word, runs two more commands of which the last is not done, and answers.
 */
test.describe('The conversation as it is read', () => {
  test('CV-01 CV-02 a run of steps is one line that opens, a long command is one line that opens, and the answer stays open', async ({
    app,
    project,
  }) => {
    await app.openHome();
    await app.createTask({ text: unique('Look around the limiter'), folder: (await project()).dir });

    // Three steps in a row are one line that says what they were. What the agent thought is not shown.
    const steps = app.fold('Ran 1 command, read 1 file, searched 1 time');
    await expect(steps).toHaveCount(1);
    await expect(steps.locator('.fold-head')).toHaveAttribute('aria-expanded', 'false');
    await expect(app.thoughts).toHaveCount(0);
    // Two steps stand as they are, and one that was not done says so.
    await expect(app.activities).toHaveText([/^Pr\s*ran\s*npm test$/, /^Pr\s*tried to run\s*npm run lint\s*not done$/]);
    // What the agent said along the way and its answer are both there, under its name once.
    await expect(app.conversation.getByRole('article')).toHaveCount(2);
    await expect(app.conversation.locator('.said-head')).toHaveCount(1);
    await expect(app.answers.first()).toHaveText('Two places set a limit.');

    // Pressed, the line opens on every step of the run, the thought among them.
    await steps.locator('.fold-head').click();
    await expect(steps.locator('.fold-head')).toHaveAttribute('aria-expanded', 'true');
    await expect(steps.locator('.thought')).toHaveCount(1);
    await expect(steps.locator('.activity')).toHaveCount(3);
    // A command of three lines is its first line until it is pressed.
    const command = steps.getByRole('button', { name: /^cd src &&/ });
    await expect(command).toHaveText('cd src && …');
    await command.click();
    await expect(command).toHaveText('cd src &&\n  grep -rn "limit" . |\n  sort');
    await command.click();
    await expect(command).toHaveText('cd src && …');
    // A file that was read still opens from its step.
    await steps.getByRole('button', { name: 'src/middleware/rateLimit.ts' }).click();
    await expect(app.page.getByRole('tab', { name: /rateLimit\.ts/ })).toBeVisible();

    // What was opened stays open while the task is left and come back to; pressed again, it closes.
    await app.showView('Changes');
    await app.showView('Code');
    await expect(steps.locator('.activity')).toHaveCount(3);
    await steps.locator('.fold-head').click();
    await expect(steps.locator('.activity')).toHaveCount(0);
  });

  test('CV-03 a turn that was answered since is folded down to what was asked and the last word', async ({
    app,
    project,
  }) => {
    await app.openHome();
    const first = unique('Look around the limiter');
    await app.createTask({ text: first, folder: (await project()).dir });
    await expect(app.conversation.getByRole('article')).toHaveCount(2);
    await app.messageBox.fill('And what is the limit?');
    await app.sendButton.click();
    await app.turnEnds();

    // Of the first turn: the message, one line for all the work, and the answer.
    const work = app.fold(/^Worked( for \d+\w*( \d+\w*)?)? · 5 steps, 1 message$/);
    await expect(work).toHaveCount(1);
    await expect(work.locator('.fold-head')).toContainText('1 not done');
    await expect(app.messages).toHaveText([first, 'And what is the limit?']);
    // The word in the middle of the first turn went into the fold; its answer and the new one are open.
    await expect(app.conversation.getByText('Two places set a limit.')).toHaveCount(0);
    await expect(app.conversation.getByRole('article')).toHaveCount(2);
    await expect(app.conversation.locator('.said-head')).toHaveCount(2);

    // The fold holds everything of that turn, in the order it happened.
    await work.locator('.fold-head').click();
    await expect(work.getByText('Two places set a limit.')).toBeVisible();
    await expect(work.locator('.activity')).toHaveCount(5);
    await expect(work.getByRole('heading', { name: 'Plan for this turn' })).toBeVisible();
  });

  test('CV-04 "All steps" shows everything one under another, and the choice is kept', async ({ app, project }) => {
    await app.openHome();
    await app.createTask({ text: unique('Look around the limiter'), folder: (await project()).dir });
    await expect(app.conversation.locator('.fold')).toHaveCount(1);

    await app.allSteps();
    await expect(app.conversation.locator('.fold')).toHaveCount(0);
    await expect(app.activities).toHaveCount(5);
    await expect(app.thoughts).toHaveCount(1);
    // A long command is one line here too.
    await expect(app.conversation.getByRole('button', { name: /^cd src &&/ })).toHaveText('cd src && …');

    // Kept in the browser: after a reload the conversation is still unfolded.
    await app.page.reload();
    await expect(app.conversation).toBeVisible();
    await expect(app.page.getByRole('button', { name: 'All steps', exact: true })).toHaveAttribute(
      'aria-pressed',
      'true',
    );
    await expect(app.activities).toHaveCount(5);
    await app.page.getByRole('button', { name: 'All steps', exact: true }).click();
    await expect(app.conversation.locator('.fold')).toHaveCount(1);
  });
});

test.describe('The conversation while the agent works', () => {
  // Slow enough to see a turn half way through.
  test.use({ mockDelayMs: 300 });

  test('CV-05 while the agent works, the fold says what it is at', async ({ app, project }) => {
    test.slow();
    await app.openHome();
    await app.createTask({ text: unique('Look around the limiter'), folder: (await project()).dir, until: 'started' });
    // The run is three steps long for a moment before the agent says its word: the line under it names the newest.
    await expect(app.conversation.locator('.fold-now')).toHaveText(/^(is searching|searched) Search for windowMs$/);
    await app.turnEnds();
    await expect(app.conversation.locator('.fold-now')).toHaveCount(0);
  });
});
