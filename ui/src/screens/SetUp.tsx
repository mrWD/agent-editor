import { createEffect, createSignal, For, Show, type JSX } from 'solid-js';
import { go, taskHref } from '../app/router';
import { addProject, startSetup, state } from '../core/store';

/** The value of the project picker that means "a folder that is not a project yet". */
const NEW_FOLDER = '__new__';

/**
 * Lets an agent set the editor up: Claude or Codex reads the editor's setup guide, looks at where things stand,
 * adds projects and picks Linear teams, and says what is left for the user (signing in to glab, gh and Linear).
 * A button on the home screen that opens into a form. It never opens by itself: the first run has its own
 * dialogs, which connect the same things by hand, and whoever skipped those does not want a form instead.
 */
export function SetUp(): JSX.Element {
  const [open, setOpen] = createSignal(false);
  const [told, setTold] = createSignal('');
  const [projectId, setProjectId] = createSignal('');
  const [folder, setFolder] = createSignal('');
  const [agentId, setAgentId] = createSignal('');
  const [busy, setBusy] = createSignal(false);
  const [problem, setProblem] = createSignal<string | null>(null);

  // Only agents with a model behind them can set anything up: the practice agent cannot.
  const agents = () => state.agents.filter((agent) => agent.spendsTokens);
  createEffect(() => {
    if (!agentId()) {
      const first = agents().find((agent) => agent.available);
      if (first) setAgentId(first.id);
    }
    if (!state.ready) return;
    if (!state.projects.some((project) => project.id === projectId()) && projectId() !== NEW_FOLDER) {
      setProjectId(state.projects[0]?.id ?? NEW_FOLDER);
    }
  });
  const addingFolder = () => projectId() === NEW_FOLDER;

  const start = async (event: Event) => {
    event.preventDefault();
    if (busy()) return;
    if (addingFolder() && folder().trim() === '') {
      setProblem('Give the folder of one of your repositories, for example /Users/you/code/app.');
      return;
    }
    if (!agents().find((agent) => agent.id === agentId())?.available) {
      setProblem('Pick an agent that can start: Claude or Codex.');
      return;
    }
    setProblem(null);
    setBusy(true);
    try {
      let target = projectId();
      if (addingFolder()) target = (await addProject(folder().trim())).id;
      const id = await startSetup(target, agentId(), told().trim());
      setTold('');
      setOpen(false);
      go(taskHref(id));
    } catch (error) {
      setProblem(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Show
      when={open()}
      fallback={
        <p class="soft setup-offer">
          <button type="button" class="btn small ghost" onClick={() => setOpen(true)}>
            Set up with an agent
          </button>{' '}
          Claude or Codex connects your projects, GitLab or GitHub and Linear.
        </p>
      }
    >
      <form class="card setup" onSubmit={start} aria-label="Set up the editor with an agent">
        <h2>Set up the editor with an agent</h2>
        <p>
          The agent reads the editor's setup guide, looks at what is connected, adds your repositories and picks your
          Linear teams. Signing in stays with you: it tells you the exact command or screen. It changes no code and
          commits nothing.
        </p>
        <label for="setup-told">What does your team use?</label>
        <textarea
          id="setup-told"
          class="git-message"
          rows="3"
          placeholder="For example: issues in Linear, team APP; code on our own GitLab at gitlab.example.com; repositories in ~/Projects/backend and ~/Projects/frontend."
          value={told()}
          onInput={(event) => setTold(event.currentTarget.value)}
        />
        <div class="setup-row">
          <select
            class="select-button"
            aria-label="Project the agent works from"
            value={projectId()}
            onChange={(event) => setProjectId(event.currentTarget.value)}
          >
            <For each={state.projects}>{(project) => <option value={project.id}>{project.name}</option>}</For>
            <option value={NEW_FOLDER}>A repository folder</option>
          </select>
          <Show when={addingFolder()}>
            <input
              class="field where"
              type="text"
              aria-label="Folder of a repository"
              placeholder={state.app?.cwd ?? 'Folder of a repository'}
              value={folder()}
              onInput={(event) => setFolder(event.currentTarget.value)}
            />
          </Show>
          <select
            class="select-button"
            aria-label="Agent that sets the editor up"
            value={agentId()}
            onChange={(event) => setAgentId(event.currentTarget.value)}
          >
            <For each={agents()}>
              {(agent) => (
                <option value={agent.id} disabled={!agent.available}>
                  {agent.name}
                  {agent.available ? '' : ' (not installed)'}
                  {agent.available && agent.speaks === 'terminal' ? ' (in a terminal)' : ''}
                </option>
              )}
            </For>
          </select>
          <span class="grow" />
          <button type="button" class="btn tall ghost" onClick={() => setOpen(false)}>
            Not now
          </button>
          <button type="submit" class="btn primary tall" disabled={busy()}>
            {busy() ? 'Starting' : 'Start setting up'}
          </button>
        </div>
        <Show when={problem()}>
          <p class="form-error" role="alert">
            {problem()}
          </p>
        </Show>
        <p class="fine">The agent spends tokens on your account, like any task you start.</p>
      </form>
    </Show>
  );
}
