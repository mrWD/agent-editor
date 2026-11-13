import { createEffect, createMemo, createSignal, For, Show, type JSX } from 'solid-js';
import { go, RACE_VIEW, taskHref } from '../app/router';
import { ago, Chip, money } from '../components/bits';
import { failedNote, setRaceNotice } from '../core/race';
import { refreshRequests, requests, stageOfTask } from '../core/requests';
import { chooseFolder, shellMark } from '../core/shell';
import {
  addProject,
  agentOf,
  answerPermission,
  archiveTask,
  cancelTurn,
  cleanUpTask,
  createTask,
  mergeRequest,
  projectFilter,
  projectName,
  startRace,
  state,
  tasksNewestFirst,
  type FeedItem,
  type Task,
  usageSummary,
} from '../core/store';

const words = ['Nothing', 'One thing', 'Two things', 'Three things', 'Four things', 'Five things'];

function countWords(count: number): string {
  return words[count] ?? `${count} things`;
}

/** The open permission question of a task, if it has one. */
function openQuestion(task: Task) {
  for (let i = task.feed.length - 1; i >= 0; i--) {
    const entry = task.feed[i];
    if (entry.kind === 'permission' && !entry.answered) return entry;
  }
  return null;
}

/** One line on where a task stands: the last thing the agent said, or what it is doing. */
function whereItStands(task: Task): string {
  const lastSaid = [...task.feed]
    .reverse()
    .find((entry): entry is Extract<FeedItem, { kind: 'agent' }> => entry.kind === 'agent');
  const when = ago(task.lastAt);
  if (task.info.state === 'running') return `Working. Last sign of life ${when}.`;
  if (lastSaid) {
    const said = lastSaid.text.replace(/\s+/g, ' ').trim();
    return `${when}: ${said.length > 140 ? `${said.slice(0, 139)}…` : said}`;
  }
  return `Nothing said yet. Last change ${when}.`;
}

/** The value of the project picker that means "a folder that is not a project yet". */
const NEW_FOLDER = '__new__';

function NewTask(): JSX.Element {
  const [what, setWhat] = createSignal('');
  const [projectId, setProjectId] = createSignal('');
  const [folder, setFolder] = createSignal('');
  const [agentId, setAgentId] = createSignal('');
  const [busy, setBusy] = createSignal(false);
  const [problem, setProblem] = createSignal<string | null>(null);

  // Fill in the defaults once the core has told us what it has.
  createEffect(() => {
    if (!agentId()) {
      const first = state.agents.find((agent) => agent.available);
      if (first) setAgentId(first.id);
    }
    if (!state.ready) return;
    const known = state.projects.some((project) => project.id === projectId());
    if (!known && projectId() !== NEW_FOLDER) {
      setProjectId(projectFilter() ?? state.projects[0]?.id ?? NEW_FOLDER);
    }
  });
  // Picking a project at the top of Home also picks it for the next task.
  createEffect(() => {
    const filtered = projectFilter();
    if (filtered) setProjectId(filtered);
  });

  const chosen = () => state.agents.find((agent) => agent.id === agentId());
  // A race: the same task goes to several agents at once. Offered only when there is more than one to send it to.
  const available = () => state.agents.filter((agent) => agent.available);
  const [racing, setRacing] = createSignal(false);
  const [ticked, setTicked] = createSignal<string[]>([]);
  const racers = () => available().filter((agent) => ticked().includes(agent.id));
  const turnRace = () => {
    // The agent picked for one task is the first one in; the others are ticked by hand, since some cost money.
    if (!racing() && racers().length === 0 && chosen()?.available) setTicked([agentId()]);
    setRacing(!racing());
    setProblem(null);
  };
  const tick = (id: string, on: boolean) => setTicked(on ? [...ticked(), id] : ticked().filter((one) => one !== id));
  const spends = () => (racing() ? racers().some((agent) => agent.spendsTokens) : Boolean(chosen()?.spendsTokens));
  /** Set when the day's budget is spent: what to tell the user before the second press. */
  const [overBudget, setOverBudget] = createSignal<string | null>(null);
  const addingFolder = () => projectId() === NEW_FOLDER;

  // Only the desktop app has a folder dialog. In a browser the path is typed, and typing works in the app too.
  const hasFolderDialog = shellMark() !== null;
  const [choosing, setChoosing] = createSignal(false);
  const choose = async () => {
    if (choosing()) return;
    setChoosing(true);
    try {
      const picked = await chooseFolder();
      // Closing the dialog changes nothing: what was typed stays.
      if (picked) {
        setFolder(picked);
        setProblem(null);
      }
    } catch (error) {
      setProblem(error instanceof Error ? error.message : String(error));
    } finally {
      setChoosing(false);
    }
  };

  const submit = async (event: Event) => {
    event.preventDefault();
    if (busy()) return;
    if (what().trim() === '') {
      setProblem('Describe the task first.');
      return;
    }
    if (addingFolder() && folder().trim() === '') {
      setProblem('Give the folder of the project, for example /Users/you/code/app.');
      return;
    }
    if (!racing() && !chosen()?.available) {
      setProblem(chosen()?.note ?? 'Pick an agent that is installed.');
      return;
    }
    if (racing() && (racers().length < 2 || racers().length > 4)) {
      setProblem(racers().length < 2 ? 'Tick at least two agents for a race.' : 'A race takes four agents at most.');
      return;
    }
    setProblem(null);
    setBusy(true);
    try {
      // Once the day's budget is spent, a task that costs money is started only on a second, knowing press.
      if (spends() && overBudget() === null) {
        const usage = await usageSummary().catch(() => null);
        const daily = usage?.budget.daily ?? null;
        if (usage && usage.budget.ask && daily !== null && usage.today >= daily) {
          setOverBudget(
            `You have spent ${money(usage.today, usage.currency)} today; your daily budget is ${money(daily, usage.currency)}.`,
          );
          return;
        }
      }
      setOverBudget(null);
      let target = projectId();
      if (addingFolder()) {
        target = (await addProject(folder().trim())).id;
        setProjectId(target);
        setFolder('');
      }
      if (racing()) {
        const started = await startRace(
          target,
          racers().map((agent) => agent.id),
          what().trim(),
        );
        const note = failedNote(started);
        setWhat('');
        if (started.race) {
          setRaceNotice(started.race.id, note);
          setRacing(false);
          go(taskHref(started.tasks[0]!.id, RACE_VIEW));
        } else {
          // One agent alone is no race: its task is on the lists below, and why the others did not start is said here.
          setProblem(note);
        }
        return;
      }
      const id = await createTask(target, agentId(), what().trim());
      setWhat('');
      go(taskHref(id));
    } catch (error) {
      setProblem(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  };

  return (
    <form class="new-task" onSubmit={submit}>
      <label class="title" for="task-what">
        New task
      </label>
      <input
        id="task-what"
        class="field what"
        type="text"
        placeholder="Describe what the agent should do"
        value={what()}
        onInput={(event) => setWhat(event.currentTarget.value)}
      />
      <select
        class="select-button"
        aria-label="Project"
        value={projectId()}
        onChange={(event) => setProjectId(event.currentTarget.value)}
      >
        <For each={state.projects}>{(project) => <option value={project.id}>{project.name}</option>}</For>
        <option value={NEW_FOLDER}>Another folder</option>
      </select>
      <Show when={addingFolder()}>
        <input
          class="field where"
          type="text"
          aria-label="Folder of the new project"
          placeholder={state.app?.cwd ?? 'Folder of the project'}
          value={folder()}
          onInput={(event) => setFolder(event.currentTarget.value)}
        />
        <Show when={hasFolderDialog}>
          <button type="button" class="btn tall" disabled={choosing()} onClick={() => void choose()}>
            Choose folder
          </button>
        </Show>
      </Show>
      <select
        class="select-button"
        aria-label="Agent"
        value={agentId()}
        style={{ display: racing() ? 'none' : undefined }}
        onChange={(event) => setAgentId(event.currentTarget.value)}
      >
        <For each={state.agents}>
          {(agent) => (
            <option value={agent.id} disabled={!agent.available}>
              {agent.name}
              {agent.available ? '' : ' (not installed)'}
            </option>
          )}
        </For>
      </select>
      <Show when={available().length > 1}>
        <button
          type="button"
          class="btn tall race-toggle"
          aria-pressed={racing()}
          title="Send this task to several agents at once, each in its own copy of the code, and pick the best result"
          onClick={turnRace}
        >
          Race
        </button>
        <Show when={racing()}>
          <div class="race-ticks" role="group" aria-label="Agents in the race">
            <For each={available()}>
              {(agent) => (
                <label class="race-tick">
                  <input
                    type="checkbox"
                    checked={ticked().includes(agent.id)}
                    onChange={(event) => tick(agent.id, event.currentTarget.checked)}
                  />
                  <Chip agentId={agent.id} short={agent.short} />
                  {agent.name}
                </label>
              )}
            </For>
          </div>
        </Show>
      </Show>
      <button type="submit" class="btn primary tall" disabled={busy()}>
        {busy() ? 'Starting' : overBudget() ? 'Start anyway' : racing() ? 'Start the race' : 'Start'}
      </button>
      <Show when={overBudget()}>
        <p class="form-note" role="status">
          {overBudget()} The budget is set under Agents.
        </p>
      </Show>
      <Show when={problem()}>
        <p class="form-error" role="alert">
          {problem()}
        </p>
      </Show>
      <Show when={!problem() && racing()}>
        <p class="form-note">
          Each agent works in its own copy of the code. You compare what they did and pick one result.
          <Show when={spends()}>
            {' '}
            {racers()
              .filter((agent) => agent.spendsTokens)
              .map((agent) => agent.name)
              .join(', ')}{' '}
            {racers().filter((agent) => agent.spendsTokens).length === 1 ? 'spends' : 'spend'} tokens on your account.
          </Show>
        </p>
      </Show>
      <Show when={!problem() && !racing() && chosen()?.spendsTokens}>
        <p class="form-note">
          {chosen()!.name} spends tokens on your account. {chosen()!.note}
        </p>
      </Show>
    </form>
  );
}

function Stage(props: {
  title: string;
  note: string;
  tasks: Task[];
  /** A button for the whole stage, shown at the right of its title. */
  head?: JSX.Element;
  /** What is said about each task instead of the usual "where it stands". */
  where?: (task: Task) => string;
  action?: (task: Task) => JSX.Element;
}): JSX.Element {
  return (
    <Show when={props.tasks.length > 0}>
      <section class="stage">
        <div class="stage-head">
          <h2 class="stage-title">
            {props.title} <span class="count">{props.tasks.length}</span>
          </h2>
          <span class="stage-note">{props.note}</span>
          <Show when={props.head}>
            <span class="grow" />
            {props.head}
          </Show>
        </div>
        <For each={props.tasks}>
          {(task) => (
            <div class="stage-row">
              <span class="project">{projectName(task.info.projectId)}</span>
              <a class="name" href={taskHref(task.info.id)}>
                {task.info.title}
              </a>
              <Show when={task.info.issue}>{(issue) => <span class="tag mono">{issue().key}</span>}</Show>
              <Show when={task.info.race}>
                <a
                  class="tag race-tag"
                  href={taskHref(task.info.id, RACE_VIEW)}
                  title="One of several agents racing on the same task. Opens the results side by side."
                >
                  Race · {agentOf(task)?.name ?? task.info.agentId}
                </a>
              </Show>
              <span class="where-stopped">{props.where ? props.where(task) : whereItStands(task)}</span>
              <a class="btn small" href={taskHref(task.info.id)}>
                Code
              </a>
              {props.action?.(task)}
            </div>
          )}
        </For>
      </section>
    </Show>
  );
}

/** The first screen: every task of every project, grouped by what it needs from you. */
export function Home(): JSX.Element {
  const tasks = createMemo(() =>
    tasksNewestFirst().filter((task) => projectFilter() === null || task.info.projectId === projectFilter()),
  );
  const inStage = (wanted: ReturnType<typeof stageOfTask>) => tasks().filter((task) => stageOfTask(task) === wanted);
  const needsYou = createMemo(() => tasks().filter((task) => task.info.state === 'waiting_permission'));
  // A merge request whose checks failed needs the user as much as a question from the agent does.
  const checksFailed = createMemo(() => inStage('needs').filter((task) => task.info.state !== 'waiting_permission'));
  const ready = createMemo(() => inStage('ready'));
  const checking = createMemo(() => inStage('checks'));
  const merged = createMemo(() => inStage('merged'));
  const yourTurn = createMemo(() => inStage('turn'));
  const working = createMemo(() => inStage('running'));

  const [bulk, setBulk] = createSignal<string | null>(null);
  const [bulkProblem, setBulkProblem] = createSignal<string | null>(null);
  /** Does one thing to several tasks, one after another, and says which of them it could not do. */
  const forAll = async (name: string, list: Task[], work: (task: Task) => Promise<unknown>) => {
    if (bulk()) return;
    setBulk(name);
    setBulkProblem(null);
    const failed: string[] = [];
    for (const task of list) {
      try {
        await work(task);
      } catch (error) {
        failed.push(`${task.info.title}: ${error instanceof Error ? error.message : String(error)}`);
      }
    }
    if (failed.length > 0) setBulkProblem(failed.join(' '));
    setBulk(null);
    void refreshRequests();
  };
  const finish = (task: Task) => mergeRequest(task.info.id, { squash: true, deleteBranch: true, archive: true });
  const projects = createMemo(() => new Set(tasks().map((task) => task.info.projectId)).size);

  const headline = () => {
    const need = needsYou().length + checksFailed().length;
    const done = ready().length;
    const turn = yourTurn().length;
    if (tasks().length === 0) return 'No tasks yet';
    const first = need === 0 ? 'Nothing needs you' : `${countWords(need)} ${need === 1 ? 'needs' : 'need'} you`;
    if (done > 0) return `${first}, ${countWords(done).toLowerCase()} ${done === 1 ? 'is' : 'are'} ready to merge`;
    return turn > 0
      ? `${first}, ${countWords(turn).toLowerCase()} ${turn === 1 ? 'waits' : 'wait'} for your next word`
      : first;
  };

  return (
    <main class="page">
      <div class="page-inner">
        <div>
          <h1 class="headline">{headline()}</h1>
          <p class="soft" style={{ 'margin-top': '4px' }}>
            <Show
              when={tasks().length > 0}
              fallback="Describe a task below. The practice agent is free and calls no model."
            >
              {tasks().length} {tasks().length === 1 ? 'task' : 'tasks'} in {projects()}{' '}
              {projects() === 1 ? 'project' : 'projects'}.
            </Show>
          </p>
        </div>

        <NewTask />

        <Show when={needsYou().length > 0}>
          <section class="stage-plain">
            <h2 class="stage-title">
              Needs you <span class="count">{needsYou().length}</span>
            </h2>
            <div class="need-grid">
              <For each={needsYou()}>
                {(task) => {
                  const agent = () => agentOf(task);
                  const question = () => openQuestion(task);
                  return (
                    <article class="need-card">
                      <div class={`need-band ${task.info.agentId}`}>
                        <Chip agentId={task.info.agentId} short={agent()?.short ?? '··'} />
                        <span class="what">{agent()?.name ?? 'The agent'} asks before it acts</span>
                        <span class="tag">{projectName(task.info.projectId)}</span>
                        <Show when={task.info.race}>
                          <span class="tag">Race</span>
                        </Show>
                        <span class="age">{ago(task.lastAt)}</span>
                      </div>
                      <div class="need-body">
                        <a class="name" href={taskHref(task.info.id)}>
                          {task.info.title}
                        </a>
                        <p>{question()?.title ?? 'It is waiting for an answer.'}</p>
                        <span class="grow" />
                        <div class="actions">
                          <For each={question()?.options ?? []}>
                            {(option, index) => (
                              <button
                                type="button"
                                class="btn"
                                classList={{ primary: index() === 0 && option.kind.startsWith('allow') }}
                                onClick={() => answerPermission(task.info.id, question()!.requestId, option.optionId)}
                              >
                                {option.name}
                              </button>
                            )}
                          </For>
                          <a class="btn" href={taskHref(task.info.id)}>
                            Open
                          </a>
                        </div>
                      </div>
                    </article>
                  );
                }}
              </For>
            </div>
          </section>
        </Show>

        <Show when={bulkProblem()}>
          <p class="banner danger flush" role="alert">
            <span class="grow">{bulkProblem()}</span>
            <button type="button" class="btn tiny" onClick={() => setBulkProblem(null)}>
              Dismiss
            </button>
          </p>
        </Show>

        <Stage
          title="Checks failed"
          note="The checks of the merge request did not pass. Open the task to see which."
          tasks={checksFailed()}
          where={(task) =>
            `Request ${requests[task.info.id]?.request.number}: ${requests[task.info.id]?.request.checks.failed} failed`
          }
        />
        <Stage
          title="Ready to merge"
          note="Finishing merges the request, sets a Linear issue to Done, archives the task and deletes the working copy and branch."
          tasks={ready()}
          where={(task) => `Request ${requests[task.info.id]?.request.number} is open and nothing stands in its way.`}
          head={
            <button
              type="button"
              class="btn small primary"
              disabled={bulk() !== null}
              onClick={() => void forAll('finish', ready(), finish)}
            >
              {bulk() === 'finish'
                ? 'Finishing'
                : ready().length === 1
                  ? 'Merge and finish'
                  : `Merge and finish all ${ready().length}`}
            </button>
          }
        />
        <Stage
          title="Checks running"
          note="Each line says where its request stands."
          tasks={checking()}
          where={(task) => {
            const checks = requests[task.info.id]?.request.checks;
            return checks ? `${checks.passed} of ${checks.passed + checks.pending + checks.failed} checks done.` : '';
          }}
        />
        <Stage
          title="Merged, not cleaned up"
          note="The request is merged, but the task, its working copy and its branch are still here."
          tasks={merged()}
          where={(task) => `Request ${requests[task.info.id]?.request.number} is merged.`}
          head={
            <button
              type="button"
              class="btn small"
              disabled={bulk() !== null}
              onClick={() => void forAll('clean', merged(), (task) => cleanUpTask(task.info.id))}
            >
              {bulk() === 'clean'
                ? 'Cleaning up'
                : merged().length === 1
                  ? 'Clean up'
                  : `Clean up all ${merged().length}`}
            </button>
          }
        />
        <Stage
          title="Your turn"
          note="The agent finished and waits for your next word."
          tasks={yourTurn()}
          action={(task) => (
            <button type="button" class="btn small ghost" onClick={() => archiveTask(task.info.id)}>
              Archive
            </button>
          )}
        />
        <Stage
          title="Running"
          note="Agents that are in the middle of a turn."
          tasks={working()}
          action={(task) => (
            <button type="button" class="btn small" onClick={() => cancelTurn(task.info.id)}>
              Stop this turn
            </button>
          )}
        />
      </div>
    </main>
  );
}
