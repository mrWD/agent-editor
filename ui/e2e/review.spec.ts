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

/** A task with one change that is not committed yet, on its Changes view. */
async function taskWithAChange(app: App, shop: Project, title: string, change = 'limit: 6,') {
  if (!app.page.url().endsWith('#/')) await app.openHome();
  const known = await app.page.getByLabel('Project', { exact: true }).getByRole('option', { name: shop.name }).count();
  const taskId = await app.createTask(known ? { text: title, project: shop.name } : { text: title, folder: shop.dir });
  const task = await app.core.task(taskId);
  writeIn(task.cwd, DEMO_FILE, DEMO_TEXT.replace('limit: 5,', change));
  await app.showView('Changes');
  await expect(commitButton(app)).toBeVisible();
  return task;
}

async function openRequest(app: App, message: string) {
  await app.git.getByRole('radio', { name: 'Push and open a pull request' }).check();
  await app.git.getByRole('textbox', { name: 'Commit message' }).fill(message);
  // The choice holds only once the core has said where the project lives; until then it falls back.
  await expect(commitButton(app)).toHaveText('Commit 1 file, push and open a pull request');
  await commitButton(app).click();
  await expect(app.request).toContainText(/Number \d+ · open/);
}

test.describe('Review comments on a merge request', () => {
  test('RV-01 RV-02 RV-03 comments are listed; one or all go to the agent; a reply goes to the host', async ({
    app,
    core,
    project,
  }) => {
    const shop = await project({ remote: 'github' });
    shop.setReviewThreads(128, [
      { id: 55, author: 'anna', path: DEMO_FILE, line: 3, body: 'Should the limit come from config, not a fixed 6?' },
      { id: 56, author: 'boris', path: 'src/app.ts', line: 1, body: 'Please put the limiter on /register as well.' },
      { id: 57, author: 'anna', path: 'README.md', line: 1, body: 'Settled long ago.', resolved: true },
    ]);
    await taskWithAChange(app, shop, unique('Describe the limiter'));
    await openRequest(app, 'Raise the limit');

    const comments = app.request.locator('.review-comment');
    await expect(app.request.getByRole('heading', { name: 'Review comments' })).toBeVisible();
    await expect(comments).toHaveCount(2);
    await expect(comments.first()).toContainText(`${DEMO_FILE}, line 3`);
    await expect(comments.first()).toContainText('anna');
    await expect(comments.first()).toContainText('Should the limit come from config, not a fixed 6?');
    await expect(app.request.getByRole('button', { name: 'Let Practice agent address both' })).toBeVisible();

    // "Open in code" goes to the file and the line.
    await comments.first().getByRole('button', { name: 'Open in code' }).click();
    await expect(app.tab('rateLimit.ts')).toHaveAttribute('aria-selected', 'true');
    await expect(app.page.getByText('Line 3, column 1')).toBeVisible();
    await app.showView('Changes');

    // A reply is sent to the host, in the comment's own thread.
    await comments.nth(1).getByRole('button', { name: 'Reply' }).click();
    await app.request.getByRole('textbox', { name: 'Reply to boris about src/app.ts, line 1' }).fill('Good point.');
    await app.request.getByRole('button', { name: 'Send the reply on GitHub' }).click();
    await expect(app.request.getByRole('textbox', { name: /^Reply to boris/ })).toBeHidden();
    expect(core.calls('gh').some((call) => /pulls\/128\/comments\/56\/replies -f body=Good point\./.test(call))).toBe(
      true,
    );

    // One comment goes to the agent as a message that quotes it.
    await comments.first().getByRole('button', { name: 'Ask Practice agent to fix' }).click();
    await expect(app.messages.last()).toContainText('A reviewer left a comment on the pull request.');
    await expect(app.messages.last()).toContainText(`anna wrote about \`${DEMO_FILE}\` line 3:`);
    await expect(app.messages.last()).toContainText('> Should the limit come from config, not a fixed 6?');
    await expect(app.question).toBeVisible();
    // While the agent has the turn, nothing more can be asked of it.
    await expect(comments.nth(1).getByRole('button', { name: 'Ask Practice agent to fix' })).toBeDisabled();
    await app.answer('Do not edit');
    await app.turnEnds();

    // Both at once, numbered.
    await app.request.getByRole('button', { name: 'Let Practice agent address both' }).click();
    await expect(app.messages.last()).toContainText('Reviewers left 2 comments on the pull request.');
    await expect(app.messages.last()).toContainText('2. boris wrote about `src/app.ts` line 1:');
  });
});

test.describe('A request that merges by itself', () => {
  test('RV-04 RV-05 "Merge by itself" waits for the checks; it can be taken back; failed checks stop it', async ({
    app,
    core,
    project,
  }) => {
    const shop = await project({ remote: 'github' });
    shop.setPullTemplate(pullTemplate({ checks: 'pending' }));
    const task = await taskWithAChange(app, shop, unique('Describe the limiter'));
    await openRequest(app, 'Raise the limit');

    const byItself = app.request.getByRole('button', { name: 'Merge by itself when everything is green' });
    await expect(app.request.getByRole('button', { name: 'Merge now' })).toBeDisabled();
    await byItself.click();
    await expect(app.request).toContainText('It will merge by itself when everything above is green.');
    await expect(byItself).toBeHidden();
    // It is remembered by the core, not by this window.
    await app.page.reload();
    await app.showView('Changes');
    await expect(app.request).toContainText('It will merge by itself when everything above is green.');

    await app.request.getByRole('button', { name: 'Do not', exact: true }).click();
    await expect(byItself).toBeVisible();
    await byItself.click();
    await expect(app.request).toContainText('It will merge by itself when everything above is green.');

    // The checks fail on the host: the wait ends and the task says why.
    shop.changePull(task.branch!, (pull) =>
      pull.replace('"status":"IN_PROGRESS","conclusion":""', '"status":"COMPLETED","conclusion":"FAILURE"'),
    );
    await expect
      .poll(
        async () => {
          const { events } = await core.request<{ events: { type: string; text?: string }[] }>('tasks.history', {
            taskId: task.id,
          });
          return events.some(
            (event) => event.text === 'The checks of the pull request failed, so it will not merge by itself',
          );
        },
        { timeout: 70_000, intervals: [2000] },
      )
      .toBe(true);
    await expect(app.request).not.toContainText('It will merge by itself');
  });
});

test.describe('Lines that two tasks both change', () => {
  test('RV-06 RV-07 an overlap with another open task is shown with their line; it can be waved away or sent to the agent', async ({
    app,
    project,
  }) => {
    const shop = await project({ remote: 'github' });
    const otherTitle = unique('Raise the limit to ten');
    await taskWithAChange(app, shop, otherTitle, 'limit: 10,');
    await app.goHome();
    await taskWithAChange(app, shop, unique('Describe the limiter'));

    const card = app.git.getByRole('region', { name: `Overlap in ${DEMO_FILE}` });
    await expect(card.getByRole('heading')).toHaveText('Another task changes the same line');
    await expect(card).toContainText(`${DEMO_FILE}, line 3, is also changed in:`);
    await expect(card.getByRole('link')).toContainText(otherTitle);
    await expect(card.getByRole('link')).toContainText('your turn');
    await expect(card).toContainText('Their line 3');
    await expect(card.locator('pre')).toHaveText('  limit: 10,');
    await expect(card).toContainText('Whichever merges second gets the conflict. You can commit as it is.');

    // The agent can be asked to keep both.
    await card.getByRole('button', { name: 'Ask Practice agent to combine them' }).click();
    await expect(app.messages.last()).toContainText(`Another task, “${otherTitle}”, changes the same place`);
    await expect(app.messages.last()).toContainText('limit: 10,');
    await expect(app.question).toBeVisible();
    await app.answer('Do not edit');
    await app.turnEnds();

    // "Not a problem" takes the card away, and it stays away.
    await card.getByRole('button', { name: 'Not a problem' }).click();
    await expect(card).toBeHidden();
    await app.page.reload();
    await app.showView('Changes');
    await expect(commitButton(app)).toBeVisible();
    await expect(card).toBeHidden();
  });
});

test.describe('A commit message from the agent', () => {
  test('RV-08 "Ask … to write it" puts what the agent answers into the message box', async ({ app, project }) => {
    const shop = await project({ remote: 'bare' });
    await taskWithAChange(app, shop, unique('Describe the limiter'));
    const box = app.git.getByRole('textbox', { name: 'Commit message' });
    await app.git.getByRole('button', { name: 'Ask Practice agent to write it' }).click();
    await expect(app.git).toContainText('Practice agent is writing it');
    await expect(box).toHaveValue(
      'Limit login attempts to five a minute\n\nThe limit comes from config now, so it can change without a release.',
    );
    // The question and the answer are a turn of the conversation, like any other.
    await expect(app.messages.last()).toContainText('write a commit message for it');
    await expect(app.messages.last()).toContainText(`\`${DEMO_FILE}\``);
    // Nothing was changed by asking.
    await expect(app.question).toBeHidden();
    await commitButton(app).click();
    await expect(app.commitRow('Limit login attempts to five a minute')).toBeVisible();
  });
});
