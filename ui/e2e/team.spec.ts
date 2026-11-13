import { DEMO_FILE, expect, readIn, test, unique } from './support/fixtures';

test.describe('A team on one task', () => {
  test.use({ twoAgents: true });

  test('TM-01 TM-02 a planner, a builder and a reviewer take the task in turn; their notes are in the conversation', async ({
    app,
    project,
  }) => {
    const shop = await project();
    await app.openHome();
    const title = unique('Take the login limit from config');
    const taskId = await app.createTask({ text: title, folder: shop.dir });
    const folder = await app.folderOf(taskId);

    // The form: who plans, who builds, who reviews. The reviewer is another agent by default.
    await app.conversation.getByRole('button', { name: 'Solo' }).click();
    const team = app.page.getByRole('region', { name: 'Team' });
    await expect(team.getByLabel('Who has the role: Plans')).toHaveValue('mock');
    await expect(team.getByLabel('Who has the role: Reviews')).toHaveValue('mock-2');
    await team.getByRole('button', { name: 'Start the team' }).click();

    // The builder asks before it edits, like any agent; the team waits for the answer.
    const pane = app.page.getByRole('complementary', { name: /^Conversation with / });
    await expect(pane.getByRole('heading', { name: /asks: Edit/ })).toBeVisible();
    await expect(team.locator('.team-step', { hasText: 'Plan' }).first()).toContainText(/\d+ min/);
    await expect(team.locator('.team-step.current')).toContainText('Build');
    await pane.getByRole('button', { name: 'Allow this edit', exact: true }).click();

    // The reviewer is the second agent; when it is content the checks follow.
    await expect(team).toContainText('The team has finished.', { timeout: 30_000 });
    await expect(team.locator('.team-step')).toHaveText([
      /^Plan.*Practice agent.*\d+ min$/,
      /^Build.*Practice agent.*\d+ min$/,
      /^Review.*Second practice agent.*\d+ min$/,
      /^Checks.*skipped$/,
    ]);
    expect(readIn(folder, DEMO_FILE)).toContain('limit: limits.login,');

    // What went from one agent to the next is shown as notes, and the user said only one thing.
    const notes = pane.locator('.team-note');
    await expect(notes).toHaveCount(3);
    await expect(notes.nth(0)).toContainText('Note · You to Practice agent');
    await expect(notes.nth(2)).toContainText('Note · Practice agent to Second practice agent');
    await expect(pane.locator('.bubble')).toHaveCount(1);
    await expect(pane.locator('.md', { hasText: 'The change does what the plan says.' })).toBeVisible();
  });

  test('TM-03 a reviewer who wants changes asks the user; "Send back" has the builder build again', async ({
    app,
    project,
  }) => {
    const shop = await project();
    await app.openHome();
    await app.createTask({ text: unique('Be strict about the limiter'), folder: shop.dir });
    await app.conversation
      .getByLabel('What Practice agent may do without asking')
      .selectOption({ label: 'Edit freely' });

    await app.conversation.getByRole('button', { name: 'Solo' }).click();
    const team = app.page.getByRole('region', { name: 'Team' });
    await team.getByRole('button', { name: 'Start the team' }).click();
    const pane = app.page.getByRole('complementary', { name: /^Conversation with / });
    // The task's own mode goes with it from agent to agent: the builder edits without asking.

    const ask = team.locator('.team-ask');
    await expect(ask.getByRole('heading')).toHaveText(
      'Second practice agent asks: send it back to Practice agent or leave it for later?',
      { timeout: 30_000 },
    );
    await expect(ask).toContainText('Line 3 still holds a number that should come from config.');
    await expect(team.locator('.team-step', { hasText: 'Review' })).toContainText('asks you');

    await ask.getByRole('button', { name: 'Send back to Practice agent' }).click();
    await expect(team).toContainText('The team has finished.', { timeout: 30_000 });
    // The builder was handed what the reviewer found.
    await expect(
      pane.locator('.team-note', { hasText: 'Note · Second practice agent to Practice agent' }),
    ).toContainText('Line 3 still holds a number');
  });

  test('TM-04 the team can be stopped and gone on with', async ({ app, project }) => {
    const shop = await project();
    await app.openHome();
    await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    await app.conversation.getByRole('button', { name: 'Solo' }).click();
    const team = app.page.getByRole('region', { name: 'Team' });
    await team.getByLabel('Who has the role: Reviews').selectOption({ label: 'Nobody' });
    await team.getByRole('button', { name: 'Start the team' }).click();
    const pane = app.page.getByRole('complementary', { name: /^Conversation with / });
    await expect(pane.getByRole('heading', { name: /asks: Edit/ })).toBeVisible();
    await expect(team.locator('.team-step')).toHaveCount(3);

    await team.getByRole('button', { name: 'Stop the team' }).click();
    await expect(team).toContainText('You stopped the team.');
    await expect(team.locator('.team-step', { hasText: 'Build' })).toContainText('stopped');

    await team.getByRole('button', { name: 'Go on with the next step' }).click();
    await pane.getByRole('button', { name: 'Allow this edit', exact: true }).click();
    await expect(team).toContainText('The team has finished.', { timeout: 30_000 });
  });
});
