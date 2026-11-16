import { expect, pullTemplate, readIn, test, type App, type Project } from './support/fixtures';

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
    await rows.nth(0).getByRole('button', { name: 'Start in editor' }).click();
    await expect(app.page).toHaveURL(/#\/task\/[^/]+\/code$/);
    await app.turnEnds();
    const taskId = app.taskId();
    await app.page.getByRole('link', { name: 'Issues', exact: true }).click();
    await expect(rows.nth(0)).toContainText('your turn');
    await expect(rows.nth(0).getByRole('link', { name: 'Open in editor' })).toHaveAttribute(
      'href',
      `#/task/${taskId}/code`,
    );
    await expect(rows.nth(0).getByRole('button', { name: 'Start in editor' })).toHaveCount(0);

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
    await rows.filter({ hasText: '#12' }).getByRole('button', { name: 'Start in editor' }).click();
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

    // The issue stands next to the task's name, as a button that reads it here at the side (ID-10),
    // and in the task's row on Home.
    const chip = app.page
      .locator('.subbar')
      .getByRole('button', { name: 'Read #12 here: Login allows unlimited attempts' });
    await expect(chip).toHaveText('#12');
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

  test('N-18 N-19 the lists are searched by name, key and label, and put in the order picked; the order is kept', async ({
    app,
    project,
  }) => {
    const shop = await project({
      remote: 'github',
      issues: [
        ...ISSUES,
        { number: 7, title: 'billing page is slow', labels: ['perf'] },
        { number: 20, title: 'Add a limit of requests' },
      ],
    });
    await app.addProject(shop.dir);
    await openIssues(app);
    const section = sectionOf(app, shop);
    const rows = section.locator('.issue-row');
    const keys = rows.locator('.issue-key');
    await expect(keys).toHaveText(['#12', '#13', '#7', '#20']);
    await expect(section).toContainText('· 4 open');

    // A word of the name, whatever its case; then a label; then a key.
    const search = app.page.getByRole('searchbox', { name: 'Search the issues' });
    await search.fill('PAGINATION');
    await expect(keys).toHaveText(['#13']);
    await expect(section).toContainText('· 1 of 4 open');
    await search.fill('perf');
    await expect(keys).toHaveText(['#7']);
    await search.fill('#12');
    await expect(keys).toHaveText(['#12']);
    // Every word has to be there, anywhere in the issue.
    await search.fill('login auth');
    await expect(keys).toHaveText(['#12']);
    await search.fill('login perf');
    await expect(rows).toHaveCount(0);
    await expect(section).toContainText('· 0 of 4 open');
    await expect(section).toContainText('Nothing here matches “login perf”.');
    // Escape empties the search.
    await search.press('Escape');
    await expect(search).toHaveValue('');
    await expect(keys).toHaveText(['#12', '#13', '#7', '#20']);
    await expect(section).toContainText('· 4 open');

    // The order: by name without minding capitals, by key with its number read as a number.
    const sort = app.page.getByRole('combobox', { name: 'Sort' });
    await expect(sort).toHaveValue('tracker');
    await sort.selectOption({ label: 'By name' });
    await expect(keys).toHaveText(['#20', '#7', '#12', '#13']);
    await sort.selectOption({ label: 'By key' });
    await expect(keys).toHaveText(['#7', '#12', '#13', '#20']);
    // What a search leaves is in the same order.
    await search.fill('limit');
    await expect(keys).toHaveText(['#12', '#20']);

    // Another screen and back: the search is as it was left. A reload: the order is kept, the search is not.
    await app.page.getByRole('navigation', { name: 'Sections' }).getByRole('link', { name: 'Home' }).click();
    await app.page.getByRole('link', { name: 'Issues', exact: true }).click();
    await expect(search).toHaveValue('limit');
    await expect(keys).toHaveText(['#12', '#20']);
    await app.page.reload();
    await expect(sort).toHaveValue('key');
    await expect(search).toHaveValue('');
    await expect(keys).toHaveText(['#7', '#12', '#13', '#20']);
    await sort.selectOption({ label: 'As the tracker lists them' });
    await expect(keys).toHaveText(['#12', '#13', '#7', '#20']);
  });

  test('N-20 N-21 an issue that has a request open already: the task goes on with it, on its branch, when asked to', async ({
    app,
    project,
  }) => {
    const shop = await project({ remote: 'github', issues: ISSUES });
    const address = `https://github.com/${shop.slug}/pull/128`;
    // Somebody pushed work for issue 12 and opened a request for it. This folder no longer has the branch.
    const trunk = shop.git(['symbolic-ref', '--short', 'HEAD']);
    shop.git(['checkout', '-q', '-b', 'feat/six-attempts']);
    shop.write('notes.txt', 'six a minute\n');
    shop.git(['add', 'notes.txt']);
    shop.git(['commit', '-q', '-m', 'Allow six attempts']);
    shop.git(['push', '-q', 'origin', 'feat/six-attempts']);
    shop.git(['checkout', '-q', trunk]);
    shop.git(['branch', '-q', '-D', 'feat/six-attempts']);
    shop.setIssueWhole(12, { number: 12, closedByPullRequestsReferences: [{ number: 128, url: address }] });
    shop.setPullWhole(128, {
      number: 128,
      title: 'Allow six attempts',
      url: address,
      state: 'OPEN',
      isDraft: true,
      headRefName: 'feat/six-attempts',
      baseRefName: trunk,
      isCrossRepository: false,
    });
    shop.setPullOf('feat/six-attempts', 128, pullTemplate({ checks: 'passed' }));
    await app.addProject(shop.dir);
    await openIssues(app);
    const row = sectionOf(app, shop).locator('.issue-row').filter({ hasText: '#12' });

    // Before the task starts, the request is found and the user is asked which way to go.
    await row.getByRole('button', { name: 'Start in editor' }).click();
    const ask = app.page.getByRole('dialog', { name: '#12 has work under way' });
    await expect(ask).toContainText('A request for it is open in this repository.');
    await expect(ask).toContainText('Allow six attempts');
    await expect(ask).toContainText(`Draft · feat/six-attempts into ${trunk}`);
    await expect(ask.getByRole('link', { name: 'PR #128' })).toHaveAttribute('href', address);
    await expect(ask.getByRole('button', { name: 'Start on a new branch' })).toBeVisible();
    // Escape leaves everything as it was: no task was made.
    await app.page.keyboard.press('Escape');
    await expect(ask).toHaveCount(0);
    await expect(row).toContainText('No task yet');

    // Asked to go on, the task works on the branch of the request, with what was pushed there.
    await row.getByRole('button', { name: 'Start in editor' }).click();
    await ask.getByRole('button', { name: 'Go on with PR #128' }).click();
    await expect(app.page).toHaveURL(/#\/task\/[^/]+\/code$/);
    await app.turnEnds();
    const task = await app.core.task(app.taskId());
    expect(task.branch).toBe('feat/six-attempts');
    expect(readIn(task.cwd, 'notes.txt')).toBe('six a minute\n');
    expect(shop.git(['branch', '--list', 'task/*'])).toBe('');
    await expect(app.page.locator('.subbar')).toContainText('feat/six-attempts');
    await expect(app.messages.first()).toContainText(
      `The work on this goes on in PR #128 (${address}), on its branch \`feat/six-attempts\``,
    );
    // The request is the task's from its first moment: Changes names its third part after it.
    await app.showView('Changes');
    await expect(app.changesTab('PR #128')).toBeVisible();
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
