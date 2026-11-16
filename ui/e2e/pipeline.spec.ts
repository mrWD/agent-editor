import { DEMO_FILE, DEMO_TEXT, expect, test, unique, writeIn, type App, type Project } from './support/fixtures';

/** The Pipeline card of the task on screen: it stands under the merge request, on Changes. */
const card = (app: App) => app.page.getByRole('region', { name: 'Pipeline' });
const run = (app: App, name: string) => card(app).getByRole('group', { name, exact: true });
/** The bar of a run: how its jobs stand, as shares. Its name says the same in words. */
const bar = (app: App, name: string) => run(app, name).getByRole('img');

/**
 * A task of a project on GitHub, on the part of its Changes view where the pipeline is: what is
 * pushed. Nothing is pushed: the stand-in answers for the branch.
 */
async function taskOn(app: App, shop: Project) {
  await app.openHome();
  const taskId = await app.createTask({ text: unique('Pipeline'), folder: shop.dir });
  await app.showView('Changes');
  await app.showChanges('Pushed');
  return taskId;
}

/** What `gh` was asked to do to the runs of a project. The listing and the reading are left out. */
const done = (app: App, shop: Project) =>
  app.core.calls('gh').filter((call) => /^run (rerun|cancel)/.test(call) && call.includes(shop.slug!));

test.describe('The pipeline of a task’s branch', () => {
  test('CI-01 CI-02 the card says how the newest commit stands: what failed is listed first, what went well is one line', async ({
    app,
    project,
  }) => {
    const shop = await project({ remote: 'github' });
    shop.setRuns([
      {
        id: 9002,
        name: 'CI',
        state: 'failure',
        jobs: [
          { id: 7001, name: 'lint', state: 'success' },
          { id: 7002, name: 'unit', state: 'failure' },
          { id: 7003, name: 'docs', state: 'skipped' },
        ],
      },
      { id: 9003, name: 'Deploy preview', state: 'success', jobs: [{ id: 7004, name: 'deploy', state: 'success' }] },
      // An older commit of the same branch: history, not the pipeline.
      {
        id: 9001,
        name: 'CI',
        state: 'success',
        commit: 'ffffffffffffffffffffffffffffffffffffffff',
        jobs: [{ id: 6001, name: 'old', state: 'success' }],
      },
    ]);
    await taskOn(app, shop);

    await expect(card(app).getByRole('heading', { name: 'Pipeline' })).toBeVisible();
    await expect(card(app)).toContainText('failed · commit 01234567');
    // CI-17: the card is in the third part, so the tab of that part is marked with how the pipeline
    // stands, and the other parts say it in a line, over all its runs. A press on the line goes to the card.
    await expect(app.changesTab('Pushed').getByTitle('Pipeline failed')).toBeVisible();
    await app.showChanges('Not committed');
    await expect(card(app)).toBeHidden();
    const brief = app.git.getByRole('button', { name: 'Pipeline failed: 1 failed, 2 passed, 1 skipped' });
    await expect(brief).toBeVisible();
    await brief.click();
    await expect(app.changesTab('Pushed')).toHaveAttribute('aria-pressed', 'true');
    await expect(card(app)).toBeInViewport();
    await expect(brief).toHaveCount(0);
    const ci = run(app, 'CI');
    await expect(bar(app, 'CI')).toHaveAccessibleName('1 failed, 1 passed, 1 skipped');
    // The bar has a part for each state somebody is in, what failed first.
    await expect(bar(app, 'CI').locator('.pipe-share')).toHaveClass([/failed/, /passed/, /skipped/]);
    await expect(ci.getByRole('link', { name: 'CI' })).toHaveAttribute(
      'href',
      `https://github.com/${shop.slug}/actions/runs/9002`,
    );
    // Only the job that failed is on screen, under the word for how it stands; the two that need
    // nobody are one line, and a press on the line lists them.
    const failed = ci.getByRole('group', { name: 'Failed' });
    await expect(failed).toContainText('Failed · 1');
    await expect(failed.getByRole('link', { name: 'unit' })).toBeVisible();
    await expect(ci.getByRole('link', { name: 'lint' })).toHaveCount(0);
    const fold = ci.getByRole('button', { name: '1 went well, 1 skipped' });
    await expect(fold).toHaveAttribute('aria-expanded', 'false');
    await fold.click();
    await expect(fold).toHaveAttribute('aria-expanded', 'true');
    await expect(ci.getByRole('link', { name: 'lint' })).toBeVisible();
    await expect(ci.getByRole('listitem').filter({ hasText: 'docs' })).toContainText('skipped');
    await fold.click();
    await expect(ci.getByRole('link', { name: 'lint' })).toHaveCount(0);

    // The second workflow of the same commit is a run of its own; the older commit's run is not shown.
    await expect(bar(app, 'Deploy preview')).toHaveAccessibleName('1 passed');
    await expect(run(app, 'Deploy preview').getByRole('button', { name: '1 went well' })).toBeVisible();
    await expect(card(app)).not.toContainText('old');
    // GitHub starts runs from what a workflow listens to: no new pipeline on request, no single job stopped.
    await expect(card(app).getByRole('button', { name: /Run a new pipeline/ })).toHaveCount(0);
  });

  test('CI-03 CI-04 a failed job is run again with one press, and the conversation says so', async ({
    app,
    project,
  }) => {
    const shop = await project({ remote: 'github' });
    shop.setRuns([
      {
        id: 9002,
        name: 'CI',
        state: 'failure',
        jobs: [
          { id: 7001, name: 'lint', state: 'success' },
          { id: 7002, name: 'unit', state: 'failure' },
        ],
      },
    ]);
    await taskOn(app, shop);
    const ci = run(app, 'CI');

    await ci.getByRole('button', { name: 'Run again: unit' }).click();
    await expect.poll(() => done(app, shop)).toEqual([`run rerun --job 7002 -R ${shop.slug}`]);
    // What the host says next is on the card without a reload: the job is going again.
    await expect(bar(app, 'CI')).toHaveAccessibleName('1 running, 1 passed');
    await expect(ci.getByRole('group', { name: 'Running' })).toContainText('unit');
    await expect(card(app)).toContainText('running · commit 01234567');
    await expect(ci.getByRole('button', { name: 'Run again: unit' })).toHaveCount(0);
    await expect(app.page.getByText('The job “unit” was started again on GitHub.')).toBeAttached();
  });

  test('CI-05 CI-06 what touches a whole run takes two presses: run all again, and stop', async ({ app, project }) => {
    const shop = await project({ remote: 'github' });
    shop.setRuns([{ id: 9002, name: 'CI', state: 'failure', jobs: [{ id: 7002, name: 'unit', state: 'failure' }] }]);
    await taskOn(app, shop);
    const ci = run(app, 'CI');

    // One press only arms the button. Leaving it takes the arming back, and so do a few seconds: a
    // browser that gives a pressed button no keyboard (WebKit) never says that it was left.
    const all = ci.getByRole('button', { name: 'Run all again: CI', exact: true });
    await all.click();
    const armed = ci.getByRole('button', { name: 'Press again: Run all again: CI', exact: true });
    await expect(armed).toBeVisible();
    expect(done(app, shop)).toEqual([]);
    await armed.blur();
    await expect(all).toBeVisible({ timeout: 8000 });
    expect(done(app, shop)).toEqual([]);

    await all.click();
    await armed.click();
    await expect.poll(() => done(app, shop)).toEqual([`run rerun 9002 -R ${shop.slug}`]);
    await expect(bar(app, 'CI')).toHaveAccessibleName('1 running');

    // A run that is going can be stopped, with two presses as well.
    await ci.getByRole('button', { name: 'Stop: CI', exact: true }).click();
    await ci.getByRole('button', { name: 'Press again: Stop: CI', exact: true }).click();
    await expect
      .poll(() => done(app, shop))
      .toEqual([`run rerun 9002 -R ${shop.slug}`, `run cancel 9002 -R ${shop.slug}`]);
    await expect(bar(app, 'CI')).toHaveAccessibleName('1 stopped');
    await expect(ci.getByRole('group', { name: 'Stopped' })).toContainText('unit');
    await expect(app.page.getByText('“CI” was stopped on GitHub.')).toBeAttached();
  });

  test('CI-07 the failed jobs of a run are started again together', async ({ app, project }) => {
    const shop = await project({ remote: 'github' });
    shop.setRuns([
      {
        id: 9002,
        name: 'CI',
        state: 'failure',
        jobs: [
          { id: 7001, name: 'unit', state: 'failure' },
          { id: 7002, name: 'browser', state: 'failure' },
          { id: 7003, name: 'lint', state: 'success' },
        ],
      },
    ]);
    await taskOn(app, shop);
    const ci = run(app, 'CI');
    await expect(bar(app, 'CI')).toHaveAccessibleName('2 failed, 1 passed');
    await ci.getByRole('button', { name: 'Run failed again: CI' }).click();
    await expect.poll(() => done(app, shop)).toEqual([`run rerun 9002 --failed -R ${shop.slug}`]);
    await expect(bar(app, 'CI')).toHaveAccessibleName('2 running, 1 passed');
  });

  test('CI-08 CI-09 a branch nothing ran for says so, and a project that is on neither host has no card', async ({
    app,
    project,
  }) => {
    const shop = await project({ remote: 'github' });
    await taskOn(app, shop);
    await expect(card(app)).toContainText('none yet');
    await expect(card(app)).toContainText(/Nothing has run for .+ on GitHub yet/);

    const local = await project({ remote: 'bare' });
    await taskOn(app, local);
    await expect(app.git).toBeVisible();
    await expect(card(app)).toHaveCount(0);
  });

  test('CI-13 a commit is noticed: the card looks again by itself and shows the pipeline the host made for it', async ({
    app,
    project,
  }) => {
    const shop = await project({ remote: 'github' });
    const taskId = await taskOn(app, shop);
    await expect(card(app)).toContainText('none yet');

    // The host starts a run for what was just committed. Nobody presses "Look again".
    shop.setRuns([{ id: 9100, name: 'CI', state: 'running', jobs: [{ id: 7100, name: 'unit', state: 'running' }] }]);
    const task = await app.core.task(taskId);
    writeIn(task.cwd, DEMO_FILE, DEMO_TEXT.replace('limit: 5,', 'limit: 6,'));
    await app.showChanges('Not committed');
    await app.commit('Raise the limit');
    await app.showChanges('Pushed');
    await expect(bar(app, 'CI')).toHaveAccessibleName('1 running');
    await expect(card(app)).toContainText('running · commit 01234567');
  });

  test('CI-14 reading the pipeline again keeps the rows that were there, so the keyboard keeps its place', async ({
    app,
    project,
  }) => {
    const shop = await project({ remote: 'github' });
    shop.setRuns([{ id: 9002, name: 'CI', state: 'failure', jobs: [{ id: 7002, name: 'unit', state: 'failure' }] }]);
    await taskOn(app, shop);
    const again = card(app).getByRole('button', { name: 'Run again: unit' });
    // The button is marked, to tell it from one made anew in its place.
    await again.evaluate((button) => ((button as HTMLElement).dataset.kept = 'yes'));
    const readings = () =>
      app.core.calls('gh').filter((call) => call.startsWith('run view 9002') && call.includes(shop.slug!)).length;
    const before = readings();
    // The same answer comes again, as it does every few seconds while a pipeline runs.
    await card(app).getByRole('button', { name: 'Look again' }).click();
    await expect.poll(readings).toBeGreaterThan(before);
    await again.focus();
    await card(app).getByRole('button', { name: 'Look again' }).focus();
    await card(app).getByRole('button', { name: 'Look again' }).press('Enter');
    await expect.poll(readings).toBeGreaterThan(before + 1);
    await expect(card(app)).toContainText('failed · commit 01234567');
    await expect(again).toHaveAttribute('data-kept', 'yes');
  });
});
