import { createSignal, For, lazy, onCleanup, onMount, Show, Suspense, type JSX } from 'solid-js';
import { createStore, reconcile } from 'solid-js/store';
import { core } from '../core/client';
import {
  agentOf,
  runLogs,
  runStart,
  runState,
  runStop,
  type RunService,
  type RunState,
  type Task,
} from '../core/store';
import { setWork } from '../core/work';

const TerminalView = lazy(() => import('./Terminal').then((module) => ({ default: module.TerminalView })));

const message = (error: unknown) => (error instanceof Error ? error.message : String(error));

const stateWords: Record<RunService['state'], string> = {
  stopped: 'stopped',
  starting: 'starting',
  running: 'running',
  failed: 'did not start',
  exited: 'finished',
};

/**
 * The Run tab: what the project can start, read from its package.json and its
 * compose file, with each part's state, address and log. Docker services run
 * in a compose project that belongs to this task alone.
 */
export function RunTab(props: { task: Task; visible: boolean }): JSX.Element {
  // eslint-disable-next-line solid/reactivity -- this component is made anew for each task, so the id never changes under it
  const taskId = props.task.info.id;
  // Kept in a store and brought in line service by service: a row stays the same row
  // while its state changes. Drawn anew on every look, a row would lose a press that
  // lands between two looks, and its buttons would flicker.
  const [held, setHeld] = createStore<{ run: RunState | null }>({ run: null });
  const run = () => held.run;
  const setRun = (next: RunState) => setHeld('run', reconcile(next, { key: 'name' }));
  const [problem, setProblem] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal<string | null>(null);
  const [logOf, setLogOf] = createSignal<{ service: string; termId: number } | null>(null);
  const agentName = () => agentOf(props.task)?.name ?? 'the agent';
  let logEl: HTMLDivElement | undefined;

  const load = async () => {
    try {
      setRun(await runState(taskId));
    } catch (error) {
      setProblem(message(error));
    }
  };

  onMount(() => {
    void load();
    // eslint-disable-next-line solid/reactivity -- called later, as an event handler
    const stop = core.onEvent((event) => {
      if (event.event === 'run.changed' && event.data.taskId === taskId) void load();
      if (event.event === 'terminal.exit' && event.data.termId === logOf()?.termId) setLogOf(null);
    });
    // Containers start and stop on their own clock; while something is starting, look again often.
    const timer = setInterval(() => {
      if (!props.visible) return;
      // A command says where it listens a moment after it starts; a container without a published port never will.
      const waiting = run()?.services.some(
        (service) =>
          service.state === 'starting' ||
          (service.kind === 'command' && service.state === 'running' && !service.address),
      );
      if (waiting) void load();
    }, 1500);
    onCleanup(() => {
      stop();
      clearInterval(timer);
    });
  });

  const act = async (name: string, work: () => Promise<RunState | unknown>) => {
    if (busy()) return;
    setBusy(name);
    setProblem(null);
    try {
      const next = await work();
      if (next && typeof next === 'object' && 'services' in next) setRun(next as RunState);
    } catch (error) {
      setProblem(message(error));
    } finally {
      setBusy(null);
    }
  };

  const showLog = (service: RunService) =>
    // eslint-disable-next-line solid/reactivity -- called later, as an event handler: it reads what is current then
    act(`log:${service.name}`, async () => {
      if (logOf()?.service === service.name) {
        setLogOf(null);
        return;
      }
      const termId = service.termId ?? (await runLogs(taskId, service.name)).termId;
      setLogOf({ service: service.name, termId });
      // The log opens under the table; in a low panel that is below what is in view.
      requestAnimationFrame(() => logEl?.scrollIntoView({ block: 'nearest' }));
    });

  /** Hands a failure to the agent: what failed and why goes along with the next message. */
  const tellAgent = (service: RunService) => {
    setWork(taskId, {
      quote: {
        path: `service ${service.name}`,
        fromLine: 1,
        toLine: 1,
        text: `${service.runs}\n${service.note ?? ''}`,
      },
    });
    document.getElementById(`msg-${taskId}`)?.focus();
  };

  const services = () => run()?.services ?? [];
  const up = () => services().filter((service) => service.state === 'running').length;
  const anyActive = () => services().some((service) => service.state === 'running' || service.state === 'starting');

  return (
    <div class="run">
      <Show when={problem()}>
        <p class="banner danger flush" role="alert">
          <span class="grow">{problem()}</span>
          <button type="button" class="btn tiny" onClick={() => setProblem(null)}>
            Dismiss
          </button>
        </p>
      </Show>
      <Show when={run()} fallback={<p class="bottom-note">Reading what this project can run</p>}>
        {(state) => (
          <>
            <div class="run-head">
              <span class="grow fine">
                <Show
                  when={services().length > 0}
                  fallback="Nothing to run was found: no dev script in package.json and no compose file. The terminal still runs anything."
                >
                  {up()} of {services().length} up. Read from{' '}
                  {[state().composeFile, services().some((s) => s.kind === 'command') ? 'package.json' : null]
                    .filter(Boolean)
                    .join(' and ')}
                  .
                </Show>
              </span>
              <Show when={services().length > 0}>
                <button
                  type="button"
                  class="btn small"
                  disabled={Boolean(busy())}
                  onClick={() => act('start', () => runStart(taskId))}
                >
                  {busy() === 'start' ? 'Starting' : 'Start all'}
                </button>
                <button
                  type="button"
                  class="btn small ghost"
                  disabled={Boolean(busy()) || !anyActive()}
                  onClick={() => act('stop', () => runStop(taskId))}
                >
                  Stop all
                </button>
              </Show>
            </div>
            <Show when={state().problem}>
              <p class="banner warn flush">{state().problem}</p>
            </Show>
            <Show when={state().composeFile && !state().dockerReady}>
              <p class="banner warn flush">{state().dockerHint}</p>
            </Show>

            <Show when={services().length > 0}>
              <div class="run-table" role="table" aria-label="Services of this task">
                <div class="run-row head" role="row">
                  <span role="columnheader">Service</span>
                  <span role="columnheader">Runs</span>
                  <span role="columnheader">Address</span>
                  <span role="columnheader" />
                </div>
                <For each={services()}>
                  {(service) => (
                    <div class="run-row" role="row">
                      <span role="cell" class="run-name">
                        <span class={`run-dot ${service.state}`} aria-hidden="true" />
                        {service.name}
                      </span>
                      <span role="cell" class="mono run-what">
                        <Show when={service.kind === 'docker'}>
                          <span class="tag">Docker</span>
                        </Show>
                        {service.runs}
                      </span>
                      <span role="cell" class="run-address">
                        <Show
                          when={service.state === 'running' && service.address}
                          fallback={
                            <span classList={{ 'run-bad': service.state === 'failed' }}>
                              {service.note ?? stateWords[service.state]}
                            </span>
                          }
                        >
                          <a class="mono" href={`http://${service.address}`} target="_blank" rel="noreferrer">
                            {service.address}
                          </a>
                          <Show when={service.note}>
                            <span class="fine">{service.note}</span>
                          </Show>
                        </Show>
                      </span>
                      <span role="cell" class="run-actions">
                        <button
                          type="button"
                          class="btn tiny ghost"
                          aria-pressed={logOf()?.service === service.name}
                          disabled={service.state === 'stopped' && service.kind === 'command'}
                          onClick={() => void showLog(service)}
                        >
                          Logs
                        </button>
                        <Show when={service.state === 'failed'}>
                          <button type="button" class="btn tiny ghost" onClick={() => tellAgent(service)}>
                            Send to {agentName()}
                          </button>
                        </Show>
                        <Show
                          when={service.state === 'running' || service.state === 'starting'}
                          fallback={
                            <button
                              type="button"
                              class="btn tiny"
                              disabled={Boolean(busy())}
                              onClick={() => act(`start:${service.name}`, () => runStart(taskId, service.name))}
                            >
                              Start
                            </button>
                          }
                        >
                          <button
                            type="button"
                            class="btn tiny ghost"
                            disabled={Boolean(busy())}
                            onClick={() => act(`stop:${service.name}`, () => runStop(taskId, service.name))}
                          >
                            Stop
                          </button>
                        </Show>
                      </span>
                    </div>
                  )}
                </For>
              </div>
              <Show when={state().composeFile}>
                <p class="fine">
                  Docker services run as the compose project <span class="mono">{state().composeProject}</span>, which
                  belongs to this task alone. A port that is taken is moved to a free one, so two tasks never fight over
                  a port, a container name or a volume.
                </p>
              </Show>
            </Show>

            <Show when={logOf()} keyed>
              {(log) => (
                <div class="run-log" ref={logEl}>
                  <div class="run-log-head">
                    <span class="grow fine">Output of {log.service}</span>
                    <button type="button" class="btn tiny ghost" onClick={() => setLogOf(null)}>
                      Close
                    </button>
                  </div>
                  <Suspense>
                    <TerminalView taskId={taskId} visible={props.visible} attach={log.termId} />
                  </Suspense>
                </div>
              )}
            </Show>
          </>
        )}
      </Show>
    </div>
  );
}
