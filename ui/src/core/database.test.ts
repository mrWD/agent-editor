import { describe, expect, it } from 'vitest';
import {
  dangers,
  migrationWords,
  pickDatabase,
  rowsForAgent,
  selectAll,
  trialForAgent,
  trialVerdict,
  type DbRows,
  type Migration,
  type TaskDatabase,
  type Trial,
  type TrialStep,
} from './database';

const database = (over: Partial<TaskDatabase>): TaskDatabase => ({
  id: 'docker:db',
  name: 'db (postgres)',
  engine: 'postgres',
  via: 'docker',
  ready: true,
  note: null,
  shared: false,
  ...over,
});

const migration = (change: Migration['change'], risky = 0): Migration => ({
  path: `migrations/${change}.sql`,
  change,
  steps: [
    { text: 'Creates table `gifts`', danger: null },
    ...Array.from({ length: risky }, () => ({ text: 'Drops `orders.total`', danger: 'What it held is gone.' })),
  ],
  wayBack: null,
});

const step = (name: TrialStep['name'], ok = true, output = ''): TrialStep => ({
  name,
  command: `make ${name}`,
  ok,
  output,
  took: 40,
});

const trial = (over: Partial<Trial>): Trial => ({
  database: 'app.db',
  steps: [step('up'), step('down'), step('up again')],
  does: ['Table `gifts` is new, with 2 columns'],
  back: 'same',
  left: [],
  again: true,
  ...over,
});

describe('the databases of a task', () => {
  it('opens on the one picked before, else on the first that can be asked', () => {
    const all = [database({ id: 'docker:db', ready: false }), database({ id: 'file:app.db', engine: 'sqlite' })];
    expect(pickDatabase(all, 'docker:db')?.id).toBe('docker:db');
    expect(pickDatabase(all, 'gone')?.id).toBe('file:app.db');
    expect(pickDatabase([all[0]!], '')?.id).toBe('docker:db');
    expect(pickDatabase([], '')).toBeNull();
  });

  it('writes the name of a table the way each database wants it', () => {
    expect(selectAll('postgres', 'Order Lines')).toBe('select * from "Order Lines" limit 50');
    expect(selectAll('sqlite', 'a"b')).toBe('select * from "a""b" limit 50');
    expect(selectAll('mysql', 'orders')).toBe('select * from `orders` limit 50');
  });
});

describe('the migrations of a task', () => {
  it('are counted by what the task did to them, and by the steps to look at twice', () => {
    expect(migrationWords([])).toBe('This task has no migrations of its own');
    const all = [migration('added', 2), migration('added'), migration('modified', 1), migration('deleted')];
    expect(migrationWords(all)).toBe('2 added, 1 changed, 1 removed');
    expect(dangers(all)).toBe(3);
  });
});

describe('a trial of the migrations', () => {
  it('is summed up by where it stopped and by what the way back left', () => {
    expect(trialVerdict(trial({}))).toEqual({
      text: 'The migrations ran, came back cleanly and ran again.',
      tone: 'good',
    });
    expect(trialVerdict(trial({ back: 'differs', left: ['Table `gifts` is new'] })).tone).toBe('bad');
    expect(trialVerdict(trial({ again: false })).text).toMatch(/^Run a second time/);
    expect(trialVerdict(trial({ back: 'unknown' })).text).toMatch(/^The migrations were in the database already/);
    expect(trialVerdict(trial({ steps: [step('up')], back: null, again: null })).text).toContain(
      'no command to take them back',
    );
    expect(trialVerdict(trial({ steps: [step('up', false)] })).text).toBe('The migrations failed.');
    expect(trialVerdict(trial({ steps: [step('up'), step('down', false)] })).text).toBe(
      'Taking the migrations back failed. The database is where the migrations left it.',
    );
    expect(trialVerdict(trial({ steps: [step('up'), step('down'), step('up again', false)] })).text).toContain(
      'a second time failed',
    );
    expect(trialVerdict(trial({ back: null, does: null, again: null })).text).toContain('no database to look at');
  });

  it('goes to the agent with what failed and what it printed', () => {
    const failed = trialForAgent(
      trial({ steps: [step('up'), step('down', false, 'column "total" does not exist')], does: null, back: null }),
    );
    expect(failed.title).toBe('the trial of the migrations failed');
    expect(failed.text).toBe(
      [
        'Taking the migrations back failed. The database is where the migrations left it.',
        '',
        'ok     up: make up',
        'FAILED down: make down',
        'column "total" does not exist',
        '',
      ].join('\n'),
    );
    const fine = trialForAgent(trial({ back: 'differs', left: ['`orders.count` is new: integer'] }));
    expect(fine.text).toContain('What the migrations did to the tables:\n- Table `gifts` is new, with 2 columns');
    expect(fine.text).toContain('What taking them back left behind:\n- `orders.count` is new: integer');
  });
});

describe('rows', () => {
  const rows = (over: Partial<DbRows> = {}): DbRows => ({
    columns: ['id', 'note'],
    rows: [
      ['1', 'tea'],
      ['2', '∅'],
    ],
    more: false,
    said: null,
    took: 3,
    ...over,
  });

  it('go to the agent as a table of plain text under the query that gave them', () => {
    expect(rowsForAgent(' select id, note from orders ', rows(), 'app.db')).toEqual({
      title: 'a query on app.db',
      text: ['select id, note from orders', '', 'id | note', '---|-----', '1  | tea', '2  | ∅', '', '2 rows'].join(
        '\n',
      ),
    });
  });

  it('are cut for the agent, and what is left out is said', () => {
    const many = rows({ rows: Array.from({ length: 45 }, (_, n) => [String(n), 'x'.repeat(200)]), more: true });
    const made = rowsForAgent('select 1', many, 'db');
    expect(made.text).toContain('… 15 more rows, and more in the database');
    expect(made.text).toContain('45+ rows');
    expect(Math.max(...made.text.split('\n').map((line) => line.length))).toBeLessThan(90);
    expect(rowsForAgent('update x', rows({ columns: [], rows: [], said: 'Done. 2 rows changed.' }), 'db').text).toBe(
      'update x\n\nDone. 2 rows changed.',
    );
  });
});
