import { createMemo, createSignal, For, onCleanup, onMount, Show, type JSX } from 'solid-js';
import { go, taskHref } from '../app/router';
import { ago, Chip, StageGlyph } from '../components/bits';
import { core } from '../core/client';
import { stageOfTask } from '../core/requests';
import { stageWord } from '../core/stages';
import {
  agentOf,
  defaultAgentId,
  integrationsStatus,
  listIssues,
  projectName,
  setLinearKey,
  setLinearTeam,
  startIssue,
  state,
  type Integrations,
  type Issue,
  type ProjectIssues,
} from '../core/store';

const message = (error: unknown) => (error instanceof Error ? error.message : String(error));

/** Which issues are shown. */
type Filter = 'open' | 'free';

/**
 * Issues of every project, from wherever each project keeps them: GitHub or
 * GitLab by the project's remote, or a Linear team. An issue becomes a task
 * with one press.
 */
export function Issues(): JSX.Element {
  const [projects, setProjects] = createSignal<ProjectIssues[] | null>(null);
  const [services, setServices] = createSignal<Integrations | null>(null);
  const [syncedAt, setSyncedAt] = createSignal<number | null>(null);
  const [syncing, setSyncing] = createSignal(false);
  const [problem, setProblem] = createSignal<string | null>(null);
  const [filter, setFilter] = createSignal<Filter>('open');
  const [agentId, setAgentId] = createSignal('');
  const [starting, setStarting] = createSignal<string | null>(null);
  const [key, setKey] = createSignal('');
  const [teamOf, setTeamOf] = createSignal<string | null>(null);
  const [team, setTeam] = createSignal('');

  const agents = () => state.agents.filter((agent) => agent.available);
  const chosenAgent = () => agentId() || defaultAgentId();

  const sync = async () => {
    if (syncing()) return;
    setSyncing(true);
    setProblem(null);
    try {
      const [listed, status] = await Promise.all([listIssues(), integrationsStatus()]);
      setProjects(listed.projects);
      setServices(status);
      setSyncedAt(Date.now());
    } catch (error) {
      setProblem(message(error));
    } finally {
      setSyncing(false);
    }
  };

  onMount(() => {
    void sync();
    // eslint-disable-next-line solid/reactivity -- called later, as an event handler
    const stop = core.onEvent((event) => {
      if (['integrations.changed', 'task.created', 'task.archived', 'projects.changed'].includes(event.event))
        void sync();
    });
    onCleanup(stop);
  });

  const act = async (work: () => Promise<unknown>) => {
    setProblem(null);
    try {
      await work();
    } catch (error) {
      setProblem(message(error));
    }
  };

  const start = (projectId: string, issue: Issue) =>
    // eslint-disable-next-line solid/reactivity -- called later, as an event handler: it reads what is current then
    act(async () => {
      setStarting(`${projectId}:${issue.key}`);
      try {
        go(taskHref(await startIssue(projectId, issue.key, chosenAgent())));
      } finally {
        setStarting(null);
      }
    });

  const saveKey = () =>
    // eslint-disable-next-line solid/reactivity -- called later, as an event handler: it reads what is current then
    act(async () => {
      await setLinearKey(key().trim() || null);
      setKey('');
    });

  const saveTeam = (projectId: string) =>
    // eslint-disable-next-line solid/reactivity -- called later, as an event handler: it reads what is current then
    act(async () => {
      await setLinearTeam(projectId, team().trim() || null);
      setTeamOf(null);
    });

  /** One line on what is connected, so nothing has to be guessed. */
  const connected = createMemo(() => {
    const status = services();
    if (!status) return '';
    const parts: string[] = [];
    if (status.github.signedIn) parts.push('GitHub');
    if (status.gitlab.signedIn) parts.push('GitLab');
    if (status.linear.hasKey) parts.push('Linear');
    if (parts.length === 0) return 'Nothing is connected yet.';
    return `${parts.join(', ')} ${parts.length === 1 ? 'is' : 'are'} connected.`;
  });

  const shown = (list: Issue[]) => (filter() === 'free' ? list.filter((issue) => !issue.taskId) : list);
  const used = (kind: 'github' | 'gitlab' | 'linear') => (projects() ?? []).some((project) => project.source === kind);

  return (
    <main class="page">
      <div class="page-inner wide">
        <div class="issues-status">
          <span class="soft">
            {projects() === null ? 'Reading the issues' : connected()}
            <Show when={syncedAt()}>{(at) => <> Synced {ago(at())}.</>}</Show>
          </span>
          <button type="button" class="btn small" disabled={syncing()} onClick={() => void sync()}>
            {syncing() ? 'Syncing' : 'Sync now'}
          </button>
        </div>

        <div class="issues-top">
          <h1 class="headline">Issues</h1>
          <div class="switcher" role="group" aria-label="Which issues to show">
            <button type="button" aria-pressed={filter() === 'open'} onClick={() => setFilter('open')}>
              Open
            </button>
            <button type="button" aria-pressed={filter() === 'free'} onClick={() => setFilter('free')}>
              Without a task
            </button>
          </div>
          <span class="grow" />
          <Show when={agents().length > 0}>
            <label class="issues-agent">
              Start with
              <select value={chosenAgent()} onChange={(event) => setAgentId(event.currentTarget.value)}>
                <For each={agents()}>{(agent) => <option value={agent.id}>{agent.name}</option>}</For>
              </select>
            </label>
          </Show>
        </div>
        <p class="fine">
          Starting a task from a Linear issue moves the issue to In progress; merging its request moves it to Done.
        </p>

        <Show when={problem()}>
          <p class="banner danger flush" role="alert">
            <span class="grow">{problem()}</span>
            <button type="button" class="btn tiny" onClick={() => setProblem(null)}>
              Dismiss
            </button>
          </p>
        </Show>

        {/* What is not connected says how to connect it, once, above the lists. */}
        <Show when={services()}>
          {(status) => (
            <>
              <Show when={used('github') && !status().github.signedIn}>
                <p class="banner warn flush">{status().github.hint}</p>
              </Show>
              <Show when={used('gitlab') && !status().gitlab.signedIn}>
                <p class="banner warn flush">{status().gitlab.hint}</p>
              </Show>
              <section class="issues-linear">
                <Show
                  when={!status().linear.hasKey}
                  fallback={
                    <>
                      <span class="grow soft">Linear is connected with a key kept on this computer.</span>
                      <button type="button" class="btn small ghost" onClick={() => act(() => setLinearKey(null))}>
                        Forget the key
                      </button>
                    </>
                  }
                >
                  <label class="grow" for="linear-key">
                    Connect Linear: paste a personal API key (Linear, Settings, Security and access)
                  </label>
                  <input
                    id="linear-key"
                    class="field"
                    type="password"
                    autocomplete="off"
                    placeholder="lin_api_…"
                    value={key()}
                    onInput={(event) => setKey(event.currentTarget.value)}
                    onKeyDown={(event) => event.key === 'Enter' && void saveKey()}
                  />
                  <button type="button" class="btn small" disabled={key().trim() === ''} onClick={() => void saveKey()}>
                    Connect
                  </button>
                </Show>
              </section>
            </>
          )}
        </Show>

        <Show when={projects() !== null && projects()!.length === 0}>
          <p class="soft">No projects yet. Start a task from Home and its folder becomes a project.</p>
        </Show>

        <For each={projects() ?? []}>
          {(project) => (
            <section class="stage">
              <div class="stage-head">
                <h2 class="stage-title">{projectName(project.projectId)}</h2>
                <span class="stage-note">
                  {project.source === 'none'
                    ? 'No tracker: its remote is not on GitHub or GitLab, and no Linear team is set'
                    : `${project.label} · ${project.issues.length} open`}
                </span>
                <span class="grow" />
                <Show
                  when={teamOf() === project.projectId}
                  fallback={
                    <button
                      type="button"
                      class="btn tiny ghost"
                      onClick={() => {
                        setTeam(project.linearTeam ?? '');
                        setTeamOf(project.projectId);
                      }}
                    >
                      {project.linearTeam ? 'Change the Linear team' : 'Use a Linear team'}
                    </button>
                  }
                >
                  <input
                    class="field narrow"
                    aria-label={`Linear team key for ${projectName(project.projectId)}`}
                    placeholder="Team key, such as APP"
                    value={team()}
                    onInput={(event) => setTeam(event.currentTarget.value)}
                    onKeyDown={(event) => event.key === 'Enter' && void saveTeam(project.projectId)}
                  />
                  <button type="button" class="btn tiny" onClick={() => void saveTeam(project.projectId)}>
                    {team().trim() === '' ? 'Use the code host' : 'Save'}
                  </button>
                  <button type="button" class="btn tiny ghost" onClick={() => setTeamOf(null)}>
                    Cancel
                  </button>
                </Show>
              </div>

              <Show when={project.problem}>
                <p class="stage-row plain danger">{project.problem}</p>
              </Show>
              <Show when={!project.problem && shown(project.issues).length === 0}>
                <p class="stage-row plain">
                  {project.source === 'none'
                    ? 'Nothing to list.'
                    : filter() === 'free' && project.issues.length > 0
                      ? 'Every open issue has a task.'
                      : 'No open issues.'}
                </p>
              </Show>
              <For each={shown(project.issues)}>
                {(issue) => {
                  const task = () => (issue.taskId ? state.tasks[issue.taskId] : undefined);
                  return (
                    <div class="stage-row issue-row">
                      <a class="issue-key" href={issue.url} target="_blank" rel="noreferrer">
                        {issue.key}
                      </a>
                      <span class="name">{issue.title}</span>
                      <For each={issue.labels.slice(0, 3)}>{(label) => <span class="tag">{label}</span>}</For>
                      <span class="issue-state">{issue.state}</span>
                      <Show
                        when={task()}
                        fallback={
                          <>
                            <span class="where-stopped">No task yet</span>
                            <button
                              type="button"
                              class="btn small"
                              disabled={starting() !== null || chosenAgent() === ''}
                              onClick={() => void start(project.projectId, issue)}
                            >
                              {starting() === `${project.projectId}:${issue.key}` ? 'Starting' : 'Start'}
                            </button>
                          </>
                        }
                      >
                        {(held) => (
                          <>
                            <span class="where-stopped issue-task">
                              <Chip agentId={held().info.agentId} short={agentOf(held())?.short ?? '··'} flat />
                              <StageGlyph stage={stageOfTask(held())} />
                              {stageWord(stageOfTask(held()))}
                            </span>
                            <a class="btn small" href={taskHref(held().info.id)}>
                              Open
                            </a>
                          </>
                        )}
                      </Show>
                    </div>
                  );
                }}
              </For>
            </section>
          )}
        </For>
      </div>
    </main>
  );
}
