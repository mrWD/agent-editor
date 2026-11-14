import { expect, test, unique, writeIn, type App, type Project } from './support/fixtures';

const TASKS = [
  'from celery import shared_task',
  '',
  '',
  '@shared_task',
  'def send_receipt(order_id, copy_to=None):',
  '    ...',
  '',
].join('\n');

/** A shop with two tasks of Celery in its files and a worker among its services. */
function withTasks(shop: Project) {
  shop.write('shop/__init__.py', '');
  shop.write('shop/celery.py', "from celery import Celery\n\napp = Celery('shop')\n");
  shop.write('shop/tasks.py', TASKS);
  shop.write('shop/mail.py', 'from celery import shared_task\n\n\n@shared_task\ndef digest():\n    ...\n');
  shop.git(['add', '.']);
  shop.git(['commit', '-q', '-m', 'Tasks']);
}

/** A task with the Jobs of the Preview on show. The task has written one job more into a file of two. */
async function openJobs(app: App, shop: Project) {
  await app.openHome();
  const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
  writeIn(await app.folderOf(taskId), 'shop/tasks.py', `${TASKS}\n\n@shared_task\ndef refund(order_id):\n    ...\n`);
  await app.showView('Preview');
  const view = app.page.getByRole('region', { name: 'Preview', exact: true });
  await view
    .getByRole('group', { name: 'What to preview on' })
    .getByRole('button', { name: 'Jobs', exact: true })
    .click();
  return view;
}

test.describe('The background jobs of a task', () => {
  test('JB-01 JB-02 the jobs of the project are listed, the ones the task touched first, and one is started with its arguments', async ({
    app,
    project,
  }) => {
    const shop = await project({ worker: true, scripts: { 'job:prices': 'echo prices done' } });
    withTasks(shop);
    const view = await openJobs(app, shop);
    const list = view.getByRole('navigation', { name: 'The jobs of the project' });
    await expect(list.getByRole('button')).toHaveText([
      /^shop\.tasks\.refund\s*Celery task\s*changed$/,
      /^shop\.tasks\.send_receipt\s*Celery task\s*changed$/,
      /^shop\.mail\.digest\s*Celery task$/,
      /^job:prices\s*Package script$/,
    ]);

    // The first is on show at once. Another shows where it is written and what it takes.
    const job = view.getByRole('region', { name: 'Job', exact: true });
    await expect(job.getByRole('heading')).toContainText('shop.tasks.refund');
    await list.getByRole('button', { name: /send_receipt/ }).click();
    await expect(job.getByRole('heading')).toContainText('shop.tasks.send_receipt');
    await expect(job).toContainText('shop/tasks.py:5');
    await expect(job.locator('.jobs-takes code')).toHaveText(['order_id', 'copy_to=None']);
    await expect(job.getByLabel('Arguments, a JSON list')).toHaveAttribute('placeholder', '[order_id]');
    // A queued job is done by a worker, and the view says whether one is there.
    await expect(job).toContainText(
      'The worker worker is not running. Start it in Run, or the job waits on the queue.',
    );

    // What is not JSON is not sent.
    await job.getByLabel('Arguments, a JSON list').fill('[42');
    await expect(job.getByRole('button', { name: 'Run', exact: true })).toBeDisabled();
    await expect(job).toContainText('The arguments are not JSON yet.');
    await job.getByLabel('Arguments, a JSON list').fill('[42]');
    await job.getByLabel('Named arguments, a JSON object').fill('{"copy_to": "a@b.c"}');

    // The project says how its jobs of this kind are started: here, a line that only says what it was given.
    await job.getByRole('button', { name: 'How it is started' }).click();
    const setup = job.getByRole('form', { name: 'How jobs of this kind are started' });
    await expect(setup.getByLabel('The command line of every celery task')).toHaveAttribute(
      'placeholder',
      /^id=\$\(celery -A shop\.celery:app call \{name\} --args \{args\} --kwargs \{kwargs\}\)/,
    );
    await setup
      .getByLabel('The command line of every celery task')
      .fill('echo called {name} with {args} and {kwargs}; exit 3');
    await setup.getByRole('button', { name: 'Save' }).click();
    await expect(setup).toHaveCount(0);

    await job.getByRole('button', { name: 'Run', exact: true }).click();
    const run = job.getByRole('region', { name: 'The run' });
    await expect(run.locator('.tag')).toHaveText('Ended with code 3');
    await expect(run).toContainText(
      `$ echo called shop.tasks.send_receipt with '[42]' and '{"copy_to":"a@b.c"}'; exit 3`,
    );
    await expect(run.getByLabel('What the job printed')).toContainText(
      'called shop.tasks.send_receipt with [42] and {"copy_to":"a@b.c"}',
    );

    // A run that went wrong goes to the agent with what was started and what it printed.
    await run.getByRole('button', { name: 'Show it to the agent' }).click();
    const card = app.conversation.getByRole('region', { name: 'From the preview: shop.tasks.send_receipt failed' });
    await expect(card).toContainText('Celery task shop.tasks.send_receipt (shop/tasks.py:5)');
    await expect(card).toContainText('It ended with code 3.');

    // Left and come back to, the view opens on the same job with its last run.
    await app.showView('Code');
    await app.showView('Preview');
    await expect(job.getByRole('heading')).toContainText('shop.tasks.send_receipt');
    await expect(job.getByLabel('Arguments, a JSON list')).toHaveValue('[42]');
    await expect(run.locator('.tag')).toHaveText('Ended with code 3');
  });

  test('JB-03 a script that is a job takes the rest of its command line, and a run that goes on can be stopped', async ({
    app,
    project,
  }) => {
    const shop = await project({
      scripts: { 'job:prices': 'echo prices done', 'cron:slow': 'echo started && sleep 30' },
    });
    withTasks(shop);
    const view = await openJobs(app, shop);
    const list = view.getByRole('navigation', { name: 'The jobs of the project' });
    await expect(list.getByRole('button')).toHaveText([
      /^shop\.tasks\.refund/,
      /^shop\.tasks\.send_receipt/,
      /^shop\.mail\.digest/,
      /^cron:slow\s*Package script$/,
      /^job:prices\s*Package script$/,
    ]);
    const job = view.getByRole('region', { name: 'Job', exact: true });
    const run = job.getByRole('region', { name: 'The run' });

    await list.getByRole('button', { name: /job:prices/ }).click();
    // A command is not put on a queue: no worker is spoken of, and what is typed goes on its command line.
    await expect(job).not.toContainText('worker');
    await job.getByLabel('The rest of the command line').fill('--fast');
    await job.getByLabel('The rest of the command line').press('Enter');
    await expect(run.locator('.tag')).toHaveText('Ended well', { timeout: 30_000 });
    await expect(run).toContainText('run job:prices -- --fast');
    await expect(run.getByLabel('What the job printed')).toContainText('prices done --fast');

    await list.getByRole('button', { name: /cron:slow/ }).click();
    await expect(run).toHaveCount(0);
    await job.getByRole('button', { name: 'Run', exact: true }).click();
    await expect(run.locator('.tag')).toHaveText('Running');
    await expect(run.getByLabel('What the job printed')).toContainText('started', { timeout: 30_000 });
    await job.getByRole('button', { name: 'Stop', exact: true }).click();
    await expect(job.getByRole('button', { name: 'Run', exact: true })).toBeEnabled();
    // What it had printed until then stays on show.
    await expect(run.locator('.tag')).toHaveText('Stopped');
    await expect(run.getByLabel('What the job printed')).toContainText('started');
  });

  test('JB-04 a project without jobs says what was looked for', async ({ app, project }) => {
    await app.openHome();
    await app.createTask({ text: unique('Describe the limiter'), folder: (await project()).dir });
    await app.showView('Preview');
    const view = app.page.getByRole('region', { name: 'Preview', exact: true });
    await view
      .getByRole('group', { name: 'What to preview on' })
      .getByRole('button', { name: 'Jobs', exact: true })
      .click();
    await expect(view.getByRole('heading', { name: 'No background jobs were found' })).toBeVisible();
    await expect(view).toContainText('Tasks of Celery and Dramatiq');
  });
});
