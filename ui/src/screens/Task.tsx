import {
  createEffect,
  createMemo,
  createSignal,
  For,
  lazy,
  Match,
  on,
  onCleanup,
  onMount,
  Show,
  Suspense,
  Switch,
  type JSX,
} from 'solid-js';
import { createStore } from 'solid-js/store';
import { taskHref, VIEWS, type View } from '../app/router';
import { AgentPane } from '../components/AgentPane';
import { LspBadge, LspStatus } from '../components/LspStatus';
import { ProblemsTab } from '../components/ProblemsTab';
import { RunTab } from '../components/RunTab';
import { Chip, Icon, modKey } from '../components/bits';
import { stageOfTask } from '../core/requests';
import { stageWord } from '../core/stages';
import { core } from '../core/client';
import { checksOf, watchChecks } from '../core/checks';
import { problemCount } from '../core/problems';
import { arrive, leave } from '../core/visits';
import {
  agentOf,
  cancelTurn,
  keepFile,
  listFiles,
  markSeen,
  projectName,
  readFile,
  state,
  undoFile,
  writeFile,
  type FileEntry,
  type FileReview,
  type Task,
  createFile,
  deleteFile,
  renameFile,
} from '../core/store';
import {
  bottomHeight,
  bottomOpen,
  openInEditor,
  openRequest,
  registerSaver,
  saveAll,
  setBottomHeight,
  setBottomOpen,
  setWork,
  workOf,
} from '../core/work';
import { watchGit, type GitWatch } from '../core/gitstate';
import { EditorHost, type FileMeta } from '../editor/host';
import { ChangesView } from './Changes';
import { ChecksView } from './Checks';
import { LookBackView } from './LookBack';
import { PreviewView } from './Preview';
import { SpanView } from './Span';
import { MapView } from './Map';
import { languageOf } from '../editor/languages';
import { LspSession } from '../editor/lsp';

// The terminal is fetched the first time it is opened.
const TerminalView = lazy(() => import('../components/Terminal').then((module) => ({ default: module.TerminalView })));

const viewNames: Record<View, string> = {
  plan: 'Plan',
  code: 'Code',
  changes: 'Changes',
  preview: 'Preview',
  map: 'Map',
  checks: 'Checks',
};

/** Which milestone brings each view that is not built yet. Shown instead of a blank pane. */
const notBuilt: Partial<Record<View, { milestone: string; what: string }>> = {
  plan: { milestone: 'M6', what: 'The flowchart the task is built from.' },
  map: { milestone: 'M7', what: 'Diagrams of the code this task touches.' },
};

const baseName = (path: string) => path.split('/').pop() ?? path;
const message = (error: unknown) => (error instanceof Error ? error.message : String(error));

/** The agent colour a part of the screen is tinted with. */
function toneOf(agentId: string): string {
  return ['claude', 'codex', 'gemini'].includes(agentId) ? `tone-${agentId}` : 'tone-plain';
}

/** The marks next to a changed file: who changed it. */
function ChangeBadges(props: { review: FileReview | undefined; task: Task }): JSX.Element {
  const agent = () => agentOf(props.task);
  return (
    <Show when={props.review}>
      {(review) => (
        <>
          <Show when={review().agentLines > 0}>
            <Chip
              agentId={props.task.info.agentId}
              short={agent()?.short ?? 'Ag'}
              flat
              label={`Changed by ${agent()?.name ?? 'the agent'}`}
            />
          </Show>
          <Show when={review().yourLines > 0}>
            <Chip agentId="you" short="You" flat label="Changed by you" />
          </Show>
        </>
      )}
    </Show>
  );
}

type Folder = { entries: FileEntry[]; open: boolean; loading: boolean; problem: string | null };

/** What is being named in the tree: a new file or folder inside `at`, or the entry `at` itself. */
type Naming = { kind: 'file' | 'folder' | 'rename'; at: string };

const parentOf = (path: string) => (path.includes('/') ? path.slice(0, path.lastIndexOf('/')) : '');

function FileTree(props: {
  task: Task;
  current: string | null;
  onOpen: (path: string) => void;
  /** Files that got a new name, so open tabs can follow them. */
  onMoved: (moved: { from: string; to: string }[]) => void;
  /** Files that are gone, so their tabs can close. */
  onGone: (paths: string[]) => void;
}): JSX.Element {
  // eslint-disable-next-line solid/reactivity -- this component is made anew for each task
  const taskId = props.task.info.id;
  const [folders, setFolders] = createStore<Record<string, Folder>>({});
  const [naming, setNaming] = createSignal<Naming | null>(null);
  const [name, setName] = createSignal('');
  const [problem, setProblem] = createSignal<string | null>(null);
  const [armed, setArmed] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal(false);
  let nameEl: HTMLInputElement | undefined;

  const load = async (path: string, quiet = false) => {
    if (!quiet) setFolders(path, { entries: [], open: true, loading: true, problem: null });
    try {
      const result = await listFiles(taskId, path);
      setFolders(path, { entries: result.entries, open: folders[path]?.open ?? true, loading: false, problem: null });
    } catch (error) {
      if (!quiet) setFolders(path, { entries: [], open: true, loading: false, problem: message(error) });
    }
  };
  const reloadAll = () => {
    for (const [path, folder] of Object.entries(folders)) {
      if (folder && !folder.loading) void load(path, true);
    }
  };

  const toggle = (path: string) => {
    const folder = folders[path];
    if (!folder) void load(path);
    else setFolders(path, 'open', !folder.open);
  };

  onMount(() => {
    void load('');
    // Files made, renamed or deleted in any window show here at once.
    // eslint-disable-next-line solid/reactivity -- called later, as an event handler: it reads what is current then
    const stop = core.onEvent((event) => {
      if (event.event === 'files.changed' && event.data.taskId === taskId) reloadAll();
    });
    onCleanup(stop);
  });

  // Files may appear and disappear when something changes: read the open folders again.
  const reviewed = (path: string) => props.task.changed.find((file) => file.path === path);
  createEffect(
    on(() => props.task.changed.map((file) => `${file.path}:${file.exists}`).join('|'), reloadAll, { defer: true }),
  );

  const startNaming = (next: Naming, value: string) => {
    setProblem(null);
    setArmed(null);
    setName(value);
    setNaming(next);
    // A new entry is typed inside its folder, so the folder has to be open.
    if (next.kind !== 'rename' && next.at !== '') {
      if (!folders[next.at]) void load(next.at);
      else setFolders(next.at, 'open', true);
    }
  };
  const stopNaming = () => {
    setNaming(null);
    setProblem(null);
  };

  const apply = async () => {
    const what = naming();
    const typed = name().trim();
    if (!what || busy()) return;
    if (typed === '') return stopNaming();
    setBusy(true);
    setProblem(null);
    try {
      if (what.kind === 'rename') {
        const to = [parentOf(what.at), typed].filter(Boolean).join('/');
        if (to !== what.at) {
          // What was typed in the file goes with it under its new name.
          await saveAll(taskId);
          props.onMoved((await renameFile(taskId, what.at, to)).moved);
        }
      } else {
        const path = [what.at, typed].filter(Boolean).join('/');
        const made = await createFile(taskId, path, what.kind === 'folder');
        if (what.kind === 'file') props.onOpen(made);
      }
      setNaming(null);
      reloadAll();
    } catch (error) {
      setProblem(message(error));
    } finally {
      setBusy(false);
      // The box was switched off while the core was asked; the caret goes back so the name can be put right.
      setTimeout(() => nameEl?.focus(), 0);
    }
  };

  // A first press that is not followed by a second one is forgotten after a moment.
  let disarm: ReturnType<typeof setTimeout> | undefined;
  onCleanup(() => clearTimeout(disarm));
  const remove = async (path: string) => {
    clearTimeout(disarm);
    if (armed() !== path) {
      setArmed(path);
      disarm = setTimeout(() => armed() === path && setArmed(null), 4000);
      return;
    }
    setArmed(null);
    setProblem(null);
    try {
      props.onGone((await deleteFile(taskId, path)).files);
      reloadAll();
    } catch (error) {
      setProblem(message(error));
    }
  };

  /** The box a name is typed in. Enter applies, Escape gives up, leaving the box applies too. */
  const nameBox = (label: string, depth: number): JSX.Element => (
    <div class="tree-line" style={{ 'padding-left': `${6 + depth * 14}px` }}>
      <input
        class="tree-name"
        aria-label={label}
        value={name()}
        disabled={busy()}
        ref={(el) => {
          nameEl = el;
          setTimeout(() => {
            el.focus();
            // In a rename the name without its ending is what is usually changed.
            const dot = el.value.lastIndexOf('.');
            el.setSelectionRange(0, naming()?.kind === 'rename' && dot > 0 ? dot : el.value.length);
          }, 0);
        }}
        onInput={(event) => setName(event.currentTarget.value)}
        onKeyDown={(event) => {
          if (event.key === 'Enter') void apply();
          if (event.key === 'Escape') stopNaming();
        }}
        onBlur={() => (problem() ? undefined : void apply())}
      />
    </div>
  );

  const level = (path: string, depth: number): JSX.Element => (
    <Show when={folders[path]}>
      {(folder) => (
        <>
          <Show when={folder().loading}>
            <span class="tree-note" style={{ 'padding-left': `${6 + depth * 14}px` }}>
              Reading the folder
            </span>
          </Show>
          <Show when={folder().problem}>
            <span class="tree-note" style={{ 'padding-left': `${6 + depth * 14}px`, color: 'var(--red)' }}>
              {folder().problem}
            </span>
          </Show>
          <Show when={naming()?.kind !== 'rename' && naming()?.at === path}>
            {nameBox(naming()!.kind === 'folder' ? 'Name of the new folder' : 'Name of the new file', depth)}
          </Show>
          <Show when={!folder().loading && !folder().problem && folder().entries.length === 0 && naming()?.at !== path}>
            <span class="tree-note" style={{ 'padding-left': `${6 + depth * 14}px` }}>
              Empty
            </span>
          </Show>
          <For each={folder().entries}>
            {(entry) => (
              <>
                <Show
                  when={naming()?.kind === 'rename' && naming()!.at === entry.path}
                  fallback={
                    <div class="tree-line">
                      <button
                        type="button"
                        class="tree-row"
                        style={{ 'padding-left': `${6 + depth * 14}px` }}
                        aria-current={entry.path === props.current ? 'true' : undefined}
                        aria-expanded={entry.isDir ? Boolean(folders[entry.path]?.open) : undefined}
                        onClick={() => (entry.isDir ? toggle(entry.path) : props.onOpen(entry.path))}
                      >
                        <span class="label">
                          {entry.name}
                          {entry.isDir ? '/' : ''}
                        </span>
                        <Show when={!entry.isDir}>
                          <ChangeBadges review={reviewed(entry.path)} task={props.task} />
                        </Show>
                      </button>
                      <span class="tree-actions" classList={{ shown: armed() === entry.path }}>
                        <Show when={entry.isDir}>
                          <button
                            type="button"
                            aria-label={`New file in ${entry.path}`}
                            title="New file in this folder"
                            onClick={() => startNaming({ kind: 'file', at: entry.path }, '')}
                          >
                            <Icon.Plus />
                          </button>
                        </Show>
                        <button
                          type="button"
                          aria-label={`Rename ${entry.path}`}
                          title="Rename"
                          onClick={() => startNaming({ kind: 'rename', at: entry.path }, entry.name)}
                        >
                          <Icon.Pencil />
                        </button>
                        <button
                          type="button"
                          class={armed() === entry.path ? 'sure' : undefined}
                          aria-label={armed() === entry.path ? `Really delete ${entry.path}?` : `Delete ${entry.path}`}
                          title={armed() === entry.path ? 'Press again to delete. It can be put back.' : 'Delete'}
                          onClick={() => void remove(entry.path)}
                          onBlur={() => armed() === entry.path && setArmed(null)}
                        >
                          <Show when={armed() === entry.path} fallback={<Icon.Trash />}>
                            Delete?
                          </Show>
                        </button>
                      </span>
                    </div>
                  }
                >
                  {nameBox(`New name of ${entry.path}`, depth)}
                </Show>
                <Show when={entry.isDir && folders[entry.path]?.open}>{level(entry.path, depth + 1)}</Show>
              </>
            )}
          </For>
        </>
      )}
    </Show>
  );

  return (
    <div class="tree">
      <div class="tree-tools">
        <button type="button" class="btn tiny ghost" onClick={() => startNaming({ kind: 'file', at: '' }, '')}>
          New file
        </button>
        <button type="button" class="btn tiny ghost" onClick={() => startNaming({ kind: 'folder', at: '' }, '')}>
          New folder
        </button>
      </div>
      <Show when={problem()}>
        <p class="tree-problem" role="alert">
          <span class="grow">{problem()}</span>
          <button type="button" class="btn tiny" onClick={stopNaming}>
            Dismiss
          </button>
        </p>
      </Show>
      {level('', 0)}
    </div>
  );
}

/** The files that are not as they were at the last review, as a flat list. */
function ChangedList(props: {
  task: Task;
  current: string | null;
  onOpen: (path: string) => void;
  onPutBack: (path: string) => void;
}): JSX.Element {
  return (
    <div class="tree">
      <Show when={props.task.changed.length === 0}>
        <span class="tree-note">Nothing has changed since your last review.</span>
      </Show>
      <For each={props.task.changed}>
        {(file) => (
          <Show
            when={file.exists}
            fallback={
              <div class="tree-row gone">
                <span class="label" title={file.path}>
                  {file.path}
                </span>
                <span class="tree-tag">deleted</span>
                <Show when={file.agentLines > 0}>
                  <button type="button" class="btn tiny" onClick={() => props.onPutBack(file.path)}>
                    Put back
                  </button>
                </Show>
              </div>
            }
          >
            <button
              type="button"
              class="tree-row"
              aria-current={file.path === props.current ? 'true' : undefined}
              onClick={() => props.onOpen(file.path)}
            >
              <span class="label" title={file.path}>
                {file.path}
              </span>
              <ChangeBadges review={file} task={props.task} />
            </button>
          </Show>
        )}
      </For>
    </div>
  );
}

type OpenFile = FileMeta & {
  path: string;
  loading: boolean;
  /** Why the file cannot be shown. */
  problem: string | null;
  /** The version the text was loaded or last saved with; `null` means "save over whatever is there". */
  version: string | null;
  language: string;
  saving: boolean;
  saveProblem: string | null;
  /** The file changed on disk while it has unsaved edits here. */
  stale: boolean;
};

const blank: Omit<OpenFile, 'path' | 'language'> = {
  loading: true,
  problem: null,
  version: null,
  dirty: false,
  line: 1,
  column: 1,
  selection: null,
  agentLines: 0,
  saving: false,
  saveProblem: null,
  stale: false,
};

type BottomTab = 'terminal' | 'run' | 'problems';

/** The panel under the code: the terminal now; running the project and problems later. */
function BottomPanel(props: { task: Task; shown: boolean }): JSX.Element {
  const [tab, setTab] = createSignal<BottomTab>('terminal');
  // The shell is started the first time the terminal is opened, and then kept.
  const [terminalUsed, setTerminalUsed] = createSignal(false);
  const terminalVisible = () => props.shown && bottomOpen() && tab() === 'terminal';
  createEffect(() => {
    if (terminalVisible()) setTerminalUsed(true);
  });
  // The same for what the project runs: it is read when the tab is first opened.
  const [runUsed, setRunUsed] = createSignal(false);
  createEffect(() => {
    if (props.shown && bottomOpen() && tab() === 'run') setRunUsed(true);
  });

  const pick = (next: BottomTab) => {
    if (bottomOpen() && tab() === next) setBottomOpen(false);
    else {
      setTab(next);
      setBottomOpen(true);
    }
  };

  const startResize = (event: PointerEvent) => {
    event.preventDefault();
    const startY = event.clientY;
    const startHeight = bottomHeight();
    const move = (moved: PointerEvent) => setBottomHeight(startHeight + (startY - moved.clientY));
    const stop = () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', stop);
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', stop);
  };

  const tabButton = (id: BottomTab, label: string) => (
    <button type="button" class="bottom-tab" aria-pressed={bottomOpen() && tab() === id} onClick={() => pick(id)}>
      {label}
    </button>
  );

  return (
    <section class="bottom" aria-label="Terminal, run and problems">
      <Show when={bottomOpen()}>
        <div
          class="bottom-grip"
          onPointerDown={startResize}
          role="separator"
          aria-orientation="horizontal"
          aria-label="Drag to resize the panel"
        />
      </Show>
      <div class="bottom-tabs">
        {tabButton('terminal', 'Terminal')}
        {tabButton('run', 'Run')}
        {tabButton(
          'problems',
          problemCount(props.task.info.id) > 0 ? `Problems ${problemCount(props.task.info.id)}` : 'Problems',
        )}
        <span class="grow" />
        <Show when={bottomOpen()}>
          <button type="button" class="btn tiny" onClick={() => setBottomOpen(false)}>
            Hide
          </button>
        </Show>
      </div>
      <div class="bottom-body" style={{ height: `${bottomHeight()}px`, display: bottomOpen() ? 'flex' : 'none' }}>
        <Show when={terminalUsed()}>
          <div class="bottom-fill" style={{ display: tab() === 'terminal' ? 'flex' : 'none' }}>
            <Suspense>
              <TerminalView taskId={props.task.info.id} visible={terminalVisible()} />
            </Suspense>
          </div>
        </Show>
        <Show when={runUsed()}>
          <div class="bottom-fill scroll" style={{ display: tab() === 'run' ? 'flex' : 'none' }}>
            <RunTab task={props.task} visible={props.shown && bottomOpen() && tab() === 'run'} />
          </div>
        </Show>
        <Show when={tab() === 'problems'}>
          <div class="bottom-fill scroll">
            <ProblemsTab
              taskId={props.task.info.id}
              onOpen={(path, line, column) => openInEditor(props.task.info.id, path, line, column)}
            />
          </div>
        </Show>
      </div>
    </section>
  );
}

function CodeView(props: { task: Task; shown: boolean }): JSX.Element {
  // eslint-disable-next-line solid/reactivity -- this component is made anew for each task, so the id never changes under it
  const taskId = props.task.info.id;
  const agent = () => agentOf(props.task);
  const agentName = () => agent()?.name ?? 'the agent';

  const [files, setFiles] = createStore<OpenFile[]>([]);
  const [current, setCurrent] = createSignal<string | null>(null);
  const [listing, setListing] = createSignal<'all' | 'changed'>('all');
  const [problem, setProblem] = createSignal<string | null>(null);
  let editorEl: HTMLDivElement | undefined;
  let host: EditorHost | undefined;
  // What language servers add to the open files. The editor works the same without any.
  const lsp = new LspSession({
    taskId,
    // eslint-disable-next-line solid/reactivity -- a task never moves to another project
    projectId: props.task.info.projectId,
    host: () => host,
    open: (path, line, column) => openInEditor(taskId, path, line, column),
  });

  const byPath = (path: string) => (file: OpenFile) => file.path === path;
  const fileAt = (path: string | null) => (path ? files.find((file) => file.path === path) : undefined);
  const shown = createMemo(() => fileAt(current()));

  /** Tells the rest of the interface what is being edited by hand. */
  const publish = () => {
    const file = shown();
    setWork(taskId, {
      editing: file && file.dirty ? { path: file.path, line: file.line } : null,
      unsaved: files.filter((entry) => entry.dirty).length,
    });
  };

  // Which files were open is remembered per task, so coming back shows the same tabs.
  const tabsKey = `ae.tabs.${taskId}`;
  const rememberTabs = () => {
    try {
      localStorage.setItem(tabsKey, JSON.stringify({ paths: files.map((file) => file.path), current: current() }));
    } catch {
      // Storage may be refused; the tabs then simply start empty next time.
    }
  };

  const open = async (path: string): Promise<boolean> => {
    setCurrent(path);
    const known = fileAt(path);
    if (known) return !known.loading && !known.problem;
    setFiles(files.length, { ...blank, path, language: languageOf(path).name });
    rememberTabs();
    try {
      const { file, review } = await readFile(taskId, path);
      if (!fileAt(path) || !host) return false;
      host.open(path, file.text, review.runs, languageOf(path));
      lsp.opened(path);
      setFiles(byPath(path), { loading: false, version: file.version, agentLines: review.agentLines });
      return true;
    } catch (error) {
      setFiles(byPath(path), { loading: false, problem: message(error) });
      return false;
    }
  };

  const save = async (path: string) => {
    const file = fileAt(path);
    let snapshot = host?.snapshot(path);
    if (!file || !snapshot || file.saving || file.loading || file.problem) return;
    if (!file.dirty && file.version !== null) return;
    setFiles(byPath(path), { saving: true, saveProblem: null });
    try {
      // The language server tidies the text first when it can do so quickly; if not, the file is saved as typed.
      await lsp.format(path);
      snapshot = host?.snapshot(path) ?? snapshot;
      const result = await writeFile(taskId, path, snapshot.text, file.version);
      setFiles(byPath(path), { version: result.file.version, stale: false });
      host?.markSaved(path, snapshot.doc, result.review.runs);
      lsp.saved(path);
    } catch (error) {
      const text = message(error);
      setFiles(byPath(path), { saveProblem: text, stale: text.includes('changed on disk') });
    } finally {
      setFiles(byPath(path), 'saving', false);
    }
  };

  const saveEverything = async () => {
    await Promise.all(files.filter((file) => file.dirty).map((file) => save(file.path)));
  };

  /** Closes the tabs of files that are gone. There is nothing left to save them to. */
  const drop = (paths: string[]) => {
    const gone = new Set(paths);
    if (!files.some((file) => gone.has(file.path))) return;
    const index = files.findIndex((file) => file.path === current());
    for (const file of files.filter((one) => gone.has(one.path))) host?.close(file.path);
    const rest = files.filter((file) => !gone.has(file.path));
    setFiles(rest);
    if (current() !== null && gone.has(current()!)) setCurrent(rest[Math.max(0, index - 1)]?.path ?? null);
    rememberTabs();
    publish();
  };

  /** Files got new names: their tabs are opened again under them. */
  const follow = async (moved: { from: string; to: string }[]) => {
    const shown = current();
    const open_ = moved.filter((one) => fileAt(one.from));
    drop(open_.map((one) => one.from));
    for (const one of open_) await open(one.to);
    const next = moved.find((one) => one.from === shown);
    if (next) setCurrent(next.to);
    else if (shown !== null && fileAt(shown)) setCurrent(shown);
  };

  const close = async (path: string) => {
    // What was typed is never thrown away: closing a tab saves it first.
    if (fileAt(path)?.dirty) await save(path);
    if (fileAt(path)?.dirty) return; // the save was refused; the tab stays so nothing is lost
    const index = files.findIndex((file) => file.path === path);
    const rest = files.filter((file) => file.path !== path);
    host?.close(path);
    setFiles(rest);
    if (current() === path) setCurrent(rest[Math.max(0, index - 1)]?.path ?? null);
    rememberTabs();
    publish();
  };

  /** Looks at a file on disk again and brings the editor in line with it. */
  const refresh = async (path: string, force = false) => {
    const before = fileAt(path);
    if (!before || before.loading || before.problem || before.saving) return;
    try {
      const { file: disk, review } = await readFile(taskId, path);
      const now = fileAt(path);
      if (!now || !host) return;
      if (disk.version === now.version && !force) {
        host.setRuns(path, review.runs);
      } else if (!now.dirty || force) {
        host.replaceFromDisk(path, disk.text, review.runs);
        setFiles(byPath(path), { version: disk.version, stale: false, saveProblem: null });
      } else {
        setFiles(byPath(path), 'stale', true);
      }
    } catch {
      // The file is gone. A tab without edits of its own goes with it.
      const now = fileAt(path);
      if (now && !now.dirty) void close(path);
    }
  };

  const act = async (work: () => Promise<unknown>) => {
    setProblem(null);
    try {
      await work();
    } catch (error) {
      setProblem(message(error));
    }
  };

  const keep = (path: string) =>
    act(async () => {
      await keepFile(taskId, path);
      host?.clearAgentMarks(path);
    });

  const undo = (path: string) =>
    // eslint-disable-next-line solid/reactivity -- called later, as an event handler: it reads what is current then
    act(async () => {
      const result = await undoFile(taskId, path);
      if (result.review.exists) await refresh(path, true);
      else if (fileAt(path)) {
        host?.close(path);
        setFiles(files.filter((file) => file.path !== path));
        if (current() === path) setCurrent(files[0]?.path ?? null);
        rememberTabs();
      }
    });

  onMount(() => {
    host = new EditorHost(editorEl!, {
      short: agent()?.short ?? 'Ag',
      onMeta: (path, meta) => {
        setFiles(byPath(path), meta);
        publish();
        lsp.changed(path);
      },
      onClosed: (path) => lsp.closed(path),
      onSave: (path) => void save(path),
      onUserEdit: () => {
        // Two hands in one file make a mess: when the user starts typing, the agent's turn stops.
        if (props.task.info.state === 'running' && !workOf(taskId).pausedByTyping) {
          setWork(taskId, { pausedByTyping: true });
          void cancelTurn(taskId).catch(() => {});
        }
      },
      onAsk: (path, fromLine, toLine, text) => {
        setWork(taskId, { quote: { path, fromLine, toLine, text } });
        document.getElementById(`msg-${taskId}`)?.focus();
      },
    });

    const stopSaver = registerSaver(taskId, saveEverything);
    // eslint-disable-next-line solid/reactivity -- called later, as an event handler: it reads what is current then
    const stopEvents = core.onEvent((event) => {
      if (event.event === 'review.changed' && event.data.taskId === taskId) {
        for (const path of event.data.paths as string[]) {
          if (fileAt(path)) void refresh(path);
        }
      }
    });
    // Coming back to the window: another program may have changed the files meanwhile.
    const onFocus = () => {
      for (const file of files) void refresh(file.path);
    };
    const onLeave = (event: BeforeUnloadEvent) => {
      if (files.some((file) => file.dirty)) event.preventDefault();
    };
    window.addEventListener('focus', onFocus);
    window.addEventListener('beforeunload', onLeave);

    // Open what was open the last time this task was on screen.
    try {
      const remembered = JSON.parse(localStorage.getItem(tabsKey) ?? 'null') as {
        paths?: string[];
        current?: string | null;
      } | null;
      const paths = (remembered?.paths ?? []).slice(0, 12);
      void (async () => {
        for (const path of paths) await open(path);
        // A file that is gone meanwhile leaves no dead tab behind.
        setFiles(files.filter((file) => !file.problem));
        const wanted = remembered?.current;
        setCurrent(wanted && fileAt(wanted) ? wanted : (files[0]?.path ?? null));
        rememberTabs();
      })();
    } catch {
      // Nothing remembered.
    }

    onCleanup(() => {
      // Leaving the task saves what was typed: nothing is lost by walking away.
      for (const file of files) {
        const snapshot = file.dirty ? host?.snapshot(file.path) : null;
        if (snapshot) void writeFile(taskId, file.path, snapshot.text, file.version).catch(() => {});
      }
      setWork(taskId, { editing: null, unsaved: 0 });
      stopSaver();
      stopEvents();
      window.removeEventListener('focus', onFocus);
      window.removeEventListener('beforeunload', onLeave);
      lsp.dispose();
      host?.destroy();
      host = undefined;
    });
  });

  // The file picked in the tabs is the one in the editor.
  createEffect(() => {
    const file = shown();
    if (file && !file.loading && !file.problem) host?.show(file.path);
    rememberTabs();
  });

  // Something elsewhere asked to see a file: a search hit, a file named in the conversation.
  createEffect(
    on(openRequest, (request) => {
      if (!request || request.taskId !== taskId) return;
      void open(request.path).then((ready) => {
        if (!ready || current() !== request.path) return;
        host?.show(request.path);
        if (request.line) host?.reveal(request.line, request.column ?? 1, request.length ?? 0);
        else host?.focus();
      });
    }),
  );

  const editorVisible = () => {
    const file = shown();
    return Boolean(file && !file.loading && !file.problem);
  };

  return (
    <>
      <section class="files" aria-label="Files of this task" style={{ display: props.shown ? undefined : 'none' }}>
        <div class="files-head">
          <h2 class="pane-title grow">Files</h2>
          <button type="button" class="seg" aria-pressed={listing() === 'all'} onClick={() => setListing('all')}>
            All
          </button>
          <button
            type="button"
            class="seg"
            aria-pressed={listing() === 'changed'}
            onClick={() => setListing('changed')}
          >
            Changed{props.task.changed.length > 0 ? ` ${props.task.changed.length}` : ''}
          </button>
        </div>
        <div style={{ display: listing() === 'all' ? undefined : 'none' }}>
          <FileTree
            task={props.task}
            current={current()}
            onOpen={(path) => void open(path)}
            onMoved={(moved) => void follow(moved)}
            onGone={(paths) => void drop(paths)}
          />
        </div>
        <Show when={listing() === 'changed'}>
          <ChangedList
            task={props.task}
            current={current()}
            onOpen={(path) => void open(path)}
            onPutBack={(path) => void undo(path)}
          />
        </Show>
      </section>

      <section
        class={`center ${toneOf(props.task.info.agentId)}`}
        style={{ display: props.shown ? undefined : 'none' }}
      >
        <Show when={files.length > 0}>
          <div class="tabs" role="tablist" aria-label="Open files">
            <For each={files}>
              {(file) => (
                <span class="tab" aria-current={file.path === current() ? 'true' : undefined}>
                  <button
                    type="button"
                    class="open"
                    role="tab"
                    aria-selected={file.path === current()}
                    title={file.dirty ? `${file.path} — not saved` : file.path}
                    onClick={() => setCurrent(file.path)}
                  >
                    {baseName(file.path)}
                    <Show when={file.dirty}>
                      <span class="unsaved-dot" role="img" aria-label="not saved" />
                    </Show>
                  </button>
                  <button
                    type="button"
                    class="close"
                    aria-label={`Close ${baseName(file.path)}`}
                    onClick={() => void close(file.path)}
                  >
                    <Icon.Close />
                  </button>
                </span>
              )}
            </For>
            <span class="grow" />
            <Show when={shown()}>
              {(file) => (
                <>
                  <LspBadge lsp={lsp} path={file().path} />
                  <span class="tab-note">{file().language}</span>
                </>
              )}
            </Show>
          </div>
        </Show>

        <Show when={problem()}>
          <p class="banner danger" role="alert">
            <span class="grow">{problem()}</span>
            <button type="button" class="btn tiny" onClick={() => setProblem(null)}>
              Dismiss
            </button>
          </p>
        </Show>

        <Show when={shown()}>
          {(file) => (
            <>
              <Show when={file().agentLines > 0}>
                <div class="banner review">
                  <Chip agentId={props.task.info.agentId} short={agent()?.short ?? 'Ag'} />
                  <span class="grow">{agentName()} wrote the highlighted lines. You have not reviewed them yet.</span>
                  <button type="button" class="btn small" onClick={() => void keep(file().path)}>
                    Keep
                  </button>
                  <button
                    type="button"
                    class="btn small ghost"
                    disabled={file().dirty}
                    title={file().dirty ? 'Save your own edits first' : undefined}
                    onClick={() => void undo(file().path)}
                  >
                    Undo
                  </button>
                </div>
              </Show>
              <Show when={file().stale}>
                <div class="banner warn" role="alert">
                  <span class="grow">{baseName(file().path)} changed on disk while you were editing it.</span>
                  <button
                    type="button"
                    class="btn small"
                    onClick={() => void refresh(file().path, true).then(() => host?.focus())}
                  >
                    Load the version on disk
                  </button>
                  <button
                    type="button"
                    class="btn small ghost"
                    onClick={() => {
                      setFiles(byPath(file().path), { stale: false, version: null, saveProblem: null });
                      // Back to the code, so the next key press, such as the one that saves, lands there.
                      host?.focus();
                    }}
                  >
                    Keep mine
                  </button>
                </div>
              </Show>
              <Show when={file().saveProblem && !file().stale}>
                <p class="banner danger" role="alert">
                  <span class="grow">Not saved: {file().saveProblem}</span>
                </p>
              </Show>
            </>
          )}
        </Show>

        <Switch>
          <Match when={files.length === 0}>
            <div class="center-note">
              <h2>Pick a file on the left</h2>
              <p>
                It opens here, next to the conversation. Edit it like in any editor; {modKey}S saves, and {agentName()}{' '}
                is told what you changed.
              </p>
            </div>
          </Match>
          <Match when={shown()?.problem}>
            <div class="center-note">
              <h2>This file cannot be shown</h2>
              <p>{shown()?.problem}</p>
            </div>
          </Match>
          <Match when={shown()?.loading}>
            <div class="center-note">
              <p>Reading the file</p>
            </div>
          </Match>
        </Switch>

        <div class="editor" ref={editorEl} style={{ display: editorVisible() ? undefined : 'none' }} />

        <Show when={editorVisible() && shown()}>
          {(file) => (
            <div class="statusline">
              <span>
                Line {file().line}, column {file().column}
              </span>
              <span classList={{ unsaved: file().dirty }}>
                {file().saving ? 'Saving' : file().dirty ? `Not saved · ${modKey}S saves` : 'Saved'}
              </span>
              <LspStatus lsp={lsp} path={file().path} />
              <span class="grow" />
              <span>
                Select code and press <span class="mono soft">{modKey}K</span> to ask {agentName()} about it
              </span>
            </div>
          )}
        </Show>

        <BottomPanel task={props.task} shown={props.shown} />
      </section>
    </>
  );
}

/** How many files of each task are not committed, for the number next to "Changes". */
const [uncommitted, setUncommitted] = createStore<Record<string, number>>({});

/** The middle of the screen for one task: the view picked in the switcher. */
function TaskViews(props: { task: Task; view: View }): JSX.Element {
  // eslint-disable-next-line solid/reactivity -- this component is made anew for each task, so the id never changes under it
  const git: GitWatch = watchGit(props.task.info.id);
  createEffect(() => setUncommitted(props.task.info.id, git.state()?.files.length ?? 0));
  const lookingBack = () => workOf(props.task.info.id).lookBack !== null;
  const changesMode = () => workOf(props.task.info.id).changes;
  // The checks are kept current for as long as the task is on screen: their count is in the switcher.
  // eslint-disable-next-line solid/reactivity -- this component is made anew for each task
  onCleanup(watchChecks(props.task.info.id));

  return (
    <>
      {/* The code view stays alive under the other views, so open files and unsaved edits survive. */}
      <CodeView task={props.task} shown={props.view === 'code' && !lookingBack()} />
      <Show when={lookingBack()}>
        <LookBackView task={props.task} />
      </Show>
      <Show when={props.view === 'changes' && !lookingBack()}>
        <Show when={changesMode() === 'head'} fallback={<SpanView task={props.task} mode={changesMode() as 'visit'} />}>
          <ChangesView task={props.task} git={git} />
        </Show>
      </Show>
      <Show when={props.view === 'checks' && !lookingBack()}>
        <ChecksView task={props.task} />
      </Show>
      <Show when={props.view === 'map' && !lookingBack()}>
        <MapView task={props.task} />
      </Show>
      <Show when={props.view === 'preview' && !lookingBack()}>
        <PreviewView task={props.task} />
      </Show>
      <Show
        when={
          props.view !== 'code' &&
          props.view !== 'changes' &&
          props.view !== 'checks' &&
          props.view !== 'map' &&
          props.view !== 'preview' &&
          !lookingBack()
        }
      >
        <section class="center">
          <div class="center-note">
            <h2>{viewNames[props.view]} is not built yet</h2>
            <p>{notBuilt[props.view]?.what}</p>
            <p>
              It arrives with milestone {notBuilt[props.view]?.milestone}. The conversation on the right already works.
            </p>
          </div>
        </section>
      </Show>
    </>
  );
}

/** One task: its files and code in the middle, its agent always on the right. */
export function TaskScreen(props: { id: string; view: View }): JSX.Element {
  const task = () => state.tasks[props.id];
  // The task last seen under this id. It keeps its value while this screen is
  // being taken down (the task was archived, or another screen was opened), so
  // whatever still reads it at that moment finds a task and not a hole.
  const held = createMemo<Task | undefined>((last) => task() ?? (last?.info.id === props.id ? last : undefined));
  const current = () => held()!;

  // Coming to a task asks what happened since the last visit; going away from it
  // (to another screen, another task, another window) is where the next visit counts from.
  createEffect(
    on(
      () => (task() ? props.id : null),
      (id) => {
        if (!id) return;
        arrive(id);
        const onVisibility = () => (document.hidden ? leave(id) : arrive(id));
        const onHide = () => leave(id);
        document.addEventListener('visibilitychange', onVisibility);
        window.addEventListener('pagehide', onHide);
        onCleanup(() => {
          document.removeEventListener('visibilitychange', onVisibility);
          window.removeEventListener('pagehide', onHide);
          leave(id);
        });
      },
    ),
  );

  // While a task is on screen, what arrives counts as read. The core keeps the mark,
  // so "what changed since you were last here" can be told later.
  createEffect(() => {
    const current = task();
    if (!current || current.lastSeq < 0 || current.lastSeq <= current.info.seenSeq) return;
    const { id } = current.info;
    const seq = current.lastSeq;
    const timer = setTimeout(() => void markSeen(id, seq).catch(() => {}), 800);
    onCleanup(() => clearTimeout(timer));
  });

  return (
    <Show
      when={task()}
      fallback={
        <main class="page">
          <div class="page-inner">
            <h1 class="headline">{state.ready ? 'This task is not here' : 'Connecting to the core'}</h1>
            <Show when={state.ready}>
              <p class="soft">
                It may have been archived. <a href="#/">Go to Home</a>
              </p>
            </Show>
          </div>
        </main>
      }
    >
      <>
        <div class="subbar">
          <span class="project">{projectName(current().info.projectId)}</span>
          <h1 title={current().info.title}>{current().info.title}</h1>
          <nav class="switcher" aria-label="Views of this task">
            <For each={VIEWS}>
              {(view) => (
                <a href={taskHref(props.id, view)} aria-current={view === props.view ? 'page' : undefined}>
                  {viewNames[view]}
                  <Show when={view === 'code' && workOf(props.id).unsaved > 0}>
                    <span class="count" title="files with unsaved edits">
                      {workOf(props.id).unsaved}
                    </span>
                  </Show>
                  <Show when={view === 'checks' && (checksOf(props.id)?.total ?? 0) > 0}>
                    <span
                      class="count plain"
                      classList={{ bad: (checksOf(props.id)?.failed ?? 0) > 0 }}
                      title="lines of “Done when” that are done"
                    >
                      {checksOf(props.id)!.done} of {checksOf(props.id)!.total}
                    </span>
                  </Show>
                  <Show when={view === 'changes' && (uncommitted[props.id] ?? 0) > 0}>
                    <span class="count plain" title="files that are not committed">
                      {uncommitted[props.id]}
                    </span>
                  </Show>
                </a>
              )}
            </For>
          </nav>
          <Show when={current().info.issue}>
            {(issue) => (
              <a class="issue-chip" href={issue().url} target="_blank" rel="noreferrer" title={issue().title}>
                {issue().key}
              </a>
            )}
          </Show>
          <Show when={current().info.branch}>
            <span class="mono muted" style={{ 'font-size': '12px' }}>
              {current().info.branch}
            </span>
          </Show>
          <span class="grow" />
          <span class="state" classList={{ needs: stageOfTask(current()) === 'needs' }}>
            {stageWord(stageOfTask(current()))}
          </span>
        </div>

        <div class="workbench">
          {/* Everything that belongs to one task is made anew for another task, and kept while the views change:
                open files and unsaved edits survive a look at the changes. */}
          <Show when={props.id} keyed>
            {(id) => (
              <Show when={state.tasks[id]}>
                <TaskViews task={current()} view={props.view} />
              </Show>
            )}
          </Show>
          <AgentPane task={current()} />
        </div>
      </>
    </Show>
  );
}
