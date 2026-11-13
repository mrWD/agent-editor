import { createEffect, createMemo, createSignal, For, on, onCleanup, onMount, Show, type JSX } from 'solid-js';
import { route, taskHref } from '../app/router';
import { alwaysShown, byFolder, finds, fitCount, foldOrder, totals } from '../core/rail';
import { requests, stageOfTask } from '../core/requests';
import { countIn, listed, restLine, STAGES, summary, type Stage } from '../core/stages';
import { agentOf, setProjectFolder, state, type Task } from '../core/store';
import { StageGlyph } from './bits';

/** Tasks that want attention come first inside a project, so they are never off to the right. */
const order: Record<Stage, number> = { needs: 0, ready: 1, checks: 2, merged: 3, turn: 4, running: 5 };

/** How many tasks are listed by name before the rest is summed up: in the hover card and in the open dock. */
const CARD_ROWS = 6;
const DOCK_ROWS = 5;
/** While something is being looked for, more of what was found is listed. */
const FOUND_ROWS = 12;
const CARD_WIDTH = 310;
/** A project with more tasks than this shows a count for each stage instead of one mark per task. */
const MARKS_PER_PROJECT = 12;
/** The room kept on the rail's line for the item the folded projects go into. */
const MORE_WIDTH = 132;
/** How short and how tall the open dock can be dragged. */
const DOCK_MIN = 120;
const dockMax = () => Math.round(window.innerHeight * 0.8);

type RailProject = { id: string; name: string; folder: string | null; createdAt: number; tasks: Task[] };
/** What the card over the rail is about: one project, a folded folder, or the projects folded into "N more". */
type Shown = { kind: 'project' | 'folder' | 'more'; id: string; left: number; bottom: number };
/** Which tasks the open dock lists. */
type Filter = 'all' | 'needs' | 'ready' | 'checks';
const FILTERS: { id: Filter; name: string }[] = [
  { id: 'all', name: 'All' },
  { id: 'needs', name: 'Needs you' },
  { id: 'ready', name: 'Ready to merge' },
  { id: 'checks', name: 'Checks running' },
];

const message = (error: unknown) => (error instanceof Error ? error.message : String(error));

/** A task with its stage worked out, which is what the counting helpers want. */
const staged = (tasks: Task[]) => tasks.map((task) => ({ info: task.info, stage: stageOfTask(task) }));
const tasksOf = (projects: RailProject[]) => projects.flatMap((project) => project.tasks);
const plural = (count: number, one: string, many: string) => `${count} ${count === 1 ? one : many}`;

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

/** What the rail keeps between visits. Storage may be refused; everything then starts as if new. */
function kept(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}
function keep(key: string, value: string | null) {
  try {
    if (value === null) localStorage.removeItem(key);
    else localStorage.setItem(key, value);
  } catch {
    // Nothing to do: the choice lasts until the page is closed.
  }
}
function keptList(key: string): string[] {
  try {
    const list: unknown = JSON.parse(kept(key) ?? '[]');
    return Array.isArray(list) ? list.filter((one): one is string => typeof one === 'string') : [];
  } catch {
    return [];
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

/** The mark in front of a folder's name: pointing down when open, to the right when folded. */
const twisty = (open: boolean) => (
  <svg
    width="12"
    height="12"
    viewBox="0 0 16 16"
    fill="none"
    stroke="currentColor"
    stroke-width="1.5"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
  >
    <path d={open ? 'M4 6l4 4 4-4' : 'M6 4l4 4-4 4'} />
  </svg>
);

const folderIcon = () => (
  <svg
    width="14"
    height="14"
    viewBox="0 0 16 16"
    fill="none"
    stroke="currentColor"
    stroke-width="1.4"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
  >
    <path d="M2 4.5a1 1 0 0 1 1-1h3l1.5 1.5H13a1 1 0 0 1 1 1V12a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1z" />
  </svg>
);

/** A mark and a count for each stage that has any task, in the order the stages are always listed. */
function Counts(props: { tasks: Task[] }): JSX.Element {
  const all = () => staged(props.tasks);
  return (
    <For each={STAGES.filter((stage) => countIn(all(), stage) > 0)}>
      {(stage) => (
        <span class="rail-count">
          <StageGlyph stage={stage} />
          {countIn(all(), stage)}
        </span>
      )}
    </For>
  );
}

/**
 * The projects dock at the bottom of every screen, so nothing has to be opened
 * to see how things stand. Closed, it is a narrow rail: every project and one
 * mark per task, with a card of the project's tasks under the pointer. Folders
 * of projects show their totals, and projects that do not fit fold into one
 * item. Open, it lists every project's tasks by name, with a box to find one.
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
      folder: project.folder,
      createdAt: project.createdAt,
      tasks: (groups.get(project.id) ?? []).sort(
        (a, b) => order[stageOfTask(a)] - order[stageOfTask(b)] || b.info.createdAt - a.info.createdAt,
      ),
    }));
  });
  const allTasks = createMemo(() => Object.values(state.tasks));
  const grouped = createMemo(() => byFolder(projects()));

  const currentId = () => {
    const r = route();
    return r.page === 'task' ? r.id : null;
  };
  const currentProject = () => {
    const id = currentId();
    return id ? (state.tasks[id]?.info.projectId ?? null) : null;
  };
  const currentFolder = () =>
    projects()
      .find((project) => project.id === currentProject())
      ?.folder?.trim() || null;

  const [open, setOpenSignal] = createSignal(kept('ae.dock') === 'open');
  const setOpen = (next: boolean) => {
    setOpenSignal(next);
    keep('ae.dock', next ? 'open' : 'closed');
    hideNow();
  };

  // ----- the folder that is open in the rail -----

  // The folder of the task on screen stays open; otherwise the one opened last.
  const [chosen, setChosen] = createSignal<string | null>(kept('ae.rail.folder'));
  const chooseFolder = (name: string | null) => {
    setChosen(name);
    keep('ae.rail.folder', name);
    hideNow();
  };
  createEffect(
    on(currentFolder, (folder) => {
      if (folder) chooseFolder(folder);
    }),
  );

  // ----- the card over the rail -----

  const [shown, setShown] = createSignal<Shown | null>(null);
  const [pointed, setPointed] = createSignal<string | null>(null);
  let hideTimer: ReturnType<typeof setTimeout> | undefined;
  onCleanup(() => clearTimeout(hideTimer));

  const show = (kind: Shown['kind'], id: string, anchor: HTMLElement) => {
    clearTimeout(hideTimer);
    const box = anchor.getBoundingClientRect();
    const rail = anchor.closest('.rail')?.getBoundingClientRect() ?? box;
    // Keep the card on screen when the project sits near the right edge.
    const left = Math.max(8, Math.min(box.left, window.innerWidth - CARD_WIDTH - 8));
    setShown({ kind, id, left, bottom: window.innerHeight - rail.top + 6 });
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
  const isShown = (kind: Shown['kind'], id: string) => shown()?.kind === kind && shown()?.id === id;
  const cardProject = () => {
    const card = shown();
    return card?.kind === 'project' ? projects().find((project) => project.id === card.id) : undefined;
  };

  // ----- projects that do not fit on the rail's line -----

  let railEl: HTMLElement | undefined;
  const [away, setAway] = createSignal<string[]>([]);
  const foldable = (project: RailProject) => ({
    id: project.id,
    needs: countIn(staged(project.tasks), 'needs') > 0,
    current: project.id === currentProject(),
    lastActive: Math.max(project.createdAt, ...project.tasks.map((task) => task.info.createdAt)),
  });
  /** The projects without a folder, in the order they stand on the rail; the folded ones come last. */
  const loose = createMemo<RailProject[]>(() => {
    const all = grouped().loose;
    if (away().length === 0) return all;
    const byId = new Map(all.map((project) => [project.id, project]));
    return foldOrder(all.map(foldable)).map((one) => byId.get(one.id)!);
  });
  const awayProjects = createMemo(() => loose().filter((project) => away().includes(project.id)));

  const measure = () => {
    const rail = railEl;
    if (!rail?.isConnected) return;
    const chips = [...rail.querySelectorAll<HTMLElement>(':scope > [data-project]')];
    const style = getComputedStyle(rail);
    const gap = Number.parseFloat(style.columnGap) || 0;
    const inner = rail.clientWidth - Number.parseFloat(style.paddingLeft) - Number.parseFloat(style.paddingRight);
    const outer = (el: Element) => {
      const own = getComputedStyle(el);
      return (
        el.getBoundingClientRect().width + Number.parseFloat(own.marginLeft) + Number.parseFloat(own.marginRight) + gap
      );
    };
    // Everything else on the line: the heading, the folders, the summary and the Expand button.
    const others = [...rail.children].filter((el) => !el.matches('[data-project], .grow, .rail-more, .rail-card'));
    const fixed = others.reduce((sum, el) => sum + outer(el), 0);
    const widths = new Map(chips.map((chip) => [chip.dataset.project!, outer(chip)]));
    const total = [...widths.values()].reduce((sum, width) => sum + width, 0);
    let next: string[] = [];
    if (fixed + total > inner + 1) {
      const ordered = foldOrder(grouped().loose.map(foldable));
      const fit = fitCount(
        ordered.map((one) => widths.get(one.id) ?? 0),
        alwaysShown(ordered),
        inner - fixed - MORE_WIDTH,
      );
      next = ordered.slice(fit).map((one) => one.id);
    }
    if (next.join('\n') !== away().join('\n')) setAway(next);
  };
  let frame = 0;
  const measureSoon = () => {
    cancelAnimationFrame(frame);
    frame = requestAnimationFrame(measure);
  };
  onMount(() => {
    const watcher = new ResizeObserver(measureSoon);
    createEffect(() => {
      watcher.disconnect();
      // The rail is drawn anew when the dock closes: watch the one that is there.
      if (!open() && railEl) watcher.observe(railEl);
    });
    window.addEventListener('resize', measureSoon);
    onCleanup(() => {
      watcher.disconnect();
      window.removeEventListener('resize', measureSoon);
      cancelAnimationFrame(frame);
    });
  });
  // What is drawn decides what fits: measure again after anything on the rail changed. A task that starts to
  // need the user changes the order without changing the list, so the stages are part of what is watched.
  const drawn = createMemo(() =>
    projects()
      .map((project) => `${project.id} ${project.name} ${project.folder ?? ''} ${summary(staged(project.tasks))}`)
      .join('\n'),
  );
  createEffect(() => {
    drawn();
    chosen();
    currentProject();
    if (!open()) measureSoon();
  });

  // ----- the open dock: finding, folding, its height, moving a project into a folder -----

  const [query, setQuery] = createSignal('');
  const [filter, setFilter] = createSignal<Filter>('all');
  const looking = () => query().trim() !== '' || filter() !== 'all';
  /** A project with only the tasks that are being looked for, or nothing when it has none of them. */
  const found = (project: RailProject): RailProject | null => {
    if (!looking()) return project;
    const words = query().trim();
    const named = words !== '' && finds(words, project.name);
    let tasks = project.tasks;
    if (filter() !== 'all') tasks = tasks.filter((task) => stageOfTask(task) === filter());
    if (words !== '' && !named) tasks = tasks.filter((task) => finds(words, task.info.title));
    if (tasks.length === 0 && !(named && filter() === 'all')) return null;
    return { ...project, tasks };
  };
  const foundIn = (list: RailProject[]) =>
    list.map(found).filter((project): project is RailProject => project !== null);
  const foundFolders = createMemo(() =>
    grouped()
      .folders.map((folder) => ({ name: folder.name, all: folder.projects, projects: foundIn(folder.projects) }))
      .filter((folder) => folder.projects.length > 0),
  );
  const foundLoose = createMemo(() => foundIn(grouped().loose));

  const [folded, setFolded] = createSignal<string[]>(keptList('ae.dock.folded'));
  const isFolded = (name: string) => !looking() && folded().includes(name);
  const fold = (name: string) => {
    const next = folded().includes(name) ? folded().filter((one) => one !== name) : [...folded(), name];
    setFolded(next);
    keep('ae.dock.folded', JSON.stringify(next));
  };

  const [height, setHeight] = createSignal<number | null>(Number(kept('ae.dock.height')) || null);
  let dockEl: HTMLElement | undefined;
  const resizeTo = (next: number) => {
    const clamped = Math.round(Math.max(DOCK_MIN, Math.min(dockMax(), next)));
    setHeight(clamped);
    keep('ae.dock.height', String(clamped));
  };
  const drag = (event: PointerEvent) => {
    const grip = event.currentTarget as HTMLElement;
    grip.setPointerCapture(event.pointerId);
    const move = (moved: PointerEvent) => {
      if (dockEl) resizeTo(dockEl.getBoundingClientRect().bottom - moved.clientY);
    };
    const stop = () => {
      grip.removeEventListener('pointermove', move);
      grip.removeEventListener('pointerup', stop);
      grip.removeEventListener('pointercancel', stop);
    };
    grip.addEventListener('pointermove', move);
    grip.addEventListener('pointerup', stop);
    grip.addEventListener('pointercancel', stop);
    event.preventDefault();
  };
  const nudge = (event: KeyboardEvent) => {
    if (event.key !== 'ArrowUp' && event.key !== 'ArrowDown') return;
    event.preventDefault();
    const now = height() ?? dockEl?.getBoundingClientRect().height ?? 320;
    resizeTo(now + (event.key === 'ArrowUp' ? 24 : -24));
  };

  // One form for all projects, outside their list: the list is drawn anew whenever a task changes.
  const [moving, setMoving] = createSignal<string | null>(null);
  const [folderDraft, setFolderDraft] = createSignal('');
  const [moveProblem, setMoveProblem] = createSignal<string | null>(null);
  const movingProject = () => projects().find((project) => project.id === moving());
  const startMoving = (project: RailProject) => {
    setMoving(project.id);
    setFolderDraft(project.folder ?? '');
    setMoveProblem(null);
    queueMicrotask(() => document.getElementById('dock-folder')?.focus());
  };
  const saveFolder = async (name: string) => {
    const id = moving();
    if (!id) return;
    try {
      await setProjectFolder(id, name);
      setMoving(null);
    } catch (error) {
      setMoveProblem(message(error));
    }
  };

  // ----- pieces drawn in more than one place -----

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
          <span class="rail-card-more">
            {looking() ? 'None of its tasks match.' : 'No tasks yet. Start one from Home.'}
          </span>
        </Show>
      </>
    );
  };

  const head = (project: RailProject, tag: 'h2' | 'h3' | 'h4', movable = false) => (
    <div class="rail-card-head">
      {tag === 'h2' ? <h2>{project.name}</h2> : tag === 'h3' ? <h3>{project.name}</h3> : <h4>{project.name}</h4>}
      <span>{project.tasks.length === 0 ? 'no tasks' : summary(staged(project.tasks))}</span>
      <Show when={movable}>
        <button
          type="button"
          class="dock-move-button"
          aria-label={
            project.folder ? `${project.name} is in ${project.folder}. Move it.` : `Put ${project.name} into a folder`
          }
          title={project.folder ? `In ${project.folder}. Move it.` : 'Put into a folder'}
          onClick={() => startMoving(project)}
        >
          {folderIcon()}
        </button>
      </Show>
    </div>
  );

  /** A task that needs the user, named with its project: for places where its project is not drawn. */
  const needing = (list: RailProject[], cls: string): JSX.Element => (
    <For
      each={list.flatMap((project) =>
        project.tasks.filter((task) => stageOfTask(task) === 'needs').map((task) => ({ task, project })),
      )}
    >
      {(one) => (
        <a class={cls} href={taskHref(one.task.info.id)} onClick={hideNow}>
          <StageGlyph stage="needs" />
          <span class="title">{one.task.info.title}</span>
          <span class="where">{one.project.name}</span>
          <span class="note needs">{noteOf(one.task)}</span>
        </a>
      )}
    </For>
  );

  /** The card of several projects at once: a folded folder, or the projects folded into "N more". */
  const groupCard = (title: string, list: RailProject[], inFolder: boolean): JSX.Element => (
    <>
      <div class="rail-card-head">
        <h2>{title}</h2>
        <span>
          {plural(list.length, 'project', 'projects')} · {plural(tasksOf(list).length, 'task', 'tasks')}
        </span>
      </div>
      <For each={list}>
        {(project) => (
          <span class="rail-card-project">
            <span class="title">{project.name}</span>
            <span class="rail-counts">
              <Counts tasks={project.tasks} />
            </span>
            <span class="note">
              {project.tasks.length === 0 ? 'no tasks' : plural(project.tasks.length, 'task', 'tasks')}
            </span>
          </span>
        )}
      </For>
      <Show when={list.some((project) => countIn(staged(project.tasks), 'needs') > 0)}>
        <span class="rail-card-label">{inFolder ? 'Needs you in this folder' : 'Needs you'}</span>
        {needing(list, 'rail-card-row')}
      </Show>
    </>
  );

  /** One project on the rail: its name and a mark for each task, or a count for each stage. */
  const chip = (project: RailProject, top: boolean): JSX.Element => {
    const isAway = () => top && away().includes(project.id);
    return (
      <div
        class="rail-project"
        role="group"
        aria-label={project.name}
        data-project={top ? project.id : undefined}
        aria-hidden={isAway() ? 'true' : undefined}
        inert={isAway()}
        classList={{
          current: project.id === currentProject(),
          open: isShown('project', project.id),
          away: isAway(),
        }}
        onPointerEnter={(event) => show('project', project.id, event.currentTarget)}
        onPointerLeave={hideSoon}
        onFocusIn={(event) => show('project', project.id, event.currentTarget)}
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
              <Counts tasks={project.tasks} />
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
    );
  };

  const dockProject = (project: RailProject, tag: 'h3' | 'h4'): JSX.Element => (
    <section class="dock-project" classList={{ current: project.id === currentProject() }}>
      {head(project, tag, true)}
      {rows(project, looking() ? FOUND_ROWS : DOCK_ROWS)}
    </section>
  );

  const headline = () => {
    if (allTasks().length === 0) return 'no tasks yet';
    const waits = summary(staged(allTasks()), false);
    return `${waits ? `${waits} · ` : ''}${totals(allTasks().length, projects().length)}`;
  };

  return (
    <Show
      when={!open()}
      fallback={
        <section
          class="dock"
          aria-label="Projects and their tasks"
          ref={dockEl}
          style={height() === null ? undefined : { height: `${height()}px`, 'max-height': `${dockMax()}px` }}
        >
          <div
            class="dock-grip"
            role="separator"
            aria-orientation="horizontal"
            aria-label="Drag to make the panel taller or shorter"
            aria-valuenow={height() ?? Math.round(dockEl?.getBoundingClientRect().height ?? DOCK_MIN)}
            aria-valuemin={DOCK_MIN}
            aria-valuemax={dockMax()}
            tabindex="0"
            onPointerDown={drag}
            onKeyDown={nudge}
          >
            <span />
          </div>
          <div class="dock-head">
            <h2>Projects</h2>
            <span class="summary">{headline()}</span>
            <span class="grow" />
            <label for="dock-search">Find</label>
            <input
              id="dock-search"
              type="search"
              placeholder="A project or a task"
              value={query()}
              onInput={(event) => setQuery(event.currentTarget.value)}
            />
            <div class="dock-filter" role="group" aria-label="Which tasks to show">
              <For each={FILTERS}>
                {(one) => (
                  <button type="button" aria-pressed={filter() === one.id} onClick={() => setFilter(one.id)}>
                    <Show when={one.id !== 'all'}>
                      <StageGlyph stage={one.id as Stage} />
                    </Show>
                    {one.name}
                    <span class="n">
                      {one.id === 'all' ? allTasks().length : countIn(staged(allTasks()), one.id as Stage)}
                    </span>
                  </button>
                )}
              </For>
            </div>
            <button type="button" class="dock-toggle" aria-expanded="true" onClick={() => setOpen(false)}>
              Collapse
              {chevron(false)}
            </button>
          </div>

          <Show when={movingProject()}>
            {(project) => (
              <form
                class="dock-move"
                onSubmit={(event) => {
                  event.preventDefault();
                  void saveFolder(folderDraft());
                }}
                onKeyDown={(event) => event.key === 'Escape' && setMoving(null)}
              >
                <label for="dock-folder">
                  Folder for <b>{project().name}</b>
                </label>
                <input
                  id="dock-folder"
                  list="dock-folders"
                  maxlength="40"
                  placeholder="A name, new or one you have"
                  value={folderDraft()}
                  onInput={(event) => setFolderDraft(event.currentTarget.value)}
                />
                <datalist id="dock-folders">
                  <For each={grouped().folders}>{(folder) => <option value={folder.name} />}</For>
                </datalist>
                <button type="submit" class="primary">
                  Save
                </button>
                <Show when={project().folder}>
                  <button type="button" onClick={() => void saveFolder('')}>
                    Take it out of {project().folder}
                  </button>
                </Show>
                <button type="button" onClick={() => setMoving(null)}>
                  Cancel
                </button>
                <Show when={moveProblem()}>
                  <span class="dock-move-problem" role="alert">
                    {moveProblem()}
                  </span>
                </Show>
              </form>
            )}
          </Show>

          <div class="dock-body">
            <For each={foundFolders()}>
              {(folder) => (
                <div class="dock-folder" role="group" aria-label={`Folder ${folder.name}`}>
                  <div class="dock-folder-head">
                    <h3>
                      <button type="button" aria-expanded={!isFolded(folder.name)} onClick={() => fold(folder.name)}>
                        {twisty(!isFolded(folder.name))}
                        {folder.name}
                      </button>
                    </h3>
                    <span class="summary">
                      {plural(folder.all.length, 'project', 'projects')} ·{' '}
                      {plural(tasksOf(folder.all).length, 'task', 'tasks')}
                      {folder.all.some((project) => project.id === currentProject()) ? ' · you are here' : ''}
                      {isFolded(folder.name) ? ' · folded' : ''}
                    </span>
                    <span class="rail-counts">
                      <Counts tasks={tasksOf(folder.all)} />
                    </span>
                    {/* Folded, a folder still names every task that needs the user. */}
                    <Show when={isFolded(folder.name)}>{needing(folder.all, 'dock-folded-task')}</Show>
                  </div>
                  <Show when={!isFolded(folder.name)}>
                    <div class="dock-grid">
                      <For each={folder.projects}>{(project) => dockProject(project, 'h4')}</For>
                    </div>
                  </Show>
                </div>
              )}
            </For>
            <Show when={foundLoose().length > 0}>
              <div class="dock-grid">
                <For each={foundLoose()}>{(project) => dockProject(project, 'h3')}</For>
              </div>
            </Show>
            <Show when={projects().length === 0}>
              <span class="rail-card-more">No projects yet. Start a task from Home and its folder becomes one.</span>
            </Show>
            <Show when={projects().length > 0 && foundFolders().length === 0 && foundLoose().length === 0}>
              <span class="rail-card-more">Nothing matches. Try fewer words, or show all tasks.</span>
            </Show>
          </div>
        </section>
      }
    >
      <section class="rail" aria-label="Projects and the state of each task" ref={railEl}>
        <h2>Projects</h2>
        <Show when={projects().length > 0} fallback={<span class="summary">No projects yet</span>}>
          <For each={grouped().folders}>
            {(folder) => (
              <Show
                when={chosen() === folder.name}
                fallback={
                  // A folded folder: its name, how many projects it holds, and a count for each stage.
                  <button
                    type="button"
                    class="rail-folded"
                    aria-expanded="false"
                    aria-label={`Folder ${folder.name}: ${plural(folder.projects.length, 'project', 'projects')}${
                      tasksOf(folder.projects).length > 0 ? `, ${summary(staged(tasksOf(folder.projects)))}` : ''
                    }. Show its projects.`}
                    classList={{ open: isShown('folder', folder.name) }}
                    onClick={() => chooseFolder(folder.name)}
                    onPointerEnter={(event) => show('folder', folder.name, event.currentTarget)}
                    onPointerLeave={hideSoon}
                    onFocus={(event) => show('folder', folder.name, event.currentTarget)}
                    onBlur={hideSoon}
                    onKeyDown={(event) => event.key === 'Escape' && hideNow()}
                  >
                    {folder.name}
                    <span class="n">{folder.projects.length}</span>
                    <Counts tasks={tasksOf(folder.projects)} />
                  </button>
                }
              >
                <div class="rail-folder" role="group" aria-label={`Folder ${folder.name}`}>
                  <button
                    type="button"
                    class="rail-folder-name"
                    aria-expanded="true"
                    title="Fold this folder"
                    onClick={() => chooseFolder(null)}
                  >
                    {folder.name}
                  </button>
                  <For each={folder.projects}>{(project) => chip(project, false)}</For>
                </div>
              </Show>
            )}
          </For>
          <For each={loose()}>{(project) => chip(project, true)}</For>
          <Show when={awayProjects().length > 0}>
            <button
              type="button"
              class="rail-more"
              aria-label={`${plural(awayProjects().length, 'more project', 'more projects')}${
                tasksOf(awayProjects()).length > 0 ? `: ${summary(staged(tasksOf(awayProjects())))}` : ''
              }. Show them by name.`}
              classList={{ open: isShown('more', 'more') }}
              onClick={() => setOpen(true)}
              onPointerEnter={(event) => show('more', 'more', event.currentTarget)}
              onPointerLeave={hideSoon}
              onFocus={(event) => show('more', 'more', event.currentTarget)}
              onBlur={hideSoon}
              onKeyDown={(event) => event.key === 'Escape' && hideNow()}
            >
              {awayProjects().length} more
              <Counts tasks={tasksOf(awayProjects())} />
            </button>
          </Show>
        </Show>
        <span class="grow" />
        <Show when={allTasks().length > 0}>
          <span class="summary">{summary(staged(allTasks()), false) || 'nothing waits for you'}</span>
        </Show>
        <button type="button" class="dock-toggle" aria-expanded="false" onClick={() => setOpen(true)}>
          Expand
          {chevron(true)}
        </button>

        <Show when={shown()}>
          {(card) => {
            const style = () => ({
              left: `${card().left}px`,
              bottom: `${card().bottom}px`,
              width: `${CARD_WIDTH}px`,
            });
            const group = () => {
              if (card().kind === 'more') {
                return { title: `${awayProjects().length} more`, list: awayProjects(), inFolder: false };
              }
              const folder = grouped().folders.find((one) => one.name === card().id);
              return folder ? { title: folder.name, list: folder.projects, inFolder: true } : null;
            };
            return (
              <Show
                when={card().kind === 'project'}
                fallback={
                  <Show when={group()}>
                    {(one) => (
                      <section
                        class="rail-card"
                        aria-label={`${one().title}, shown while the pointer is over its part of the rail`}
                        style={style()}
                        onPointerEnter={() => clearTimeout(hideTimer)}
                        onPointerLeave={hideSoon}
                      >
                        {groupCard(one().title, one().list, one().inFolder)}
                      </section>
                    )}
                  </Show>
                }
              >
                <Show when={cardProject()}>
                  {(project) => (
                    <section
                      class="rail-card"
                      aria-label={`${project().name}, shown while the pointer is over its part of the rail`}
                      style={style()}
                      onPointerEnter={() => clearTimeout(hideTimer)}
                      onPointerLeave={hideSoon}
                    >
                      {head(project(), 'h2')}
                      {rows(project(), CARD_ROWS)}
                    </section>
                  )}
                </Show>
              </Show>
            );
          }}
        </Show>
      </section>
    </Show>
  );
}
