import { createEffect, createSignal, For, lazy, on, onCleanup, onMount, Show, Suspense, type JSX } from 'solid-js';
import { createStore, reconcile } from 'solid-js/store';
import { core } from '../core/client';
import { readFrom, setRunCount, shownServices, size } from '../core/run';
import {
  agentOf,
  envGet,
  resourcesGet,
  runLogs,
  runPickProfile,
  runRemoveProfile,
  runRestart,
  runSaveProfile,
  runShell,
  runStart,
  runState,
  runStop,
  type RunProfile,
  type RunResources,
  type RunService,
  type RunState,
  type Task,
  type TaskEnv,
} from '../core/store';
import { setWork } from '../core/work';
import { RunCards } from './RunCards';

const TerminalView = lazy(() => import('./Terminal').then((module) => ({ default: module.TerminalView })));

const message = (error: unknown) => (error instanceof Error ? error.message : String(error));

const stateWords: Record<RunService['state'], string> = {
  stopped: 'stopped',
  starting: 'starting',
  running: 'running',
  failed: 'did not start',
  exited: 'finished',
};

/** What the dot of a row says to someone who cannot see its colour. */
const dotWords: Record<RunService['state'], string> = {
  stopped: 'Stopped',
  starting: 'Starting',
  running: 'Running',
  failed: 'Failed',
  exited: 'Finished',
};

/** The choices of the profile list that are not profiles. */
/** They begin with a space, which no profile's name does: the core takes the spaces off both ends of a name. */
const NEW_PROFILE = ' new';
const CHANGE_PROFILE = ' change';
const REMOVE_PROFILE = ' remove';

/** What the panel under the table shows: the output of a program, or a shell to type into. */
type Shown = { title: string; termId: number; shell: boolean };

/**
 * The Run tab: what the project can start, read from its package.json and its
 * compose file, with each part's state, address, memory and log. One button
 * starts a profile: the commands and services that belong together. Under the
 * table: the task's environment, its packages, its database and what runs in
 * other tasks. Docker services run in a compose project that belongs to this
 * task alone.
 */
export function RunTab(props: { task: Task; visible: boolean }): JSX.Element {
  // eslint-disable-next-line solid/reactivity -- this component is made anew for each task, so the id never changes under it
  const taskId = props.task.info.id;
  // Kept in a store and brought in line service by service: a row stays the same row
  // while its state changes. Drawn anew on every look, a row would lose a press that
  // lands between two looks, and its buttons would flicker.
  const [held, setHeld] = createStore<{ run: RunState | null; env: TaskEnv | null; use: RunResources | null }>({
    run: null,
    env: null,
    use: null,
  });
  const run = () => held.run;
  const setRun = (next: RunState) => {
    setHeld('run', reconcile(next, { key: 'name' }));
    const shown = shownServices(next);
    setRunCount(taskId, shown.filter((service) => service.state === 'running').length, shown.length);
  };
  const [problem, setProblem] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal<string | null>(null);
  const [shown, setShown] = createSignal<Shown | null>(null);
  /** The profile being made or changed: its old name, or '' for a new one. Null while the form is closed. */
  const [editing, setEditing] = createSignal<string | null>(null);
  const [draftName, setDraftName] = createSignal('');
  const [draftServices, setDraftServices] = createSignal<string[]>([]);
  const agentName = () => agentOf(props.task)?.name ?? 'the agent';
  let logEl: HTMLDivElement | undefined;
  let profileEl: HTMLSelectElement | undefined;

  const load = async () => {
    try {
      setRun(await runState(taskId));
    } catch (error) {
      setProblem(message(error));
    }
  };

  const loadEnv = async () => {
    try {
      setHeld('env', reconcile(await envGet(taskId), { key: 'name' }));
    } catch (error) {
      setProblem(message(error));
    }
  };

  /** What is up and where it listens, in a word: when it reads differently, what is used has changed too. */
  const upNow = () =>
    (run()?.services ?? [])
      .filter((service) => service.state === 'running')
      .map((service) => `${service.name}@${service.address ?? ''}`)
      .join(' ');
  /** What was up when the use was last asked for. */
  let askedFor = '';
  // Docker takes a moment to say what its containers use. One question at a time:
  // a second one asked meanwhile is put after the first, not next to it.
  let asking = false;
  let askAgain: boolean | null = null;
  const loadUse = async (disk: boolean) => {
    if (asking) {
      askAgain = (askAgain ?? false) || disk;
      return;
    }
    asking = true;
    askedFor = upNow();
    try {
      const next = await resourcesGet(taskId, disk);
      // An answer without the disk says nothing about it: what was known before stays.
      const kept = disk ? next : { ...next, disk: held.use?.disk, volumes: held.use?.volumes };
      setHeld('use', reconcile(kept, { key: 'taskId' }));
    } catch {
      // The numbers are a courtesy: without them the tab still works, and the next look asks again.
    } finally {
      asking = false;
      if (askAgain !== null) {
        const withDisk = askAgain;
        askAgain = null;
        void loadUse(withDisk);
      }
    }
  };

  const loadAll = () => {
    void load();
    void loadEnv();
    void loadUse(true);
  };

  onMount(() => {
    loadAll();
    // eslint-disable-next-line solid/reactivity -- called later, as an event handler
    const stop = core.onEvent((event) => {
      if (event.event === 'run.changed') {
        if (event.data.taskId === taskId) loadAll();
        // Something started or stopped in another task: the list of those is no longer true.
        else if (props.visible) void loadUse(false);
      }
      if (event.event === 'terminal.exit' && event.data.termId === shown()?.termId) setShown(null);
    });
    // Containers start and stop on their own clock; while something is starting, look again often.
    const timer = setInterval(() => {
      if (!props.visible) return;
      // A command says where it listens a moment after it starts; a container without a published port never will.
      const waiting =
        run()?.services.some(
          (service) =>
            service.state === 'starting' ||
            (service.kind === 'command' && service.state === 'running' && !service.address),
        ) || run()?.dependencies?.state === 'installing';
      // Looking again is cheap; asking what is used is not, so that is asked only when something came up or went.
      if (waiting) {
        void load().then(() => {
          if (upNow() !== askedFor) void loadUse(false);
        });
      }
    }, 1500);
    // What a program holds in memory moves all the time. Asked now and then, and only while someone looks.
    const slow = setInterval(() => {
      if (!props.visible) return;
      const anything = run()?.services.some((service) => service.state === 'running') || held.use?.others.length;
      if (anything) void loadUse(false);
    }, 10_000);
    onCleanup(() => {
      stop();
      clearInterval(timer);
      clearInterval(slow);
      // Nobody keeps the count true from here on, so the tab's label stops saying one.
      setRunCount(taskId, 0, 0);
    });
  });

  // Coming back to the tab: profiles are the project's, so another task may have changed them meanwhile.
  createEffect(
    on(
      () => props.visible,
      (visible) => {
        if (visible) loadAll();
      },
      { defer: true },
    ),
  );

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

  /** Shows a terminal under the table, or hides it when it is the one shown. */
  const show = (next: Shown) => {
    setShown(next);
    // The panel opens under the table; in a low tab that is below what is in view.
    requestAnimationFrame(() => logEl?.scrollIntoView({ block: 'nearest' }));
  };

  const showLog = (service: RunService) =>
    // eslint-disable-next-line solid/reactivity -- called later, as an event handler: it reads what is current then
    act(`log:${service.name}`, async () => {
      const title = `Output of ${service.name}`;
      if (shown()?.title === title) {
        setShown(null);
        return;
      }
      const termId = service.termId ?? (await runLogs(taskId, service.name)).termId;
      show({ title, termId, shell: false });
    });

  const showShell = (service: RunService) =>
    // eslint-disable-next-line solid/reactivity -- called later, as an event handler: it reads what is current then
    act(`shell:${service.name}`, async () => {
      const title = `Shell in ${service.name}`;
      if (shown()?.title === title) {
        setShown(null);
        return;
      }
      show({ title, termId: (await runShell(taskId, service.name)).termId, shell: true });
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
  const rows = () => {
    const state = run();
    return state ? shownServices(state) : [];
  };
  const anyActive = () => services().some((service) => service.state === 'running' || service.state === 'starting');
  const profiles = () => run()?.profiles ?? [];
  const chosen = (): RunProfile | undefined => profiles().find((profile) => profile.name === run()?.profile);
  const others = () => profiles().filter((profile) => profile.name !== run()?.profile);

  /** Opens the form for a new profile, or for the chosen one. */
  const editProfile = (profile: RunProfile | null) => {
    setEditing(profile ? profile.name : '');
    setDraftName(profile ? profile.name : '');
    // A new profile starts from what is on screen now: unticking is quicker than ticking.
    setDraftServices(profile ? [...profile.services] : rows().map((service) => service.name));
  };

  const pickProfile = (value: string) => {
    if (![NEW_PROFILE, CHANGE_PROFILE, REMOVE_PROFILE].includes(value)) {
      void act('profile', () => runPickProfile(taskId, value));
      return;
    }
    // "New profile" is something to do, not something to be: the list goes back to the chosen profile.
    if (profileEl) profileEl.value = run()?.profile ?? '';
    if (value === NEW_PROFILE) editProfile(null);
    else if (value === CHANGE_PROFILE) editProfile(chosen() ?? null);
    // eslint-disable-next-line solid/reactivity -- called later, as an event handler: it reads what is current then
    else void act('profile', () => runRemoveProfile(taskId, run()?.profile ?? ''));
  };

  const saveProfile = (event: SubmitEvent) => {
    event.preventDefault();
    const was = editing();
    if (was === null) return;
    // eslint-disable-next-line solid/reactivity -- called later, as an event handler: it reads what is current then
    void act('profile', async () => {
      // In the order of the table, whatever order they were ticked in.
      const picked = services()
        .map((service) => service.name)
        .filter((name) => draftServices().includes(name));
      const next = await runSaveProfile(taskId, draftName(), picked, was || undefined);
      setEditing(null);
      return next;
    });
  };

  const tick = (name: string, ticked: boolean) =>
    setDraftServices((names) => [...names.filter((one) => one !== name), ...(ticked ? [name] : [])]);

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
              <Show
                when={services().length > 0}
                fallback={
                  <span class="grow fine">
                    Nothing to run was found: no dev script in package.json and no compose file. The terminal still runs
                    anything.
                  </span>
                }
              >
                <label class="run-profile">
                  <span>Start profile</span>
                  <select
                    ref={profileEl}
                    // As wide as the chosen name, as the design draws it; a list is as wide as its longest line otherwise.
                    style={{ width: `calc(${state().profile.length + 1}ch + 28px)` }}
                    disabled={Boolean(busy())}
                    onChange={(event) => pickProfile(event.currentTarget.value)}
                  >
                    <For each={profiles()}>
                      {(profile) => (
                        <option value={profile.name} selected={profile.name === state().profile}>
                          {profile.name}
                        </option>
                      )}
                    </For>
                    <option value={NEW_PROFILE}>New profile…</option>
                    <Show when={chosen() && !chosen()!.builtIn}>
                      <option value={CHANGE_PROFILE}>Change {state().profile}…</option>
                      <option value={REMOVE_PROFILE}>Remove {state().profile}</option>
                    </Show>
                  </select>
                </label>
                <span class="grow fine run-read">
                  Read from {readFrom(state())}.
                  <Show when={others().length > 0}>
                    {' '}
                    Other profiles:{' '}
                    {others()
                      .map((profile) => profile.name)
                      .join(', ')}
                    .
                  </Show>
                </span>
                <Show
                  when={anyActive()}
                  fallback={
                    <button
                      type="button"
                      class="btn"
                      disabled={Boolean(busy())}
                      onClick={() => act('start', () => runStart(taskId))}
                    >
                      {busy() === 'start' ? 'Starting' : 'Start all'}
                    </button>
                  }
                >
                  <button
                    type="button"
                    class="btn"
                    disabled={Boolean(busy())}
                    onClick={() => act('restart', () => runRestart(taskId))}
                  >
                    {busy() === 'restart' ? 'Restarting' : 'Restart all'}
                  </button>
                </Show>
                <button
                  type="button"
                  class="btn"
                  disabled={Boolean(busy()) || !anyActive()}
                  onClick={() => act('stop', () => runStop(taskId))}
                >
                  Stop all
                </button>
              </Show>
            </div>

            <Show when={editing() !== null}>
              <form
                class="run-profile-form"
                aria-label={editing() ? `Change the profile ${editing()}` : 'New profile'}
                onSubmit={saveProfile}
              >
                <label class="run-profile-name">
                  <span class="fine">Name</span>
                  <input
                    class="field"
                    value={draftName()}
                    maxLength={40}
                    placeholder="Web only"
                    onInput={(event) => setDraftName(event.currentTarget.value)}
                    ref={(el) => requestAnimationFrame(() => el.focus())}
                  />
                </label>
                <div class="run-profile-ticks" role="group" aria-label="Starts together">
                  <span class="fine">Starts together</span>
                  <For each={services()}>
                    {(service) => (
                      <label class="check">
                        <input
                          type="checkbox"
                          checked={draftServices().includes(service.name)}
                          onChange={(event) => tick(service.name, event.currentTarget.checked)}
                        />
                        {service.name}
                      </label>
                    )}
                  </For>
                </div>
                <span class="grow" />
                <button
                  type="submit"
                  class="btn small"
                  disabled={Boolean(busy()) || !draftName().trim() || draftServices().length === 0}
                >
                  Save the profile
                </button>
                <button type="button" class="btn small ghost" onClick={() => setEditing(null)}>
                  Cancel
                </button>
              </form>
            </Show>

            <Show when={state().problem}>
              <p class="banner warn flush">{state().problem}</p>
            </Show>
            <Show when={state().composeFile && !state().dockerReady}>
              <p class="banner warn flush">{state().dockerHint}</p>
            </Show>

            <Show when={services().length > 0}>
              <div class="run-table-wrap">
                <table class="run-services" aria-label="Services of this task">
                  <thead>
                    <tr>
                      <th scope="col">Service</th>
                      <th scope="col">Runs</th>
                      <th scope="col">Address</th>
                      <th scope="col" class="run-num">
                        Memory
                      </th>
                      <th scope="col" aria-label="Actions" />
                    </tr>
                  </thead>
                  <tbody>
                    <For each={rows()}>
                      {(service) => (
                        <tr classList={{ failed: service.state === 'failed', outside: !service.inProfile }}>
                          <td>
                            <span class="run-name">
                              {service.name}{' '}
                              {/* After the name for whoever listens, before it for whoever looks: see the style. */}
                              <span
                                class="run-dot"
                                data-state={service.state}
                                role="img"
                                aria-label={dotWords[service.state]}
                              />
                            </span>
                          </td>
                          <td class="run-what">
                            <Show when={service.kind === 'docker'}>
                              <span class="run-docker">Docker</span>
                            </Show>
                            {service.runs}
                          </td>
                          <Show
                            when={service.state !== 'failed'}
                            fallback={
                              <td colSpan={2} class="run-bad">
                                {service.note ?? stateWords.failed}
                              </td>
                            }
                          >
                            <td class="run-address">
                              <Show
                                when={service.state === 'running' && service.address}
                                fallback={<span>{service.note ?? stateWords[service.state]}</span>}
                              >
                                <Show when={service.web} fallback={<span class="mono">{service.address}</span>}>
                                  <a class="mono" href={`http://${service.address}`} target="_blank" rel="noreferrer">
                                    {service.address}
                                  </a>
                                </Show>
                                <Show when={service.note}>
                                  <span class="fine">{service.note}</span>
                                </Show>
                              </Show>
                              <Show when={!service.inProfile}>
                                <span class="fine">Not in this profile.</span>
                              </Show>
                            </td>
                            <td class="run-num run-memory">
                              <Show when={service.state === 'running' && held.use?.memory[service.name] !== undefined}>
                                {size(held.use!.memory[service.name]!)}
                              </Show>
                            </td>
                          </Show>
                          <td class="run-actions">
                            <button
                              type="button"
                              class="btn tiny ghost"
                              aria-pressed={shown()?.title === `Output of ${service.name}`}
                              disabled={service.state === 'stopped' && service.kind === 'command'}
                              onClick={() => void showLog(service)}
                            >
                              Logs
                            </button>
                            <Show when={service.kind === 'docker' && service.state === 'running'}>
                              <button
                                type="button"
                                class="btn tiny ghost"
                                aria-pressed={shown()?.title === `Shell in ${service.name}`}
                                onClick={() => void showShell(service)}
                              >
                                Shell
                              </button>
                            </Show>
                            <Show
                              when={service.state === 'running' || service.state === 'starting'}
                              fallback={
                                <button
                                  type="button"
                                  class="btn tiny ghost"
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
                                onClick={() => act(`restart:${service.name}`, () => runRestart(taskId, service.name))}
                              >
                                Restart
                              </button>
                              <button
                                type="button"
                                class="btn tiny ghost"
                                disabled={Boolean(busy())}
                                onClick={() => act(`stop:${service.name}`, () => runStop(taskId, service.name))}
                              >
                                Stop
                              </button>
                            </Show>
                            <Show when={service.state === 'failed'}>
                              <button type="button" class="btn tiny primary" onClick={() => tellAgent(service)}>
                                Send to {agentName()}
                              </button>
                            </Show>
                          </td>
                        </tr>
                      )}
                    </For>
                  </tbody>
                </table>
              </div>
              <Show when={state().composeFile}>
                <p class="fine">
                  Each task gets its own ports and its own compose project, here{' '}
                  <span class="mono">{state().composeProject}</span>, so two tasks never fight over a port, a container
                  name or a volume.
                </p>
              </Show>
            </Show>

            <Show when={shown()} keyed>
              {(panel) => (
                <div class="run-log" classList={{ shell: panel.shell }} ref={logEl}>
                  <div class="run-log-head">
                    <span class="grow fine">{panel.title}</span>
                    <button type="button" class="btn tiny ghost" onClick={() => setShown(null)}>
                      Close
                    </button>
                  </div>
                  <Suspense>
                    <TerminalView taskId={taskId} visible={props.visible} attach={panel.termId} follow={panel.shell} />
                  </Suspense>
                </div>
              )}
            </Show>

            <RunCards
              taskId={taskId}
              run={state()}
              env={held.env}
              use={held.use}
              busy={busy()}
              act={act}
              onEnv={(next) => setHeld('env', reconcile(next, { key: 'name' }))}
              onUse={() => void loadUse(true)}
              onProblem={setProblem}
              onInstallLog={(termId) => {
                const title = `Output of ${state().dependencies?.command ?? 'the install'}`;
                if (shown()?.title === title) setShown(null);
                else show({ title, termId, shell: false });
              }}
            />
          </>
        )}
      </Show>
    </div>
  );
}
