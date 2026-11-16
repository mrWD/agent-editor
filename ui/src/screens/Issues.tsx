import { createDetails, DetailDock, IssuePanel, RequestPanel } from '../components/DetailPanel';
import { SentryIssues } from '../components/SentryIssues';
import { requestName, type Detail } from '../core/details';
import { refreshRequests, requests } from '../core/requests';
import { createMemo, createSignal, For, onCleanup, onMount, Show, type JSX } from 'solid-js';
import { createKept } from '../core/kept';
import { go, taskHref } from '../app/router';
import { ago, Chip, StageGlyph } from '../components/bits';
import { core } from '../core/client';
import { stageOfTask } from '../core/requests';
import { stageWord } from '../core/stages';
import { countWords, ISSUE_SORTS, issueSort, looked, setIssueSort, wordsOf, type IssueSort } from '../core/issueList';
import {
  agentOf,
  defaultAgentId,
  integrationsStatus,
  issueRequests,
  linearAccount,
  listIssues,
  projectName,
  setLinearKey,
  setLinearTeam,
  startIssue,
  state,
  type Integrations,
  type Issue,
  type IssueRequest,
  type LinearAccount,
  type LinearMine,
  type ProjectIssues,
} from '../core/store';

/** Where Linear makes personal API keys. It opens in the system's browser. */
const LINEAR_KEYS = 'https://linear.app/settings/account/security';

const message = (error: unknown) => (error instanceof Error ? error.message : String(error));

/** Which issues are shown. */
type Filter = 'open' | 'free';

/** The items of a list that read the same as before, kept as the very things they were. */
function steady<T>(before: T[], now: T[]): T[] {
  const kept = new Map(before.map((item) => [JSON.stringify(item), item]));
  return now.map((item) => kept.get(JSON.stringify(item)) ?? item);
}

/** What can be done with the issue that is open at the side: start a task on it, or go to the one it has. */
function IssueFoot(props: { taskId: string | null; busy: boolean; onStart: () => void }): JSX.Element {
  return (
    <Show
      when={props.taskId}
      fallback={
        <button
          type="button"
          class="btn"
          title="Start a task on it and open it in the editor: the agent, the code, the changes and the preview"
          disabled={props.busy}
          onClick={() => props.onStart()}
        >
          Start in editor
        </button>
      }
    >
      {(taskId) => (
        <a
          class="btn"
          href={taskHref(taskId())}
          title="Open its task in the editor: the agent, the code, the changes and the preview"
        >
          Open in editor
        </a>
      )}
    </Show>
  );
}

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
  // What is typed into the search stays while the window does: coming back from a task, the list is as it was left.
  const [query, setQuery] = createKept('issues.search', '');
  const searching = () => wordsOf(query()).length > 0;
  const [agentId, setAgentId] = createSignal('');
  const [starting, setStarting] = createSignal<string | null>(null);
  const [key, setKey] = createSignal('');
  const [teamOf, setTeamOf] = createSignal<string | null>(null);
  const [team, setTeam] = createSignal('');

  /** The issue that is open at the side of the screen, whole. */
  const details = createDetails();
  const openIssue = (projectId: string, issue: Issue, source: 'linear' | undefined, foot: () => JSX.Element) =>
    details.open({
      id: `issue:${source ?? projectId}:${issue.key}`,
      title: issue.title,
      projectId,
      ask: () => core.request<Detail>('issues.detail', { projectId, key: issue.key, source }),
      foot,
    });

  /** The request of a task that works on an issue, whole, in a panel of its own beside the issue's. */
  const requestDetails = createDetails();
  const requestOf = (taskId: string | null) => (taskId ? requests[taskId]?.request : undefined);
  const openRequest = (taskId: string, projectId: string) =>
    requestDetails.open({
      id: `request:${taskId}`,
      title: requestOf(taskId)?.title ?? 'Request',
      projectId,
      taskId,
      ask: () => core.request<Detail>('mr.detail', { taskId }),
    });

  const agents = () => state.agents.filter((agent) => agent.available);
  const chosenAgent = () => agentId() || defaultAgentId();

  // One reading at a time. A reading asked for while one is on its way is not dropped: what the one
  // on its way brings may be older than what it was asked for (a key that was just saved), so another
  // follows it.
  let again = false;
  const sync = async () => {
    if (syncing()) {
      again = true;
      return;
    }
    setSyncing(true);
    setProblem(null);
    try {
      const [listed, status] = await Promise.all([listIssues(), integrationsStatus()]);
      // What reads the same as before stays the same thing, so its row stays as drawn: a press
      // that lands while the list is read again is not lost.
      const before = projects() ?? [];
      setProjects(
        steady(
          before,
          listed.projects.map((project) => {
            const was = before.find((one) => one.projectId === project.projectId);
            return { ...project, issues: steady(was?.issues ?? [], project.issues) };
          }),
        ),
      );
      const mineBefore = mine();
      setMine(listed.linear && { ...listed.linear, issues: steady(mineBefore?.issues ?? [], listed.linear.issues) });
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
      if (again) {
        again = false;
        void sync();
      }
    }
  };

  onMount(() => {
    // Which tasks have a request is known to Home; this screen asks too, for the ones opened straight here.
    void refreshRequests();
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

  /**
   * An issue somebody works on already: the open requests attached to it, found when a task was
   * about to start. The user says whether the task goes on with one of them or starts anew.
   */
  const [underWay, setUnderWay] = createSignal<{
    projectId: string;
    issue: Issue;
    source?: 'linear';
    requests: IssueRequest[];
  } | null>(null);

  /** Starts the task, on the branch of a request when one is given, and opens it. */
  const startNow = (projectId: string, issue: Issue, source?: 'linear', request?: number) =>
    // eslint-disable-next-line solid/reactivity -- called later, as an event handler: it reads what is current then
    act(async () => {
      setUnderWay(null);
      setStarting(`${projectId}:${issue.key}`);
      try {
        go(taskHref(await startIssue(projectId, issue.key, chosenAgent(), source, request)));
      } finally {
        setStarting(null);
      }
    });

  /**
   * What "Start in editor" does: looks first whether the issue has a request open in this
   * repository. With none, which is the usual case, the task starts at once on a branch of its own.
   */
  const start = async (projectId: string, issue: Issue, source?: 'linear') => {
    setProblem(null);
    setStarting(`${projectId}:${issue.key}`);
    let requests: IssueRequest[] = [];
    try {
      requests = await issueRequests(projectId, issue.key, source);
    } catch {
      // Not knowing is no reason not to start: the task gets a branch of its own, as ever.
    }
    if (requests.length > 0) {
      setStarting(null);
      setUnderWay({ projectId, issue, source, requests });
      return;
    }
    await startNow(projectId, issue, source);
  };

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

  /** The issues of a list that are on show: the ones the filter leaves and the search finds, in the order picked. */
  const shown = (list: Issue[]) =>
    looked(filter() === 'free' ? list.filter((issue) => !issue.taskId) : list, query(), issueSort(), (issue) => ({
      key: issue.key,
      title: issue.title,
      state: issue.state,
      changedAt: issue.updatedAt,
      more: [...issue.labels, issue.assignee],
    }));
  /** Why a list that has issues shows none: the search found nothing, or every one has a task. */
  const noneWords = (list: Issue[], allTaken: string, none: string) => {
    if (list.length === 0) return none;
    if (searching()) return `Nothing here matches “${query().trim()}”.`;
    return filter() === 'free' ? allTaken : none;
  };
  const used = (kind: 'github' | 'gitlab' | 'linear') => (projects() ?? []).some((project) => project.source === kind);

  return (
    <div class="page-with-side">
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
            <input
              class="field issues-search"
              type="search"
              aria-label="Search the issues"
              title="Finds by name, key, state, label and who has it; every word has to be there"
              placeholder="Search by name, key or label"
              spellcheck={false}
              value={query()}
              onInput={(event) => setQuery(event.currentTarget.value)}
              onKeyDown={(event) => {
                if (event.key !== 'Escape' || query() === '') return;
                // Escape empties the search first; the panels at the side are closed by the next one.
                event.preventDefault();
                setQuery('');
              }}
            />
            <label class="issues-agent">
              Sort
              <select value={issueSort()} onChange={(event) => setIssueSort(event.currentTarget.value as IssueSort)}>
                <For each={ISSUE_SORTS}>{([sort, words]) => <option value={sort}>{words}</option>}</For>
              </select>
            </label>
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
                    <button
                      type="button"
                      class="btn small"
                      disabled={key().trim() === ''}
                      onClick={() => void saveKey()}
                    >
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
                    {countWords(shown(linear().issues).length, linear().issues.length, query())} · every team
                  </span>
                </div>
                <Show when={linear().problem}>
                  <p class="stage-row plain danger">{linear().problem}</p>
                </Show>
                <Show when={!linear().problem && shown(linear().issues).length === 0}>
                  <p class="stage-row plain">
                    {noneWords(
                      linear().issues,
                      'Every issue assigned to you has a task.',
                      'Nothing is assigned to you.',
                    )}
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
                        <button
                          type="button"
                          class="name issue-open"
                          title={`${issue.title}\nRead it here: the description, the comments and the pictures`}
                          onClick={() =>
                            openIssue(placeFor(issue.key), issue, 'linear', () => (
                              <IssueFoot
                                taskId={issue.taskId}
                                busy={starting() !== null || chosenAgent() === '' || placeFor(issue.key) === ''}
                                onStart={() => void start(placeFor(issue.key), issue, 'linear')}
                              />
                            ))
                          }
                        >
                          {issue.title}
                        </button>
                        <span class="issue-state">{issue.state}</span>
                        <Show
                          when={task()}
                          fallback={
                            <>
                              <select
                                class="issue-place"
                                aria-label={`Project for ${issue.key}`}
                                value={placeFor(issue.key)}
                                onChange={(event) =>
                                  setPlaceOf({ ...placeOf(), [issue.key]: event.currentTarget.value })
                                }
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
                                title="Start a task on it and open it in the editor: the agent, the code, the changes and the preview"
                                disabled={starting() !== null || chosenAgent() === '' || placeFor(issue.key) === ''}
                                onClick={() => void start(placeFor(issue.key), issue, 'linear')}
                              >
                                {starting() === `${placeFor(issue.key)}:${issue.key}` ? 'Starting' : 'Start in editor'}
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
                              <Show when={requestOf(held().info.id)}>
                                {(request) => (
                                  <button
                                    type="button"
                                    class="btn small ghost"
                                    title="Read the request here: what it says, its comments, its changes and its checks"
                                    onClick={() => openRequest(held().info.id, held().info.projectId)}
                                  >
                                    {requestName(request())}
                                  </button>
                                )}
                              </Show>
                              <a
                                class="btn small"
                                href={taskHref(held().info.id)}
                                title="Open its task in the editor: the agent, the code, the changes and the preview"
                              >
                                Open in editor
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
                      : `${project.label} · ${countWords(shown(project.issues).length, project.issues.length, query())}`}
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
                      : noneWords(project.issues, 'Every open issue has a task.', 'No open issues.')}
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
                        <button
                          type="button"
                          class="name issue-open"
                          title={`${issue.title}\nRead it here: the description, the comments and the pictures`}
                          onClick={() =>
                            openIssue(project.projectId, issue, undefined, () => (
                              <IssueFoot
                                taskId={issue.taskId}
                                busy={starting() !== null || chosenAgent() === ''}
                                onStart={() => void start(project.projectId, issue)}
                              />
                            ))
                          }
                        >
                          {issue.title}
                        </button>
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
                                title="Start a task on it and open it in the editor: the agent, the code, the changes and the preview"
                                disabled={starting() !== null || chosenAgent() === ''}
                                onClick={() => void start(project.projectId, issue)}
                              >
                                {starting() === `${project.projectId}:${issue.key}` ? 'Starting' : 'Start in editor'}
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
                              <Show when={requestOf(held().info.id)}>
                                {(request) => (
                                  <button
                                    type="button"
                                    class="btn small ghost"
                                    title="Read the request here: what it says, its comments, its changes and its checks"
                                    onClick={() => openRequest(held().info.id, held().info.projectId)}
                                  >
                                    {requestName(request())}
                                  </button>
                                )}
                              </Show>
                              <a
                                class="btn small"
                                href={taskHref(held().info.id)}
                                title="Open its task in the editor: the agent, the code, the changes and the preview"
                              >
                                Open in editor
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

          <SentryIssues agentId={chosenAgent()} onlyFree={filter() === 'free'} query={query()} sort={issueSort()} />
        </div>
      </main>
      <Show when={underWay()}>
        {(found) => (
          <>
            <div class="scrim" onClick={() => setUnderWay(null)} />
            <section
              class="ask"
              role="dialog"
              aria-modal="true"
              aria-label={`${found().issue.key} has work under way`}
              onKeyDown={(event) => event.key === 'Escape' && (event.stopPropagation(), setUnderWay(null))}
            >
              <h2 class="ask-title">{found().issue.key} has work under way</h2>
              <p class="soft">
                {found().requests.length === 1
                  ? 'A request for it is open in this repository.'
                  : 'Requests for it are open in this repository.'}{' '}
                The task can go on with that work, on its branch, or start on a new branch of its own.
              </p>
              <For each={found().requests}>
                {(request, index) => (
                  <div class="ask-row">
                    <div class="ask-what">
                      <a href={request.url} target="_blank" rel="noreferrer">
                        {request.name}
                      </a>{' '}
                      {request.title}
                      <span class="fine">
                        {request.draft ? 'Draft · ' : ''}
                        <span class="mono">{request.branch}</span> into <span class="mono">{request.base}</span>
                      </span>
                    </div>
                    <button
                      type="button"
                      class="btn small"
                      classList={{ primary: index() === 0 }}
                      ref={(el) => index() === 0 && queueMicrotask(() => el.focus())}
                      title={`The task works on ${request.branch}: what is committed there is where it begins, and what it pushes goes to ${request.name}`}
                      onClick={() => void startNow(found().projectId, found().issue, found().source, request.number)}
                    >
                      Go on with {request.name}
                    </button>
                  </div>
                )}
              </For>
              <div class="actions">
                <button
                  type="button"
                  class="btn small"
                  title="A branch of the task's own, from what the project's folder has checked out now"
                  onClick={() => void startNow(found().projectId, found().issue, found().source)}
                >
                  Start on a new branch
                </button>
                <button type="button" class="btn small ghost" onClick={() => setUnderWay(null)}>
                  Cancel
                </button>
              </div>
            </section>
          </>
        )}
      </Show>
      {/* Two panels, each of its own thing: the issue, and the request of the task that works on it. */}
      <DetailDock beside>
        <IssuePanel details={details} />
        <RequestPanel details={requestDetails} />
      </DetailDock>
    </div>
  );
}
