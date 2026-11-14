import { execFileSync } from 'node:child_process';
import path from 'node:path';
import { expect, test, unique, writeIn, type App, type Project } from './support/fixtures';

/** The migration the task writes: a new table, a new column, and a column that goes. */
const GIFTS = [
  'create table gifts (id integer primary key, name text);',
  'alter table orders add column count integer not null default 1;',
  'alter table orders drop column note;',
  '',
].join('\n');

/** The same taken back, as the project's own command would do it. */
const BACK = 'drop table gifts; alter table orders drop column count; alter table orders add column note text;';

/** A shop that keeps its rows in a file of SQLite and its migrations as files of SQL. */
function withMigrations(shop: Project) {
  shop.write(
    'migrations/0001_start.sql',
    'create table orders (id integer primary key, sku text not null, note text);\n',
  );
  shop.write('.env', 'DATABASE_URL=sqlite:///app.db\n');
  shop.write('.gitignore', 'app.db\n');
  shop.git(['add', '.']);
  shop.git(['commit', '-q', '-m', 'Keep the migrations']);
}

/** A task with the task's own database made, its own migration written, and the Database of the Preview on show. */
async function openDatabase(app: App, shop: Project) {
  await app.openHome();
  const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
  const folder = await app.folderOf(taskId);
  const sql =
    "create table orders (id integer primary key, sku text not null, note text); insert into orders (sku, note) values ('tea-1', 'fragile'), ('cup-2', null);";
  execFileSync('sqlite3', [path.join(folder, 'app.db'), sql]);
  writeIn(folder, 'migrations/0002_gifts.sql', GIFTS);
  await app.showView('Preview');
  const view = app.page.getByRole('region', { name: 'Preview', exact: true });
  await view
    .getByRole('group', { name: 'What to preview on' })
    .getByRole('button', { name: 'Database', exact: true })
    .click();
  return { view, folder };
}

test.describe('The database of a task', () => {
  test('DB-01 DB-02 the migrations a task wrote are read step by step, and a trial runs them, takes them back and runs them again', async ({
    app,
    project,
  }) => {
    const shop = await project();
    withMigrations(shop);
    const { view, folder } = await openDatabase(app, shop);

    // The task's own migration is read from its text; what loses data is marked and said why.
    const migrations = view.getByRole('region', { name: 'Migrations of this task' });
    await expect(migrations.getByRole('heading', { level: 2 })).toHaveText('Migrations SQL files: 1 added');
    await expect(migrations).toContainText('1 to look at twice');
    const file = migrations.locator('.db-migrations > li');
    await expect(file).toHaveCount(1);
    await expect(file).toContainText('migrations/0002_gifts.sql');
    await expect(file.locator('.db-steps li')).toHaveText([
      'Creates table gifts',
      'Adds orders.count (integer not null default 1)',
      'Drops orders.note What it held is gone, and code that still reads it breaks.',
    ]);

    // Plain files of SQL have no command of their own: until one is given there is nothing to try.
    await expect(migrations.getByRole('button', { name: 'Try them' })).toHaveCount(0);
    await view.getByRole('button', { name: 'Set up' }).click();
    const setup = view.getByRole('form', { name: 'How the migrations are run' });
    await setup.getByLabel('Run the migrations').fill('sqlite3 app.db < migrations/0002_gifts.sql');
    await setup.getByLabel('Take them back').fill(`sqlite3 app.db "${BACK}"`);
    await setup.getByRole('button', { name: 'Save' }).click();
    await expect(setup).toHaveCount(0);

    // A trial changes the database, so it is asked for twice.
    await migrations.getByRole('button', { name: 'Try them' }).click();
    await expect(migrations.getByRole('status')).toContainText('Rows the migrations drop are lost.');
    await expect(migrations.getByRole('status')).toContainText('on app.db');
    await migrations.getByRole('button', { name: 'Run the trial' }).click();
    const trial = migrations.getByRole('region', { name: 'The trial of the migrations' });
    await expect(trial.locator('strong')).toHaveText('The migrations ran, came back cleanly and ran again.');
    await expect(trial.locator('.db-ran > li')).toHaveText([/^ok\s*up/, /^ok\s*down/, /^ok\s*up again/]);
    await expect(trial.locator('.db-steps li')).toHaveText([
      'Table gifts is new, with 2 columns',
      'orders.count is new: integer, never empty, 1 by default',
      'orders.note is gone',
    ]);
    // The database is left where the migrations lead.
    const tables = execFileSync('sqlite3', [path.join(folder, 'app.db'), '.tables'], { encoding: 'utf8' });
    expect(tables.split(/\s+/).filter(Boolean).sort()).toEqual(['gifts', 'orders']);

    // A way back that fails stops the trial there, with what the command printed, and it can be shown to the agent.
    await view.getByRole('button', { name: 'Set up' }).click();
    await setup.getByLabel('Take them back').fill('sqlite3 app.db "drop table nowhere"');
    await setup.getByRole('button', { name: 'Save' }).click();
    await migrations.getByRole('button', { name: 'Try them' }).click();
    await migrations.getByRole('button', { name: 'Run the trial' }).click();
    // The first step fails now: the migration is in the database already and cannot be run on top of itself.
    await expect(trial.locator('strong')).toHaveText('The migrations failed.');
    await expect(trial.locator('.db-ran > li')).toHaveCount(1);
    await expect(trial.locator('pre')).toContainText('already exists');
    await trial.getByRole('button', { name: 'Show it to the agent' }).click();
    const card = app.conversation.getByRole('region', { name: 'From the preview: the trial of the migrations failed' });
    await expect(card).toContainText('FAILED up: sqlite3 app.db < migrations/0002_gifts.sql');
  });

  test('DB-03 DB-04 the tables are listed, a table shows its rows, and a query changes nothing until changes are allowed', async ({
    app,
    project,
  }) => {
    const shop = await project();
    withMigrations(shop);
    const { view } = await openDatabase(app, shop);
    await expect(view.getByLabel('Database')).toHaveValue('file:app.db');

    const tables = view.getByRole('region', { name: 'Tables' });
    await expect(tables.getByRole('heading')).toHaveText('Tables 1');
    const orders = tables.locator('details', { hasText: 'orders' });
    await expect(orders).toContainText('3 columns');
    await orders.locator('summary').click();
    await expect(orders.locator('tr')).toHaveText([
      /^id\s*integer\s*may be empty/,
      /^sku\s*text\s*never empty/,
      /^note\s*text\s*may be empty/,
    ]);

    // "Rows" writes the query for the table and runs it. Nothing is not an empty text.
    await orders.getByRole('button', { name: 'Look at the rows of orders' }).click();
    const query = view.getByRole('region', { name: 'Query', exact: true });
    await expect(query.getByLabel('A query in SQL')).toHaveValue('select * from "orders" limit 50');
    const rows = query.getByRole('region', { name: 'Rows', exact: true });
    await expect(rows).toContainText('2 rows');
    const grid = rows.getByRole('region', { name: 'The rows the query gave' });
    await expect(grid.locator('th')).toHaveText(['id', 'sku', 'note']);
    await expect(grid.locator('tbody tr')).toHaveText([/^1\s*tea-1\s*fragile$/, /^2\s*cup-2\s*∅$/]);

    // A query that writes is refused until it is allowed, and the refusal says how.
    await query.getByLabel('A query in SQL').fill("update orders set note = 'checked'");
    await expect(query).toContainText('The query is run for reading only.');
    await query.getByRole('button', { name: 'Run', exact: true }).click();
    await expect(query.getByRole('alert')).toContainText('Allow changes to run one that writes.');
    await query.getByLabel('Allow changes').check();
    await expect(query).toContainText('The query may change and delete rows.');
    await query.getByRole('button', { name: 'Run', exact: true }).click();
    await expect(rows).toContainText('Done. 2 rows changed.');
    await expect(query.getByRole('alert')).toHaveCount(0);

    // What a query gave goes to the agent as a table of text.
    await query.getByLabel('A query in SQL').fill('select sku, note from orders order by id');
    await query.getByLabel('A query in SQL').press('ControlOrMeta+Enter');
    await expect(grid.locator('tbody tr')).toHaveText([/^tea-1\s*checked$/, /^cup-2\s*checked$/]);
    await rows.getByRole('button', { name: 'Show it to the agent' }).click();
    const card = app.conversation.getByRole('region', { name: 'From the preview: a query on app.db' });
    await expect(card).toContainText('sku   | note');
    await expect(card).toContainText('tea-1 | checked');

    // A query the database cannot read says so in its own words.
    await query.getByLabel('A query in SQL').fill('select * from nowhere');
    await query.getByRole('button', { name: 'Run', exact: true }).click();
    await expect(query.getByRole('alert')).toContainText('no such table: nowhere');
  });

  test('DB-05 a database that is not running says where to start it, and a project without migrations says so', async ({
    app,
    project,
  }) => {
    await app.openHome();
    await app.createTask({ text: unique('Describe the limiter'), folder: (await project()).dir });
    await app.showView('Preview');
    const view = app.page.getByRole('region', { name: 'Preview', exact: true });
    await view
      .getByRole('group', { name: 'What to preview on' })
      .getByRole('button', { name: 'Database', exact: true })
      .click();
    // The compose file of the project has a Postgres that nobody started.
    await expect(view.getByLabel('Database')).toHaveValue('docker:db');
    await expect(view.getByRole('region', { name: 'Tables' })).toContainText('It is not running. Start it in Run.');
    await expect(
      view.getByRole('region', { name: 'Query', exact: true }).getByRole('button', { name: 'Run' }),
    ).toBeDisabled();
    const migrations = view.getByRole('region', { name: 'Migrations of this task' });
    await expect(migrations).toContainText('This task has no migrations of its own');
    await expect(migrations).toContainText('No migration tool was found in this project.');
    await expect(migrations.getByRole('button', { name: 'Try them' })).toHaveCount(0);
  });
});
