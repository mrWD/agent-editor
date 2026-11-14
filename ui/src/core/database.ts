/**
 * The Database view of the Preview: what the core says about a task's
 * migrations and databases, and the words made of it. Nothing here asks the core.
 */

export type MigrationStep = { text: string; danger: string | null };

export type Migration = {
  path: string;
  change: 'added' | 'modified' | 'deleted';
  steps: MigrationStep[];
  /** Whether it says how to take itself back. `null` when its text does not tell. */
  wayBack: boolean | null;
};

export type MigrationTool = { kind: string; name: string; folder: string; up: string; down: string };

export type TaskDatabase = {
  id: string;
  name: string;
  engine: 'postgres' | 'mysql' | 'sqlite' | 'other';
  via: 'docker' | 'address' | 'file';
  ready: boolean;
  note: string | null;
  /** Whether other tasks may keep their rows in it too. */
  shared: boolean;
};

export type DbState = {
  tool: MigrationTool | null;
  /** The command that runs the migrations, and the one that takes them back. Empty when none is known. */
  up: string;
  down: string;
  migrations: Migration[];
  note: string | null;
  databases: TaskDatabase[];
  settings: { up: string; down: string };
};

export type DbColumn = { name: string; kind: string; nullable: boolean; default: string };
export type DbTable = { name: string; columns: DbColumn[]; indexes: { name: string; on: string; unique: boolean }[] };

export type DbRows = { columns: string[]; rows: string[][]; more: boolean; said: string | null; took: number };

export type TrialStep = {
  name: 'up' | 'down' | 'up again';
  command: string;
  ok: boolean;
  output: string;
  took: number;
};

export type Trial = {
  database: string | null;
  steps: TrialStep[];
  /** What the migrations did to the tables. `null` when no database could be looked at. */
  does: string[] | null;
  /** Whether taking them back left the tables as they were. */
  back: 'same' | 'differs' | 'unknown' | null;
  /** What taking them back left behind. */
  left: string[];
  /** Whether running them again gave the same tables as the first time. */
  again: boolean | null;
};

/** What stands for "nothing" in a cell, as the core writes it. */
export const NOTHING = '∅';

/** The database a view opens on: the one picked before if it is still there, else the first that can be asked. */
export function pickDatabase(databases: TaskDatabase[], picked: string): TaskDatabase | null {
  return databases.find((one) => one.id === picked) ?? databases.find((one) => one.ready) ?? databases[0] ?? null;
}

/** A query that shows the start of a table, with the name written the way the database wants it. */
export function selectAll(engine: TaskDatabase['engine'], table: string): string {
  const name = engine === 'mysql' ? `\`${table.replace(/`/g, '``')}\`` : `"${table.replace(/"/g, '""')}"`;
  return `select * from ${name} limit 50`;
}

/** "2 added, 1 changed": the migrations of a task, counted. */
export function migrationWords(migrations: Migration[]): string {
  const count = (change: Migration['change']) => migrations.filter((one) => one.change === change).length;
  const parts = [
    [count('added'), 'added'],
    [count('modified'), 'changed'],
    [count('deleted'), 'removed'],
  ] as const;
  const said = parts
    .filter(([n]) => n > 0)
    .map(([n, word]) => `${n} ${word}`)
    .join(', ');
  return said || 'This task has no migrations of its own';
}

/** How many steps of the task's migrations deserve a second look. */
export function dangers(migrations: Migration[]): number {
  return migrations.reduce((sum, one) => sum + one.steps.filter((step) => step.danger !== null).length, 0);
}

/** The step a trial stopped at, when one failed. */
export function failedStep(trial: Trial): TrialStep | null {
  return trial.steps.find((step) => !step.ok) ?? null;
}

/** What a trial came to, in a sentence, and how it stands. */
export function trialVerdict(trial: Trial): { text: string; tone: 'good' | 'bad' | 'plain' } {
  const failed = failedStep(trial);
  if (failed) {
    const where =
      failed.name === 'up'
        ? 'The migrations failed'
        : failed.name === 'down'
          ? 'Taking the migrations back failed'
          : 'Running the migrations a second time failed';
    const left =
      failed.name === 'down'
        ? ' The database is where the migrations left it.'
        : failed.name === 'up again'
          ? ' The database is where taking them back left it.'
          : '';
    return { text: `${where}.${left}`, tone: 'bad' };
  }
  if (trial.steps.length === 1)
    return { text: 'The migrations ran. There is no command to take them back, so that was not tried.', tone: 'plain' };
  if (trial.back === 'differs')
    return { text: 'The migrations ran, but taking them back did not leave the tables as they were.', tone: 'bad' };
  if (trial.again === false)
    return { text: 'Run a second time, the migrations gave other tables than the first time.', tone: 'bad' };
  if (trial.back === 'same') return { text: 'The migrations ran, came back cleanly and ran again.', tone: 'good' };
  if (trial.back === 'unknown')
    return {
      text: 'The migrations were in the database already. They were taken back and run again; whether the way back is clean shows only on a database without them.',
      tone: 'plain',
    };
  return {
    text: 'The migrations ran, were taken back and ran again. There was no database to look at the tables of.',
    tone: 'plain',
  };
}

const pad = (text: string, width: number) => text + ' '.repeat(Math.max(0, width - text.length));

/** How many rows go to the agent. More are said to be there. */
const ROWS_FOR_AGENT = 30;
const CELL_FOR_AGENT = 80;

/** Rows as a table of plain text, for the agent. */
export function rowsForAgent(sql: string, rows: DbRows, database: string): { title: string; text: string } {
  const title = `a query on ${database}`;
  if (rows.columns.length === 0) return { title, text: `${sql.trim()}\n\n${rows.said ?? 'It gave nothing.'}` };
  const cut = (cell: string) =>
    (cell.length > CELL_FOR_AGENT ? `${cell.slice(0, CELL_FOR_AGENT - 1)}…` : cell).replace(/\n/g, '⏎');
  const shown = rows.rows.slice(0, ROWS_FOR_AGENT).map((row) => row.map(cut));
  const widths = rows.columns.map((name, at) => Math.max(name.length, ...shown.map((row) => (row[at] ?? '').length)));
  const line = (cells: string[]) =>
    cells
      .map((cell, at) => pad(cell, widths[at] ?? 0))
      .join(' | ')
      .trimEnd();
  const rest = rows.rows.length - shown.length;
  const more =
    rest > 0 || rows.more
      ? [`… ${rest > 0 ? `${rest} more rows` : 'more rows'}${rows.more ? ', and more in the database' : ''}`]
      : [];
  const count = `${rows.rows.length}${rows.more ? '+' : ''} ${rows.rows.length === 1 ? 'row' : 'rows'}`;
  return {
    title,
    text: [
      sql.trim(),
      '',
      line(rows.columns),
      widths.map((width) => '-'.repeat(width)).join('-|-'),
      ...shown.map(line),
      ...more,
      '',
      count,
    ].join('\n'),
  };
}

/** A trial as text for the agent: what was run, what it printed where it failed, what it did to the tables. */
export function trialForAgent(trial: Trial): { title: string; text: string } {
  const verdict = trialVerdict(trial);
  const lines = [verdict.text, ''];
  for (const step of trial.steps) {
    lines.push(`${step.ok ? 'ok    ' : 'FAILED'} ${step.name}: ${step.command}`);
    if (!step.ok && step.output) lines.push(step.output, '');
  }
  if (trial.does && trial.does.length > 0)
    lines.push('', 'What the migrations did to the tables:', ...trial.does.map((one) => `- ${one}`));
  if (trial.does && trial.does.length === 0) lines.push('', 'The migrations changed nothing in the tables.');
  if (trial.left.length > 0)
    lines.push('', 'What taking them back left behind:', ...trial.left.map((one) => `- ${one}`));
  return {
    title: failedStep(trial) ? 'the trial of the migrations failed' : 'the trial of the migrations',
    text: lines.join('\n'),
  };
}
