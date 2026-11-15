import { createSignal, For, onCleanup, onMount, Show, type JSX } from 'solid-js';
import { go, taskHref } from '../app/router';
import { core } from '../core/client';
import { createKept } from '../core/kept';
import { stageOfTask } from '../core/requests';
import { stageWord } from '../core/stages';
import { addStartedTask, agentOf, projectName, state, type TaskInfo } from '../core/store';
import { ago, Chip, StageGlyph } from './bits';

/** What of Sentry is connected. The token is never sent back: only that there is one. */
type Status = { connected: boolean; url: string; org: string; projects: Record<string, string> };

/** An unresolved issue of Sentry as the core lists it. */
export type SentryIssue = {
  /** Sentry's number for it. */
  id: string;
  /** Its short name, such as SHOP-1A. */
  key: string;
  title: string;
  culprit: string;
  level: string;
  count: number;
  users: number;
  firstSeen: number;
  lastSeen: number;
  url: string;
  taskId: string | null;
};

type Listed = { slug: string | null; issues: SentryIssue[] };

const said = (error: unknown) => (error instanceof Error ? error.message : String(error));

/** "1,204 times, 87 users": how often an error happens and to how many. */
export function sentryNumbers(issue: Pick<SentryIssue, 'count' | 'users'>): string {
  const times = `${issue.count.toLocaleString('en-US')} ${issue.count === 1 ? 'time' : 'times'}`;
  if (issue.users === 0) return times;
  return `${times}, ${issue.users.toLocaleString('en-US')} ${issue.users === 1 ? 'user' : 'users'}`;
}

/**
 * The part of the Issues screen about Sentry: connecting it, joining each
 * project to one of Sentry's, and the unresolved errors of the joined ones,
 * the most frequent first. "Start" makes a task whose first message is the
 * error with its stack trace.
 */
export function SentryIssues(props: {
  /** The agent a task starts with, as picked at the head of the screen. */
  agentId: string;
  /** Only the errors nobody works on yet. */
  onlyFree: boolean;
}): JSX.Element {
  const [status, setStatus] = createKept<Status | null>('sentry.status', null);
  const [listed, setListed] = createKept<Record<string, Listed>>('sentry.issues', {});
  const [problems, setProblems] = createSignal<Record<string, string>>({});
  const [problem, setProblem] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal<string | null>(null);
  /** The projects of the organization, read when one is about to be picked. */
  const [theirs, setTheirs] = createSignal<{ slug: string; name: string }[] | null>(null);
  /** The project of the editor that is being joined, and what is picked for it. */
  const [joining, setJoining] = createSignal<string | null>(null);
  const [picked, setPicked] = createSignal('');

  const [token, setToken] = createSignal('');
  const [org, setOrg] = createSignal('');
  const [url, setUrl] = createSignal('');

  const loadIssues = async (now: Status) => {
    for (const projectId of Object.keys(now.projects)) {
      try {
        const found = await core.request<Listed>('sentry.issues', { projectId });
        setListed((before) => ({ ...before, [projectId]: found }));
        setProblems((before) => Object.fromEntries(Object.entries(before).filter(([id]) => id !== projectId)));
      } catch (error) {
        setProblems((before) => ({ ...before, [projectId]: said(error) }));
      }
    }
  };

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
      const now = await core.request<Status>('sentry.status');
      setStatus(now);
      if (now.connected) await loadIssues(now);
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
    const stop = core.onEvent((event) => {
      if (
        ['integrations.changed', 'task.created', 'task.archived', 'projects.changed', 'link.open'].includes(event.event)
      )
        void load();
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
      setProblem(said(error));
    } finally {
      setBusy(null);
    }
  };

  const connect = () => {
    const given = { token: token().trim(), org: org().trim(), url: url().trim() };
    return act('connect', async () => {
      setStatus(await core.request<Status>('sentry.connect', given));
      setToken('');
    });
  };

  const openJoin = (projectId: string) => {
    setPicked(status()?.projects[projectId] ?? '');
    setJoining(projectId);
    if (!theirs()) {
      core
        .request<{ projects: { slug: string; name: string }[] }>('sentry.projects')
        .then((found) => setTheirs(found.projects))
        .catch((error) => setProblem(said(error)));
    }
  };

  const join = (projectId: string) => {
    const slug = picked();
    return act(`join:${projectId}`, async () => {
      setStatus(await core.request<Status>('sentry.join', { projectId, slug }));
      if (!slug) setListed((before) => Object.fromEntries(Object.entries(before).filter(([id]) => id !== projectId)));
      setJoining(null);
    });
  };

  const start = (projectId: string, issue: SentryIssue) => {
    const agentId = props.agentId;
    return act(`start:${issue.id}`, async () => {
      const info = await core.request<TaskInfo>('sentry.start', { projectId, issueId: issue.id, agentId });
      await addStartedTask(info);
      go(taskHref(info.id));
    });
  };

  const shown = (projectId: string) => {
    const all = listed()[projectId]?.issues ?? [];
    return props.onlyFree ? all.filter((issue) => !issue.taskId) : all;
  };

  return (
    <Show when={status()}>
      {(now) => (
        <>
          <Show when={problem()}>
            <p class="banner danger flush" role="alert">
              <span class="grow">{problem()}</span>
              <button type="button" class="btn tiny" onClick={() => setProblem(null)}>
                Dismiss
              </button>
            </p>
          </Show>
          <section class="issues-connect" aria-label="Sentry">
            <Show
              when={!now().connected}
              fallback={
                <>
                  <span class="grow soft">
                    Connected to Sentry, organization {now().org}
                    {now().url === 'https://sentry.io' ? '' : ` at ${now().url}`}. The token is kept on this computer.
                    Errors are only read: Sentry resolves one itself when a commit that says “Fixes” with its name
                    reaches it.
                  </span>
                  <button
                    type="button"
                    class="btn small ghost"
                    disabled={busy() !== null}
                    onClick={() =>
                      void act('connect', async () =>
                        setStatus(await core.request<Status>('sentry.connect', { token: '' })),
                      )
                    }
                  >
                    Forget the token
                  </button>
                </>
              }
            >
              <div class="issues-linear-how grow">
                <strong>Connect Sentry</strong> to start a task from an error, with its stack trace.{' '}
                <span class="soft">
                  Make a token under User settings, Auth tokens, that may read events, projects and the organization.
                </span>
              </div>
              <label class="sr-only" for="sentry-org">
                Sentry organization
              </label>
              <input
                id="sentry-org"
                class="field narrow"
                placeholder="organization"
                autocomplete="off"
                value={org()}
                onInput={(event) => setOrg(event.currentTarget.value)}
              />
              <label class="sr-only" for="sentry-url">
                Address of your own Sentry
              </label>
              <input
                id="sentry-url"
                class="field"
                placeholder="sentry.io, or https://your.sentry"
                autocomplete="off"
                value={url()}
                onInput={(event) => setUrl(event.currentTarget.value)}
              />
              <label class="sr-only" for="sentry-token">
                Sentry auth token
              </label>
              <input
                id="sentry-token"
                class="field"
                type="password"
                autocomplete="off"
                placeholder="sntryu_…"
                value={token()}
                onInput={(event) => setToken(event.currentTarget.value)}
                onKeyDown={(event) => event.key === 'Enter' && void connect()}
              />
              <button
                type="button"
                class="btn small"
                disabled={busy() !== null || token().trim() === '' || org().trim() === ''}
                onClick={() => void connect()}
              >
                {busy() === 'connect' ? 'Asking Sentry' : 'Connect Sentry'}
              </button>
            </Show>
          </section>

          <Show when={now().connected}>
            <For each={state.projects}>
              {(project) => {
                const slug = () => now().projects[project.id];
                return (
                  <section class="stage" aria-label={`Errors of ${project.name} in Sentry`}>
                    <div class="stage-head">
                      <h2 class="stage-title">{projectName(project.id)}</h2>
                      <span class="stage-note">
                        {slug()
                          ? `Errors in Sentry · ${slug()} · ${(listed()[project.id]?.issues ?? []).length} unresolved`
                          : 'Errors in Sentry: not joined to a project of Sentry yet'}
                      </span>
                      <span class="grow" />
                      <Show
                        when={joining() === project.id}
                        fallback={
                          <button type="button" class="btn tiny ghost" onClick={() => openJoin(project.id)}>
                            {slug() ? 'Change the Sentry project' : 'Pick its Sentry project'}
                          </button>
                        }
                      >
                        <select
                          class="issue-place"
                          aria-label={`Sentry project for ${project.name}`}
                          value={picked()}
                          onChange={(event) => setPicked(event.currentTarget.value)}
                        >
                          <option value="">None</option>
                          <For each={theirs() ?? (slug() ? [{ slug: slug()!, name: slug()! }] : [])}>
                            {(one) => (
                              <option value={one.slug}>
                                {one.name} ({one.slug})
                              </option>
                            )}
                          </For>
                        </select>
                        <button
                          type="button"
                          class="btn tiny"
                          disabled={busy() !== null}
                          onClick={() => void join(project.id)}
                        >
                          Save
                        </button>
                        <button type="button" class="btn tiny ghost" onClick={() => setJoining(null)}>
                          Cancel
                        </button>
                      </Show>
                    </div>
                    <Show when={problems()[project.id]}>
                      <p class="stage-row plain danger">{problems()[project.id]}</p>
                    </Show>
                    <Show when={slug() && !problems()[project.id] && shown(project.id).length === 0}>
                      <p class="stage-row plain">
                        {props.onlyFree && (listed()[project.id]?.issues ?? []).length > 0
                          ? 'Every unresolved error has a task.'
                          : 'No unresolved errors.'}
                      </p>
                    </Show>
                    <For each={shown(project.id)}>
                      {(issue) => {
                        const task = () => (issue.taskId ? state.tasks[issue.taskId] : undefined);
                        return (
                          <div class="stage-row issue-row">
                            <a class="issue-key" href={issue.url || undefined} target="_blank" rel="noreferrer">
                              {issue.key}
                            </a>
                            <span class="name">{issue.title}</span>
                            <span class="tag">{issue.level}</span>
                            <span class="issue-state">
                              {sentryNumbers(issue)} · last {ago(issue.lastSeen)}
                            </span>
                            <Show
                              when={task()}
                              fallback={
                                <>
                                  <span class="where-stopped">No task yet</span>
                                  <button
                                    type="button"
                                    class="btn small"
                                    title="Start a task on it and open it in the editor: the agent, the code, the changes and the preview"
                                    disabled={busy() !== null || props.agentId === ''}
                                    aria-label={`Start a task for ${issue.key} in the editor`}
                                    onClick={() => void start(project.id, issue)}
                                  >
                                    {busy() === `start:${issue.id}` ? 'Starting' : 'Start in editor'}
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
                );
              }}
            </For>
          </Show>
        </>
      )}
    </Show>
  );
}
