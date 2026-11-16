import { core } from './client';

/**
 * The pipeline of a task's branch on GitLab or GitHub: what the core reads,
 * what can be done to it, and how a long list of jobs is told briefly.
 */

export type CiState = 'passed' | 'failed' | 'running' | 'pending' | 'manual' | 'canceled' | 'skipped';

export type CiJob = {
  id: string;
  name: string;
  /** GitLab's stage; empty on GitHub. */
  stage: string;
  state: CiState;
  url: string;
  /** Its failure does not fail the pipeline. */
  allowedToFail: boolean;
};

/** A GitLab pipeline, or one workflow run on GitHub. */
export type CiRun = { id: string; name: string; state: CiState; url: string; jobs: CiJob[] };

export type CiPipeline = { state: CiState; commit: string; runs: CiRun[] };

export type CiInfo = {
  host: string;
  branch: string;
  /** `null` when nothing ever ran for the branch. */
  pipeline: CiPipeline | null;
  can: { runNew: boolean; playJob: boolean; cancelJob: boolean; retryFailed: boolean };
};

/** What can be done: to a job, to a run, or to the branch. */
export type CiAct = 'retryJob' | 'playJob' | 'cancelJob' | 'retryRun' | 'retryFailed' | 'cancelRun' | 'runNew';

export function getPipeline(taskId: string) {
  return core.request<CiInfo>('ci.get', { taskId });
}

/** Does one thing and answers how the pipeline looks right after. The host may need a moment to show it. */
export function actOnPipeline(taskId: string, what: CiAct, id = '', name = '') {
  return core.request<CiInfo>('ci.act', { taskId, what, id, name });
}

/** How a state is said. */
export const CI_WORDS: Record<CiState, string> = {
  passed: 'passed',
  failed: 'failed',
  running: 'running',
  pending: 'waiting',
  manual: 'waits to be started',
  canceled: 'stopped',
  skipped: 'skipped',
};

/** Whether something is still going on, so it is worth looking again soon. */
export const stillGoing = (state: CiState | undefined) => state === 'running' || state === 'pending';

/** The jobs of a run by stage, in the order the stages come. On GitHub there is one group without a name. */
export function byStage(jobs: CiJob[]): { stage: string; jobs: CiJob[] }[] {
  const groups: { stage: string; jobs: CiJob[] }[] = [];
  for (const job of jobs) {
    const group = groups.find((one) => one.stage === job.stage);
    if (group) group.jobs.push(job);
    else groups.push({ stage: job.stage, jobs: [job] });
  }
  return groups;
}

/** The stages of a run in the order they come, each named once. On GitHub there is one, without a name. */
export function stagesOf(jobs: CiJob[]): string[] {
  return [...new Set(jobs.map((job) => job.stage))];
}

/** A job that went well, or was left out, needs nobody: it is listed only when the whole stage is unfolded. */
export const needsNobody = (job: CiJob) => job.state === 'passed' || job.state === 'skipped';

/**
 * The states that want a look, in the order a person cares, each with the words its jobs are listed
 * under. What failed comes first; what waits for a person to start it is "Waits for you".
 */
export const WANTS_A_LOOK: readonly (readonly [CiState, string])[] = [
  ['failed', 'Failed'],
  ['running', 'Running'],
  ['pending', 'Waiting'],
  ['manual', 'Waits for you'],
  ['canceled', 'Stopped'],
];

/** How many jobs stand where, in the order a person cares: what failed first. A state nobody is in is left out. */
export function shares(jobs: CiJob[]): { state: CiState; count: number }[] {
  const order: CiState[] = [...WANTS_A_LOOK.map(([state]) => state), 'passed', 'skipped'];
  return order
    .map((state) => ({ state, count: jobs.filter((job) => job.state === state).length }))
    .filter((share) => share.count > 0);
}

/** The same in words: "3 failed, 2 running, 44 passed". */
export function summary(jobs: CiJob[]): string {
  const counted = shares(jobs).map(
    ({ state, count }) =>
      `${count} ${state === 'manual' ? (count === 1 ? 'waits to be started' : 'wait to be started') : CI_WORDS[state]}`,
  );
  return counted.length > 0 ? counted.join(', ') : 'no jobs';
}

/** The jobs that need nobody, in one line to press: "32 went well, 2 skipped". Empty when there are none. */
export function quietWords(jobs: CiJob[]): string {
  const passed = jobs.filter((job) => job.state === 'passed').length;
  const skipped = jobs.filter((job) => job.state === 'skipped').length;
  return [passed > 0 ? `${passed} went well` : '', skipped > 0 ? `${skipped} skipped` : ''].filter(Boolean).join(', ');
}

/** What can be done to a job in the state it is in, on this host. */
export function jobAct(job: CiJob, can: CiInfo['can']): { what: CiAct; label: string; twice: boolean } | null {
  switch (job.state) {
    case 'failed':
    case 'canceled':
      return { what: 'retryJob', label: 'Run again', twice: false };
    // Starting a job that waits for a person may deploy something: it takes a second press.
    case 'manual':
      return can.playJob ? { what: 'playJob', label: 'Start', twice: true } : null;
    case 'running':
    case 'pending':
      return can.cancelJob ? { what: 'cancelJob', label: 'Stop', twice: true } : null;
    default:
      return null;
  }
}
