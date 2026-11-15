import { expect, test, type App, type Project } from './support/fixtures';

const ISSUES = [
  { number: 12, title: 'Login allows unlimited attempts', body: 'Five a minute is enough.', labels: ['auth'] },
  { number: 13, title: 'Orders need pagination' },
];

/** The part of the Issues screen that belongs to one project. */
const sectionOf = (app: App, project: Project) =>
  app.page.locator('section', { has: app.page.getByRole('heading', { level: 2, name: project.name }) });

async function openIssues(app: App) {
  await app.page.goto(`${app.core.url}/#/issues`);
  await expect(app.page.getByRole('heading', { level: 1, name: 'Issues' })).toBeVisible();
}

test.describe('Issues', () => {
  test('N-01 N-12 the issues of a GitHub project are listed, and "Without a task" hides the ones in work', async ({
    app,
    project,
  }) => {
    const shop = await project({ remote: 'github', issues: ISSUES });
    await app.addProject(shop.dir);
    await openIssues(app);

    await expect(app.page.getByText('GitHub is connected.')).toBeVisible();
    await expect(app.page.getByText('Synced just now.')).toBeVisible();
    const section = sectionOf(app, shop);
    await expect(section).toContainText(`GitHub ${shop.slug} · 2 open`);
    const rows = section.locator('.issue-row');
    await expect(rows).toHaveCount(2);
    await expect(rows.nth(0)).toContainText('#12Login allows unlimited attemptsauthopenNo task yet');
    await expect(rows.nth(1)).toContainText('#13Orders need paginationopenNo task yet');
    await expect(rows.nth(0).getByRole('link', { name: '#12' })).toHaveAttribute(
      'href',
      `https://github.com/${shop.slug}/issues/12`,
    );
    // It was the stand-in that was asked, with the project's own address.
    expect(app.core.calls('gh')).toContainEqual(expect.stringContaining(`issue list -R ${shop.slug} --state open`));

    // Start a task for one issue, then come back.
    await rows.nth(0).getByRole('button', { name: 'Start' }).click();
    await expect(app.page).toHaveURL(/#\/task\/[^/]+\/code$/);
    await app.turnEnds();
    const taskId = app.taskId();
    await app.page.getByRole('link', { name: 'Issues', exact: true }).click();
    await expect(rows.nth(0)).toContainText('your turn');
    await expect(rows.nth(0).getByRole('link', { name: 'Open' })).toHaveAttribute('href', `#/task/${taskId}/code`);
    await expect(rows.nth(0).getByRole('button', { name: 'Start' })).toHaveCount(0);

    const filter = app.page.getByRole('group', { name: 'Which issues to show' });
    await filter.getByRole('button', { name: 'Without a task' }).click();
    await expect(filter.getByRole('button', { name: 'Without a task' })).toHaveAttribute('aria-pressed', 'true');
    await expect(rows).toHaveCount(1);
    await expect(rows).toContainText('#13Orders need pagination');
    await filter.getByRole('button', { name: 'Open', exact: true }).click();
    await expect(rows).toHaveCount(2);
  });

  test('N-02 N-03 N-04 a task started from an issue has its title and text, and shows the issue', async ({
    app,
    project,
  }) => {
    const shop = await project({ remote: 'github', issues: ISSUES });
    const projectId = await app.addProject(shop.dir);
    await openIssues(app);
    const rows = sectionOf(app, shop).locator('.issue-row');
    await rows.filter({ hasText: '#12' }).getByRole('button', { name: 'Start' }).click();
    await expect(app.page).toHaveURL(/#\/task\/[^/]+\/code$/);
    await app.turnEnds();
    const taskId = app.taskId();
    const address = `https://github.com/${shop.slug}/issues/12`;

    // The task is named after the issue, and the agent got what the issue says.
    await expect(app.page.getByRole('heading', { level: 1, name: 'Login allows unlimited attempts' })).toBeVisible();
    await expect(app.messages.first()).toContainText('Login allows unlimited attempts');
    await expect(app.messages.first()).toContainText('Five a minute is enough.');
    await expect(app.messages.first()).toContainText(`This is issue #12 (${address}).`);
    // What the agent was sent is read from its thought, and a thought is shown only with every step.
    await app.allSteps();
    await expect(app.thoughts.first()).toContainText('Five a minute is enough.');

    // The issue stands next to the task's name, as a link to it, and in the task's row on Home.
    const chip = app.page.locator('.subbar').getByRole('link', { name: '#12' });
    await expect(chip).toHaveAttribute('href', address);
    await expect(chip).toHaveAttribute('title', 'Login allows unlimited attempts');
    await app.goHome();
    const row = app
      .stage('Your turn')
      .locator('.stage-row', { has: app.page.locator(`a[href="#/task/${taskId}/code"]`) });
    await expect(row.first()).toContainText(`${shop.name}Login allows unlimited attempts#12`);

    // The same issue cannot be started twice. The screen offers no button; the core refuses as well.
    await expect(app.core.request('issues.start', { projectId, key: '#12', agentId: 'mock' })).rejects.toThrow(
      'A task for #12 is already open',
    );
  });

  test('N-06 N-07 a project without a known host has no tracker; a Linear team without a key says what is missing', async ({
    app,
    project,
  }) => {
    const shop = await project();
    await app.addProject(shop.dir);
    await openIssues(app);
    const section = sectionOf(app, shop);
    await expect(section).toContainText('No tracker: its remote is not on GitHub or GitLab, and no Linear team is set');
    await expect(section).toContainText('Nothing to list.');
    // Linear says how to connect it where it is: a link that opens Linear's page of keys, and a box for the key.
    const linear = app.page.getByRole('region', { name: 'Linear' });
    await expect(linear).toContainText('Connect Linear to see the issues assigned to you.');
    await expect(linear.getByRole('link', { name: 'Open Linear' })).toHaveAttribute(
      'href',
      'https://linear.app/settings/account/security',
    );
    await expect(linear.getByRole('link', { name: 'Open Linear' })).toHaveAttribute('target', '_blank');
    await expect(linear.getByLabel('Linear API key')).toBeVisible();
    await expect(app.page.getByRole('button', { name: 'Connect', exact: true })).toBeDisabled();
    // Without a key there is no list of one's own Linear issues.
    await expect(app.page.getByRole('region', { name: 'Assigned to you in Linear' })).toHaveCount(0);

    await section.getByRole('button', { name: 'Use a Linear team' }).click();
    const team = section.getByRole('textbox', { name: `Linear team key for ${shop.name}` });
    await team.fill('app');
    await section.getByRole('button', { name: 'Save' }).click();
    await expect(section).toContainText('Linear team APP · 0 open');
    await expect(section).toContainText('Linear is not connected yet. Paste a key above.');

    // Without a team key the project goes back to its code host.
    await section.getByRole('button', { name: 'Change the Linear team' }).click();
    await team.fill('');
    await section.getByRole('button', { name: 'Use the code host' }).click();
    await expect(section).toContainText('No tracker');
    await expect(section.getByRole('button', { name: 'Use a Linear team' })).toBeVisible();
  });
});
