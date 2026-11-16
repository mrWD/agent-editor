import { describe, expect, it } from 'vitest';
import {
  byStage,
  jobAct,
  needsNobody,
  quietWords,
  shares,
  stagesOf,
  stillGoing,
  summary,
  WANTS_A_LOOK,
  type CiJob,
  type CiState,
} from './ci';

const job = (name: string, stage: string, state: CiState): CiJob => ({
  id: name,
  name,
  stage,
  state,
  url: `https://ci.example/${name}`,
  allowedToFail: false,
});

describe('a pipeline told briefly', () => {
  const jobs = [
    job('lint', 'checks', 'passed'),
    job('types', 'checks', 'passed'),
    job('unit', 'test', 'failed'),
    job('e2e', 'test', 'running'),
    job('deploy', 'deploy', 'manual'),
    job('notify', 'deploy', 'skipped'),
  ];

  it('groups jobs by stage in the order the stages come', () => {
    expect(byStage(jobs).map((group) => [group.stage, group.jobs.map((one) => one.name)])).toEqual([
      ['checks', ['lint', 'types']],
      ['test', ['unit', 'e2e']],
      ['deploy', ['deploy', 'notify']],
    ]);
    // GitHub has no stages: one group, without a name.
    expect(byStage([job('a', '', 'passed'), job('b', '', 'failed')])).toHaveLength(1);
    // The names alone, each once, in the order they come: what the card keys its stages by.
    expect(stagesOf(jobs)).toEqual(byStage(jobs).map((group) => group.stage));
    expect(stagesOf([job('a', '', 'passed'), job('b', '', 'failed')])).toEqual(['']);
  });

  it('counts what failed first, and says "wait" for more than one', () => {
    expect(summary(jobs)).toBe('1 failed, 1 running, 1 waits to be started, 2 passed, 1 skipped');
    expect(summary([job('a', 's', 'manual'), job('b', 's', 'manual')])).toBe('2 wait to be started');
    expect(summary([])).toBe('no jobs');
  });

  it('gives each state its share, what failed first, and leaves out a state nobody is in', () => {
    expect(shares(jobs)).toEqual([
      { state: 'failed', count: 1 },
      { state: 'running', count: 1 },
      { state: 'manual', count: 1 },
      { state: 'passed', count: 2 },
      { state: 'skipped', count: 1 },
    ]);
    expect(shares([])).toEqual([]);
  });

  it('lists what wants a look under a word for how it stands, and nothing that needs nobody', () => {
    expect(WANTS_A_LOOK.map(([, words]) => words)).toEqual([
      'Failed',
      'Running',
      'Waiting',
      'Waits for you',
      'Stopped',
    ]);
    expect(WANTS_A_LOOK.some(([state]) => needsNobody(job('a', 's', state)))).toBe(false);
  });

  it('says what needs nobody in one line', () => {
    expect(quietWords(jobs)).toBe('2 went well, 1 skipped');
    expect(quietWords([job('a', 's', 'passed')])).toBe('1 went well');
    expect(quietWords([job('a', 's', 'skipped')])).toBe('1 skipped');
    expect(quietWords([job('a', 's', 'failed')])).toBe('');
  });

  it('lists a job that went well only when asked', () => {
    expect(jobs.filter((one) => !needsNobody(one)).map((one) => one.name)).toEqual(['unit', 'e2e', 'deploy']);
  });

  it('knows what is still going on', () => {
    expect(['running', 'pending'].map((state) => stillGoing(state as CiState))).toEqual([true, true]);
    expect(['passed', 'failed', 'manual', 'canceled', 'skipped'].some((state) => stillGoing(state as CiState))).toBe(
      false,
    );
    expect(stillGoing(undefined)).toBe(false);
  });
});

describe('what can be done to a job', () => {
  const gitlab = { runNew: true, playJob: true, cancelJob: true, retryFailed: true };
  const github = { runNew: false, playJob: false, cancelJob: false, retryFailed: true };

  it('runs a failed or stopped job again with one press', () => {
    expect(jobAct(job('unit', 'test', 'failed'), gitlab)).toEqual({
      what: 'retryJob',
      label: 'Run again',
      twice: false,
    });
    expect(jobAct(job('unit', 'test', 'canceled'), github)?.what).toBe('retryJob');
  });

  it('asks twice before starting a job that waits for a person, or stopping one that runs', () => {
    expect(jobAct(job('deploy', 'deploy', 'manual'), gitlab)).toEqual({ what: 'playJob', label: 'Start', twice: true });
    expect(jobAct(job('e2e', 'test', 'running'), gitlab)).toEqual({ what: 'cancelJob', label: 'Stop', twice: true });
  });

  it('offers nothing a host cannot do, and nothing for a job that is done', () => {
    expect(jobAct(job('deploy', '', 'manual'), github)).toBeNull();
    expect(jobAct(job('e2e', '', 'running'), github)).toBeNull();
    expect(jobAct(job('lint', 'checks', 'passed'), gitlab)).toBeNull();
    expect(jobAct(job('notify', 'deploy', 'skipped'), gitlab)).toBeNull();
  });
});
