import { createSignal, For, Show, type JSX } from 'solid-js';
import { canReset, databaseWords, depsButton, depsWords, diskWords, envOwn, envSource, size } from '../core/run';
import {
  dbReset,
  depsInstall,
  envSet,
  resourcesCleanUp,
  runStop,
  type EnvValue,
  type RunOther,
  type RunResources,
  type RunState,
  type TaskEnv,
} from '../core/store';
import { Sure } from '../screens/Changes';
import { ago, span } from './bits';

const message = (error: unknown) => (error instanceof Error ? error.message : String(error));

/** How many of the task's own values the Environment card shows before it says "and 2 more". */
const OWN_SHOWN = 3;

/** Does one thing at a time for the whole tab, and shows what went wrong. */
type Act = (name: string, work: () => Promise<RunState | unknown>) => Promise<void>;

/** One value of the task's environment, with a field to set it for this task only. */
function EnvRow(props: {
  taskId: string;
  value: EnvValue;
  onEnv: (env: TaskEnv) => void;
  onProblem: (problem: string | null) => void;
}): JSX.Element {
  // A secret's field starts empty: its value never came here, so there is nothing to put in it.
  const fresh = (value: EnvValue | undefined) => (value && !value.secret ? value.value : '');
  // eslint-disable-next-line solid/reactivity -- the starting text of the field; later changes are typed, or set on saving
  const [draft, setDraft] = createSignal(fresh(props.value));
  const [saving, setSaving] = createSignal(false);
  const changed = () => (props.value.secret ? draft() !== '' : draft() !== props.value.value);
  // eslint-disable-next-line solid/reactivity -- the name of a row never changes: a row is kept by its name
  const id = `env-${props.taskId}-${props.value.name}`;

  const set = async (value: string | null) => {
    if (saving()) return;
    setSaving(true);
    props.onProblem(null);
    try {
      const next = await envSet(props.taskId, props.value.name, value);
      setDraft(fresh(next.values.find((one) => one.name === props.value.name)));
      props.onEnv(next);
    } catch (error) {
      props.onProblem(message(error));
    } finally {
      setSaving(false);
    }
  };

  const from = () => {
    const project = props.value.project;
    if (!props.value.own) return `From ${project?.from ?? 'the project'}.`;
    if (!project) return 'Set for this task only.';
    return `Set for this task. ${project.from} has ${project.value === '' ? 'it empty' : project.value}.`;
  };

  return (
    <div class="run-env-row" role="group" aria-label={props.value.name}>
      <label class="mono run-env-name" for={id}>
        {props.value.name}
      </label>
      <input
        id={id}
        class="field mono"
        type={props.value.secret ? 'password' : 'text'}
        autocomplete={props.value.secret ? 'new-password' : 'off'}
        spellcheck={false}
        value={draft()}
        placeholder={props.value.secret ? props.value.value : ''}
        onInput={(event) => setDraft(event.currentTarget.value)}
        onKeyDown={(event) => {
          if (event.key === 'Enter' && changed()) void set(draft());
        }}
      />
      <span class="fine run-env-from">{from()}</span>
      <span class="run-env-buttons">
        <Show when={props.value.own}>
          <button type="button" class="btn tiny ghost" disabled={saving()} onClick={() => void set(null)}>
            {props.value.project ? 'Use the project’s value' : 'Remove'}
          </button>
        </Show>
        <button type="button" class="btn tiny" disabled={!changed() || saving()} onClick={() => void set(draft())}>
          Set for this task
        </button>
      </span>
    </div>
  );
}

/** The task's environment, value by value, and a way to add one. */
function EnvEditor(props: {
  taskId: string;
  env: TaskEnv;
  running: boolean;
  onEnv: (env: TaskEnv) => void;
  onProblem: (problem: string | null) => void;
  onClose: () => void;
}): JSX.Element {
  const [name, setName] = createSignal('');
  const [value, setValue] = createSignal('');
  const [adding, setAdding] = createSignal(false);

  const add = async (event: SubmitEvent) => {
    event.preventDefault();
    if (adding() || !name().trim()) return;
    setAdding(true);
    props.onProblem(null);
    try {
      props.onEnv(await envSet(props.taskId, name().trim(), value()));
      setName('');
      setValue('');
    } catch (error) {
      props.onProblem(message(error));
    } finally {
      setAdding(false);
    }
  };

  return (
    <section class="run-env" aria-label="Environment of this task">
      <div class="run-log-head">
        <span class="grow fine">
          Given to everything the Run tab starts. What you set here is kept for this task only, outside its folder. A
          secret is never shown whole.
          <Show when={props.running}> What is running keeps the values it was started with: restart it.</Show>
        </span>
        <button type="button" class="btn tiny ghost" onClick={() => props.onClose()}>
          Close
        </button>
      </div>
      <For each={props.env.values} fallback={<span class="run-card-text">No values yet.</span>}>
        {(one) => <EnvRow taskId={props.taskId} value={one} onEnv={props.onEnv} onProblem={props.onProblem} />}
      </For>
      <form class="run-env-add" aria-label="Add a value for this task" onSubmit={(event) => void add(event)}>
        <input
          class="field mono"
          aria-label="Name of the new value"
          placeholder="NAME"
          autocomplete="off"
          spellcheck={false}
          value={name()}
          onInput={(event) => setName(event.currentTarget.value)}
        />
        <input
          class="field mono"
          aria-label="Its value"
          placeholder="value"
          autocomplete="off"
          spellcheck={false}
          value={value()}
          onInput={(event) => setValue(event.currentTarget.value)}
        />
        <button type="submit" class="btn tiny" disabled={adding() || !name().trim()}>
          Add for this task
        </button>
      </form>
    </section>
  );
}

/** What another task runs: how long it has been idle and what it holds, where that is known. */
function otherWords(other: RunOther): string {
  const state = other.inUse
    ? 'in use'
    : other.idleSince === null
      ? 'idle'
      : `idle ${span(Date.now() - other.idleSince)}`;
  return other.memory === null ? state : `${state} · ${size(other.memory)}`;
}

/**
 * The cards under the table of the Run tab: the task's environment, its
 * packages, its database, and what runs in other tasks.
 */
export function RunCards(props: {
  taskId: string;
  run: RunState;
  env: TaskEnv | null;
  use: RunResources | null;
  busy: string | null;
  act: Act;
  onEnv: (env: TaskEnv) => void;
  /** Something in another task was stopped or removed: what is used has to be asked again. */
  onUse: () => void;
  onProblem: (problem: string | null) => void;
  /** Shows the terminal of the running install. */
  onInstallLog: (termId: number) => void;
}): JSX.Element {
  const [editingEnv, setEditingEnv] = createSignal(false);
  const own = () => props.env?.values.filter((value) => value.own) ?? [];
  const others = () => props.use?.others ?? [];
  const running = () => props.run.services.some((service) => service.state === 'running');
  const somethingToClean = () => others().some((other) => !other.inUse) || (props.use?.leftovers ?? 0) > 0;

  /** Does something to another task, then asks again what is used, whether it worked or not. */
  const elsewhere = (name: string, work: () => Promise<unknown>) =>
    // eslint-disable-next-line solid/reactivity -- called later, as an event handler: it reads what is current then
    props.act(name, async () => {
      try {
        await work();
      } finally {
        props.onUse();
      }
    });

  return (
    <>
      <div class="run-cards">
        <Show when={props.run.services.length > 0 && props.env}>
          {(env) => (
            <section class="run-card" aria-label="Environment">
              <h3>Environment</h3>
              <span class="run-card-text">
                {envSource(env())} {envOwn(env())}
              </span>
              <For each={own().slice(0, OWN_SHOWN)}>
                {(value) => (
                  <span class="run-card-value">
                    {value.name}={value.value}
                  </span>
                )}
              </For>
              <Show when={own().length > OWN_SHOWN}>
                <span class="fine">and {own().length - OWN_SHOWN} more</span>
              </Show>
              <span class="grow" />
              <button
                type="button"
                class="btn tiny"
                aria-expanded={editingEnv()}
                onClick={() => setEditingEnv(!editingEnv())}
              >
                Edit
              </button>
            </section>
          )}
        </Show>

        <Show when={props.run.dependencies}>
          {(deps) => {
            const words = () => depsWords(deps(), deps().installedAt === null ? null : ago(deps().installedAt!));
            return (
              <section class="run-card" aria-label="Dependencies">
                <h3>Dependencies</h3>
                <span class="run-card-text">
                  {words().before}
                  <Show when={deps().state !== 'none'}>
                    {' '}
                    <span class="mono">{deps().command}</span> {words().after}
                  </Show>
                </span>
                <span class="grow" />
                <div class="run-card-buttons">
                  <Show when={depsButton(deps())}>
                    {(label) => (
                      <button
                        type="button"
                        class="btn tiny"
                        disabled={Boolean(props.busy)}
                        // eslint-disable-next-line solid/reactivity -- called later, as an event handler: it reads what is current then
                        onClick={() => void props.act('install', () => depsInstall(props.taskId))}
                      >
                        {label()}
                      </button>
                    )}
                  </Show>
                  <Show when={deps().termId !== null}>
                    <button type="button" class="btn tiny" onClick={() => props.onInstallLog(deps().termId!)}>
                      Logs
                    </button>
                  </Show>
                </div>
              </section>
            );
          }}
        </Show>

        <Show when={props.run.databases.length > 0}>
          <section class="run-card" aria-label="Database">
            <h3>Database</h3>
            <For each={props.run.databases}>
              {(database) => (
                <span class="run-card-text">
                  <Show when={props.run.databases.length > 1}>
                    <span class="mono">{database.name}</span>:{' '}
                  </Show>
                  {databaseWords(database, props.use?.volumes)}
                </span>
              )}
            </For>
            <span class="grow" />
            <div class="run-card-buttons">
              <For each={props.run.databases.filter(canReset)}>
                {(database) => (
                  <Sure
                    class="btn tiny"
                    label={`Reset ${props.run.databases.length > 1 ? `${database.name} ` : ''}to ${database.seed ? 'seed' : 'empty'}`}
                    sure="Press again to throw its data away"
                    disabled={Boolean(props.busy) || !props.run.dockerReady}
                    about={database.name}
                    // eslint-disable-next-line solid/reactivity -- called later, as an event handler: it reads what is current then
                    onSure={() => void props.act(`reset:${database.name}`, () => dbReset(props.taskId, database.name))}
                  />
                )}
              </For>
            </div>
          </section>
        </Show>

        <section class="run-card wide" aria-label="Running in other tasks">
          <div class="run-card-head">
            <h3 class="grow">Running in other tasks</h3>
            <Show when={props.use?.othersMemory}>{(bytes) => <span class="fine">{size(bytes())} memory</span>}</Show>
          </div>
          <For each={others()} fallback={<span class="run-card-text">Nothing is running in other tasks.</span>}>
            {(other) => (
              <div class="run-other" role="group" aria-label={other.title}>
                <span class="run-other-title" title={`${other.title}: ${other.running.join(', ')}`}>
                  {other.title}
                </span>
                <span class="fine">{otherWords(other)}</span>
                <Show when={!other.inUse}>
                  <button
                    type="button"
                    class="btn tiny"
                    disabled={Boolean(props.busy)}
                    onClick={() => void elsewhere(`stop:${other.taskId}`, () => runStop(other.taskId))}
                  >
                    Stop
                  </button>
                </Show>
              </div>
            )}
          </For>
          <Show when={diskWords(props.use?.disk)}>{(words) => <span class="fine">{words()}</span>}</Show>
          <Show when={somethingToClean()}>
            <Sure
              class="btn tiny ghost run-clean"
              label="Stop everything idle and remove volumes of finished tasks"
              sure="Press again to stop and remove"
              disabled={Boolean(props.busy)}
              // eslint-disable-next-line solid/reactivity -- called later, as an event handler: it reads what is current then
              onSure={() => void elsewhere('clean', () => resourcesCleanUp(props.taskId))}
            />
          </Show>
        </section>
      </div>

      <Show when={editingEnv() && props.env}>
        {(env) => (
          <EnvEditor
            taskId={props.taskId}
            env={env()}
            running={running()}
            onEnv={props.onEnv}
            onProblem={props.onProblem}
            onClose={() => setEditingEnv(false)}
          />
        )}
      </Show>
    </>
  );
}
