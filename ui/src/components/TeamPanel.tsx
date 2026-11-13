import { createSignal, For, Show, type JSX } from 'solid-js';
import { state, type Task } from '../core/store';
import {
  answerTeam,
  continueTeam,
  pauseTeam,
  roleName,
  startTeam,
  stepWords,
  stopTeam,
  teamOf,
  type TeamStep,
} from '../core/team';
import { Check, Chip, Ring, RingDot } from './bits';

const message = (error: unknown) => (error instanceof Error ? error.message : String(error));

/**
 * The team of a task: who plans, who builds, who reviews, and how each step
 * stands. With no team yet, the form that starts one.
 */
export function TeamPanel(props: { task: Task; onClose: () => void }): JSX.Element {
  const taskId = () => props.task.info.id;
  const team = () => teamOf(taskId());
  const agents = () => state.agents.filter((agent) => agent.available);
  const nameOf = (id: string | null) => state.agents.find((agent) => agent.id === id)?.name ?? id ?? '';
  const shortOf = (id: string | null) => state.agents.find((agent) => agent.id === id)?.short ?? '··';
  const [plan, setPlan] = createSignal('');
  const [build, setBuild] = createSignal('');
  const [review, setReview] = createSignal<string | null>(null);
  const [problem, setProblem] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal(false);
  const first = () => props.task.info.agentId;
  /**
   * Whether the editor can read what an agent answers. A program in a terminal answers on its own
   * screen: it can build, and it cannot hand a plan or a verdict on.
   */
  const answers = (id: string) => state.agents.find((agent) => agent.id === id)?.speaks !== 'terminal';
  /** The task's own agent where it can take the role, else the first one that can. */
  const firstThatAnswers = () =>
    answers(first()) ? first() : (agents().find((agent) => answers(agent.id))?.id ?? first());
  const reviewer = () =>
    review() ??
    agents().find((agent) => agent.id !== (build() || first()) && answers(agent.id))?.id ??
    firstThatAnswers();
  const idle = () => props.task.info.state === 'idle' || props.task.info.state === 'asleep';

  const act = async (work: () => Promise<unknown>) => {
    if (busy()) return;
    setBusy(true);
    setProblem(null);
    try {
      await work();
    } catch (error) {
      setProblem(message(error));
    } finally {
      setBusy(false);
    }
  };

  const asking = () => team()?.steps.find((step) => step.state === 'asks');
  const builder = () => nameOf(team()?.steps.find((step) => step.role === 'build')?.agentId ?? null);
  const finished = () => team()?.steps.every((step) => step.state === 'done' || step.state === 'skipped') ?? false;
  const canGoOn = () => {
    const now = team();
    return Boolean(now) && !now!.running && !finished() && !asking();
  };

  const mark = (step: TeamStep): JSX.Element => {
    if (step.state === 'done' || step.state === 'skipped') return <Check />;
    if (step.state === 'running' || step.state === 'asks') return <RingDot />;
    return <Ring />;
  };

  const roleSelect = (
    label: string,
    value: () => string | null,
    set: (id: string) => void,
    optional = false,
    mustAnswer = false,
  ) => (
    <label class="team-role">
      <span>{label}</span>
      <select
        class="pick"
        aria-label={`Who has the role: ${label}`}
        value={value() ?? ''}
        onChange={(event) => set(event.currentTarget.value)}
      >
        <Show when={optional}>
          <option value="">Nobody</option>
        </Show>
        <For each={agents()}>
          {(agent) => (
            <option value={agent.id} disabled={mustAnswer && !answers(agent.id)}>
              {agent.name}
              {mustAnswer && !answers(agent.id) ? ' (in a terminal: it can only build)' : ''}
            </option>
          )}
        </For>
      </select>
    </label>
  );

  return (
    <section class="team" aria-label="Team">
      <Show when={problem()}>
        <p class="notice danger" role="alert">
          {problem()}
        </p>
      </Show>
      <Show
        when={team()}
        fallback={
          <>
            <span class="fine">
              Several agents on this task, one after another. A new agent is told what was said so far, and what the one
              before it handed over.
            </span>
            {roleSelect(
              'Plans',
              () => plan() || firstThatAnswers(),
              (id) => setPlan(id),
              false,
              true,
            )}
            {roleSelect(
              'Builds',
              () => build() || first(),
              (id) => setBuild(id),
            )}
            {roleSelect('Reviews', reviewer, (id) => setReview(id), true, true)}
            <span class="fine">Then the task's checks run.</span>
            <div class="actions">
              <button
                type="button"
                class="btn primary"
                disabled={busy() || !idle()}
                title={idle() ? undefined : 'Wait for the turn to end first'}
                onClick={() =>
                  act(() =>
                    startTeam(taskId(), {
                      plan: plan() || firstThatAnswers(),
                      build: build() || first(),
                      review: review() === '' ? null : reviewer(),
                    }),
                  )
                }
              >
                Start the team
              </button>
              <button type="button" class="btn ghost" onClick={() => props.onClose()}>
                Not now
              </button>
            </div>
          </>
        }
      >
        {(now) => (
          <>
            <For each={now().steps}>
              {(step) => (
                <div class="team-step" classList={{ current: step.state === 'running' || step.state === 'asks' }}>
                  {mark(step)}
                  <span class="team-role-name">{roleName(step.role)}</span>
                  <Show when={step.agentId} fallback={<span class="grow soft">Lint, tests and the rest</span>}>
                    <span class="grow team-agent">
                      <Chip agentId={step.agentId!} short={shortOf(step.agentId)} flat />
                      {nameOf(step.agentId)}
                    </span>
                  </Show>
                  <span class="fine" classList={{ 'text-bad': step.state === 'failed' }}>
                    {stepWords(step, Date.now())}
                  </span>
                </div>
              )}
            </For>
            <Show when={now().stopped}>
              <p class="fine">{now().stopped}</p>
            </Show>
            <Show when={asking()}>
              {(step) => (
                <div class="card needs team-ask">
                  <h3>
                    {nameOf(step().agentId)} asks: send it back to {builder()} or leave it for later?
                  </h3>
                  <p class="team-found">{step().note}</p>
                  <div class="actions">
                    <button
                      type="button"
                      class="btn primary"
                      disabled={busy()}
                      onClick={() => act(() => answerTeam(taskId(), true))}
                    >
                      Send back to {builder()}
                    </button>
                    <button
                      type="button"
                      class="btn"
                      disabled={busy()}
                      onClick={() => act(() => answerTeam(taskId(), false))}
                    >
                      Leave for later
                    </button>
                  </div>
                </div>
              )}
            </Show>
            <div class="team-controls">
              <Show when={now().running}>
                <label class="check">
                  <input
                    type="checkbox"
                    checked={now().pauseAfter}
                    onChange={(event) => act(() => pauseTeam(taskId(), event.currentTarget.checked))}
                  />
                  Pause after this step
                </label>
                <span class="grow" />
                <button type="button" class="btn tiny" disabled={busy()} onClick={() => act(() => stopTeam(taskId()))}>
                  Stop the team
                </button>
              </Show>
              <Show when={canGoOn()}>
                <span class="grow" />
                <button
                  type="button"
                  class="btn tiny"
                  disabled={busy()}
                  onClick={() => act(() => continueTeam(taskId()))}
                >
                  Go on with the next step
                </button>
              </Show>
              <Show when={finished()}>
                <span class="grow fine">The team has finished.</span>
              </Show>
            </div>
          </>
        )}
      </Show>
    </section>
  );
}
