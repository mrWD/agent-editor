import { createStore } from 'solid-js/store';
import type { RunDatabase, RunDependencies, RunResources, RunService, RunState, TaskEnv } from './store';

/**
 * How many of a task's services are up, for the label of the Run tab. Known
 * once the tab was opened: before that nobody asked the core.
 */
const [counts, setCounts] = createStore<Record<string, { up: number; of: number } | undefined>>({});

export const runCount = (taskId: string) => counts[taskId];

export function setRunCount(taskId: string, up: number, of: number) {
  setCounts(taskId, of > 0 ? { up, of } : undefined);
}

/**
 * A size in plain words: "18 MB", "310 MB", "1.7 GB". In thousands, like the
 * sizes Docker prints for its disk, so the same number reads the same everywhere.
 */
export function size(bytes: number): string {
  const mb = bytes / 1e6;
  if (mb < 1) return 'under 1 MB';
  // 999.6 MB would round to "1000 MB": from there on it is said in GB.
  if (mb < 999.5) return `${Math.round(mb)} MB`;
  return `${(bytes / 1e9).toFixed(1)} GB`;
}

/** "3 values" or "1 value". */
export function count(n: number, one: string, many = `${one}s`): string {
  return `${n} ${n === 1 ? one : many}`;
}

/** The rows of the table: what the chosen profile starts, and whatever else is not at rest. */
export function shownServices(run: RunState): RunService[] {
  return run.services.filter((service) => service.inProfile || service.state !== 'stopped');
}

/** Where the list of things to run was read from: "compose.yaml and package.json". */
export function readFrom(run: RunState): string {
  const files = [run.composeFile, run.services.some((service) => service.kind === 'command') ? 'package.json' : null];
  return files.filter(Boolean).join(' and ');
}

/** The first line of the Environment card: which file of the project the values come from. */
export function envSource(env: TaskEnv): string {
  if (env.files.includes('.env')) return '.env read from the project.';
  if (env.files.length > 0) return `${env.files[0]} read from the project: it has no .env.`;
  return 'The project has no .env.';
}

/** The second line: how many values are the task's own. */
export function envOwn(env: TaskEnv): string {
  if (env.own === 0) return 'Nothing is set for this task only.';
  return `${count(env.own, 'value')} ${env.own === 1 ? 'is' : 'are'} set for this task only.`;
}

/**
 * What the Dependencies card says, in two halves: the command that installs
 * stands between them. `ago` is "4 min ago" for the last install.
 */
export function depsWords(deps: RunDependencies, ago: string | null): { before: string; after: string } {
  switch (deps.state) {
    case 'none':
      return { before: 'package.json names no packages, so there is nothing to install.', after: '' };
    case 'missing':
      return { before: 'Not installed in this task yet. Starting runs', after: 'first.' };
    case 'stale':
      return {
        before: `${deps.lockFile ?? 'The lock file'} changed after the last install. Starting runs`,
        after: 'first.',
      };
    case 'installing':
      return { before: 'Installing:', after: '' };
    case 'failed':
      return { before: '', after: `did not finish. ${deps.note ?? ''}`.trim() };
    case 'installed':
      return { before: `Installed${ago ? ` ${ago}` : ''}. Reinstall runs`, after: 'again.' };
  }
}

/** What the button of the Dependencies card says, or null when there is nothing to do. */
export function depsButton(deps: RunDependencies): string | null {
  switch (deps.state) {
    case 'none':
    case 'installing':
      return null;
    case 'installed':
      return 'Reinstall';
    case 'failed':
      return 'Try again';
    default:
      return 'Install';
  }
}

/**
 * Where a database keeps its data, in a sentence. `sizes` are the sizes of
 * this task's volumes, where Docker gave them.
 */
export function databaseWords(database: RunDatabase, sizes: Record<string, number> | undefined): string {
  const shared = database.volumes.find((volume) => !volume.own);
  if (shared) {
    return `The compose file names the volume ${shared.name} itself, so every task uses the same one. It is not reset from here.`;
  }
  if (database.volumes.length === 0) {
    return 'The compose file gives it no volume: its data lives in its container and goes when the container is removed.';
  }
  const names = database.volumes.map((volume) => volume.name);
  const known = names.map((name) => sizes?.[name]).filter((bytes): bytes is number => bytes !== undefined);
  // A size is said only when every volume has one: half a sum would read as the whole.
  const sum = size(known.reduce((all, bytes) => all + bytes, 0));
  // It is a sentence of its own, so "under 1 MB" starts with a capital.
  const total = known.length === names.length ? ` ${sum[0]!.toUpperCase()}${sum.slice(1)}.` : '';
  return `${names.length === 1 ? 'Its own volume' : 'Its own volumes'}, ${names.join(' and ')}.${total}`;
}

/** Whether a database can be reset from the task: not when its volume is another task's too. */
export function canReset(database: RunDatabase): boolean {
  return database.volumes.every((volume) => volume.own);
}

/** Docker's space on disk, in the words of the card. Null when Docker did not say. */
export function diskWords(disk: RunResources['disk']): string | null {
  if (!disk || (disk.images === null && disk.tasks === 0)) return null;
  const volumes =
    disk.tasks === 0 ? 'no task has a volume' : `volumes ${size(disk.volumes)} in ${count(disk.tasks, 'task')}`;
  if (disk.images === null) return `Docker on disk: ${volumes}.`;
  return `Docker on disk: images ${size(disk.images)}, shared by all tasks. ${volumes[0]!.toUpperCase()}${volumes.slice(1)}.`;
}
