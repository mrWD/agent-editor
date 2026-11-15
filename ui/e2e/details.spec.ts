import {
  expect,
  pullTemplate,
  test,
  unique,
  writeIn,
  DEMO_FILE,
  DEMO_TEXT,
  type App,
  type Project,
} from './support/fixtures';

const ISSUES = [
  { number: 12, title: 'Login allows unlimited attempts', body: 'Five a minute is enough.', labels: ['auth'] },
  { number: 13, title: 'Orders need pagination' },
];

/** The smallest PNG there is: one transparent pixel. */
const PIXEL = Buffer.from(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==',
  'base64',
);

/** Issue 12 as GitHub gives it whole: a description with two pictures and something that would run, and two comments. */
const whole = (slug: string) => ({
  number: 12,
  title: 'Login allows unlimited attempts',
  body: [
    'Five a minute is **enough**.',
    '',
    `![the form](https://github.com/user-attachments/assets/form-1)`,
    '',
    '<img width="900" src="https://tracker.example.com/pixel.png" onerror="window.hacked = 1">',
    '<script>window.hacked = 2</script>',
  ].join('\n'),
  state: 'OPEN',
  author: { login: 'ann' },
  assignees: [{ login: 'bob' }],
  labels: [{ name: 'auth' }, { name: 'security' }],
  createdAt: '2026-11-10T08:00:00Z',
  updatedAt: '2026-11-12T19:05:30Z',
  url: `https://github.com/${slug}/issues/12`,
  comments: [
    { author: { login: 'bob' }, body: 'Seen it on staging too.', createdAt: '2026-11-11T10:00:00Z' },
    { author: { login: 'ann' }, body: 'Then let us do it this week.', createdAt: '2026-11-12T10:00:00Z' },
  ],
});

const sectionOf = (app: App, project: Project) =>
  app.page.locator('section', { has: app.page.getByRole('heading', { level: 2, name: project.name }) });
const panel = (app: App) => app.page.getByRole('complementary', { name: /^Details of / });

test.describe('An issue and a request in full', () => {
  test('ID-01 ID-02 ID-03 the title of an issue opens it at the side: the description, the people, the comments and the pictures', async ({
    app,
    project,
  }) => {
    const shop = await project({ remote: 'github', issues: ISSUES });
    shop.setIssueWhole(12, whole(shop.slug!));
    await app.addProject(shop.dir);
    // The core is asked for each picture; the one GitHub keeps is given here, as the core would give it.
    const asked: string[] = [];
    await app.page.route('**/api/tracker/picture**', (route) => {
      const src = new URL(route.request().url()).searchParams.get('src') ?? '';
      asked.push(src);
      if (src.includes('user-attachments')) return route.fulfill({ contentType: 'image/png', body: PIXEL });
      return route.continue();
    });
    await app.page.goto(`${app.core.url}/#/issues`);
    const rows = sectionOf(app, shop).locator('.issue-row');
    await expect(rows).toHaveCount(2);
    await expect(panel(app)).toHaveCount(0);

    await rows.filter({ hasText: '#12' }).getByRole('button', { name: 'Login allows unlimited attempts' }).click();
    const side = panel(app);
    await expect(side).toHaveAccessibleName('Details of #12');
    await expect(side.getByRole('heading', { level: 2 })).toHaveText('Login allows unlimited attempts');
    await expect(side.locator('.detail-head .tag')).toHaveText('open');
    await expect(side.getByRole('link', { name: 'Open in GitHub' })).toHaveAttribute(
      'href',
      `https://github.com/${shop.slug}/issues/12`,
    );
    await expect(side.locator('.detail-people')).toContainText('Opened by ann');
    await expect(side.locator('.detail-people')).toContainText('Assigned to bob');
    await expect(side.locator('.detail-labels .tag')).toHaveText(['auth', 'security']);

    // The description is shown as it is meant, and what would run in it is not there and did not run.
    const text = side.getByRole('region', { name: 'Description' });
    await expect(text.locator('strong')).toHaveText('enough');
    await expect(text.locator('script')).toHaveCount(0);
    expect(await app.page.evaluate(() => (window as unknown as { hacked?: number }).hacked)).toBeUndefined();

    // A picture the tracker keeps is asked of the core, never of the address the text names.
    const shot = text.getByRole('img', { name: 'the form' });
    await expect(shot).toBeVisible();
    await expect(shot).toHaveAttribute(
      'src',
      /^\/api\/tracker\/picture\?project=[^&]+&src=https%3A%2F%2Fgithub\.com%2Fuser-attachments/,
    );
    // One kept anywhere else is refused by the core: a line says so and links to where it is.
    const lost = text.locator('.md-lost');
    await expect(lost).toHaveText('Picture not shown: https://tracker.example.com/pixel.png');
    await expect(lost).toHaveAttribute('href', 'https://tracker.example.com/pixel.png');
    await expect(text.getByRole('img')).toHaveCount(1);
    expect(asked.sort()).toEqual([
      'https://github.com/user-attachments/assets/form-1',
      'https://tracker.example.com/pixel.png',
    ]);

    // What was written under it, oldest first.
    const notes = side.getByRole('region', { name: 'Comments' });
    await expect(notes.getByRole('heading')).toHaveText('2 comments');
    await expect(notes.getByRole('article')).toHaveText([
      /^bob[\s\S]*Seen it on staging too\.\s*$/,
      /^ann[\s\S]*Then let us do it this week\.\s*$/,
    ]);

    // The list stays where it is: another issue opens in the same panel. One the tracker cannot give says so.
    await rows.filter({ hasText: '#13' }).getByRole('button', { name: 'Orders need pagination' }).click();
    await expect(side.getByRole('heading', { level: 2 })).toHaveText('Orders need pagination');
    await expect(side.getByRole('alert')).toContainText('Could not resolve to an Issue with the number of 13');
    await expect(side.getByRole('region', { name: 'Comments' })).toHaveCount(0);

    // Come back to, the first is there at once from what was read before; Escape closes the panel.
    await rows.filter({ hasText: '#12' }).getByRole('button', { name: 'Login allows unlimited attempts' }).click();
    await expect(notes.getByRole('article')).toHaveCount(2);
    await app.page.keyboard.press('Escape');
    await expect(side).toHaveCount(0);
  });

  test('ID-04 a task is started from the panel, and the panel of an issue in work leads to its task', async ({
    app,
    project,
  }) => {
    const shop = await project({ remote: 'github', issues: ISSUES });
    shop.setIssueWhole(12, whole(shop.slug!));
    await app.addProject(shop.dir);
    await app.page.goto(`${app.core.url}/#/issues`);
    const row = sectionOf(app, shop).locator('.issue-row').filter({ hasText: '#12' });
    await row.getByRole('button', { name: 'Login allows unlimited attempts' }).click();
    await panel(app).getByRole('button', { name: 'Start a task on it' }).click();
    await expect(app.page).toHaveURL(/#\/task\/[^/]+\/code$/);
    await app.turnEnds();
    const taskId = app.taskId();

    await app.page.goto(`${app.core.url}/#/issues`);
    await row.getByRole('button', { name: 'Login allows unlimited attempts' }).click();
    await expect(panel(app).getByRole('link', { name: 'Open its task' })).toHaveAttribute(
      'href',
      `#/task/${taskId}/code`,
    );
    await expect(panel(app).getByRole('button', { name: 'Start a task on it' })).toHaveCount(0);
  });

  test('ID-05 "Details" on the card of a pull request opens it whole, with what the reviewers said of it', async ({
    app,
    project,
  }) => {
    const shop = await project({ remote: 'github' });
    shop.setPullTemplate(pullTemplate({ checks: 'passed' }));
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    const task = await app.core.task(taskId);
    writeIn(task.cwd, DEMO_FILE, DEMO_TEXT.replace('limit: 5,', 'limit: 6,'));
    await app.showView('Changes');
    await app.git.getByRole('radio', { name: 'Push and open a pull request' }).check();
    await app.git.getByLabel('Commit message').fill('Allow six attempts');
    await app.git.getByRole('button', { name: /^Commit \d+ files?/ }).click();
    await expect(app.request.getByRole('link', { name: 'Open on GitHub' })).toBeVisible();
    shop.setPullWhole(128, {
      number: 128,
      title: 'Allow six attempts',
      body: 'Closes #12.\n\n- [x] tested by hand',
      state: 'OPEN',
      isDraft: true,
      author: { login: 'ann' },
      assignees: [],
      reviewRequests: [{ login: 'cy' }],
      labels: [{ name: 'auth' }],
      createdAt: '2026-11-10T08:00:00Z',
      updatedAt: '2026-11-12T19:05:30Z',
      url: `https://github.com/${shop.slug}/pull/128`,
      baseRefName: 'main',
      headRefName: task.branch,
      comments: [{ author: { login: 'ann' }, body: 'Ready for a look.', createdAt: '2026-11-11T10:00:00Z' }],
      reviews: [
        {
          author: { login: 'cy' },
          state: 'CHANGES_REQUESTED',
          body: 'Six is too many.',
          submittedAt: '2026-11-12T09:00:00Z',
        },
      ],
    });

    await app.request.getByRole('button', { name: 'Details', exact: true }).click();
    const side = panel(app);
    await expect(side).toHaveAccessibleName('Details of #128');
    await expect(side.getByRole('heading', { level: 2 })).toHaveText('Allow six attempts');
    await expect(side.locator('.detail-head .tag')).toHaveText(['open', 'draft']);
    await expect(side.locator('.detail-branch')).toHaveText(`${task.branch} → main`);
    await expect(side.locator('.detail-people')).toContainText('Review asked of cy');
    await expect(side.getByRole('region', { name: 'Description' })).toContainText('Closes #12.');
    const notes = side.getByRole('region', { name: 'Comments' });
    await expect(notes.getByRole('heading')).toHaveText('1 comment and 1 review');
    await expect(notes.getByRole('article')).toHaveText([
      /^ann[\s\S]*Ready for a look\.\s*$/,
      /^cy\s*asked for changes[\s\S]*Six is too many\.\s*$/,
    ]);

    // "Read again" asks the host anew; "Close" puts the panel away and leaves the card as it was.
    shop.setPullWhole(128, {
      number: 128,
      title: 'Allow six attempts, for now',
      state: 'OPEN',
      url: 'https://github.com/x/y/pull/128',
    });
    await side.getByRole('button', { name: 'Read again' }).click();
    await expect(side.getByRole('heading', { level: 2 })).toHaveText('Allow six attempts, for now');
    await side.getByRole('button', { name: 'Close', exact: true }).click();
    await expect(side).toHaveCount(0);
    await expect(app.request.getByRole('link', { name: 'Open on GitHub' })).toBeVisible();
  });
});
