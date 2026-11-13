import { createStore, reconcile } from 'solid-js/store';
import { core } from './client';

/** One step of a team: who has the role and how the step stands. */
export type TeamStep = {
  role: 'plan' | 'build' | 'review' | 'checks';
  /** `null` for the checks, which no agent runs. */
  agentId: string | null;
  state: 'waiting' | 'running' | 'done' | 'asks' | 'skipped' | 'failed' | 'stopped';
  startedAt: number | null;
  endedAt: number | null;
  /** What the agent said at the end of its step: what the next one is handed. */
  note: string | null;
};

/** Several agents on one task, one after another, each in its role. */
export type Team = {
  text: string;
  steps: TeamStep[];
  running: boolean;
  pauseAfter: boolean;
  rounds: number;
  /** Why the team stopped, when it stopped before the end. */
  stopped: string | null;
};

const [teams, setTeams] = createStore<Record<string, { team: Team | null }>>({});

/** The team of a task. `undefined` until asked for, `null` when the task has none. */
export function teamOf(taskId: string): Team | null | undefined {
  return teams[taskId]?.team;
}

function keep(taskId: string, team: Team | null) {
  if (!teams[taskId]) setTeams(taskId, { team: null });
  setTeams(taskId, 'team', team === null ? null : reconcile(team));
}

/** Keeps a task's team current while its screen is shown. Returns what stops it. */
export function watchTeam(taskId: string): () => void {
  void core
    .request<{ team: Team | null }>('team.get', { taskId })
    .then((got) => keep(taskId, got.team))
    .catch(() => {});
  // eslint-disable-next-line solid/reactivity -- called later, as an event handler
  return core.onEvent((event) => {
    if (event.event === 'team.changed' && event.data.taskId === taskId) keep(taskId, event.data.team as Team);
  });
}

const ask = async (method: string, params: Record<string, unknown>) => {
  const got = await core.request<{ team: Team }>(method, params);
  keep(String(params.taskId), got.team);
};

export const startTeam = (taskId: string, roles: { plan: string; build: string; review: string | null }) =>
  ask('team.start', { taskId, ...roles });
export const pauseTeam = (taskId: string, on: boolean) => ask('team.pause', { taskId, on });
export const continueTeam = (taskId: string) => ask('team.continue', { taskId });
export const answerTeam = (taskId: string, back: boolean) => ask('team.answer', { taskId, back });
export const stopTeam = (taskId: string) => ask('team.stop', { taskId });

const ROLE_NAMES = { plan: 'Plan', build: 'Build', review: 'Review', checks: 'Checks' } as const;
export const roleName = (role: TeamStep['role']) => ROLE_NAMES[role];

/** How a step stands, in a word or two. `now` is the moment to count a running step up to. */
export function stepWords(step: TeamStep, now: number): string {
  const minutes = (from: number, to: number) => {
    const count = Math.max(1, Math.round((to - from) / 60_000));
    return `${count} min`;
  };
  switch (step.state) {
    case 'waiting':
      return 'waiting';
    case 'running':
      return step.startedAt ? `working, ${minutes(step.startedAt, now)}` : 'working';
    case 'done':
      return step.startedAt && step.endedAt ? minutes(step.startedAt, step.endedAt) : 'done';
    case 'asks':
      return 'asks you';
    case 'skipped':
      return 'skipped';
    case 'failed':
      return 'failed';
    case 'stopped':
      return 'stopped';
  }
}
