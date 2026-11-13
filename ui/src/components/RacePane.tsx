import { createEffect, createSignal, For, on, Show, type JSX } from 'solid-js';
import { go, RACE_VIEW, route, taskHref } from '../app/router';
import { askReview, countWord, discardRace, entrantTasks, pickResult, useRace } from '../core/race';
import { agentOf, type Task } from '../core/store';
import { Sure } from '../screens/Changes';
import { Chip } from './bits';

/** The agent colour a button is tinted with. */
const toneOf = (agentId: string) =>
  ['claude', 'codex', 'gemini'].includes(agentId) ? `tone-${agentId}` : 'tone-plain';

/**
 * The first row of the conversation pane of a race's entrant: that this is a
 * race, and which of its agents the conversation below is with. A press on
 * another agent goes to that entrant's task and keeps the view.
 */
export function TalkingTo(props: { task: Task }): JSX.Element {
  const view = () => {
    const now = route();
    return now.page === 'task' ? now.view : 'code';
  };
  return (
    <div class="race-talk">
      <a class="race-mode" href={taskHref(props.task.info.id, RACE_VIEW)} title="The results of the race, side by side">
        Race
      </a>
      <span class="grow" />
      <div class="race-agents" role="group" aria-label="Talking to">
        <span class="race-talk-label">Talking to</span>
        <For each={entrantTasks(props.task)}>
          {(other) => {
            const name = () => agentOf(other)?.name ?? other.info.agentId;
            const current = () => other.info.id === props.task.info.id;
            return (
              <button
                type="button"
                class={`race-agent ${toneOf(other.info.agentId)}`}
                aria-pressed={current()}
                aria-label={name()}
                title={current() ? undefined : `Talk to ${name()}`}
                onClick={() => !current() && go(taskHref(other.info.id, view()))}
              >
                <Chip agentId={other.info.agentId} short={agentOf(other)?.short ?? '··'} />
                <Show when={current()}>{name()}</Show>
              </button>
            );
          }}
        </For>
      </div>
    </div>
  );
}

/**
 * The card that ends a race: take the result of the entrant on screen, ask
 * another entrant's agent for a second opinion first, or throw everything away.
 */
export function PickResult(props: { task: Task }): JSX.Element {
  const race = useRace(() => props.task);
  const raceId = () => props.task.info.race?.id;
  const taskId = () => props.task.info.id;
  const name = () => agentOf(props.task)?.name ?? props.task.info.agentId;
  const all = () => entrantTasks(props.task);
  const others = () => all().filter((one) => one.info.id !== taskId());
  /** The agent asked for a second opinion: the next one of the race after this one. */
  const reviewer = () => {
    const list = all();
    const at = list.findIndex((one) => one.info.id === taskId());
    return list.length > 1 ? list[(at + 1) % list.length] : undefined;
  };
  const reviewerName = () => (reviewer() ? (agentOf(reviewer()!)?.name ?? reviewer()!.info.agentId) : '');

  const [busy, setBusy] = createSignal<'pick' | 'review' | 'discard' | null>(null);
  const [problem, setProblem] = createSignal<string | null>(null);
  const [asked, setAsked] = createSignal<string | null>(null);
  // What was said about one entrant is not about the next one.
  createEffect(
    on(
      taskId,
      () => {
        setProblem(null);
        setAsked(null);
      },
      { defer: true },
    ),
  );

  const act = async (what: 'pick' | 'review' | 'discard', work: () => Promise<unknown>) => {
    if (busy()) return;
    setBusy(what);
    setProblem(null);
    try {
      await work();
    } catch (error) {
      setProblem(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(null);
    }
  };

  /** The entrant on screen is in the middle of a turn: there is no result of it to take yet. */
  const working = () => props.task.info.state === 'running';
  const waiting = () => props.task.info.state === 'waiting_permission';

  const explain = () => {
    const rest = others().length;
    if (waiting()) {
      return `${name()} asked before doing something and waits for your answer. Its result can be used once it has finished.`;
    }
    if (working()) return `${name()} is still working. Its result can be used once it has finished.`;
    if (rest === 0) return 'This is the only result left of the race. Using it ends the race.';
    if (race() && !race()!.over) {
      return `Not everyone has finished. Picking one stops the ${rest === 1 ? 'other' : 'others'} and removes ${rest === 1 ? 'its copy' : 'their copies'} of the code.`;
    }
    return `Picking one removes the other ${rest === 1 ? 'copy' : `${countWord(rest)} copies`} of the code.`;
  };
  const sureToPick = () => {
    const rest = others().length;
    if (rest === 0) return 'Really use it?';
    return rest === 1 ? 'Really remove the other copy?' : `Really remove the other ${countWord(rest)}?`;
  };
  const discardLabel = () => {
    const count = all().length;
    if (count === 1) return 'Discard it';
    return count === 2 ? 'Discard both' : `Discard all ${countWord(count)}`;
  };

  /** Why the second opinion cannot be asked for right now, if it cannot. */
  const noReview = () => {
    const other = reviewer();
    if (!other) return null;
    const mine = race()?.entrants.find((one) => one.taskId === taskId());
    if (mine && mine.files.length === 0) return `${name()} changed no file, so there is nothing to review.`;
    if (other.info.state === 'waiting_permission') {
      return `${reviewerName()} waits for your answer in its own conversation. Answer it first.`;
    }
    if (other.info.state === 'running')
      return `${reviewerName()} is still working. Wait for it or stop its turn first.`;
    return null;
  };

  // eslint-disable-next-line solid/reactivity -- called later, as an event handler: it reads what is current then
  const pick = () => void act('pick', () => pickResult(raceId()!, taskId()));
  const review = () =>
    // eslint-disable-next-line solid/reactivity -- called later, as an event handler: it reads what is current then
    void act('review', async () => {
      const who = reviewerName();
      await askReview(raceId()!, taskId(), reviewer()!.info.id);
      setAsked(`${who} is reviewing it. Its answer comes in its own conversation.`);
    });
  const discard = () =>
    // eslint-disable-next-line solid/reactivity -- called later, as an event handler: it reads what is current then
    void act('discard', async () => {
      await discardRace(raceId()!);
      // Every task of the race is gone now, this one too.
      go('#/');
    });

  return (
    <section class="card pinned race-pick" aria-label="Pick a result">
      <h2>Pick a result</h2>
      <p>{explain()}</p>
      <div class="actions">
        <Show when={!working() && !waiting()}>
          <Sure
            class="btn tall primary"
            label={
              busy() === 'pick'
                ? others().length === 0
                  ? 'Ending the race'
                  : 'Removing the others'
                : `Use ${name()}'s result`
            }
            sure={sureToPick()}
            about={taskId()}
            disabled={busy() !== null}
            onSure={pick}
          />
        </Show>
        <Show when={reviewer() && !working() && !waiting()}>
          <button
            type="button"
            class="btn tall"
            disabled={busy() !== null || noReview() !== null}
            title={
              noReview() ?? `${reviewerName()} gets the change as text and says what it thinks. Nothing else changes.`
            }
            onClick={review}
          >
            {busy() === 'review' ? `Asking ${reviewerName()}` : `Ask ${reviewerName()} to review it first`}
          </button>
        </Show>
        <Sure
          class="btn tall ghost"
          label={busy() === 'discard' ? 'Discarding' : discardLabel()}
          sure="Really discard?"
          about={taskId()}
          disabled={busy() !== null}
          onSure={discard}
        />
      </div>
      <Show when={asked()}>
        <span class="fine" role="status">
          {asked()}
        </span>
      </Show>
      <Show when={problem()}>
        <p class="notice danger" role="alert">
          {problem()}
        </p>
      </Show>
    </section>
  );
}
