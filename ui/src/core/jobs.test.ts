import { describe, expect, it } from 'vitest';
import {
  argsHint,
  argsProblem,
  matchingJobs,
  runForAgent,
  runWords,
  workerOf,
  workerWords,
  type Job,
  type JobRun,
} from './jobs';

const job = (over: Partial<Job> = {}): Job => ({
  id: 'celery:shop.tasks.send_receipt',
  kind: 'celery',
  name: 'shop.tasks.send_receipt',
  file: 'shop/tasks.py',
  line: 6,
  takes: ['order_id', 'copy_to=None', '**options'],
  changed: false,
  folder: '',
  ...over,
});

const run = (over: Partial<JobRun> = {}): JobRun => ({
  running: false,
  termId: null,
  exit: 0,
  output: 'sent\n',
  ...over,
});

describe('the arguments of a job', () => {
  it('are hinted at in the shape they are typed in', () => {
    expect(argsHint(job())).toBe('[order_id]');
    expect(argsHint(job({ takes: [] }))).toBe('[]');
    expect(argsHint(job({ kind: 'django', takes: ['--older-than', '--dry-run'] }))).toBe('--older-than --dry-run');
    expect(argsHint(job({ kind: 'script', takes: [] }))).toContain('as you would type it');
  });

  it('are checked before they are sent to a queue', () => {
    expect(argsProblem('celery', '', '')).toBeNull();
    expect(argsProblem('celery', '[42, "tea"]', '{"force": true}')).toBeNull();
    expect(argsProblem('celery', '[42', '')).toBe('The arguments are not JSON yet');
    expect(argsProblem('celery', '42', '')).toBe('The arguments are a JSON list, such as [42, "tea"]');
    expect(argsProblem('celery', '', '[1]')).toBe('The named arguments are a JSON object, such as {"force": true}');
    // A command takes whatever is typed.
    expect(argsProblem('django', '--older-than [', '')).toBeNull();
  });
});

describe('a run of a job', () => {
  it('is said in a few words', () => {
    expect(runWords(null)).toBeNull();
    expect(runWords(run({ exit: null, output: null }))).toBeNull();
    expect(runWords(run({ running: true, exit: null }))).toEqual({ text: 'Running', tone: 'plain' });
    expect(runWords(run())).toEqual({ text: 'Ended well', tone: 'good' });
    expect(runWords(run({ exit: 4 }))).toEqual({ text: 'Ended with code 4', tone: 'bad' });
    expect(runWords(run({ exit: null, stopped: true }))).toEqual({ text: 'Stopped', tone: 'plain' });
    expect(runForAgent(job(), 'x', run({ exit: null, stopped: true })).text).toMatch(/It was stopped by hand\.$/);
  });

  it('goes to the agent with what was started and what it printed', () => {
    expect(
      runForAgent(job(), 'celery call shop.tasks.send_receipt', run({ exit: 1, output: 'Traceback\nKeyError\n' })),
    ).toEqual({
      title: 'shop.tasks.send_receipt failed',
      text: [
        'Celery task shop.tasks.send_receipt (shop/tasks.py:6)',
        '$ celery call shop.tasks.send_receipt',
        '',
        'Traceback\nKeyError',
        '',
        'It ended with code 1.',
      ].join('\n'),
    });
    expect(runForAgent(job(), 'x', run({ output: '' })).text).toContain('(it printed nothing)');
    expect(runForAgent(job(), 'x', run()).title).toBe('shop.tasks.send_receipt ran');
  });
});

describe('the worker and the list', () => {
  const service = (name: string, runs: string, state: string) => ({ name, runs, state });

  it('finds the service that does the queued jobs, a running one before a stopped one', () => {
    const all = [
      service('web', 'vite', 'running'),
      service('worker', 'node worker.js', 'stopped'),
      service('tasks', 'celery -A shop worker', 'running'),
    ];
    expect(workerOf(all)?.name).toBe('tasks');
    expect(workerOf(all.slice(0, 2))?.name).toBe('worker');
    expect(workerOf([all[0]!])).toBeNull();
  });

  it('says who will do a queued job', () => {
    expect(workerWords(null)).toMatch(/^No worker is among what this task runs/);
    expect(workerWords({ name: 'worker', state: 'running' })).toBe(
      'The worker worker is running: what the job prints there is in Run.',
    );
    expect(workerWords({ name: 'worker', state: 'exited' })).toMatch(/is not running\. Start it in Run/);
  });

  it('finds jobs by every word typed', () => {
    const all = [
      job(),
      job({
        id: 'django:expire_carts',
        kind: 'django',
        name: 'expire_carts',
        file: 'shop/management/commands/expire_carts.py',
      }),
    ];
    expect(matchingJobs(all, '')).toHaveLength(2);
    expect(matchingJobs(all, 'django expire').map((one) => one.name)).toEqual(['expire_carts']);
    expect(matchingJobs(all, 'celery receipt').map((one) => one.name)).toEqual(['shop.tasks.send_receipt']);
  });
});
