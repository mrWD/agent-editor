import { createEffect, createSignal, For, Match, Show, Switch, type JSX } from 'solid-js';
import { go, RACE_VIEW, taskHref } from '../app/router';
import { ago, Chip } from '../components/bits';
import {
  checksWord,
  countWord,
  diffsOf,
  entrantTasks,
  failedNote,
  filesWord,
  loadDiffs,
  raceNotice,
  raceProblem,
  raceTime,
  setRaceNotice,
  SHOWN_FILES,
  useRace,
  type Entrant,
  type RaceChecks,
} from '../core/race';
import { raceAgain, type FileDiff, type Task } from '../core/store';
import { openInEditor } from '../core/work';
import { DiffRows, FileCount } from './Span';

const message = (error: unknown) => (error instanceof Error ? error.message : String(error));
const capital = (word: string) => word.charAt(0).toUpperCase() + word.slice(1);

/**
 * The race's own line in the sub bar of an entrant: what the race is, how it
 * stands, and a way to run it once more.
 */
export function RaceHead(props: { task: Task }): JSX.Element {
  const race = useRace(() => props.task);
  const [starting, setStarting] = createSignal(false);
  const count = () => race()?.entrants.length ?? entrantTasks(props.task).length;

  const what = () =>
    count() > 1
      ? `Same task, ${countWord(count())} agents, each in its own copy of the code`
      : 'The same task went to several agents. One result is left.';

  /** Where the race stands, in a few words. */
  const standing = () => {
    const known = race();
    if (!known) return '';
    const waiting = known.entrants.filter((one) => one.state === 'waiting_permission').length;
    const working = known.entrants.filter((one) => one.state === 'running').length;
    if (waiting > 0) return `${capital(countWord(waiting))} ${waiting === 1 ? 'waits' : 'wait'} for your answer`;
    if (working > 0) return `${capital(countWord(working))} ${working === 1 ? 'is' : 'are'} still working`;
    return known.finishedAt === null ? 'Nobody has finished a turn' : `Finished ${ago(known.finishedAt)}`;
  };

  const again = async () => {
    const raceId = props.task.info.race?.id;
    if (!raceId || starting()) return;
    setStarting(true);
    try {
      const started = await raceAgain(raceId);
      const note = failedNote(started);
      // The new race is shown at once; the old one stays where it was, to pick from or to discard.
      if (started.race) {
        setRaceNotice(started.race.id, note);
        go(taskHref(started.tasks[0]!.id, RACE_VIEW));
      } else {
        setRaceNotice(raceId, note);
      }
    } catch (error) {
      setRaceNotice(raceId, message(error));
    } finally {
      setStarting(false);
    }
  };

  return (
    <>
      <span class="race-line race-what" title={what()}>
        {what()}
      </span>
      <span class="race-end">
        <span class="race-line" role="status">
          {standing()}
        </span>
        <button type="button" class="btn race-again" disabled={starting()} onClick={() => void again()}>
          {starting() ? 'Starting' : 'Run the race again'}
        </button>
      </span>
    </>
  );
}

/** How the checks of one result stand. A press opens them. */
function ChecksPill(props: { taskId: string; checks: RaceChecks }): JSX.Element {
  const word = () => checksWord(props.checks);
  return (
    <a
      class={`race-pill ${word().tone}`}
      href={taskHref(props.taskId, 'checks')}
      title="Open the checks of this result"
    >
      <Show when={word().tone !== 'plain'}>
        <svg
          width="12"
          height="12"
          viewBox="0 0 16 16"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <path d={word().tone === 'good' ? 'M3.5 8.5l3 3 6-6.5' : 'M8 4v5M8 12v.5'} />
        </svg>
      </Show>
      {word().text}
    </a>
  );
}

/** One agent's result: who, how long, what it says it did, its checks and how much it changed. */
function Card(props: { one: Entrant; shown: boolean }): JSX.Element {
  const working = () => props.one.state === 'running';
  const waiting = () => props.one.state === 'waiting_permission';
  const time = () => {
    if (working()) return 'working';
    if (waiting()) return 'waits for you';
    return props.one.ms === null ? 'not finished' : raceTime(props.one.ms);
  };
  const said = () => {
    if (props.one.said) return props.one.said;
    if (working()) return 'Working on it.';
    return waiting() ? 'It asked a question and waits for your answer.' : 'It has said nothing yet.';
  };

  return (
    <article class="race-card" classList={{ shown: props.shown }} aria-label={`${props.one.agentName}'s result`}>
      <div class="race-card-head">
        <Chip agentId={props.one.agentId} short={props.one.short ?? '··'} />
        <span class="who">{props.one.agentName}</span>
        <span class="time" classList={{ needs: waiting() }}>
          {time()}
        </span>
      </div>
      <p classList={{ quiet: !props.one.said }}>{said()}</p>
      <div class="race-card-facts">
        <Show when={props.one.checks}>{(checks) => <ChecksPill taskId={props.one.taskId} checks={checks()} />}</Show>
        <Show
          when={props.one.files.length > 0}
          fallback={<span class="race-files quiet">{props.one.problem ?? 'No file changed'}</span>}
        >
          <span class="race-files">
            {filesWord(props.one.files.length)}
            <Show when={props.one.added > 0}>
              {' '}
              <span class="plus">+{props.one.added}</span>
            </Show>
            <Show when={props.one.removed > 0}>
              {' '}
              <span class="minus">−{props.one.removed}</span>
            </Show>
          </span>
        </Show>
      </div>
      <span class="grow" />
      <button
        type="button"
        class="race-show"
        aria-pressed={props.shown}
        // The result whose diff is shown is the one the conversation on the right is with,
        // so "Use this result" can never mean another one than the one on screen.
        onClick={() => !props.shown && go(taskHref(props.one.taskId, RACE_VIEW))}
      >
        {props.shown ? 'Diff shown below' : 'Show diff'}
      </button>
    </article>
  );
}

/**
 * The Compare view of a race's entrant: one card per agent, and below them
 * the change of the entrant on screen, file by file.
 */
export function CompareView(props: { task: Task }): JSX.Element {
  const race = useRace(() => props.task);
  const raceId = () => props.task.info.race?.id;
  const taskId = () => props.task.info.id;
  const mine = () => race()?.entrants.find((one) => one.taskId === taskId());
  const name = () => mine()?.agentName ?? 'This agent';
  const files = () => mine()?.files ?? [];
  const lines = () => (mine() ? diffsOf(mine()!) : undefined);

  // The lines of the result on screen are read anew whenever its files change.
  createEffect(() => {
    const one = mine();
    if (one && one.files.length > 0) void loadDiffs(one).catch(() => {});
  });

  // Room for the longest line number, and no less than the three digits most files need.
  const numberWidth = () => {
    let longest = 0;
    for (const file of lines()?.files ?? []) {
      if ('problem' in file) continue;
      for (const hunk of file.hunks) longest = Math.max(longest, hunk.toLine);
    }
    return `${Math.max(24, String(longest).length * 8)}px`;
  };

  const openFile = (path: string, line: number) => {
    go(taskHref(taskId(), 'code'));
    setTimeout(() => openInEditor(taskId(), path, line), 0);
  };

  return (
    <section class="race" aria-label="The results of the race, side by side">
      <Show when={raceNotice(raceId())}>
        <p class="banner warn flush" role="status">
          <span class="grow">{raceNotice(raceId())}</span>
          <button type="button" class="btn tiny" onClick={() => setRaceNotice(raceId()!, null)}>
            Dismiss
          </button>
        </p>
      </Show>
      <Show when={raceProblem(raceId())}>
        <p class="banner danger flush" role="alert">
          {raceProblem(raceId())}
        </p>
      </Show>

      <div class="race-cards">
        <Show when={race()} fallback={<p class="fine">Reading the race</p>}>
          <For each={race()!.entrants}>{(one) => <Card one={one} shown={one.taskId === taskId()} />}</For>
        </Show>
      </div>

      <section class="race-diff" aria-label={`${name()}'s change`}>
        <div class="race-diff-head">
          <h2>{name()}'s change</h2>
          <Show when={files().length === 1}>
            <span class="path">{files()[0]!.path}</span>
          </Show>
          <Show when={files().length > 1}>
            <span class="path">
              {filesWord(files().length)} <span class="plus">+{mine()!.added}</span>{' '}
              <span class="minus">−{mine()!.removed}</span>
            </span>
          </Show>
        </div>
        <div class="race-diff-body" style={{ '--race-no': numberWidth() }}>
          <Switch>
            <Match when={!mine()}>
              <p class="diff-note">{race() ? 'This result is no longer part of the race.' : 'Reading the race'}</p>
            </Match>
            <Match when={mine()!.problem}>
              <p class="diff-note">{mine()!.problem}</p>
            </Match>
            <Match when={files().length === 0}>
              <p class="diff-note">
                {mine()!.state === 'running' || mine()!.state === 'waiting_permission'
                  ? `${name()} has not changed a file yet.`
                  : `${name()} changed no file.`}
              </p>
            </Match>
            <Match when={!lines()}>
              <p class="diff-note">Reading the change</p>
            </Match>
            <Match when={lines()}>
              <For each={lines()!.files}>
                {(file) => (
                  <>
                    <Show when={files().length > 1}>
                      <div class="race-file">
                        <span class="path">{file.path}</span>
                        <Show when={!('problem' in file)}>
                          <FileCount file={file as FileDiff} />
                        </Show>
                      </div>
                    </Show>
                    <Show
                      when={!('problem' in file)}
                      fallback={<p class="diff-note">{(file as { problem: string }).problem}</p>}
                    >
                      <DiffRows diff={file as FileDiff} onLine={(line) => openFile(file.path, line)} />
                    </Show>
                  </>
                )}
              </For>
              <Show when={files().length > SHOWN_FILES}>
                <p class="diff-note">
                  {filesWord(files().length - SHOWN_FILES)} more changed. The{' '}
                  <a href={taskHref(taskId(), 'changes')}>Changes view</a> lists them all.
                </p>
              </Show>
            </Match>
          </Switch>
        </div>
      </section>
    </section>
  );
}
