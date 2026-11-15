import { createMemo, createSignal, For, onCleanup, onMount, Show, type JSX } from 'solid-js';
import { core } from '../core/client';
import {
  byTool,
  changeWord,
  changeWords,
  checkForAgent,
  dangers,
  folderWords,
  hasWords,
  infraForAgent,
  toolWord,
  type InfraCheck,
  type InfraState,
} from '../core/infra';
import { runWords, type JobRun } from '../core/jobs';
import { createKept } from '../core/kept';
import { addShown } from '../core/previewState';
import { Marked, SETTLE } from './PreviewApi';

const said = (error: unknown) => (error instanceof Error ? error.message : String(error));

/**
 * The Preview of what a task changes in the infrastructure of a project: its
 * Terraform files, manifests, charts, Dockerfiles, compose files and
 * pipelines, each read before and after, with what deserves a second look
 * marked. Nothing is run by looking. A check of a tool that is on this
 * computer runs when its button is pressed, and nothing is ever applied.
 */
export function InfraView(props: { taskId: string }): JSX.Element {
  // eslint-disable-next-line solid/reactivity -- made anew for each task
  const taskId = props.taskId;
  const [found, setFound] = createKept<InfraState | null>(`task:${taskId}:infra.state`, null);
  /** The last run of each check that was looked at, and the command it was started with. */
  const [runs, setRuns] = createKept<Record<string, JobRun>>(`task:${taskId}:infra.runs`, {});
  const [commands, setCommands] = createKept<Record<string, string>>(`task:${taskId}:infra.commands`, {});
  const [problem, setProblem] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal<string | null>(null);
  const [handed, setHanded] = createSignal<string | null>(null);

  /** Asks how a check stands, and keeps asking while it runs. One timer for each check. */
  const watching = new Map<string, ReturnType<typeof setTimeout>>();
  /**
   * The runs the user stopped, each as it stood at that moment. Of a run that was stopped the core
   * keeps nothing, and it may take a moment to die: what it had printed stays on show from here.
   */
  const stopping = new Map<string, JobRun>();
  const watch = async (id: string) => {
    clearTimeout(watching.get(id));
    try {
      let now = await core.request<JobRun>('infra.state', { taskId, id });
      const stopped = stopping.get(id);
      if (stopped && now.running) stopping.set(id, now);
      if (stopped && !now.running) {
        stopping.delete(id);
        // It may have ended by itself in the same moment: then how it ended is what there is to say.
        if (now.exit === null) now = { ...stopped, running: false, termId: null, exit: null, stopped: true };
      }
      const whole = now;
      // A check that never ran has nothing to keep; one that was stopped keeps what it had printed.
      if (whole.running || whole.exit !== null || whole.stopped) setRuns((before) => ({ ...before, [id]: whole }));
      if (now.running)
        watching.set(
          id,
          setTimeout(() => void watch(id), stopped ? 200 : 700),
        );
    } catch {
      // The task may be gone; the next look says so.
    }
  };

  // One reading at a time; one asked for meanwhile follows it.
  let looked = false;
  let reading = false;
  let again = false;
  const load = async () => {
    if (reading) {
      again = true;
      return;
    }
    reading = true;
    try {
      const state = await core.request<InfraState>('infra.get', { taskId });
      setFound(state);
      setProblem(null);
      // A check may have been started before this view was opened, and be running still: asked once.
      if (!looked) for (const check of state.checks) if (check.ready) void watch(check.id);
      looked = true;
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

  onMount(() => {
    void load();
    let soon: ReturnType<typeof setTimeout> | undefined;
    const stop = core.onEvent((event) => {
      if (event.event === 'link.open') void load();
      if (event.data?.taskId !== taskId) return;
      // The infrastructure is files of the task.
      if (['files.changed', 'review.changed', 'git.changed'].includes(event.event)) {
        clearTimeout(soon);
        soon = setTimeout(() => void load(), SETTLE);
      }
    });
    onCleanup(() => {
      stop();
      clearTimeout(soon);
      for (const timer of watching.values()) clearTimeout(timer);
    });
  });

  const files = () => found()?.files ?? [];
  const checks = () => found()?.checks ?? [];
  const byId = createMemo(() => new Map(checks().map((check) => [check.id, check])));
  /** The rows of the checks, by name: a row stays the same row when the infrastructure is read again. */
  const checkIds = createMemo(() => checks().map((check) => check.id), [] as string[], {
    equals: (a, b) => a.join('\n') === b.join('\n'),
  });

  const start = async (check: InfraCheck) => {
    if (busy()) return;
    setBusy(check.id);
    setProblem(null);
    setHanded(null);
    stopping.delete(check.id);
    try {
      const started = await core.request<{ command: string }>('infra.run', { taskId, id: check.id });
      setCommands((before) => ({ ...before, [check.id]: started.command }));
      await watch(check.id);
    } catch (error) {
      setProblem(said(error));
    } finally {
      setBusy(null);
    }
  };

  const stop = async (check: InfraCheck) => {
    const before = runs()[check.id];
    if (before) stopping.set(check.id, before);
    try {
      await core.request('infra.stop', { taskId, id: check.id });
      await watch(check.id);
    } catch (error) {
      setProblem(said(error));
    }
  };

  const show = (made: { title: string; text: string }) => {
    addShown(taskId, made.title, made.text);
    setHanded(made.title);
  };

  const Check = (row: { id: string }) => (
    <Show when={byId().get(row.id)}>{(check) => <CheckRow check={check()} />}</Show>
  );
  const CheckRow = (row: { check: InfraCheck }) => {
    const run = (): JobRun | null => runs()[row.check.id] ?? null;
    const command = () => commands()[row.check.id] ?? row.check.command;
    const label = () => `${row.check.name} in ${folderWords(row.check.folder)}`;
    return (
      <li class="infra-check">
        <div class="db-file">
          <span class="mono">{row.check.name}</span>
          <span class="fine">in {folderWords(row.check.folder)}</span>
          <Show when={row.check.changed}>
            <span class="tag accent">changed by this task</span>
          </Show>
          <span class="grow" />
          <Show
            when={run()?.running}
            fallback={
              <button
                type="button"
                class="btn small"
                aria-label={`Run ${label()}`}
                disabled={!row.check.ready || busy() !== null}
                onClick={() => void start(row.check)}
              >
                {busy() === row.check.id ? 'Starting' : 'Run'}
              </button>
            }
          >
            <button type="button" class="btn small" aria-label={`Stop ${label()}`} onClick={() => void stop(row.check)}>
              Stop
            </button>
          </Show>
        </div>
        <Show when={row.check.note}>
          <p class="fine">
            <Marked text={row.check.note!} />
          </p>
        </Show>
        <Show when={run()}>
          {(ran) => (
            <section class="api-answer" aria-label={`The run of ${label()}`}>
              <div class="api-answer-head">
                <Show when={runWords(ran())}>
                  {(words) => (
                    <span class="tag" data-tone={words().tone === 'bad' ? 'failed' : words().tone}>
                      {words().text}
                    </span>
                  )}
                </Show>
                <span class="grow" />
                <Show when={handed() === checkForAgent(row.check, command(), ran()).title}>
                  <span class="fine">It goes along with your next message.</span>
                </Show>
                <button
                  type="button"
                  class="btn small"
                  onClick={() => show(checkForAgent(row.check, command(), ran()))}
                >
                  Show it to the agent
                </button>
              </div>
              <p class="mono api-url">$ {command()}</p>
              <Show
                when={ran().output}
                fallback={<p class="soft">It has printed nothing{ran().running ? ' yet' : ''}.</p>}
              >
                <pre class="api-body mono" tabindex="0" aria-label={`What ${label()} printed`}>
                  {ran().output}
                </pre>
              </Show>
            </section>
          )}
        </Show>
      </li>
    );
  };

  return (
    <div class="api infra">
      <Show when={problem()}>
        <p class="banner danger flush" role="alert">
          <span class="grow">{problem()}</span>
          <button type="button" class="btn tiny" onClick={() => setProblem(null)}>
            Dismiss
          </button>
        </p>
      </Show>
      <Show when={found()} fallback={<p class="bottom-note">Reading the infrastructure of this project</p>}>
        {(now) => (
          <Show
            when={now().tools.length > 0 || files().length > 0}
            fallback={
              <div class="center-note">
                <h2>No infrastructure files were found</h2>
                <p>
                  Terraform files, Helm charts, Kubernetes manifests, Dockerfiles, compose files and the pipelines of
                  GitLab and GitHub are looked for in the files of the project.
                </p>
              </div>
            }
          >
            <div class="db-body">
              <section class="db-part" aria-label="What this task changes in the infrastructure">
                <div class="db-head">
                  <h2 class="api-title">
                    Infrastructure
                    <span class="fine"> {changeWords(files())}</span>
                  </h2>
                  <Show when={dangers(files()) > 0}>
                    <span class="tag" data-tone="failed">
                      {dangers(files())} to look at twice
                    </span>
                  </Show>
                  <span class="grow" />
                  <Show when={files().length > 0}>
                    <Show when={handed() === infraForAgent(files()).title}>
                      <span class="fine">It goes along with your next message.</span>
                    </Show>
                    <button type="button" class="btn small" onClick={() => show(infraForAgent(files()))}>
                      Show it to the agent
                    </button>
                  </Show>
                </div>
                <p class="fine">
                  <Show when={now().tools.length > 0} fallback="The project has no infrastructure files left.">
                    The project has {hasWords(now().tools)}.
                  </Show>{' '}
                  What the task changed since its branch left the base is read from the files; nothing is run for it.
                </p>
                <Show when={now().note}>
                  <p class="fine">{now().note}</p>
                </Show>
                <For each={byTool(files())}>
                  {(group) => (
                    <section class="infra-tool" aria-label={toolWord[group.tool]}>
                      <h3 class="api-level">{toolWord[group.tool]}</h3>
                      <ul class="db-migrations">
                        <For each={group.files}>
                          {(one) => (
                            <li>
                              <div class="db-file">
                                <span class="mono">{one.path}</span>
                                <span
                                  class="tag"
                                  classList={{ good: one.change === 'added', accent: one.change === 'modified' }}
                                >
                                  {changeWord[one.change]}
                                </span>
                              </div>
                              <ul class="db-steps">
                                <For each={one.steps}>
                                  {(step) => (
                                    <li classList={{ danger: step.danger !== null }}>
                                      <Show when={step.field}>
                                        <span class="tag infra-field">{step.field}</span>{' '}
                                      </Show>
                                      <Marked text={step.text} />
                                      <Show when={step.danger}>
                                        <span class="fine"> {step.danger}</span>
                                      </Show>
                                    </li>
                                  )}
                                </For>
                              </ul>
                              <Show when={one.steps.length === 0 && one.change !== 'deleted'}>
                                <p class="fine">Nothing in it could be read as a change.</p>
                              </Show>
                            </li>
                          )}
                        </For>
                      </ul>
                    </section>
                  )}
                </For>
              </section>
              <Show when={checks().length > 0}>
                <section class="db-part" aria-label="Checks of the infrastructure">
                  <h2 class="api-title">Checks</h2>
                  <p class="fine">
                    Each one runs when you press it, in a terminal of this task, and nothing is ever applied. A plan is
                    asked not to lock or refresh the state; a provider may still want the credentials of its cloud.
                  </p>
                  <ul class="db-migrations">
                    <For each={checkIds()}>{(id) => <Check id={id} />}</For>
                  </ul>
                </section>
              </Show>
            </div>
          </Show>
        )}
      </Show>
    </div>
  );
}
