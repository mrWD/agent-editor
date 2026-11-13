import { createMemo, createSignal, For, onCleanup, onMount, Show, type JSX } from 'solid-js';
import { createKept } from '../core/kept';
import { go, taskHref } from '../app/router';
import { ago, Chip, StageGlyph } from '../components/bits';
import { core } from '../core/client';
import { stageOfTask } from '../core/requests';
import { stageWord } from '../core/stages';
import {
  agentOf,
  defaultAgentId,
  integrationsStatus,
  linearAccount,
  listIssues,
  projectName,
  setLinearKey,
  setLinearTeam,
  startIssue,
  state,
  type Integrations,
  type Issue,
  type LinearAccount,
  type LinearMine,
  type ProjectIssues,
} from '../core/store';

/** Where Linear makes personal API keys. It opens in the system's browser. */
const LINEAR_KEYS = 'https://linear.app/settings/account/security';

const message = (error: unknown) => (error instanceof Error ? error.message : String(error));

/** Which issues are shown. */
type Filter = 'open' | 'free';

/**
 * Issues of every project, from wherever each project keeps them: GitHub or
 * GitLab by the project's remote, or a Linear team. An issue becomes a task
 * with one press.
 */
export function Issues(): JSX.Element {
  // What was read last time is shown at once; it is read again behind that.
  const [projects, setProjects] = createKept<ProjectIssues[] | null>('issues.projects', null);
  const [mine, setMine] = createKept<LinearMine>('issues.mine', null);
  const [account, setAccount] = createKept<LinearAccount | null>('issues.account', null);
  const [accountProblem, setAccountProblem] = createSignal<string | null>(null);
  /** For each of the user's Linear issues, the project its task would start in. */
  const [placeOf, setPlaceOf] = createKept<Record<string, string>>('issues.placeOf', {});
  const [services, setServices] = createKept<Integrations | null>('issues.services', null);
  const [syncedAt, setSyncedAt] = createKept<number | null>('issues.syncedAt', null);
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
      setMine(listed.linear);
      setServices(status);
      setSyncedAt(Date.now());
      // Who the key belongs to, said back so the user sees the right account is connected, and its teams.
      if (status.linear.hasKey) {
        try {
          setAccount(await linearAccount());
          setAccountProblem(null);
        } catch (error) {
          setAccount(null);
          setAccountProblem(message(error));
        }
      } else {
        setAccount(null);
        setAccountProblem(null);
      }
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

  const start = (projectId: string, issue: Issue, source?: 'linear') =>
    // eslint-disable-next-line solid/reactivity -- called later, as an event handler: it reads what is current then
    act(async () => {
      setStarting(`${projectId}:${issue.key}`);
      try {
        go(taskHref(await startIssue(projectId, issue.key, chosenAgent(), source)));
      } finally {
        setStarting(null);
      }
    });

  /**
   * The project a Linear issue of the user's would start in: the one picked here, else the project that takes
   * its issues from the issue's team ("OPS-12" from team OPS), else the first project.
   */
  const placeFor = (key: string): string => {
    const picked = placeOf()[key];
    const all = projects() ?? [];
    if (picked && all.some((project) => project.projectId === picked)) return picked;
    const team = key.split('-')[0];
    return all.find((project) => project.linearTeam === team)?.projectId ?? all[0]?.projectId ?? '';
  };

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
              <section class="issues-linear" aria-label="Linear">
                <Show
                  when={!status().linear.hasKey}
                  fallback={
                    <>
                      <span class="grow soft">
                        <Show
                          when={account()}
                          fallback={accountProblem() ?? 'Linear is connected with a key kept on this computer.'}
                        >
                          {(who) => (
                            <>
                              Connected to Linear
                              {who().name ? ` as ${who().name}` : ''}
                              {who().workspace ? ` in ${who().workspace}` : ''}. The key is kept on this computer.
                              <Show when={status().linear.readOnly}>
                                {' '}
                                It may only read: issues are not moved when a task starts or merges. A key with write
                                access changes that.
                              </Show>
                            </>
                          )}
                        </Show>
                      </span>
                      <button type="button" class="btn small ghost" onClick={() => act(() => setLinearKey(null))}>
                        Forget the key
                      </button>
                    </>
                  }
                >
                  <div class="issues-linear-how grow">
                    <strong>Connect Linear</strong> to see the issues assigned to you.{' '}
                    <span class="soft">
                      Linear opens in your browser: press “New API key”, copy it and paste it here.
                    </span>
                  </div>
                  <a class="btn small" href={LINEAR_KEYS} target="_blank" rel="noreferrer">
                    Open Linear
                  </a>
                  <label class="sr-only" for="linear-key">
                    Linear API key
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

        <Show when={mine()}>
          {(linear) => (
            <section class="stage" aria-label="Assigned to you in Linear">
              <div class="stage-head">
                <h2 class="stage-title">Assigned to you in Linear</h2>
                <span class="stage-note">
                  {linear().issues.length === 1 ? '1 open' : `${linear().issues.length} open`} · every team
                </span>
              </div>
              <Show when={linear().problem}>
                <p class="stage-row plain danger">{linear().problem}</p>
              </Show>
              <Show when={!linear().problem && shown(linear().issues).length === 0}>
                <p class="stage-row plain">
                  {filter() === 'free' && linear().issues.length > 0
                    ? 'Every issue assigned to you has a task.'
                    : 'Nothing is assigned to you.'}
                </p>
              </Show>
              <For each={shown(linear().issues)}>
                {(issue) => {
                  const task = () => (issue.taskId ? state.tasks[issue.taskId] : undefined);
                  return (
                    <div class="stage-row issue-row">
                      <a class="issue-key" href={issue.url} target="_blank" rel="noreferrer">
                        {issue.key}
                      </a>
                      <span class="name">{issue.title}</span>
                      <span class="issue-state">{issue.state}</span>
                      <Show
                        when={task()}
                        fallback={
                          <>
                            <select
                              class="issue-place"
                              aria-label={`Project for ${issue.key}`}
                              value={placeFor(issue.key)}
                              onChange={(event) => setPlaceOf({ ...placeOf(), [issue.key]: event.currentTarget.value })}
                            >
                              <For each={projects() ?? []}>
                                {(project) => (
                                  <option value={project.projectId}>{projectName(project.projectId)}</option>
                                )}
                              </For>
                            </select>
                            <button
                              type="button"
                              class="btn small"
                              disabled={starting() !== null || chosenAgent() === '' || placeFor(issue.key) === ''}
                              onClick={() => void start(placeFor(issue.key), issue, 'linear')}
                            >
                              {starting() === `${placeFor(issue.key)}:${issue.key}` ? 'Starting' : 'Start'}
                            </button>
                          </>
                        }
                      >
                        {(held) => (
                          <>
                            <span class="where-stopped issue-task">
                              <Chip agentId={held().info.agentId} short={agentOf(held())?.short ?? '··'} flat />
                              <StageGlyph stage={stageOfTask(held())} />
                              {stageWord(stageOfTask(held()))} · {projectName(held().info.projectId)}
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
                  <Show
                    when={account()?.teams.length}
                    fallback={
                      <input
                        class="field narrow"
                        aria-label={`Linear team key for ${projectName(project.projectId)}`}
                        placeholder="Team key, such as APP"
                        value={team()}
                        onInput={(event) => setTeam(event.currentTarget.value)}
                        onKeyDown={(event) => event.key === 'Enter' && void saveTeam(project.projectId)}
                      />
                    }
                  >
                    <select
                      class="issue-place"
                      aria-label={`Linear team for ${projectName(project.projectId)}`}
                      value={team()}
                      onChange={(event) => setTeam(event.currentTarget.value)}
                    >
                      <option value="">The code host</option>
                      <For each={account()!.teams}>
                        {(one) => (
                          <option value={one.key}>
                            {one.name} ({one.key})
                          </option>
                        )}
                      </For>
                    </select>
                  </Show>
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
