import { createEffect, createMemo, createSignal, For, on, onCleanup, onMount, Show, type JSX } from 'solid-js';
import { core } from '../core/client';
import {
  argsHint,
  argsProblem,
  isQueued,
  kindWord,
  matchingJobs,
  runForAgent,
  runWords,
  takesNamed,
  workerOf,
  workerWords,
  type Job,
  type JobRun,
  type JobsState,
} from '../core/jobs';
import { createKept } from '../core/kept';
import { addShown } from '../core/previewState';
import type { RunService } from '../core/store';

const said = (error: unknown) => (error instanceof Error ? error.message : String(error));

type Draft = { args: string; kwargs: string };

/**
 * The Preview of the work a project does away from any request: its queued
 * tasks and its commands. One is started by hand with the arguments it takes,
 * and what it prints is shown as it comes. `services` is what the task runs,
 * to say whether a worker is there to take a queued job.
 */
export function JobsView(props: { taskId: string; services: RunService[] }): JSX.Element {
  // eslint-disable-next-line solid/reactivity -- made anew for each task
  const taskId = props.taskId;
  const [found, setFound] = createKept<JobsState | null>(`task:${taskId}:jobs.state`, null);
  const [picked, setPicked] = createKept<string>(`task:${taskId}:jobs.picked`, '');
  const [drafts, setDrafts] = createKept<Record<string, Draft>>(`task:${taskId}:jobs.drafts`, {});
  /** The last run of each job that was looked at, and the command it was started with. */
  const [runs, setRuns] = createKept<Record<string, JobRun>>(`task:${taskId}:jobs.runs`, {});
  const [commands, setCommands] = createKept<Record<string, string>>(`task:${taskId}:jobs.commands`, {});
  const [filter, setFilter] = createSignal('');
  const [problem, setProblem] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal(false);
  const [setting, setSetting] = createSignal(false);
  const [line, setLine] = createSignal('');
  const [handed, setHanded] = createSignal<string | null>(null);

  // One reading at a time; one asked for meanwhile follows it.
  let reading = false;
  let again = false;
  const load = async () => {
    if (reading) {
      again = true;
      return;
    }
    reading = true;
    try {
      setFound(await core.request<JobsState>('jobs.get', { taskId }));
      setProblem(null);
    } catch (error) {
      setProblem(said(error));
    } finally {
      reading = false;
      if (again) {
        again = false;
        void load();
      }
    }
  };

  const jobs = () => found()?.jobs ?? [];
  const byId = createMemo(() => new Map(jobs().map((job) => [job.id, job])));
  /** The rows of the list, by name: a row stays the same row when the list is read again. */
  const shownIds = createMemo(() => matchingJobs(jobs(), filter()).map((job) => job.id));
  /** The job on show: the one picked, else the first, so the view never opens on nothing. */
  const job = (): Job | null => byId().get(picked()) ?? jobs()[0] ?? null;
  const draft = (): Draft => (job() ? drafts()[job()!.id] : undefined) ?? { args: '', kwargs: '' };
  const change = (next: Partial<Draft>) => {
    const one = job();
    if (!one) return;
    const whole = { ...draft(), ...next };
    setDrafts((before) => ({ ...before, [one.id]: whole }));
  };
  const run = (): JobRun | null => (job() ? (runs()[job()!.id] ?? null) : null);
  const worker = () => workerOf(props.services);

  /** Asks how a job stands, and keeps asking while it runs and is the one on show. */
  let watching: ReturnType<typeof setTimeout> | undefined;
  const watch = async (id: string) => {
    clearTimeout(watching);
    try {
      const now = await core.request<JobRun>('jobs.state', { taskId, id });
      setRuns((before) => ({ ...before, [id]: now }));
      if (now.running && job()?.id === id) watching = setTimeout(() => void watch(id), 700);
    } catch {
      // The task may be gone; the next look says so.
    }
  };

  onMount(() => {
    void load();
    let soon: ReturnType<typeof setTimeout> | undefined;
    const stop = core.onEvent((event) => {
      if (event.event === 'link.open') void load();
      if (event.data?.taskId !== taskId) return;
      // A job is written in a file of the task.
      if (['files.changed', 'review.changed', 'git.changed'].includes(event.event)) {
        clearTimeout(soon);
        soon = setTimeout(() => void load(), 600);
      }
    });
    onCleanup(() => {
      stop();
      clearTimeout(soon);
      clearTimeout(watching);
    });
  });
  // Another job on show: how its last run stands is asked.
  createEffect(
    on(
      () => job()?.id,
      (id) => {
        setHanded(null);
        setSetting(false);
        if (id) void watch(id);
      },
    ),
  );

  const refusal = () => (job() ? argsProblem(job()!.kind, draft().args, draft().kwargs) : null);

  const start = async () => {
    const one = job();
    if (!one || busy() || refusal()) return;
    setBusy(true);
    setProblem(null);
    setHanded(null);
    try {
      const started = await core.request<{ command: string }>('jobs.run', { taskId, id: one.id, ...draft() });
      setCommands((before) => ({ ...before, [one.id]: started.command }));
      await watch(one.id);
    } catch (error) {
      setProblem(said(error));
    } finally {
      setBusy(false);
    }
  };

  const stop = async () => {
    const one = job();
    if (!one) return;
    const before = run();
    try {
      await core.request('jobs.stop', { taskId, id: one.id });
      await watch(one.id);
      // Of a run that was stopped the core keeps nothing: what it had printed until then stays on show here.
      if (before && !run()?.running && (run()?.exit ?? null) === null) {
        const kept: JobRun = { ...before, running: false, termId: null, exit: null, stopped: true };
        setRuns((all) => ({ ...all, [one.id]: kept }));
      }
    } catch (error) {
      setProblem(said(error));
    }
  };

  const openSettings = () => {
    setLine(found()?.settings[job()?.kind ?? ''] ?? '');
    setSetting(true);
  };
  const saveSettings = async () => {
    const one = job();
    if (!one) return;
    setProblem(null);
    try {
      await core.request('jobs.set', { taskId, kind: one.kind, template: line() });
      setSetting(false);
      await load();
    } catch (error) {
      setProblem(said(error));
    }
  };

  const Row = (row: { id: string }) => {
    const one = () => byId().get(row.id);
    return (
      <Show when={one()}>
        {(shown) => (
          <li>
            <button
              type="button"
              class="api-row"
              aria-current={job()?.id === row.id ? 'true' : undefined}
              onClick={() => setPicked(row.id)}
            >
              <span class="api-path mono">{shown().name}</span>
              <span class="fine">{kindWord[shown().kind]}</span>
              <Show when={shown().changed}>
                <span class="tag accent">changed</span>
              </Show>
            </button>
          </li>
        )}
      </Show>
    );
  };

  return (
    <div class="api jobs">
      <Show when={problem()}>
        <p class="banner danger flush" role="alert">
          <span class="grow">{problem()}</span>
          <button type="button" class="btn tiny" onClick={() => setProblem(null)}>
            Dismiss
          </button>
        </p>
      </Show>
      <Show when={found()} fallback={<p class="bottom-note">Looking for the jobs of this project</p>}>
        <Show
          when={jobs().length > 0}
          fallback={
            <div class="center-note">
              <h2>No background jobs were found</h2>
              <p>
                Tasks of Celery and Dramatiq, commands of Django and Laravel, workers of Sidekiq, Active Job classes and
                package scripts named like jobs are looked for in the files of the project.
              </p>
            </div>
          }
        >
          <div class="api-split">
            <nav class="api-list" aria-label="The jobs of the project">
              <Show when={jobs().length > 6}>
                <label class="sr-only" for={`jobs-filter-${taskId}`}>
                  Find a job
                </label>
                <input
                  id={`jobs-filter-${taskId}`}
                  class="field"
                  type="search"
                  placeholder="Find"
                  value={filter()}
                  onInput={(event) => setFilter(event.currentTarget.value)}
                />
              </Show>
              <ul>
                <For each={shownIds()}>{(id) => <Row id={id} />}</For>
              </ul>
            </nav>
            <Show when={job()}>
              {(one) => (
                <section class="api-one" aria-label="Job">
                  <h2 class="api-title">
                    <span class="mono">{one().name}</span>
                    <span class="fine">{kindWord[one().kind]}</span>
                    <Show when={one().changed}>
                      <span class="tag accent">changed by this task</span>
                    </Show>
                  </h2>
                  <p class="soft mono jobs-where">
                    {one().file}:{one().line}
                  </p>
                  <Show when={one().takes.length > 0}>
                    <p class="jobs-takes">
                      <span class="fine">Takes</span>
                      <For each={one().takes}>{(name) => <code class="mono api-name">{name}</code>}</For>
                    </p>
                  </Show>

                  <label class="api-label" for={`jobs-args-${taskId}`}>
                    {isQueued(one().kind) ? 'Arguments, a JSON list' : 'The rest of the command line'}
                  </label>
                  <input
                    id={`jobs-args-${taskId}`}
                    class="field mono"
                    spellcheck={false}
                    placeholder={argsHint(one())}
                    value={draft().args}
                    onInput={(event) => change({ args: event.currentTarget.value })}
                    onKeyDown={(event) => event.key === 'Enter' && void start()}
                  />
                  <Show when={takesNamed(one().kind)}>
                    <label class="api-label" for={`jobs-kwargs-${taskId}`}>
                      Named arguments, a JSON object
                    </label>
                    <input
                      id={`jobs-kwargs-${taskId}`}
                      class="field mono"
                      spellcheck={false}
                      placeholder="{}"
                      value={draft().kwargs}
                      onInput={(event) => change({ kwargs: event.currentTarget.value })}
                      onKeyDown={(event) => event.key === 'Enter' && void start()}
                    />
                  </Show>

                  <div class="api-send">
                    <Show
                      when={run()?.running}
                      fallback={
                        <button
                          type="button"
                          class="btn"
                          disabled={busy() || refusal() !== null}
                          onClick={() => void start()}
                        >
                          {busy() ? 'Starting' : 'Run'}
                        </button>
                      }
                    >
                      <button type="button" class="btn" onClick={() => void stop()}>
                        Stop
                      </button>
                    </Show>
                    <Show when={refusal()}>
                      <span class="fine">{refusal()}.</span>
                    </Show>
                    <span class="grow" />
                    <button
                      type="button"
                      class="btn small ghost"
                      aria-expanded={setting()}
                      onClick={() => (setting() ? setSetting(false) : openSettings())}
                    >
                      How it is started
                    </button>
                  </div>
                  <Show when={setting()}>
                    <form
                      class="jobs-setup"
                      aria-label="How jobs of this kind are started"
                      onSubmit={(event) => {
                        event.preventDefault();
                        void saveSettings();
                      }}
                    >
                      <label for={`jobs-line-${taskId}`}>
                        The command line of every {kindWord[one().kind].toLowerCase()}
                      </label>
                      <input
                        id={`jobs-line-${taskId}`}
                        class="field mono"
                        spellcheck={false}
                        placeholder={found()?.templates[one().kind] ?? ''}
                        value={line()}
                        onInput={(event) => setLine(event.currentTarget.value)}
                      />
                      <p class="fine">
                        {isQueued(one().kind)
                          ? 'It stands for the name of the job as {name} and for its arguments as {args} and {kwargs}, in quotes already.'
                          : 'It stands for the name of the job as {name} and for the rest of the command line as {tail}.'}{' '}
                        Leave it empty to use the line shown. Kept for the project.
                      </p>
                      <span class="api-setup-end">
                        <button type="submit" class="btn small">
                          Save
                        </button>
                        <button type="button" class="btn small ghost" onClick={() => setSetting(false)}>
                          Cancel
                        </button>
                      </span>
                    </form>
                  </Show>
                  <Show when={isQueued(one().kind)}>
                    <p class="fine">{workerWords(worker())}</p>
                  </Show>

                  <Show when={run() && (run()!.running || run()!.exit !== null || run()!.stopped)}>
                    <section class="api-answer" aria-label="The run">
                      <div class="api-answer-head">
                        <Show when={runWords(run())}>
                          {(words) => (
                            <span class="tag" data-tone={words().tone === 'bad' ? 'failed' : words().tone}>
                              {words().text}
                            </span>
                          )}
                        </Show>
                        <span class="grow" />
                        <Show when={handed()}>
                          <span class="fine">It goes along with your next message.</span>
                        </Show>
                        <button
                          type="button"
                          class="btn small"
                          onClick={() => {
                            const made = runForAgent(one(), commands()[one().id] ?? one().name, run()!);
                            addShown(taskId, made.title, made.text);
                            setHanded(made.title);
                          }}
                        >
                          Show it to the agent
                        </button>
                      </div>
                      <Show when={commands()[one().id]}>
                        <p class="mono api-url">$ {commands()[one().id]}</p>
                      </Show>
                      <Show
                        when={run()!.output}
                        fallback={<p class="soft">It has printed nothing{run()!.running ? ' yet' : ''}.</p>}
                      >
                        <pre class="api-body mono" tabindex="0" aria-label="What the job printed">
                          {run()!.output}
                        </pre>
                      </Show>
                    </section>
                  </Show>
                </section>
              )}
            </Show>
          </div>
        </Show>
      </Show>
    </div>
  );
}
