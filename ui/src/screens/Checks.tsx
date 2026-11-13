import { createMemo, createSignal, For, Match, Show, Switch, type JSX } from 'solid-js';
import { go, taskHref } from '../app/router';
import { ago } from '../components/bits';
import {
  addCheck,
  addLine,
  checkNote,
  checksOf,
  removeCheck,
  removeLine,
  runChecks,
  setLine,
  stopCheck,
  turnCheck,
  type Check,
  type DoneLine,
} from '../core/checks';
import { agentOf, sendPrompt, type Task } from '../core/store';
import { openInEditor } from '../core/work';

const message = (error: unknown) => (error instanceof Error ? error.message : String(error));

/** The mark in front of a line of "Done when". */
function Mark(props: { state: 'done' | 'bad' | 'busy' | 'open' | 'off' }): JSX.Element {
  const sign = () => ({ done: '✓', bad: '!', busy: '…', open: '○', off: '–' })[props.state];
  return (
    <span class={`check-mark ${props.state}`} aria-hidden="true">
      {sign()}
    </span>
  );
}

const markOf = (check: Check) => {
  if (check.state === 'running') return 'busy' as const;
  if (!check.on && check.state === 'none') return 'off' as const;
  if (check.stale || check.state === 'none') return 'open' as const;
  return check.state === 'passed' ? ('done' as const) : ('bad' as const);
};

/** What the agent is told about a check that failed. */
export function failedPrompt(check: Check): string {
  const end = (check.tail ?? '').split('\n').slice(-40).join('\n');
  return `The check “${check.title}” (\`${check.command}\`) failed with exit code ${check.exitCode ?? 1}. The end of what it printed:\n\n\`\`\`\n${end}\n\`\`\`\n\nFix what it complains about, then say what you changed.`;
}

/**
 * The Checks view of a task: "Done when". Lines a machine can check are the
 * project's own commands; the others wait for the user's OK.
 */
export function ChecksView(props: { task: Task }): JSX.Element {
  // eslint-disable-next-line solid/reactivity -- this component is made anew for each task
  const taskId = props.task.info.id;
  const agentName = () => agentOf(props.task)?.name ?? 'the agent';
  const canAsk = () => props.task.info.state === 'idle' || props.task.info.state === 'asleep';
  const state = () => checksOf(taskId);
  const [picked, setPicked] = createSignal<string | null>(null);
  const [problem, setProblem] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal(false);
  const [draft, setDraft] = createSignal('');
  const [why, setWhy] = createSignal<string | null>(null);

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

  // The line shown in the middle: the one picked, else the first that needs a look.
  const shown = createMemo<{ check?: Check; line?: DoneLine } | null>(() => {
    const known = state();
    if (!known) return null;
    const id = picked();
    const check = known.checks.find((one) => `c:${one.id}` === id);
    if (check) return { check };
    const line = known.lines.find((one) => `l:${one.id}` === id);
    if (line) return { line };
    const first =
      known.checks.find((one) => one.on && one.state === 'failed' && !one.stale) ??
      known.checks.find((one) => one.on) ??
      known.checks[0];
    if (first) return { check: first };
    return known.lines[0] ? { line: known.lines[0] } : null;
  });
  const isShown = (id: string) => (shown()?.check ? `c:${shown()!.check!.id}` : `l:${shown()?.line?.id}`) === id;
  const anyOn = () => (state()?.checks ?? []).some((check) => check.on);
  const everRun = () => (state()?.checks ?? []).some((check) => check.state !== 'none');

  const add = (as: 'line' | 'command') =>
    // eslint-disable-next-line solid/reactivity -- called later, as an event handler: it reads what is current then
    act(async () => {
      const text = draft().trim();
      if (!text) return;
      await (as === 'line' ? addLine(taskId, text) : addCheck(taskId, text));
      setDraft('');
    });

  const openFile = (path: string, line: number, column?: number) => {
    go(taskHref(taskId, 'code'));
    setTimeout(() => openInEditor(taskId, path, line, column), 0);
  };

  return (
    <>
      <section class="git" aria-label="Done when">
        <div class="git-head">
          <h2 class="pane-title grow">Done when</h2>
          <Show when={state()}>
            {(known) => (
              <span class="fine">
                {known().done} of {known().total}
              </span>
            )}
          </Show>
          <button
            type="button"
            class="btn tiny"
            disabled={busy() || !anyOn() || (state()?.running ?? 0) > 0}
            onClick={() => act(() => runChecks(taskId))}
          >
            {(state()?.running ?? 0) > 0 ? 'Running' : everRun() ? 'Run all again' : 'Run all'}
          </button>
        </div>
        <Show when={problem()}>
          <p class="banner danger flush" role="alert">
            <span class="grow">{problem()}</span>
            <button type="button" class="btn tiny" onClick={() => setProblem(null)}>
              Dismiss
            </button>
          </p>
        </Show>
        <Show when={state() && state()!.total === 0 && state()!.checks.length === 0}>
          <p class="fine">
            No checks were found in this project: it has no lint, type or test command in its own files. Add a command
            or a line below.
          </p>
        </Show>

        <div class="check-list">
          <For each={state()?.checks ?? []}>
            {(check) => (
              <button
                type="button"
                class="check-row"
                aria-current={isShown(`c:${check.id}`) ? 'true' : undefined}
                onClick={() => setPicked(`c:${check.id}`)}
              >
                <Mark state={markOf(check)} />
                <span class="check-title">{check.title}</span>
                <span class="fine">{checkNote(check)}</span>
              </button>
            )}
          </For>
          <For each={state()?.lines ?? []}>
            {(line) => (
              <button
                type="button"
                class="check-row"
                aria-current={isShown(`l:${line.id}`) ? 'true' : undefined}
                onClick={() => setPicked(`l:${line.id}`)}
              >
                <Mark state={line.done ? 'done' : 'open'} />
                <span class="check-title">{line.text}</span>
                <span class="fine">
                  {line.done ? 'you said it is done' : 'waiting for your OK'}
                  {line.from === 'issue' ? ' · from the issue' : ''}
                </span>
              </button>
            )}
          </For>
        </div>

        <div class="git-block">
          <label class="pane-title" for={`check-add-${taskId}`}>
            Add a line
          </label>
          <input
            id={`check-add-${taskId}`}
            class="field"
            placeholder="What must be true, or a command that checks it"
            value={draft()}
            onInput={(event) => setDraft(event.currentTarget.value)}
          />
          <div class="actions">
            <button
              type="button"
              class="btn tiny"
              disabled={busy() || draft().trim() === ''}
              onClick={() => add('line')}
            >
              Add as a line I confirm
            </button>
            <button
              type="button"
              class="btn tiny"
              disabled={busy() || draft().trim() === ''}
              onClick={() => add('command')}
            >
              Add as a command to run
            </button>
          </div>
          <span class="fine">
            Checks come from the project's own files and count for every task of the project. Lines you confirm belong
            to this task; a task list in its issue becomes such lines.
          </span>
        </div>
      </section>

      <section class="center">
        <Switch
          fallback={
            <div class="center-note">
              <h2>{state() ? 'Nothing to check yet' : 'Reading the checks'}</h2>
              <Show when={state()}>
                <p>Add a command or a line on the left. A project with a lint or test command brings its checks.</p>
              </Show>
            </div>
          }
        >
          <Match when={shown()?.check}>
            {(check) => (
              <>
                <div class="diff-head">
                  <h2>{check().title}</h2>
                  <span class="fine">{checkNote(check())}</span>
                  <span class="grow" />
                  <Show
                    when={check().state === 'running'}
                    fallback={
                      <button
                        type="button"
                        class="btn small"
                        disabled={busy()}
                        onClick={() => act(() => runChecks(taskId, check().id))}
                      >
                        {check().state === 'none' ? 'Run' : 'Run again'}
                      </button>
                    }
                  >
                    <button
                      type="button"
                      class="btn small"
                      disabled={busy()}
                      onClick={() => act(() => stopCheck(taskId, check().id))}
                    >
                      Stop
                    </button>
                  </Show>
                  <Show when={check().state === 'failed'}>
                    <button
                      type="button"
                      class="btn small primary"
                      disabled={busy() || !canAsk()}
                      title={canAsk() ? undefined : `${agentName()} is working. Send it when its turn ends.`}
                      onClick={() => act(() => sendPrompt(taskId, failedPrompt(check())))}
                    >
                      Send to {agentName()}
                    </button>
                  </Show>
                </div>
                <div class="diff-body check-detail">
                  <p class="check-command">
                    <code>{check().command}</code>
                    <span class="fine">
                      {check().from === 'you' ? 'added by you' : `from ${check().from}`}
                      <Show when={check().at}> · last run {ago(check().at!)}</Show>
                    </span>
                    <span class="grow" />
                    <button
                      type="button"
                      class="btn tiny ghost"
                      disabled={busy()}
                      onClick={() => act(() => turnCheck(taskId, check().id, !check().on))}
                    >
                      {check().on ? 'Turn off for this project' : 'Turn on for this project'}
                    </button>
                    <Show when={check().from === 'you'}>
                      <button
                        type="button"
                        class="btn tiny ghost"
                        disabled={busy()}
                        onClick={() => act(() => removeCheck(taskId, check().id))}
                      >
                        Remove
                      </button>
                    </Show>
                  </p>
                  <Show when={check().stale}>
                    <p class="diff-note">The files changed after this run. Run it again to know how it stands now.</p>
                  </Show>
                  <Show when={(check().problems?.length ?? 0) > 0}>
                    <h3 class="git-sub">What it names</h3>
                    <div class="problems">
                      <For each={check().problems}>
                        {(found) => (
                          <button
                            type="button"
                            class="problem"
                            onClick={() => openFile(found.path, found.line, found.column)}
                          >
                            <span class={`problem-mark ${found.severity}`} aria-hidden="true">
                              {found.severity === 'error' ? '!' : '?'}
                            </span>
                            <span class="problem-text">{found.message}</span>
                            <span class="mono muted">
                              {found.path}:{found.line}
                            </span>
                          </button>
                        )}
                      </For>
                    </div>
                  </Show>
                  <Show
                    when={check().tail}
                    fallback={
                      <p class="diff-note">
                        {check().state === 'running' ? 'Nothing printed yet.' : 'It has not been run in this task.'}
                      </p>
                    }
                  >
                    <h3 class="git-sub">What it printed{check().state === 'running' ? ' so far' : ''}</h3>
                    <pre class="check-output" aria-label={`Output of ${check().title}`}>
                      {check().tail}
                    </pre>
                  </Show>
                </div>
              </>
            )}
          </Match>
          <Match when={shown()?.line}>
            {(line) => (
              <>
                <div class="diff-head">
                  <h2>{line().text}</h2>
                  <span class="fine">{line().done ? 'You said it is done' : 'Waiting for your OK'}</span>
                </div>
                <div class="diff-body check-detail">
                  <p class="diff-note">
                    No command can check this, so it counts as done when you say so.
                    {line().from === 'issue' ? ' It comes from the task list of the issue.' : ''}
                  </p>
                  <div class="actions">
                    <Show
                      when={!line().done}
                      fallback={
                        <button
                          type="button"
                          class="btn"
                          disabled={busy()}
                          onClick={() => act(() => setLine(taskId, line().id, false))}
                        >
                          Not done after all
                        </button>
                      }
                    >
                      <button
                        type="button"
                        class="btn primary"
                        disabled={busy()}
                        onClick={() => act(() => setLine(taskId, line().id, true))}
                      >
                        Yes, this is done
                      </button>
                      <button
                        type="button"
                        class="btn"
                        aria-expanded={why() !== null}
                        onClick={() => setWhy(why() === null ? '' : null)}
                      >
                        Not yet
                      </button>
                    </Show>
                    <Show when={line().from === 'you'}>
                      <button
                        type="button"
                        class="btn ghost"
                        disabled={busy()}
                        onClick={() => act(() => removeLine(taskId, line().id))}
                      >
                        Remove
                      </button>
                    </Show>
                  </div>
                  <Show when={why() !== null && !line().done}>
                    <label class="fine" for={`why-${taskId}`}>
                      What is missing? {agentName()} is told.
                    </label>
                    <textarea
                      id={`why-${taskId}`}
                      class="git-message"
                      rows="3"
                      value={why() ?? ''}
                      onInput={(event) => setWhy(event.currentTarget.value)}
                    />
                    <div class="actions">
                      <button
                        type="button"
                        class="btn"
                        disabled={busy() || !canAsk() || (why() ?? '').trim() === ''}
                        onClick={() =>
                          act(async () => {
                            await sendPrompt(
                              taskId,
                              `This is not done yet: “${line().text}”.\n\n${(why() ?? '').trim()}\n\nPlease take care of it.`,
                            );
                            setWhy(null);
                          })
                        }
                      >
                        Send to {agentName()}
                      </button>
                    </div>
                  </Show>
                </div>
              </>
            )}
          </Match>
        </Switch>
        <div class="statusline">
          <span>A failed check stands in the way of "Merge now" until it passes or is turned off</span>
        </div>
      </section>
    </>
  );
}
