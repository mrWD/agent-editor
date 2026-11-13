import { chmodSync, existsSync, readFileSync, rmSync, utimesSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { freePort, type Core } from './support/core';
import { DEV_SCRIPT, expect, test, unique, type App, type Project } from './support/fixtures';

/** A key that reads as a secret by its name, and long enough to show its last four characters. */
const API_KEY = 'sk_live_0123456789abcdef';

async function openRun(app: App) {
  await app.openBottom('Run');
  await expect(app.services).toBeVisible();
}

/** How many services are up, as the label of the Run tab says it: "1 of 2". */
async function expectUp(app: App, count: string) {
  await expect(app.bottom.getByRole('button', { name: /^Run / })).toHaveText(`Run ${count}`);
}

/** One of the cards under the table, by its title. */
const card = (app: App, title: string) => app.bottom.getByRole('region', { name: title, exact: true });

/** The calls the `docker` stand-in got for one compose project. */
const composeCalls = (app: App, composeProject: string) =>
  app.core.calls('docker').filter((call) => call.includes(`-p ${composeProject} `));

/** The text of what is shown under the table: a log or a shell. */
async function expectInPanel(app: App, expected: string | RegExp) {
  const rows = app.bottom.locator('.run-log .xterm-rows');
  await rows.scrollIntoViewIfNeeded();
  await expect(rows).toContainText(expected);
}

/**
 * Gives a project a database with a volume and a seed, and a cache. The
 * stand-in for Docker names the volume after the compose project it is asked
 * about, as Docker does: `__PROJECT__` is where that name goes.
 */
async function withDatabase(shop: Project) {
  const services = {
    db: {
      image: 'postgres:16',
      ports: [{ target: 5432, published: String(await freePort()), protocol: 'tcp' }],
      volumes: [
        { type: 'volume', source: 'dbdata', target: '/var/lib/postgresql/data' },
        { type: 'bind', source: '/shop/seed', target: '/docker-entrypoint-initdb.d' },
      ],
    },
    cache: { image: 'redis:7' },
  };
  const volumes = { dbdata: { name: '__PROJECT___dbdata' } };
  shop.write('compose.yaml', JSON.stringify({ name: '__PROJECT__', services, volumes }, null, 2) + '\n');
  shop.git(['commit', '-q', '-a', '-m', 'A database with a volume']);
}

/** The volumes the stand-in for Docker "has", by name. */
const volumesOf = (core: Core) => path.join(core.toolsDir, 'state', 'docker', 'volumes');

test.describe('The Run tab as the design has it', () => {
  test('EN-01 the profile, the columns, the compose project of the task and the cards stand where the design puts them', async ({
    app,
    project,
  }) => {
    const shop = await project();
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    await openRun(app);
    const run = app.bottom;

    await expectUp(app, '0 of 2');
    await expect(run.getByLabel('Start profile')).toHaveValue('Full stack');
    await expect(run).toContainText('Read from compose.yaml and package.json.');
    await expect(run).not.toContainText('Other profiles');
    await expect(app.services.getByRole('columnheader')).toHaveText(['Service', 'Runs', 'Address', 'Memory', '']);
    await expect(run).toContainText(
      `Each task gets its own ports and its own compose project, here ae-${shop.name}-${taskId.slice(0, 4)}, so two tasks never fight over a port, a container name or a volume.`,
    );
    await expect(card(app, 'Environment').getByRole('heading', { level: 3 })).toHaveText('Environment');
    await expect(card(app, 'Environment')).toContainText('The project has no .env. Nothing is set for this task only.');
    await expect(card(app, 'Dependencies')).toContainText(
      'package.json names no packages, so there is nothing to install.',
    );
    await expect(card(app, 'Running in other tasks').getByRole('heading', { level: 3 })).toBeVisible();
    // This project's compose file gives its database no volume, and the card says what that means.
    await expect(card(app, 'Database')).toContainText(
      'The compose file gives it no volume: its data lives in its container and goes when the container is removed.',
    );
    await expect(card(app, 'Database').getByRole('button', { name: 'Reset to empty' })).toBeVisible();

    // At rest a row offers its log and a start; one button starts everything.
    const dev = app.service('dev');
    const db = app.service('db');
    await expect(dev.getByRole('button')).toHaveText(['Logs', 'Start']);
    await expect(run.getByRole('button', { name: 'Stop all' })).toBeDisabled();
    await run.getByRole('button', { name: 'Start all', exact: true }).click();
    await expectUp(app, '2 of 2');
    // Up, a command can be restarted and stopped; a container also has a shell.
    await expect(dev.getByRole('button')).toHaveText(['Logs', 'Restart', 'Stop']);
    await expect(db.getByRole('button')).toHaveText(['Logs', 'Shell', 'Restart', 'Stop']);
    await expect(dev.getByRole('img', { name: 'Running' })).toHaveCSS('background-color', 'rgb(91, 208, 138)');
    await expect(run.getByRole('button', { name: 'Start all', exact: true })).toHaveCount(0);
    await expect(run.getByRole('button', { name: 'Restart all' })).toBeVisible();

    await run.getByRole('button', { name: 'Stop all' }).click();
    await expectUp(app, '0 of 2');
  });

  test('EN-02 a launch profile names what starts together, is made, changed and removed, and is kept for the project', async ({
    app,
    project,
  }) => {
    const shop = await project();
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    await openRun(app);
    const run = app.bottom;
    const profile = run.getByLabel('Start profile');
    const composeProject = `ae-${shop.name}-${taskId.slice(0, 4)}`;

    await profile.selectOption({ label: 'New profile…' });
    const form = run.getByRole('form', { name: 'New profile' });
    await expect(form.getByLabel('Name')).toBeFocused();
    // The list goes back to the profile that is chosen: "New profile" is not one.
    await expect(profile).toHaveValue('Full stack');
    await expect(form.getByRole('checkbox', { name: 'dev' })).toBeChecked();
    await form.getByLabel('Name').fill('Database only');
    await form.getByRole('checkbox', { name: 'dev' }).uncheck();
    await form.getByRole('button', { name: 'Save the profile' }).click();

    await expect(form).toHaveCount(0);
    await expect(profile).toHaveValue('Database only');
    await expect(run).toContainText('Read from compose.yaml and package.json. Other profiles: Full stack.');
    await expect(app.service('dev')).toHaveCount(0);
    await expectUp(app, '0 of 1');

    // One button starts the profile, and nothing outside it.
    await run.getByRole('button', { name: 'Start all', exact: true }).click();
    await expect(app.service('db').getByText(/^localhost:\d+$/)).toBeVisible();
    await expectUp(app, '1 of 1');
    const taskFile = path.join(app.core.dataDir, 'run', taskId, 'compose.json');
    expect(composeCalls(app, composeProject)).toContain(
      `compose -p ${composeProject} -f ${taskFile} up -d --remove-orphans db`,
    );
    await profile.selectOption('Full stack');
    await expect(app.service('dev')).toContainText('stopped');
    await expect(run).toContainText('Other profiles: Database only.');
    await expectUp(app, '1 of 2');

    // What runs outside the chosen profile stays in sight until it is stopped.
    await profile.selectOption({ label: 'New profile…' });
    await form.getByLabel('Name').fill('Web only');
    await form.getByRole('checkbox', { name: 'db' }).uncheck();
    await form.getByRole('button', { name: 'Save the profile' }).click();
    await expect(profile).toHaveValue('Web only');
    await expect(run).toContainText('Other profiles: Full stack, Database only.');
    await expect(app.service('db')).toContainText('Not in this profile.');
    await app.service('db').getByRole('button', { name: 'Stop' }).click();
    await expect(app.service('db')).toHaveCount(0);
    await expectUp(app, '0 of 1');

    // A name that is taken is refused in words, and the form stays for another try.
    await profile.selectOption({ label: 'Change Web only…' });
    const change = run.getByRole('form', { name: 'Change the profile Web only' });
    await change.getByLabel('Name').fill('database ONLY');
    await change.getByRole('button', { name: 'Save the profile' }).click();
    await expect(run.getByRole('alert')).toContainText('There is a profile called database ONLY already');
    await change.getByLabel('Name').fill('Web');
    await change.getByRole('button', { name: 'Save the profile' }).click();
    await expect(profile).toHaveValue('Web');
    await expect(run.getByRole('alert')).toHaveCount(0);

    // The profiles are the project's: another task of it has them, with the same one chosen.
    await app.goHome();
    await app.createTask({ text: unique('Describe the limiter'), project: shop.name });
    await openRun(app);
    await expect(profile).toHaveValue('Web');
    await expect(profile.getByRole('option')).toHaveText([
      'Full stack',
      'Database only',
      'Web',
      'New profile…',
      'Change Web…',
      'Remove Web',
    ]);
    await profile.selectOption({ label: 'Remove Web' });
    await expect(profile).toHaveValue('Full stack');
    await expect(profile.getByRole('option')).toHaveText(['Full stack', 'Database only', 'New profile…']);
    await expect(app.service('db')).toBeVisible();
  });

  test('EN-06 EN-10 memory is shown where it is known; a service and everything are restarted', async ({
    app,
    project,
  }) => {
    // Each start of the dev script leaves a line, so a restart can be told from a program that just went on.
    const shop = await project({ devScript: `echo started >> starts.log; ${DEV_SCRIPT}` });
    await withDatabase(shop);
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    const starts = async () => {
      const file = path.join((await app.core.task(taskId)).cwd, 'starts.log');
      return existsSync(file) ? readFileSync(file, 'utf8').trim().split('\n').length : 0;
    };
    await openRun(app);
    const run = app.bottom;
    const [dev, db, cache] = [app.service('dev'), app.service('db'), app.service('cache')];
    const memory = (row: typeof dev) => row.getByRole('cell').nth(3);
    await expect(memory(db)).toHaveText('');

    await run.getByRole('button', { name: 'Start all', exact: true }).click();
    await expectUp(app, '3 of 3');
    // What Docker says its containers use: 120 MiB and 18 MiB, said in MB.
    await expect(memory(db)).toHaveText('126 MB');
    await expect(memory(cache)).toHaveText('19 MB');
    // What the system says the command and everything it started use.
    await expect(memory(dev)).toHaveText(/^(\d+ MB|\d\.\d GB)$/);
    // A cache has no address a browser could open, and none is made up for it.
    await expect(cache.getByRole('link')).toHaveCount(0);
    await expect.poll(starts).toBe(1);

    // One command is stopped and started again.
    await dev.getByRole('button', { name: 'Restart' }).click();
    await expect.poll(starts).toBe(2);
    await expect(dev.getByRole('link', { name: 'localhost:4567' })).toBeVisible();
    // One container is made anew.
    const composeProject = `ae-${shop.name}-${taskId.slice(0, 4)}`;
    const taskFile = path.join(app.core.dataDir, 'run', taskId, 'compose.json');
    const up = `compose -p ${composeProject} -f ${taskFile} up -d --remove-orphans`;
    await db.getByRole('button', { name: 'Restart' }).click();
    await expect.poll(() => composeCalls(app, composeProject)).toContain(`${up} --force-recreate db`);
    await expect(db.getByText(/^localhost:\d+$/)).toBeVisible();
    // And everything at once, once the start of that container has ended: two starts do not run side by side.
    await expect
      .poll(async () => {
        const listed = await app.core.request<{ terminals: { purpose: string }[] }>('terminal.list', { taskId });
        return listed.terminals.filter((terminal) => terminal.purpose === 'docker:up').length;
      })
      .toBe(0);
    await expect(run.getByRole('button', { name: 'Restart all' })).toBeEnabled();
    await run.getByRole('button', { name: 'Restart all' }).click();
    await expect.poll(starts).toBe(3);
    await expect.poll(() => composeCalls(app, composeProject)).toContain(`${up} --force-recreate`);
    await expectUp(app, '3 of 3');

    await run.getByRole('button', { name: 'Stop all' }).click();
    await expectUp(app, '0 of 3');
    await expect(memory(db)).toHaveText('');
    await expect(memory(dev)).toHaveText('');
  });

  test('EN-03 the environment is the project’s .env with the task’s own values; a secret is never shown whole', async ({
    app,
    project,
  }) => {
    // The dev script says what it was given: of a secret, how long it is rather than what it is.
    const said = `printf 'port=%s token-length=%s key-length=%s\\n' "$SHOP_PORT" "\${#SHOP_TOKEN}" "\${#SHOP_API_KEY}"`;
    const shop = await project({ devScript: `${said}; ${DEV_SCRIPT}` });
    shop.write('.env.example', 'SHOP_PORT=8080\nSHOP_API_KEY=\nSHOP_MAIL=shop@example.com\n');
    shop.git(['add', '.']);
    shop.git(['commit', '-q', '-m', 'An example of the environment']);
    // The real values are not in git: the task's working copy never gets them as a file.
    shop.write('.env', `SHOP_PORT=3000\nSHOP_API_KEY=${API_KEY}\n`);
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    await openRun(app);
    const environment = card(app, 'Environment');
    await expect(environment).toContainText('.env read from the project. Nothing is set for this task only.');

    await environment.getByRole('button', { name: 'Edit' }).click();
    const editor = app.bottom.getByRole('region', { name: 'Environment of this task' });
    const row = (name: string) => editor.getByRole('group', { name, exact: true });
    await expect(row('SHOP_PORT').getByRole('textbox')).toHaveValue('3000');
    await expect(row('SHOP_PORT')).toContainText('From .env.');
    await expect(row('SHOP_MAIL').getByRole('textbox')).toHaveValue('shop@example.com');
    await expect(row('SHOP_MAIL')).toContainText('From .env.example.');
    // A secret's field is empty and hides what is typed; all it shows of the value is its end.
    const key = row('SHOP_API_KEY').getByLabel('SHOP_API_KEY');
    await expect(key).toHaveValue('');
    await expect(key).toHaveAttribute('type', 'password');
    await expect(key).toHaveAttribute('placeholder', '••••••••cdef');

    // A value of the task's own, in place of the project's.
    await expect(row('SHOP_PORT').getByRole('button', { name: 'Set for this task' })).toBeDisabled();
    await row('SHOP_PORT').getByRole('textbox').fill('3012');
    await row('SHOP_PORT').getByRole('button', { name: 'Set for this task' }).click();
    await expect(row('SHOP_PORT')).toContainText('Set for this task. .env has 3000.');
    await expect(environment).toContainText('1 value is set for this task only.');
    await expect(environment).toContainText('SHOP_PORT=3012');
    // And one only the task has. Its name says it is a secret, so it is masked at once.
    const add = editor.getByRole('form', { name: 'Add a value for this task' });
    await add.getByLabel('Name of the new value').fill('SHOP_TOKEN');
    await add.getByLabel('Its value').fill('tok_abcdefgh');
    await add.getByRole('button', { name: 'Add for this task' }).click();
    await expect(row('SHOP_TOKEN')).toContainText('Set for this task only.');
    await expect(environment).toContainText('2 values are set for this task only.');
    await expect(environment).toContainText('SHOP_TOKEN=••••••••');
    await expect(add.getByLabel('Its value')).toHaveValue('');
    // A name no program could be given is refused in words.
    await add.getByLabel('Name of the new value').fill('2 many');
    await add.getByRole('button', { name: 'Add for this task' }).click();
    await expect(app.bottom.getByRole('alert')).toContainText('letters, digits and underscores');
    await app.bottom.getByRole('alert').getByRole('button', { name: 'Dismiss' }).click();

    // No secret reached the page whole, in what is shown or in what the core answered.
    const page = await app.page.content();
    expect(page).not.toContain(API_KEY);
    expect(page).not.toContain('tok_abcdefgh');
    const answered = JSON.stringify(await app.core.request('env.get', { taskId }));
    expect(answered).toContain('SHOP_TOKEN');
    expect(answered).not.toContain(API_KEY);
    expect(answered).not.toContain('tok_abcdefgh');
    // The task's own values are the core's to keep: nothing is written where the agent reads.
    const cwd = (await app.core.task(taskId)).cwd;
    expect(cwd).not.toBe(shop.dir);
    expect(existsSync(path.join(cwd, '.env'))).toBe(false);
    expect(existsSync(path.join(app.core.dataDir, 'run', taskId, 'env.json'))).toBe(true);

    // What the Run tab starts is given all of it: the command, and Docker for the compose file.
    await app.bottom.getByRole('button', { name: 'Start all', exact: true }).click();
    await expectUp(app, '2 of 2');
    await expect(editor).toContainText('What is running keeps the values it was started with: restart it.');
    await app.service('dev').getByRole('button', { name: 'Logs' }).click();
    await expectInPanel(app, `port=3012 token-length=12 key-length=${API_KEY.length}`);
    const composeProject = `ae-${shop.name}-${taskId.slice(0, 4)}`;
    const given = readFileSync(path.join(app.core.toolsDir, 'state', 'docker', `${composeProject}.env`), 'utf8');
    expect(given.trim().split('\n')).toEqual([
      `SHOP_API_KEY=${API_KEY}`,
      'SHOP_MAIL=shop@example.com',
      'SHOP_PORT=3012',
      'SHOP_TOKEN=tok_abcdefgh',
    ]);

    // Taking the task's own value away gives the project's back.
    await row('SHOP_PORT').getByRole('button', { name: 'Use the project’s value' }).click();
    await expect(row('SHOP_PORT').getByRole('textbox')).toHaveValue('3000');
    await expect(row('SHOP_PORT')).toContainText('From .env.');
    await row('SHOP_TOKEN').getByRole('button', { name: 'Remove' }).click();
    await expect(row('SHOP_TOKEN')).toHaveCount(0);
    await expect(environment).toContainText('Nothing is set for this task only.');
    await editor.getByRole('button', { name: 'Close' }).click();
    await expect(editor).toHaveCount(0);
    await app.core.request('run.stop', { taskId });
  });

  test('EN-05 a database keeps its data in a volume of the task alone, and is reset without touching another task’s', async ({
    app,
    project,
  }) => {
    const shop = await project();
    await withDatabase(shop);
    await app.openHome();
    const first = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    const volumeOf = (taskId: string) => `ae-${shop.name}-${taskId.slice(0, 4)}_dbdata`;
    await openRun(app);
    const database = card(app, 'Database');
    await expect(database).toContainText(`Its own volume, ${volumeOf(first)}.`);
    await expect(database).not.toContainText('MB');

    await app.service('db').getByRole('button', { name: 'Start' }).click();
    await expect(app.service('db').getByText(/^localhost:\d+$/)).toBeVisible();
    // Its size, once Docker has made it and can say.
    await expect(database).toContainText(`Its own volume, ${volumeOf(first)}. 84 MB.`);

    // A second task of the same project: the same service, another volume.
    await app.goHome();
    const second = await app.createTask({ text: unique('Describe the limiter'), project: shop.name });
    await openRun(app);
    await expect(database).toContainText(`Its own volume, ${volumeOf(second)}.`);
    await app.service('db').getByRole('button', { name: 'Start' }).click();
    await expect(app.service('db').getByText(/^localhost:\d+$/)).toBeVisible();
    await expect(database).toContainText('84 MB.');
    const kept = (taskId: string) => existsSync(path.join(volumesOf(app.core), volumeOf(taskId)));
    expect([kept(first), kept(second)]).toEqual([true, true]);

    // Resetting asks twice, then removes the container and the volume and starts the database on a new one.
    const before = app.core.calls('docker').length;
    const reset = database.getByRole('button', { name: 'Reset to seed' });
    await reset.click();
    await expect(database.getByRole('button', { name: 'Press again to throw its data away' })).toBeVisible();
    expect(
      app.core
        .calls('docker')
        .slice(before)
        .filter((call) => call.includes(' rm ')),
    ).toEqual([]);
    await database.getByRole('button', { name: 'Press again to throw its data away' }).click();
    await expect(reset).toBeEnabled();
    await expect(app.service('db').getByText(/^localhost:\d+$/)).toBeVisible();
    const composeProject = `ae-${shop.name}-${second.slice(0, 4)}`;
    const taskFile = path.join(app.core.dataDir, 'run', second, 'compose.json');
    const steps = app.core
      .calls('docker')
      .slice(before)
      .filter((call) => call.includes(' rm ') || call.startsWith('volume ') || call.includes(' up -d'));
    expect(steps).toEqual([
      `compose -p ${composeProject} -f ${taskFile} rm --stop --force --volumes db`,
      `volume rm ${volumeOf(second)}`,
      `compose -p ${composeProject} -f ${taskFile} up -d --remove-orphans db`,
    ]);
    // The other task's volume was never named, and is still there.
    expect([kept(first), kept(second)]).toEqual([true, true]);
    expect(app.core.calls('docker')).not.toContain(`volume rm ${volumeOf(first)}`);
    for (const taskId of [first, second]) await app.core.request('run.stop', { taskId });
  });

  test('EN-07 Shell opens a terminal inside the container of a running service', async ({ app, project }) => {
    const shop = await project();
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    await openRun(app);
    const db = app.service('db');
    // Only a running container has a shell to open.
    await expect(db.getByRole('button', { name: 'Shell' })).toHaveCount(0);
    await db.getByRole('button', { name: 'Start' }).click();
    await expect(db.getByText(/^localhost:\d+$/)).toBeVisible();
    await expect(app.service('dev').getByRole('button', { name: 'Shell' })).toHaveCount(0);

    await db.getByRole('button', { name: 'Shell' }).click();
    await expect(app.bottom.getByText('Shell in db')).toBeVisible();
    const composeProject = `ae-${shop.name}-${taskId.slice(0, 4)}`;
    await expectInPanel(app, `A shell in ${composeProject}-db-1`);
    expect(composeCalls(app, composeProject).filter((call) => call.includes(' exec db sh -c '))).toHaveLength(1);

    // It is a terminal to type into, not a log.
    await app.bottom.locator('.run-log').getByRole('textbox', { name: 'Terminal input' }).focus();
    await app.page.keyboard.type('echo "sum $((40 + 2))"');
    await app.page.keyboard.press('Enter');
    await expectInPanel(app, 'sum 42');

    // Closing the panel leaves the shell open: it is the same one when asked for again.
    await app.bottom.locator('.run-log').getByRole('button', { name: 'Close' }).click();
    await expect(app.bottom.getByText('Shell in db')).toBeHidden();
    await db.getByRole('button', { name: 'Shell' }).click();
    await expectInPanel(app, 'sum 42');
    expect(composeCalls(app, composeProject).filter((call) => call.includes(' exec db sh -c '))).toHaveLength(1);

    // It ends with its container.
    await db.getByRole('button', { name: 'Stop' }).click();
    await expect(db).toContainText('stopped');
    await expect(app.bottom.getByText('Shell in db')).toBeHidden();
    await expect(db.getByRole('button', { name: 'Shell' })).toHaveCount(0);
  });
});

/** A stand-in for pnpm: it installs nothing, leaves what pnpm leaves, and runs a script of package.json. */
const PNPM = String.raw`#!/bin/sh
here="$(cd "$(dirname "$0")" && pwd)"
echo "$@" >> "$here/pnpm.log"
case "$1" in
  install)
    echo "Lockfile is up to date, resolution step is skipped"
    # The test holds the install here for as long as it wants to look at it.
    while [ -f "$here/state/pnpm-wait" ]; do sleep 0.1; done
    if [ -f "$here/state/pnpm-fails" ]; then echo "ERR_PNPM_OUTDATED_LOCKFILE the lock file is not up to date"; exit 1; fi
    mkdir -p node_modules && echo "layoutVersion: 5" > node_modules/.modules.yaml
    echo "Done in 0.4s" ;;
  dev) exec sh -c "$(node -p 'require("./package.json").scripts.dev')" ;;
  *) echo "unexpected call: $*" >&2; exit 2 ;;
esac
`;

test.describe('Installing the packages of a task', () => {
  // The stand-in for pnpm is put in front of everything a login shell finds, for this core alone.
  test.use({ freshCore: true });

  test('EN-04 missing packages are installed before the start, in a terminal, and again when the lock file moves on', async ({
    app,
    project,
  }) => {
    const { core } = app;
    writeFileSync(path.join(core.toolsDir, 'pnpm'), PNPM);
    chmodSync(path.join(core.toolsDir, 'pnpm'), 0o755);
    // A login shell puts the system's own folders first. This puts the stand-ins back in front of a real pnpm.
    writeFileSync(path.join(core.root, 'home', '.profile'), `PATH="${core.toolsDir}:$PATH"\nexport PATH\n`);
    const marker = (name: string) => path.join(core.toolsDir, 'state', name);
    const calls = () =>
      existsSync(path.join(core.toolsDir, 'pnpm.log'))
        ? readFileSync(path.join(core.toolsDir, 'pnpm.log'), 'utf8').trim().split('\n')
        : [];

    const shop = await project();
    const scripts = { dev: DEV_SCRIPT };
    const dependencies = { 'left-pad': '1.3.0' };
    shop.write('package.json', JSON.stringify({ name: 'shop', private: true, scripts, dependencies }, null, 2) + '\n');
    shop.write('pnpm-lock.yaml', "lockfileVersion: '9.0'\n");
    shop.git(['add', '.']);
    shop.git(['commit', '-q', '-m', 'A package to install']);
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    await openRun(app);
    const install = 'pnpm install --frozen-lockfile';
    const deps = card(app, 'Dependencies');
    const dev = app.service('dev');

    // What will run is said before it runs.
    await expect(dev).toContainText('pnpm dev');
    await expect(deps).toContainText(`Not installed in this task yet. Starting runs ${install} first.`);
    await expect(deps.getByRole('button', { name: 'Install', exact: true })).toBeVisible();
    expect(calls()).toEqual([]);

    // Starting installs first, in a terminal that can be watched; the script waits for it.
    writeFileSync(marker('pnpm-wait'), '');
    await dev.getByRole('button', { name: 'Start' }).click();
    await expect(dev).toContainText(`Waiting for ${install} to finish.`);
    await expect(deps).toContainText(`Installing: ${install}`);
    await expect(deps.getByRole('button', { name: 'Install', exact: true })).toHaveCount(0);
    await deps.getByRole('button', { name: 'Logs' }).click();
    await expect(app.bottom.getByText(`Output of ${install}`)).toBeVisible();
    await expectInPanel(app, 'Lockfile is up to date, resolution step is skipped');
    expect(calls()).toEqual(['install --frozen-lockfile']);

    rmSync(marker('pnpm-wait'));
    await expect(dev.getByRole('link', { name: 'localhost:4567' })).toBeVisible();
    await expect(deps).toContainText(`Installed just now. Reinstall runs ${install} again.`);
    await expect(app.bottom.getByText(`Output of ${install}`)).toBeHidden();
    expect(calls()).toEqual(['install --frozen-lockfile', 'dev']);
    // Installed: stopping and starting again runs the script alone.
    await dev.getByRole('button', { name: 'Stop' }).click();
    await expect(dev).toContainText('stopped');
    await dev.getByRole('button', { name: 'Start' }).click();
    await expect(dev.getByRole('link', { name: 'localhost:4567' })).toBeVisible();
    expect(calls()).toEqual(['install --frozen-lockfile', 'dev', 'dev']);
    await dev.getByRole('button', { name: 'Stop' }).click();
    await expect(dev).toContainText('stopped');

    // The lock file is newer than the install, as it is once the branch was updated: here the install is made old.
    const modules = path.join((await core.task(taskId)).cwd, 'node_modules');
    const before = new Date(Date.now() - 600_000);
    for (const mark of [path.join(modules, '.modules.yaml'), modules]) utimesSync(mark, before, before);
    await app.openBottom('Terminal');
    await app.openBottom('Run');
    await expect(deps).toContainText(`pnpm-lock.yaml changed after the last install. Starting runs ${install} first.`);

    // An install that fails says so in its own last words, and nothing is started after it.
    writeFileSync(marker('pnpm-fails'), '');
    await dev.getByRole('button', { name: 'Start' }).click();
    await expect(deps).toContainText(
      `${install} did not finish. It stopped with code 1. Its last words: ERR_PNPM_OUTDATED_LOCKFILE the lock file is not up to date`,
    );
    await expect(dev).toContainText('stopped');
    expect(calls().filter((call) => call === 'dev')).toHaveLength(2);

    // "Try again" is the install alone.
    rmSync(marker('pnpm-fails'));
    await deps.getByRole('button', { name: 'Try again' }).click();
    await expect(deps).toContainText(`Installed just now. Reinstall runs ${install} again.`);
    await expect(deps.getByRole('button', { name: 'Reinstall' })).toBeVisible();
    await expect(dev).toContainText('stopped');
    expect(calls().filter((call) => call === 'dev')).toHaveLength(2);
  });
});

test.describe('What runs in other tasks', () => {
  // The list is of every other task of this core, so the core is this test's own.
  test.use({ freshCore: true });

  test('EN-08 EN-09 other tasks are listed with what they use; one is stopped, then everything idle', async ({
    app,
    project,
  }) => {
    const { core } = app;
    const shop = await project();
    await withDatabase(shop);
    await app.openHome();
    const titles = { idle: unique('Describe the upload'), also: unique('Describe the checkout'), busy: unique('Fix') };
    // Two tasks that run things and wait for nobody.
    const idle = await app.createTask({ text: titles.idle, folder: shop.dir });
    await core.request('run.start', { taskId: idle });
    await app.goHome();
    const also = await app.createTask({ text: titles.also, project: shop.name });
    await core.request('run.start', { taskId: also, service: 'dev' });
    // One whose agent waits for an answer: it is in use.
    await app.goHome();
    const busy = await app.createTask({ text: titles.busy, project: shop.name, until: 'question' });
    await core.request('run.start', { taskId: busy, service: 'dev' });
    // And the one on screen, which runs nothing.
    await app.goHome();
    await app.createTask({ text: unique('Describe the limiter'), project: shop.name });
    await openRun(app);

    const others = card(app, 'Running in other tasks');
    const row = (title: string) => others.getByRole('group', { name: title });
    // The heaviest first: a database, a cache and a command; then the two with one command each.
    await expect(others.getByRole('group').first()).toContainText(titles.idle);
    await expect(row(titles.idle)).toContainText(/idle \d+ min · \d+ MB/);
    await expect(row(titles.also)).toContainText(/idle \d+ min · \d+ MB/);
    await expect(row(titles.busy)).toContainText(/in use · \d+ MB/);
    await expect(others.getByText(/^\d+ MB memory$/)).toBeVisible();
    // The sum on top is the sum of the rows.
    const megabytes = async (locator: typeof others) => Number(/(\d+) MB/.exec(await locator.innerText())![1]);
    await expect(async () => {
      const rows = await Promise.all([titles.idle, titles.also, titles.busy].map((title) => megabytes(row(title))));
      const total = await megabytes(others.getByText(/^\d+ MB memory$/));
      // Each number is rounded on its own, so the sum may be off by one for each row.
      expect(Math.abs(total - rows.reduce((sum, one) => sum + one, 0))).toBeLessThanOrEqual(rows.length);
      // 126 MB of database and 19 MB of cache, and the command on top.
      expect(rows[0]).toBeGreaterThan(145);
    }).toPass();
    await expect(others).toContainText('Docker on disk: images 2.2 GB, shared by all tasks. Volumes 84 MB in 1 task.');
    // A task that is in use is not one to take things away from.
    await expect(row(titles.busy).getByRole('button')).toHaveCount(0);

    // One of them is stopped from here.
    const composeProject = `ae-${shop.name}-${idle.slice(0, 4)}`;
    await row(titles.idle).getByRole('button', { name: 'Stop' }).click();
    await expect(row(titles.idle)).toHaveCount(0);
    expect(composeCalls(app, composeProject).filter((call) => call.endsWith(' down'))).toHaveLength(1);
    const stopped = await core.request<{ services: { state: string }[] }>('run.state', { taskId: idle });
    expect(stopped.services.map((service) => service.state)).toEqual(['stopped', 'stopped', 'stopped']);
    await expect(row(titles.also)).toBeVisible();

    // Everything idle at once: asked twice, and the task in use is left as it is.
    const clean = 'Stop everything idle and remove volumes of finished tasks';
    await others.getByRole('button', { name: clean }).click();
    await expect(row(titles.also)).toBeVisible();
    await others.getByRole('button', { name: 'Press again to stop and remove' }).click();
    await expect(row(titles.also)).toHaveCount(0);
    await expect(row(titles.busy)).toContainText('in use');
    await expect(others.getByRole('group')).toHaveCount(1);
    // Nothing idle is left to stop, so the button is not offered.
    await expect(others.getByRole('button', { name: clean })).toHaveCount(0);
    const still = await core.request<{ services: { name: string; state: string }[] }>('run.state', { taskId: busy });
    expect(still.services.find((service) => service.name === 'dev')?.state).toBe('running');
    const gone = await core.request<{ services: { name: string; state: string }[] }>('run.state', { taskId: also });
    expect(gone.services.find((service) => service.name === 'dev')?.state).toBe('stopped');
    await core.request('run.stop', { taskId: busy });
    await expect(others).toContainText('Nothing is running in other tasks.');
  });
});
