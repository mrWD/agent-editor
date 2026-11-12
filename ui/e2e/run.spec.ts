import { createServer, type Server } from 'node:net';
import { rmSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { expect, test, unique, type App } from './support/fixtures';

/** Takes a port on every address of this machine, so the core must see it as taken. */
async function holdPort(): Promise<{ port: number; server: Server }> {
  const server = createServer();
  await new Promise<void>((resolve) => server.listen(0, '0.0.0.0', resolve));
  const address = server.address();
  return { port: typeof address === 'object' && address ? address.port : 0, server };
}

async function openRun(app: App) {
  await app.openBottom('Run');
  await expect(app.services).toBeVisible();
}

/**
 * Looks for a text in the log under the table. The log opens below what the panel shows at its
 * usual height, and a terminal draws nothing while it is out of sight: it is brought into view first.
 */
async function expectInLog(app: App, expected: string) {
  const log = app.bottom.locator('.run-log .xterm-rows');
  await log.scrollIntoViewIfNeeded();
  await expect(log).toContainText(expected);
}

/** The line of the `docker` stand-in's log for one compose project. */
const composeCalls = (app: App, composeProject: string) =>
  app.core.calls('docker').filter((call) => call.includes(`-p ${composeProject} `));

test.describe('Running the project of a task', () => {
  test('O-01 O-02 O-03 the dev script is read from package.json, started, found at its address and stopped', async ({
    app,
    project,
  }) => {
    const shop = await project();
    await app.openHome();
    await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    await openRun(app);

    // What can run: the dev script and the service of the compose file.
    await expect(app.bottom).toContainText('0 of 2 up. Read from compose.yaml and package.json.');
    const dev = app.service('dev');
    const db = app.service('db');
    await expect(dev).toContainText('npm run dev');
    await expect(dev).toContainText('stopped');
    await expect(db).toContainText('Docker');
    await expect(db).toContainText('postgres:16');
    await expect(dev.getByRole('button', { name: 'Logs' })).toBeDisabled();

    await dev.getByRole('button', { name: 'Start' }).click();
    const address = dev.getByRole('link', { name: 'localhost:4567' });
    await expect(address).toHaveAttribute('href', 'http://localhost:4567');
    await expect(app.bottom).toContainText('1 of 2 up.');
    await expect(db).toContainText('stopped');

    // Its output can be read under the table.
    await dev.getByRole('button', { name: 'Logs' }).click();
    await expect(app.bottom.getByText('Output of dev')).toBeVisible();
    await expectInLog(app, 'Local: http://localhost:4567/');
    await app.bottom.getByRole('button', { name: 'Close' }).click();
    await expect(app.bottom.getByText('Output of dev')).toBeHidden();

    // Stopped on purpose is "stopped", not "did not start".
    await dev.getByRole('button', { name: 'Stop' }).click();
    await expect(address).toBeHidden();
    await expect(dev).toContainText('stopped');
    await expect(dev).not.toContainText('did not start');
    await expect(dev.getByRole('button', { name: 'Start' })).toBeVisible();
    await expect(app.bottom).toContainText('0 of 2 up.');
  });

  test('O-04 a command that ends by itself with an error "did not start", and can be handed to the agent', async ({
    app,
    project,
  }) => {
    const shop = await project({ devScript: 'echo "cannot find the config"; exit 7' });
    await app.openHome();
    await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    await openRun(app);
    const dev = app.service('dev');
    await dev.getByRole('button', { name: 'Start' }).click();
    await expect(dev).toContainText('It stopped with code 7. The log says why.');

    await dev.getByRole('button', { name: 'Send to Practice agent' }).click();
    await expect(app.conversation.locator('.quote')).toContainText('service dev line 1');
    await expect(app.messageBox).toBeFocused();
  });

  test('O-05 O-07 O-08 Docker services run in a compose project of the task; logs are shown; "Stop all" takes them down', async ({
    app,
    project,
  }) => {
    const shop = await project();
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    await openRun(app);
    const composeProject = `ae-${shop.name}-${taskId.slice(0, 4)}`;
    await expect(app.bottom).toContainText(
      `Docker services run as the compose project ${composeProject}, which belongs to this task alone.`,
    );

    await app.bottom.getByRole('button', { name: 'Start all' }).click();
    const db = app.service('db');
    const dev = app.service('dev');
    await expect(db.getByRole('link', { name: /^localhost:\d+$/ })).toBeVisible();
    await expect(dev.getByRole('link', { name: 'localhost:4567' })).toBeVisible();
    await expect(app.bottom).toContainText('2 of 2 up.');
    // The stand-in for Docker was what got called, with the task's own project name and compose file.
    const taskFile = path.join(app.core.dataDir, 'run', taskId, 'compose.json');
    expect(composeCalls(app, composeProject)).toContain(
      `compose -p ${composeProject} -f ${taskFile} up -d --remove-orphans`,
    );

    await db.getByRole('button', { name: 'Logs' }).click();
    await expect(app.bottom.getByText('Output of db')).toBeVisible();
    await expectInLog(app, 'database system is ready to accept connections');

    await app.bottom.getByRole('button', { name: 'Stop all' }).click();
    await expect(app.bottom).toContainText('0 of 2 up.');
    await expect(db).toContainText('stopped');
    await expect(dev).toContainText('stopped');
    await expect(app.bottom.getByRole('button', { name: 'Stop all' })).toBeDisabled();
    expect(composeCalls(app, composeProject)).toContain(`compose -p ${composeProject} -f ${taskFile} down`);
  });

  test('O-06 a port that is taken is moved to a free one, and the task says so', async ({ app, project }) => {
    const held = await holdPort();
    try {
      const shop = await project({ dbPort: held.port });
      await app.openHome();
      await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
      await openRun(app);
      const db = app.service('db');
      await db.getByRole('button', { name: 'Start' }).click();

      const address = db.getByRole('link', { name: /^localhost:\d+$/ });
      await expect(address).toBeVisible();
      const given = Number((await address.textContent())!.split(':')[1]);
      expect(given).toBeGreaterThan(held.port);
      await expect(db).toContainText(`Port ${held.port} was taken, so this task uses ${given}.`);
      // Only the service that was asked for is started.
      await expect(app.service('dev')).toContainText('stopped');

      await db.getByRole('button', { name: 'Stop' }).click();
      await expect(db).toContainText('stopped');
      await expect(db).not.toContainText('was taken');
    } finally {
      held.server.close();
    }
  });

  test('O-11 O-12 a container that stops after its start "did not start" with its code; archiving removes the containers', async ({
    app,
    project,
  }) => {
    const shop = await project({ worker: true });
    const title = unique('Describe the limiter');
    await app.openHome();
    const taskId = await app.createTask({ text: title, folder: shop.dir });
    await openRun(app);
    const composeProject = `ae-${shop.name}-${taskId.slice(0, 4)}`;
    const worker = app.service('worker');
    await expect(worker).toContainText('built from a Dockerfile');

    await app.bottom.getByRole('button', { name: 'Start all' }).click();
    await expect(app.service('db').getByRole('link', { name: /^localhost:\d+$/ })).toBeVisible();
    await expect(worker).toContainText('It stopped with code 3. The log says why.');
    await worker.getByRole('button', { name: 'Send to Practice agent' }).click();
    await expect(app.conversation.locator('.quote')).toContainText('service worker line 1');

    // Archiving the task takes its containers and volumes with it.
    await app.goHome();
    await app.stageRow('Your turn', title).getByRole('button', { name: 'Archive' }).click();
    await expect(app.page.getByRole('main').getByRole('link', { name: title })).toHaveCount(0);
    await expect
      .poll(() => composeCalls(app, composeProject).filter((call) => call.endsWith('down --volumes --remove-orphans')))
      .toHaveLength(1);
  });

  test('a project with nothing to run says so', async ({ app, project }) => {
    const shop = await project({ git: false });
    writeFileSync(path.join(shop.dir, 'package.json'), '{ "name": "shop", "scripts": { "build": "true" } }\n');
    rmSync(path.join(shop.dir, 'compose.yaml'));
    await app.openHome();
    await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    await app.openBottom('Run');
    await expect(app.bottom).toContainText(
      'Nothing to run was found: no dev script in package.json and no compose file.',
    );
    await expect(app.bottom.getByRole('button', { name: 'Start all' })).toHaveCount(0);
  });
});

test.describe('Docker that is not running', () => {
  // The stand-in's engine is switched off for the whole core, so this test gets a core of its own.
  test.use({ freshCore: true });

  test('O-10 when Docker is not running the tab says so and refuses to start the services', async ({
    app,
    project,
  }) => {
    writeFileSync(path.join(app.core.toolsDir, 'state', 'docker', 'engine-down'), '');
    const shop = await project();
    await app.openHome();
    await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    await openRun(app);
    await expect(app.bottom).toContainText(
      'Docker is installed but not running. Start Docker Desktop, then try again.',
    );

    await app.service('db').getByRole('button', { name: 'Start' }).click();
    await expect(app.bottom.getByRole('alert')).toContainText('Docker is installed but not running.');
    await expect(app.service('db')).toContainText('stopped');
    // The dev script needs no Docker and still starts.
    await app.service('dev').getByRole('button', { name: 'Start' }).click();
    await expect(app.service('dev').getByRole('link', { name: 'localhost:4567' })).toBeVisible();
    expect(app.core.calls('docker').filter((call) => call.includes(' up -d'))).toEqual([]);
  });
});

test.describe('Leaving a task', () => {
  test('Home shows after the Run tab was open in a task', async ({ app, project }) => {
    const shop = await project();
    const title = unique('Describe the limiter');
    await app.openHome();
    await app.createTask({ text: title, folder: shop.dir });
    await openRun(app);
    await expect(app.service('dev')).toBeVisible();

    await app.page.getByRole('link', { name: /^Home/ }).click();
    await expect(app.page.getByRole('textbox', { name: 'New task' })).toBeVisible();
    await expect(app.conversation).toHaveCount(0);
    await expect(app.stageRow('Your turn', title)).toBeVisible();
    // And the same from the task opened anew, to another section.
    await app.stageRow('Your turn', title).getByRole('link', { name: title }).click();
    await openRun(app);
    await expect(app.service('dev')).toBeVisible();
    await app.page.getByRole('link', { name: 'Issues', exact: true }).click();
    await expect(app.page.getByRole('heading', { level: 1, name: 'Issues' })).toBeVisible();
    await expect(app.conversation).toHaveCount(0);
  });
});
