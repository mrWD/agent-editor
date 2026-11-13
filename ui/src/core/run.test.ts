import { describe, expect, it } from 'vitest';
import {
  canReset,
  databaseWords,
  depsButton,
  depsWords,
  diskWords,
  envOwn,
  envSource,
  readFrom,
  shownServices,
  size,
} from './run';
import type { RunDependencies, RunService, RunState } from './store';

const service = (name: string, more: Partial<RunService> = {}): RunService => ({
  name,
  kind: 'command',
  runs: `npm run ${name}`,
  state: 'stopped',
  address: null,
  note: null,
  termId: null,
  web: true,
  inProfile: true,
  ...more,
});

const run = (services: RunService[], composeFile: string | null = null): RunState => ({
  services,
  composeFile,
  composeProject: 'ae-shop-1a2b',
  dockerReady: true,
  dockerHint: null,
  problem: null,
  profiles: [],
  profile: 'Full stack',
  dependencies: null,
  databases: [],
});

const deps = (state: RunDependencies['state'], more: Partial<RunDependencies> = {}): RunDependencies => ({
  command: 'npm ci',
  lockFile: 'package-lock.json',
  state,
  installedAt: null,
  note: null,
  termId: null,
  ...more,
});

describe('a size in plain words', () => {
  it('is said in MB up to a thousand and in GB from there', () => {
    expect(size(18_000_000)).toBe('18 MB');
    expect(size(125_829_120)).toBe('126 MB');
    expect(size(999_400_000)).toBe('999 MB');
    expect(size(999_600_000)).toBe('1.0 GB');
    expect(size(1_700_000_000)).toBe('1.7 GB');
    expect(size(2_576_000_000)).toBe('2.6 GB');
  });

  it('does not round something small down to nothing', () => {
    expect(size(400_000)).toBe('under 1 MB');
    expect(size(0)).toBe('under 1 MB');
  });
});

describe('the rows of the Run tab', () => {
  it('are what the profile starts and whatever else is not at rest', () => {
    const shown = shownServices(
      run([
        service('dev'),
        service('api', { inProfile: false }),
        service('worker', { inProfile: false, state: 'running' }),
        service('mail', { inProfile: false, state: 'failed' }),
      ]),
    );
    expect(shown.map((one) => one.name)).toEqual(['dev', 'worker', 'mail']);
  });

  it('say which files they were read from', () => {
    expect(readFrom(run([service('dev'), service('db', { kind: 'docker' })], 'compose.yaml'))).toBe(
      'compose.yaml and package.json',
    );
    expect(readFrom(run([service('db', { kind: 'docker' })], 'docker-compose.yml'))).toBe('docker-compose.yml');
    expect(readFrom(run([service('dev')]))).toBe('package.json');
  });
});

describe('the Environment card', () => {
  it('says which file the values come from', () => {
    expect(envSource({ files: ['.env', '.env.example'], values: [], own: 0 })).toBe('.env read from the project.');
    expect(envSource({ files: ['.env.example'], values: [], own: 0 })).toBe(
      '.env.example read from the project: it has no .env.',
    );
    expect(envSource({ files: [], values: [], own: 2 })).toBe('The project has no .env.');
  });

  it('counts what is set for the task alone', () => {
    expect(envOwn({ files: [], values: [], own: 0 })).toBe('Nothing is set for this task only.');
    expect(envOwn({ files: [], values: [], own: 1 })).toBe('1 value is set for this task only.');
    expect(envOwn({ files: [], values: [], own: 3 })).toBe('3 values are set for this task only.');
  });
});

describe('the Dependencies card', () => {
  it('says what will run before it runs', () => {
    expect(depsWords(deps('missing'), null)).toEqual({
      before: 'Not installed in this task yet. Starting runs',
      after: 'first.',
    });
    expect(depsWords(deps('stale'), null).before).toBe(
      'package-lock.json changed after the last install. Starting runs',
    );
    expect(depsWords(deps('installed'), '4 min ago')).toEqual({
      before: 'Installed 4 min ago. Reinstall runs',
      after: 'again.',
    });
    expect(depsWords(deps('failed', { note: 'It stopped with code 1.' }), null)).toEqual({
      before: '',
      after: 'did not finish. It stopped with code 1.',
    });
    expect(depsWords(deps('none'), null).before).toBe(
      'package.json names no packages, so there is nothing to install.',
    );
  });

  it('offers the step that makes sense', () => {
    expect(depsButton(deps('missing'))).toBe('Install');
    expect(depsButton(deps('stale'))).toBe('Install');
    expect(depsButton(deps('failed'))).toBe('Try again');
    expect(depsButton(deps('installed'))).toBe('Reinstall');
    expect(depsButton(deps('installing'))).toBeNull();
    expect(depsButton(deps('none'))).toBeNull();
  });
});

describe('the Database card', () => {
  const own = { name: 'db', image: 'postgres:16', volumes: [{ name: 'ae-shop-1a2b_dbdata', own: true }], seed: true };

  it('names the volume of the task, with its size when Docker gave one', () => {
    expect(databaseWords(own, undefined)).toBe('Its own volume, ae-shop-1a2b_dbdata.');
    expect(databaseWords(own, { 'ae-shop-1a2b_dbdata': 84_200_000 })).toBe(
      'Its own volume, ae-shop-1a2b_dbdata. 84 MB.',
    );
    // A new database holds next to nothing, and that is said as a sentence too.
    expect(databaseWords(own, { 'ae-shop-1a2b_dbdata': 0 })).toBe('Its own volume, ae-shop-1a2b_dbdata. Under 1 MB.');
    const two = { ...own, volumes: [...own.volumes, { name: 'ae-shop-1a2b_wal', own: true }] };
    // One size of two is not the size of both.
    expect(databaseWords(two, { 'ae-shop-1a2b_dbdata': 84_200_000 })).toBe(
      'Its own volumes, ae-shop-1a2b_dbdata and ae-shop-1a2b_wal.',
    );
    expect(canReset(own)).toBe(true);
  });

  it('says so when the data is not the task’s alone, or is in no volume at all', () => {
    const shared = { ...own, volumes: [{ name: 'shop-data', own: false }] };
    expect(databaseWords(shared, undefined)).toContain('names the volume shop-data itself');
    expect(canReset(shared)).toBe(false);
    const none = { ...own, volumes: [] };
    expect(databaseWords(none, undefined)).toContain('gives it no volume');
    expect(canReset(none)).toBe(true);
  });
});

describe('Docker on disk', () => {
  it('is said in the words of the card, and not at all when Docker did not say', () => {
    expect(diskWords({ images: 2_200_000_000, volumes: 900_000_000, tasks: 4 })).toBe(
      'Docker on disk: images 2.2 GB, shared by all tasks. Volumes 900 MB in 4 tasks.',
    );
    expect(diskWords({ images: 2_576_000_000, volumes: 84_200_000, tasks: 1 })).toBe(
      'Docker on disk: images 2.6 GB, shared by all tasks. Volumes 84 MB in 1 task.',
    );
    expect(diskWords({ images: 2_576_000_000, volumes: 0, tasks: 0 })).toBe(
      'Docker on disk: images 2.6 GB, shared by all tasks. No task has a volume.',
    );
    expect(diskWords({ images: null, volumes: 84_200_000, tasks: 2 })).toBe(
      'Docker on disk: volumes 84 MB in 2 tasks.',
    );
    expect(diskWords({ images: null, volumes: 0, tasks: 0 })).toBeNull();
    expect(diskWords(null)).toBeNull();
    expect(diskWords(undefined)).toBeNull();
  });
});
