import { createSignal, For, onCleanup, onMount, Show, type JSX } from 'solid-js';
import { go, taskHref } from '../app/router';
import { whenLabel } from '../components/bits';
import { core } from '../core/client';
import {
  auditHealth,
  createTask,
  getHealth,
  ignoreFinding,
  projectName,
  state,
  turnProjectCheck,
  type Finding,
  type Health as HealthState,
} from '../core/store';

const message = (error: unknown) => (error instanceof Error ? error.message : String(error));

/** "Four things your projects are missing", with small numbers as words. */
export function missingHeadline(count: number): string {
  const words = ['Nothing', 'One thing', 'Two things', 'Three things', 'Four things', 'Five things', 'Six things'];
  if (count === 0) return 'Nothing is missing that the editor can see';
  const what = words[count] ?? `${count} things`;
  return `${what} your projects ${count === 1 ? 'is' : 'are'} missing`;
}

/** What each project is missing, and the checks every task of it must pass. */
export function Health(): JSX.Element {
  const [health, setHealth] = createSignal<HealthState | null>(null);
  const [problem, setProblem] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal<string | null>(null);
  const [agentId, setAgentId] = createSignal('');

  const agents = () => state.agents.filter((agent) => agent.available);
  const chosenAgent = () => agentId() || agents()[0]?.id || '';
  const agentName = () => state.agents.find((agent) => agent.id === chosenAgent())?.name ?? 'an agent';

  const load = async () => {
    try {
      setHealth(await getHealth());
    } catch (error) {
      setProblem(message(error));
    }
  };
  onMount(() => {
    void load();
    const stop = core.onEvent((event) => {
      if (['projects.changed', 'checks.changed', 'health.changed'].includes(event.event)) void load();
    });
    onCleanup(stop);
  });

  const act = async (name: string, work: () => Promise<unknown>) => {
    if (busy()) return;
    setBusy(name);
    setProblem(null);
    try {
      await work();
    } catch (error) {
      setProblem(message(error));
    } finally {
      setBusy(null);
    }
  };

  const fix = (finding: Finding) =>
    act(finding.id, async () => go(taskHref(await createTask(finding.projectId, chosenAgent(), finding.fix))));

  return (
    <main class="page">
      <div class="page-inner wide">
        <div class="issues-status">
          <span class="soft">
            <Show when={health()?.auditedAt} fallback="The package managers have not been asked for an audit yet.">
              {(at) => <>Last audit of the dependencies: {whenLabel(at())}.</>}
            </Show>
          </span>
          <button
            type="button"
            class="btn small"
            disabled={Boolean(busy())}
            title="Asks each project's package manager for known vulnerabilities. This reaches the network."
            onClick={() => act('audit', async () => setHealth(await auditHealth()))}
          >
            {busy() === 'audit' ? 'Checking' : 'Check now'}
          </button>
        </div>

        <h1 class="headline">{health() ? missingHeadline(health()!.findings.length) : 'Looking at the projects'}</h1>
        <p class="soft">
          Found by looking at each project's own files, and by agents that noticed something while they worked. Fix
          starts a task for an agent.
        </p>
        <Show when={problem()}>
          <p class="banner danger flush" role="alert">
            <span class="grow">{problem()}</span>
            <button type="button" class="btn tiny" onClick={() => setProblem(null)}>
              Dismiss
            </button>
          </p>
        </Show>

        <Show when={(health()?.findings.length ?? 0) > 0 && agents().length > 0}>
          <label class="issues-agent">
            Fix with
            <select value={chosenAgent()} onChange={(event) => setAgentId(event.currentTarget.value)}>
              <For each={agents()}>{(agent) => <option value={agent.id}>{agent.name}</option>}</For>
            </select>
          </label>
        </Show>

        <section class="stage" aria-label="What is missing">
          <For each={health()?.findings ?? []}>
            {(finding) => (
              <div class="stage-row finding">
                <div class="finding-what">
                  <span class="finding-top">
                    <span class={`tag finding-${finding.category.toLowerCase()}`}>{finding.category}</span>
                    <span class="project-tag">{projectName(finding.projectId)}</span>
                  </span>
                  <span class="name">{finding.title}</span>
                  <span class="soft">{finding.detail}</span>
                </div>
                <button
                  type="button"
                  class="btn small primary"
                  disabled={Boolean(busy()) || chosenAgent() === ''}
                  title={`Starts a task in ${projectName(finding.projectId)} and tells ${agentName()} what is missing`}
                  onClick={() => void fix(finding)}
                >
                  {busy() === finding.id ? 'Starting' : 'Fix'}
                </button>
                <button
                  type="button"
                  class="btn small ghost"
                  disabled={Boolean(busy())}
                  onClick={() => act('ignore', () => ignoreFinding(finding.id).then(load))}
                >
                  Ignore
                </button>
              </div>
            )}
          </For>
          <Show when={(health()?.ignored ?? 0) > 0}>
            <p class="stage-row plain">
              <span class="grow">
                {health()!.ignored === 1 ? 'One finding is ignored.' : `${health()!.ignored} findings are ignored.`}
              </span>
              <button
                type="button"
                class="btn tiny"
                disabled={Boolean(busy())}
                onClick={() => act('ignore', () => ignoreFinding().then(load))}
              >
                Show them again
              </button>
            </p>
          </Show>
        </section>

        <section class="stage" aria-label="Checks on every task">
          <div class="stage-head">
            <h2 class="stage-title">Checks on every task</h2>
            <span class="stage-note">
              A failed check stands in the way of "Merge now". Each project brings its own from its files.
            </span>
          </div>
          <Show when={health() && health()!.projects.length === 0}>
            <p class="stage-row plain">No projects yet.</p>
          </Show>
          <For each={health()?.projects ?? []}>
            {(project) => (
              <>
                <Show
                  when={project.checks.length > 0}
                  fallback={
                    <p class="stage-row plain">
                      <span class="project-tag">{projectName(project.projectId)}</span>
                      No check was found in its files. A lint or test command in the project becomes one.
                    </p>
                  }
                >
                  <For each={project.checks}>
                    {(check) => (
                      <div class="stage-row health-check">
                        <span class="project-tag">{projectName(project.projectId)}</span>
                        <span class="name">{check.title}</span>
                        <code class="mono muted">{check.command}</code>
                        <span class="grow" />
                        <button
                          type="button"
                          class="btn small"
                          classList={{ primary: check.on }}
                          aria-pressed={check.on}
                          aria-label={`${check.title} in ${projectName(project.projectId)}: ${check.on ? 'on' : 'off'}`}
                          disabled={Boolean(busy())}
                          onClick={() =>
                            act('turn', () => turnProjectCheck(project.projectId, check.id, !check.on).then(load))
                          }
                        >
                          {check.on ? 'On' : 'Off'}
                        </button>
                      </div>
                    )}
                  </For>
                </Show>
              </>
            )}
          </For>
        </section>
      </div>
    </main>
  );
}
