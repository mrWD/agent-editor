import { createSignal, For, onCleanup, onMount, Show, type JSX } from 'solid-js';
import { ago, Chip } from '../components/bits';
import { core } from '../core/client';
import {
  listAutomations,
  projectName,
  removeAutomation,
  saveAutomation,
  state,
  turnAutomation,
  type Automation,
  type AutomationThen,
  type AutomationWhen,
} from '../core/store';

const message = (error: unknown) => (error instanceof Error ? error.message : String(error));

/** What sets an automation off, as a sentence that starts its row. */
export function whenWords(when: AutomationWhen): string {
  switch (when.kind) {
    case 'merged':
      return 'When a merge request is merged';
    case 'requestOpened':
      return 'When a merge request opens';
    case 'daily':
      return `Every day at ${when.at}`;
    case 'issueLabeled':
      return `When a new issue has the label ${when.label}`;
  }
}

/** When an automation last ran, in words. */
export function ranWords(automation: Automation): string {
  if (!automation.lastRun) return 'Never ran';
  const today = new Date(automation.lastRun).toDateString() === new Date().toDateString();
  if (today && automation.runsThatDay > 1) return `Ran ${automation.runsThatDay} times today`;
  return `Ran ${ago(automation.lastRun)}`;
}

type Trigger = AutomationWhen['kind'];

/** Work that starts without the user. */
export function Automations(): JSX.Element {
  const [list, setList] = createSignal<Automation[] | null>(null);
  const [problem, setProblem] = createSignal<string | null>(null);
  const [form, setForm] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  // The new automation, as it is typed.
  const [name, setName] = createSignal('');
  const [trigger, setTrigger] = createSignal<Trigger>('daily');
  const [at, setAt] = createSignal('03:00');
  const [label, setLabel] = createSignal('bug');
  const [agentId, setAgentId] = createSignal('');
  const [text, setText] = createSignal('');
  const [which, setWhich] = createSignal<'all' | 'e2e'>('all');
  const [projects, setProjects] = createSignal<string[]>([]);

  const agents = () => state.agents.filter((agent) => agent.available);
  const chosenAgent = () => agentId() || agents()[0]?.id || '';
  const startsTask = () => trigger() === 'daily' || trigger() === 'issueLabeled';

  const load = async () => {
    try {
      setList((await listAutomations()).automations);
    } catch (error) {
      setProblem(message(error));
    }
  };
  onMount(() => {
    void load();
    const stop = core.onEvent((event) => {
      if (event.event === 'automations.changed') void load();
    });
    onCleanup(stop);
  });

  const act = async (work: () => Promise<unknown>) => {
    if (busy()) return;
    setBusy(true);
    setProblem(null);
    try {
      await work();
      await load();
    } catch (error) {
      setProblem(message(error));
    } finally {
      setBusy(false);
    }
  };

  const save = () =>
    // eslint-disable-next-line solid/reactivity -- called later, as an event handler: it reads what is current then
    act(async () => {
      const when: AutomationWhen =
        trigger() === 'daily'
          ? { kind: 'daily', at: at() }
          : trigger() === 'issueLabeled'
            ? { kind: 'issueLabeled', label: label().trim() }
            : { kind: trigger() as 'merged' | 'requestOpened' };
      const then: AutomationThen = startsTask()
        ? { kind: 'startTask', agentId: chosenAgent(), text: text().trim() }
        : { kind: 'runChecks', which: which() };
      await saveAutomation({ name: name().trim(), when, then, projects: projects() });
      setForm(false);
      setName('');
      setText('');
    });

  const toggleProject = (id: string) =>
    setProjects((now) => (now.includes(id) ? now.filter((one) => one !== id) : [...now, id]));

  /** What the automation does, as the second line of its row. */
  const thenWords = (automation: Automation) => {
    const then = automation.then;
    switch (then.kind) {
      case 'cleanUp':
        return 'Archive the task and delete its working copy and branch. It holds back when files are not committed.';
      case 'runChecks':
        return then.which === 'e2e' ? 'Start the end-to-end tests of the task' : 'Run the checks of the task';
      case 'startTask':
        return then.text;
    }
  };

  return (
    <main class="page">
      <div class="page-inner wide">
        <div class="issues-top">
          <h1 class="headline">Automations</h1>
          <span class="grow" />
          <button type="button" class="btn" aria-expanded={form()} onClick={() => setForm(!form())}>
            New automation
          </button>
        </div>
        <p class="soft">Work that starts without you. Every run that an agent does shows up on Home as a task.</p>
        <Show when={problem()}>
          <p class="banner danger flush" role="alert">
            <span class="grow">{problem()}</span>
            <button type="button" class="btn tiny" onClick={() => setProblem(null)}>
              Dismiss
            </button>
          </p>
        </Show>

        <Show when={form()}>
          <form
            class="card auto-form"
            aria-label="New automation"
            onSubmit={(event) => {
              event.preventDefault();
              void save();
            }}
          >
            <label for="auto-name">Name</label>
            <input
              id="auto-name"
              class="field"
              placeholder="Update dependencies"
              value={name()}
              onInput={(event) => setName(event.currentTarget.value)}
            />
            <label for="auto-when">When</label>
            <div class="auto-row">
              <select
                id="auto-when"
                class="select-button"
                value={trigger()}
                onChange={(event) => setTrigger(event.currentTarget.value as Trigger)}
              >
                <option value="daily">Every day at a time</option>
                <option value="issueLabeled">A new issue has a label</option>
                <option value="requestOpened">A merge request opens</option>
                <option value="merged">A merge request is merged</option>
              </select>
              <Show when={trigger() === 'daily'}>
                <input
                  class="field narrow"
                  type="time"
                  aria-label="Time of day"
                  value={at()}
                  onInput={(event) => setAt(event.currentTarget.value)}
                />
              </Show>
              <Show when={trigger() === 'issueLabeled'}>
                <input
                  class="field narrow"
                  aria-label="Label"
                  value={label()}
                  onInput={(event) => setLabel(event.currentTarget.value)}
                />
              </Show>
            </div>
            <Show
              when={startsTask()}
              fallback={
                <>
                  <label for="auto-checks">Do</label>
                  <select
                    id="auto-checks"
                    class="select-button"
                    value={which()}
                    onChange={(event) => setWhich(event.currentTarget.value as 'all' | 'e2e')}
                  >
                    <option value="all">Run the checks of the task</option>
                    <option value="e2e">Start the end-to-end tests of the task</option>
                  </select>
                </>
              }
            >
              <label for="auto-agent">Start a task with</label>
              <select
                id="auto-agent"
                class="select-button"
                value={chosenAgent()}
                onChange={(event) => setAgentId(event.currentTarget.value)}
              >
                <For each={agents()}>{(agent) => <option value={agent.id}>{agent.name}</option>}</For>
              </select>
              <label for="auto-text">
                {trigger() === 'issueLabeled' ? 'What to tell it after the issue itself' : 'What to tell it'}
              </label>
              <textarea
                id="auto-text"
                class="git-message"
                rows="3"
                placeholder={
                  trigger() === 'issueLabeled'
                    ? 'Reproduce it and write a failing test'
                    : 'Update the dependencies and say what changed'
                }
                value={text()}
                onInput={(event) => setText(event.currentTarget.value)}
              />
              <Show when={state.agents.find((agent) => agent.id === chosenAgent())?.spendsTokens}>
                <span class="fine">Each run spends tokens on your account, like a task you start yourself.</span>
              </Show>
            </Show>
            <fieldset class="auto-projects">
              <legend>In which projects</legend>
              <span class="fine">{projects().length === 0 ? 'All projects. Tick some to narrow it.' : ''}</span>
              <For each={state.projects}>
                {(project) => (
                  <label class="check">
                    <input
                      type="checkbox"
                      checked={projects().includes(project.id)}
                      onChange={() => toggleProject(project.id)}
                    />
                    {project.name}
                  </label>
                )}
              </For>
            </fieldset>
            <div class="actions">
              <button type="submit" class="btn primary" disabled={busy()}>
                Add the automation
              </button>
              <button type="button" class="btn ghost" onClick={() => setForm(false)}>
                Cancel
              </button>
            </div>
          </form>
        </Show>

        <section class="stage">
          <Show when={list() === null}>
            <p class="stage-row plain">Reading the automations</p>
          </Show>
          <For each={list() ?? []}>
            {(automation) => (
              <div class="stage-row auto-item" aria-label={whenWords(automation.when)}>
                <div class="auto-what">
                  <span class="name">{whenWords(automation.when)}</span>
                  <span class="soft">
                    {automation.builtIn ? '' : `${automation.name}: `}
                    {thenWords(automation)}
                  </span>
                  <span class="auto-meta">
                    <Show
                      when={automation.then.kind === 'startTask' && automation.then}
                      fallback={<span class="tag">{automation.builtIn ? 'Built in' : 'Yours'}</span>}
                    >
                      {(then) => (
                        <span class="auto-agent">
                          <Chip
                            agentId={then().agentId}
                            short={state.agents.find((agent) => agent.id === then().agentId)?.short ?? '··'}
                            flat
                          />
                          {state.agents.find((agent) => agent.id === then().agentId)?.name ?? then().agentId}
                        </span>
                      )}
                    </Show>
                    <span>
                      {automation.projects.length === 0
                        ? 'All projects'
                        : automation.projects.map(projectName).join(', ')}
                    </span>
                    <span title={automation.lastNote ?? undefined}>
                      {ranWords(automation)}
                      {automation.lastNote ? `: ${automation.lastNote}` : ''}
                    </span>
                  </span>
                </div>
                <Show when={!automation.builtIn}>
                  <button
                    type="button"
                    class="btn small ghost"
                    disabled={busy()}
                    onClick={() => act(() => removeAutomation(automation.id))}
                  >
                    Remove
                  </button>
                </Show>
                <button
                  type="button"
                  class="btn small"
                  classList={{ primary: automation.on }}
                  aria-pressed={automation.on}
                  aria-label={`${whenWords(automation.when)}: ${automation.on ? 'on' : 'off'}`}
                  disabled={busy()}
                  onClick={() => act(() => turnAutomation(automation.id, !automation.on))}
                >
                  {automation.on ? 'On' : 'Off'}
                </button>
              </div>
            )}
          </For>
        </section>
        <p class="fine">
          The core looks at what is due about once a minute, and asks GitHub, GitLab and Linear for new issues every
          five minutes. An automation never does what you could not do by hand.
        </p>
      </div>
    </main>
  );
}
