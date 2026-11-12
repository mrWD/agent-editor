import { createMemo, createSignal, For, onCleanup, Show, type JSX } from 'solid-js';
import { route, taskHref } from '../app/router';
import { requests, stageOfTask } from '../core/requests';
import { countIn, listed, restLine, STAGES, summary, type Stage } from '../core/stages';
import { agentOf, state, type Task } from '../core/store';
import { StageGlyph } from './bits';

/** Tasks that want attention come first inside a project, so they are never off to the right. */
const order: Record<Stage, number> = { needs: 0, ready: 1, checks: 2, merged: 3, turn: 4, running: 5 };

/** How many tasks are listed by name before the rest is summed up: in the hover card and in the open dock. */
const CARD_ROWS = 6;
const DOCK_ROWS = 5;
const CARD_WIDTH = 310;
/** A project with more tasks than this shows a count for each stage instead of one mark per task. */
const MARKS_PER_PROJECT = 12;

type RailProject = { id: string; name: string; tasks: Task[] };

/** A task with its stage worked out, which is what the counting helpers want. */
const staged = (tasks: Task[]) => tasks.map((task) => ({ info: task.info, stage: stageOfTask(task) }));

/** What is said next to a task's name: who is asking, or where it stands. */
function noteOf(task: Task): string {
  switch (stageOfTask(task)) {
    case 'needs':
      return task.info.state === 'waiting_permission' ? `${agentOf(task)?.name ?? 'The agent'} asks` : 'checks failed';
    case 'ready':
      return 'ready';
    case 'checks': {
      const checks = requests[task.info.id]?.request.checks;
      return checks ? `checks ${checks.passed} of ${checks.passed + checks.pending + checks.failed}` : 'checks running';
    }
    case 'merged':
      return 'merged';
    case 'turn':
      return 'your turn';
    case 'running':
      return 'running';
  }
}

function remember(open: boolean) {
  try {
    localStorage.setItem('ae.dock', open ? 'open' : 'closed');
  } catch {
    // Storage may be refused; the dock then starts closed next time.
  }
}
function remembered(): boolean {
  try {
    return localStorage.getItem('ae.dock') === 'open';
  } catch {
    return false;
  }
}

const chevron = (up: boolean) => (
  <svg
    width="14"
    height="14"
    viewBox="0 0 16 16"
    fill="none"
    stroke="currentColor"
    stroke-width="1.5"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
  >
    <path d={up ? 'M4 10l4-4 4 4' : 'M4 6l4 4 4-4'} />
  </svg>
);

/**
 * The projects dock at the bottom of every screen, so nothing has to be opened
 * to see how things stand. Closed, it is a narrow rail: every project and one
 * mark per task, with a card of the project's tasks under the pointer. Open,
 * it lists every project's tasks by name.
 */
export function ProjectsRail(): JSX.Element {
  const projects = createMemo<RailProject[]>(() => {
    const groups = new Map<string, Task[]>();
    for (const task of Object.values(state.tasks)) {
      groups.set(task.info.projectId, [...(groups.get(task.info.projectId) ?? []), task]);
    }
    // Every project is listed, also one without tasks: it is still a place to start work.
    return state.projects.map((project) => ({
      id: project.id,
      name: project.name,
      tasks: (groups.get(project.id) ?? []).sort(
        (a, b) => order[stageOfTask(a)] - order[stageOfTask(b)] || b.info.createdAt - a.info.createdAt,
      ),
    }));
  });
  const allTasks = createMemo(() => Object.values(state.tasks));

  const currentId = () => {
    const r = route();
    return r.page === 'task' ? r.id : null;
  };
  const currentProject = () => {
    const id = currentId();
    return id ? (state.tasks[id]?.info.projectId ?? null) : null;
  };

  const [open, setOpenSignal] = createSignal(remembered());
  const setOpen = (next: boolean) => {
    setOpenSignal(next);
    remember(next);
    hideNow();
  };

  // The card: which project it shows, where it sits, and which task the pointer is on.
  const [shown, setShown] = createSignal<{ projectId: string; left: number; bottom: number } | null>(null);
  const [pointed, setPointed] = createSignal<string | null>(null);
  let hideTimer: ReturnType<typeof setTimeout> | undefined;
  onCleanup(() => clearTimeout(hideTimer));

  const show = (projectId: string, anchor: HTMLElement) => {
    clearTimeout(hideTimer);
    const box = anchor.getBoundingClientRect();
    const rail = anchor.closest('.rail')?.getBoundingClientRect() ?? box;
    // Keep the card on screen when the project sits near the right edge.
    const left = Math.max(8, Math.min(box.left, window.innerWidth - CARD_WIDTH - 8));
    setShown({ projectId, left, bottom: window.innerHeight - rail.top + 6 });
  };
  // A short wait before hiding lets the pointer travel from the rail up into the card.
  const hideSoon = () => {
    clearTimeout(hideTimer);
    hideTimer = setTimeout(hideNow, 160);
  };
  function hideNow() {
    clearTimeout(hideTimer);
    setShown(null);
    setPointed(null);
  }

  const cardProject = () => projects().find((project) => project.id === shown()?.projectId);

  /** A project's tasks by name: the same rows in the hover card and in the open dock. */
  const rows = (project: RailProject, limit: number): JSX.Element => {
    const named = () => listed(project.tasks, limit, currentId());
    const rest = () => project.tasks.filter((task) => !named().includes(task));
    return (
      <>
        <For each={named()}>
          {(task) => (
            <a
              class="rail-card-row"
              href={taskHref(task.info.id)}
              aria-current={task.info.id === currentId() ? 'page' : undefined}
              classList={{ pointed: task.info.id === pointed() }}
              onClick={hideNow}
            >
              <StageGlyph stage={stageOfTask(task)} />
              <span class="title">{task.info.title}</span>
              <span class="note" classList={{ needs: stageOfTask(task) === 'needs' }}>
                {noteOf(task)}
              </span>
            </a>
          )}
        </For>
        <Show when={rest().length > 0}>
          <span class="rail-card-more">{restLine(staged(rest()))}</span>
        </Show>
        <Show when={project.tasks.length === 0}>
          <span class="rail-card-more">No tasks yet. Start one from Home.</span>
        </Show>
      </>
    );
  };

  const head = (project: RailProject, tag: 'h2' | 'h3') => (
    <div class="rail-card-head">
      {tag === 'h2' ? <h2>{project.name}</h2> : <h3>{project.name}</h3>}
      <span>{project.tasks.length === 0 ? 'no tasks' : summary(staged(project.tasks))}</span>
    </div>
  );

  return (
    <Show
      when={!open()}
      fallback={
        <section class="dock" aria-label="Projects and their tasks">
          <div class="dock-head">
            <h2>Projects</h2>
            <span class="summary">{allTasks().length === 0 ? 'no tasks yet' : summary(staged(allTasks()))}</span>
            <span class="grow" />
            <button type="button" class="dock-toggle" aria-expanded="true" onClick={() => setOpen(false)}>
              Collapse
              {chevron(false)}
            </button>
          </div>
          <div class="dock-grid">
            <For each={projects()}>
              {(project) => (
                <section class="dock-project" classList={{ current: project.id === currentProject() }}>
                  {head(project, 'h3')}
                  {rows(project, DOCK_ROWS)}
                </section>
              )}
            </For>
            <Show when={projects().length === 0}>
              <span class="rail-card-more">No projects yet. Start a task from Home and its folder becomes one.</span>
            </Show>
          </div>
        </section>
      }
    >
      <section class="rail" aria-label="Projects and the state of each task">
        <h2>Projects</h2>
        <Show when={projects().length > 0} fallback={<span class="summary">No projects yet</span>}>
          <For each={projects()}>
            {(project) => (
              <div
                class="rail-project"
                role="group"
                aria-label={project.name}
                classList={{ current: project.id === currentProject(), open: shown()?.projectId === project.id }}
                onPointerEnter={(event) => show(project.id, event.currentTarget)}
                onPointerLeave={hideSoon}
                onFocusIn={(event) => show(project.id, event.currentTarget)}
                onFocusOut={hideSoon}
                onKeyDown={(event) => event.key === 'Escape' && hideNow()}
              >
                <span class="name">{project.name}</span>
                <Show
                  when={project.tasks.length <= MARKS_PER_PROJECT}
                  fallback={
                    // Too many tasks to draw one by one: a mark and a count for each stage that has any.
                    <button
                      type="button"
                      class="rail-counts"
                      aria-label={`${project.name}: ${summary(staged(project.tasks))}. Show its tasks by name.`}
                      onClick={() => setOpen(true)}
                    >
                      <For each={STAGES.filter((stage) => countIn(staged(project.tasks), stage) > 0)}>
                        {(stage) => (
                          <span class="rail-count">
                            <StageGlyph stage={stage} />
                            {countIn(staged(project.tasks), stage)}
                          </span>
                        )}
                      </For>
                    </button>
                  }
                >
                  <For each={project.tasks}>
                    {(task) => (
                      <a
                        class="rail-task"
                        href={taskHref(task.info.id)}
                        aria-current={task.info.id === currentId() ? 'page' : undefined}
                        aria-label={`${task.info.title}: ${noteOf(task)}`}
                        onPointerEnter={() => setPointed(task.info.id)}
                        onPointerLeave={() => setPointed(null)}
                        onFocus={() => setPointed(task.info.id)}
                        onClick={hideNow}
                      >
                        <StageGlyph stage={stageOfTask(task)} />
                      </a>
                    )}
                  </For>
                </Show>
                <Show when={project.tasks.length === 0}>
                  <span class="none">no tasks</span>
                </Show>
              </div>
            )}
          </For>
        </Show>
        <span class="grow" />
        <Show when={allTasks().length > 0}>
          <span class="summary">{summary(staged(allTasks()), false) || 'nothing waits for you'}</span>
        </Show>
        <button type="button" class="dock-toggle" aria-expanded="false" onClick={() => setOpen(true)}>
          Expand
          {chevron(true)}
        </button>

        <Show when={shown() && cardProject()}>
          {(project) => (
            <section
              class="rail-card"
              aria-label={`${project().name}, shown while the pointer is over its part of the rail`}
              style={{ left: `${shown()!.left}px`, bottom: `${shown()!.bottom}px`, width: `${CARD_WIDTH}px` }}
              onPointerEnter={() => clearTimeout(hideTimer)}
              onPointerLeave={hideSoon}
            >
              {head(project(), 'h2')}
              {rows(project(), CARD_ROWS)}
            </section>
          )}
        </Show>
      </section>
    </Show>
  );
}
