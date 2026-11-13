import { createEffect, createMemo, createSignal, For, on, onCleanup, onMount, Show, type JSX } from 'solid-js';
import { go, route, taskHref, VIEWS } from '../app/router';
import { showCommit } from '../core/commits';
import { onboardingOpen } from '../core/onboarding';
import {
  agentOf,
  projectName,
  searchChats,
  searchCommits,
  searchFiles,
  searchFilesEverywhere,
  searchText,
  searchTextEverywhere,
  state,
  tasksNewestFirst,
  type ChatHit,
  type CommitHit,
  type FileHit,
  type FileHitIn,
  type Task,
  type TextHit,
  type TextHitIn,
} from '../core/store';
import { openInEditor, setBottomOpen, setWork } from '../core/work';
import { ago, Chip, Icon, modKey, StateGlyph, stateLabel } from './bits';

const [open, setOpen] = createSignal(false);

/** Opens the search over everything. */
export function openSearch() {
  setOpen(true);
}

type Kind = 'tasks' | 'files' | 'chats' | 'commits' | 'code' | 'commands';
/** The kinds of results, in the order their groups are shown. */
const KINDS: Kind[] = ['tasks', 'files', 'chats', 'commits', 'code', 'commands'];
const kindNames: Record<Kind, string> = {
  tasks: 'Tasks',
  files: 'Files',
  chats: 'Chats',
  commits: 'Commits',
  code: 'Code',
  commands: 'Commands',
};

/** One row of results. `text` is what is shown, `marks` the characters to highlight in it. */
type Row = {
  kind: Kind;
  key: string;
  text: string;
  marks: number[];
  note: string;
  mono?: boolean;
  lead: () => JSX.Element;
  run: () => void;
};

/** What the core found, kept together with what it was found for: the query, and the task on screen. */
type Found<T> = { query: string; here: string | undefined; hits: T[] };
const nothing = { query: '', here: undefined, hits: [] };

/**
 * Asks the core one thing at a time, and always the newest: while an answer
 * is on its way, a newer question waits and replaces any that waited before
 * it. Typing fast then costs the core one search, not one per key.
 */
function oneAtATime<Q>(ask: (question: Q) => Promise<void>): { ask: (question: Q) => void; forget: () => void } {
  let busy = false;
  let waiting: { question: Q } | null = null;
  const run = async () => {
    busy = true;
    while (waiting) {
      const { question } = waiting;
      waiting = null;
      await ask(question);
    }
    busy = false;
  };
  return {
    ask: (question) => {
      waiting = { question };
      if (!busy) void run();
    },
    forget: () => (waiting = null),
  };
}

/** Where `needle` sits in `text`, as character positions, ignoring case. Empty when it does not. */
function positionsOf(text: string, needle: string): number[] | null {
  if (!needle) return [];
  const at = text.toLowerCase().indexOf(needle.toLowerCase());
  if (at < 0) return null;
  return Array.from({ length: needle.length }, (_, i) => at + i);
}

/**
 * A found line of code as a row shows it. A row is narrow, and a match far
 * into the line would be cut off: then the line starts a little before the match.
 */
function aroundMatch(line: string, column: number, length: number): { text: string; marks: number[] } {
  const from = column > 28 ? column - 16 : 0;
  const text = from > 0 ? `…${Array.from(line).slice(from).join('')}` : line;
  const start = from > 0 ? column - from + 1 : column;
  return { text, marks: Array.from({ length }, (_, i) => start + i) };
}

function Marked(props: { text: string; marks: number[] }): JSX.Element {
  // Runs of marked and plain characters, so a long text is a handful of nodes, not one per letter.
  const pieces = createMemo(() => {
    const marked = new Set(props.marks);
    const out: { text: string; on: boolean }[] = [];
    Array.from(props.text).forEach((char, index) => {
      const on = marked.has(index);
      const last = out[out.length - 1];
      if (last && last.on === on) last.text += char;
      else out.push({ text: char, on });
    });
    return out;
  });
  return <For each={pieces()}>{(piece) => (piece.on ? <mark>{piece.text}</mark> : <>{piece.text}</>)}</For>;
}

const arrow = () => (
  <svg
    width="12"
    height="12"
    viewBox="0 0 12 12"
    fill="none"
    stroke="currentColor"
    stroke-width="1.5"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
  >
    <path d="M3 2l5 4-5 4" />
  </svg>
);
const fileGlyph = () => (
  <svg
    width="12"
    height="12"
    viewBox="0 0 12 12"
    fill="none"
    stroke="currentColor"
    stroke-width="1.3"
    stroke-linejoin="round"
    aria-hidden="true"
  >
    <path d="M3 1.5h4l2.5 2.5v6.5h-6.5z" />
  </svg>
);

/**
 * The search over everything: tasks, files and code, things said in any task,
 * commits and commands. Opens with the search key.
 *
 * The task on screen is searched first, and its answer is quick. The other
 * tasks, of every project, are asked about apart from it: their files and
 * lines are added below when they arrive, each with the project and the task
 * it belongs to.
 */
export function SearchPalette(): JSX.Element {
  const [query, setQuery] = createSignal('');
  const [filter, setFilter] = createSignal<Kind | 'all'>('all');
  // What the core found, kept together with the query it was found for. Hits of an
  // older query are never shown: for a moment after a key press they would still be
  // on screen, and a quick click or Enter would open something nobody searched for.
  const [foundFiles, setFoundFiles] = createSignal<{ query: string; hits: FileHit[] }>({ query: '', hits: [] });
  const [foundChats, setFoundChats] = createSignal<{ query: string; hits: ChatHit[] }>({ query: '', hits: [] });
  const files = () => (foundFiles().query === parsed().text ? foundFiles().hits : []);
  const chats = () => (foundChats().query === parsed().text ? foundChats().hits : []);
  const setFiles = (hits: FileHit[], query = parsed().text) => setFoundFiles({ query, hits });
  const setChats = (hits: ChatHit[], query = parsed().text) => setFoundChats({ query, hits });
  const [code, setCode] = createSignal<TextHit[] | null>(null);
  const [searchingCode, setSearchingCode] = createSignal(false);
  // The same rule for what came later: commits, and the files and lines of the other tasks.
  // An answer is shown only while it is the answer to what is in the field now.
  const [foundCommits, setFoundCommits] = createSignal<Found<CommitHit>>(nothing);
  const [foundElsewhere, setFoundElsewhere] = createSignal<Found<FileHitIn>>(nothing);
  const current = <T,>(found: Found<T>): T[] =>
    found.query === parsed().text && found.here === here()?.info.id ? found.hits : [];
  const commits = () => current(foundCommits());
  const filesElsewhere = () => current(foundElsewhere());
  const [codeElsewhere, setCodeElsewhere] = createSignal<TextHitIn[] | null>(null);
  const [searchingElsewhere, setSearchingElsewhere] = createSignal(false);
  // The picked row is remembered by what it is, not by its place: rows that arrive
  // later must not put another row under the user's Enter. `null` is the first row.
  const [pickedKey, setPickedKey] = createSignal<string | null>(null);
  // Counts the searches through the code, and the times the search was opened: an answer to an earlier one is dropped.
  let codeRun = 0;
  let session = 0;
  let input: HTMLInputElement | undefined;
  let list: HTMLDivElement | undefined;

  const here = (): Task | undefined => {
    const current = route();
    return current.page === 'task' ? state.tasks[current.id] : undefined;
  };

  // The files of the other tasks are asked for one question at a time: the core
  // looks through every project for each, and only the newest question matters.
  const elsewhere = oneAtATime<{ query: string; here: string | undefined; session: number }>(async (asked) => {
    const hits = await searchFilesEverywhere(asked.query, asked.here, 50).then(
      (result) => result.hits,
      () => [],
    );
    if (asked.session !== session) return;
    const found = { query: asked.query, here: asked.here, hits };
    if (current(found).length > 0) holdPick();
    setFoundElsewhere(found);
  });

  const close = () => setOpen(false);
  const finish = (work: () => void) => () => {
    close();
    work();
  };

  // A first character can narrow the search: > commands, @ tasks, # files.
  const parsed = createMemo(() => {
    const raw = query();
    const prefix: Record<string, Kind> = { '>': 'commands', '@': 'tasks', '#': 'files' };
    const forced = prefix[raw[0] ?? ''];
    return { text: (forced ? raw.slice(1) : raw).trim(), forced };
  });
  const scope = (): Kind | 'all' => parsed().forced ?? filter();

  const showFile = (task: Task, path: string, line?: number, column?: number, length?: number) => {
    go(taskHref(task.info.id, 'code'));
    // The code view may need a moment to appear when coming from another screen.
    setTimeout(() => openInEditor(task.info.id, path, line, column, length), 0);
  };

  const commands = createMemo<Row[]>(() => {
    const rows: { text: string; note: string; run: () => void }[] = [
      { text: 'Start a new task', note: 'Home', run: () => go('#/') },
      { text: 'Go to Home', note: 'Home', run: () => go('#/') },
      { text: 'Open Issues', note: 'Issues', run: () => go('#/issues') },
      { text: 'Open Health', note: 'Health', run: () => go('#/health') },
      { text: 'Open Automations', note: 'Automations', run: () => go('#/automations') },
      { text: 'Open limits and cost', note: 'Agents', run: () => go('#/agents') },
    ];
    const task = here();
    if (task) {
      const names = {
        plan: 'Plan',
        code: 'Code',
        changes: 'Changes',
        preview: 'Preview',
        map: 'Map',
        checks: 'Checks',
      };
      for (const view of VIEWS) {
        rows.push({ text: `Show ${names[view]}`, note: 'this task', run: () => go(taskHref(task.info.id, view)) });
      }
      rows.push({
        text: 'Open the terminal',
        note: 'this task',
        run: () => {
          go(taskHref(task.info.id, 'code'));
          setBottomOpen(true);
        },
      });
    }
    const text = parsed().text;
    return rows.flatMap((row) => {
      const marks = positionsOf(row.text, text);
      if (marks === null) return [];
      return [
        {
          kind: 'commands' as const,
          key: `command:${row.text}`,
          text: row.text,
          marks,
          note: row.note,
          lead: arrow,
          run: finish(row.run),
        },
      ];
    });
  });

  const tasks = createMemo<Row[]>(() => {
    const text = parsed().text;
    return tasksNewestFirst().flatMap((task) => {
      const marks = positionsOf(task.info.title, text);
      if (marks === null) return [];
      return [
        {
          kind: 'tasks' as const,
          key: `task:${task.info.id}`,
          text: task.info.title,
          marks,
          note: `${projectName(task.info.projectId)} · ${stateLabel(task.info.state)}`,
          lead: () => <StateGlyph state={task.info.state} />,
          run: finish(() => go(taskHref(task.info.id))),
        },
      ];
    });
  });

  /** The project and the task a hit of another task belongs to. */
  const whose = (task: Task) => `${projectName(task.info.projectId)} · ${task.info.title}`;

  /** The files of the task on screen. */
  const fileRows = createMemo<Row[]>(() => {
    const task = here();
    return !task
      ? []
      : files().map((hit) => ({
          kind: 'files' as const,
          key: `file:${hit.path}`,
          text: hit.path,
          marks: hit.positions,
          note: task.changed.some((file) => file.path === hit.path)
            ? `${projectName(task.info.projectId)} · changed in this task`
            : projectName(task.info.projectId),
          mono: true,
          lead: fileGlyph,
          run: finish(() => showFile(task, hit.path)),
        }));
  });

  /** The files of the other tasks. They come after the ones of the task on screen, and later. */
  const fileRowsElsewhere = createMemo<Row[]>(() =>
    filesElsewhere().flatMap((hit): Row[] => {
      const owner = state.tasks[hit.taskId];
      if (!owner) return [];
      return [
        {
          kind: 'files',
          key: `file:${hit.taskId}:${hit.path}`,
          text: hit.path,
          marks: hit.positions,
          note: whose(owner),
          mono: true,
          lead: fileGlyph,
          run: finish(() => showFile(owner, hit.path)),
        },
      ];
    }),
  );

  const commitRows = createMemo<Row[]>(() => {
    const task = here();
    if (!task) return [];
    const text = parsed().text;
    return commits().map((hit) => ({
      kind: 'commits' as const,
      key: `commit:${hit.hash}`,
      text: hit.subject,
      // A commit found by its id has nothing to mark in its first line.
      marks: positionsOf(hit.subject, text) ?? [],
      note: `${hit.onBranch ? 'this task' : projectName(task.info.projectId)} · ${hit.author}, ${ago(hit.at)}`,
      lead: () => <span class="hit-id">{hit.short}</span>,
      run: finish(() => {
        // The commit is shown in the Changes view of the task, whatever that view was showing.
        setWork(task.info.id, { lookBack: null });
        showCommit(task.info.id, hit.hash);
        go(taskHref(task.info.id, 'changes'));
      }),
    }));
  });

  const chatRows = createMemo<Row[]>(() =>
    chats().flatMap((hit) => {
      const task = state.tasks[hit.taskId];
      if (!task) return [];
      const agent = agentOf(task);
      const where = here()?.info.id === hit.taskId ? 'this task' : task.info.title;
      return [
        {
          kind: 'chats' as const,
          key: `chat:${hit.taskId}:${hit.at}:${hit.column}`,
          text: hit.text,
          marks: Array.from({ length: hit.length }, (_, i) => hit.column + i),
          note: `${where} · ${ago(hit.at)}`,
          lead: () =>
            hit.who === 'you' ? (
              <Chip agentId="you" short="You" flat />
            ) : (
              <Chip agentId={task.info.agentId} short={agent?.short ?? 'Ag'} flat />
            ),
          run: finish(() => go(taskHref(hit.taskId))),
        },
      ];
    }),
  );

  const codeRows = createMemo<Row[]>(() => {
    const task = here();
    const mine: Row[] = !task
      ? []
      : (code() ?? []).map((hit) => ({
          kind: 'code' as const,
          key: `code:${hit.path}:${hit.line}:${hit.column}`,
          ...aroundMatch(hit.text, hit.column, hit.length),
          note: `${hit.path}:${hit.line}`,
          mono: true,
          lead: fileGlyph,
          run: finish(() => showFile(task, hit.path, hit.line, hit.column + 1, hit.length)),
        }));
    const others = (codeElsewhere() ?? []).flatMap((hit): Row[] => {
      const owner = state.tasks[hit.taskId];
      if (!owner) return [];
      return [
        {
          kind: 'code',
          key: `code:${hit.taskId}:${hit.path}:${hit.line}:${hit.column}`,
          ...aroundMatch(hit.text, hit.column, hit.length),
          // The place first: it is what tells two lines apart when the note is cut short.
          note: `${hit.path}:${hit.line} · ${whose(owner)}`,
          mono: true,
          lead: fileGlyph,
          run: finish(() => showFile(owner, hit.path, hit.line, hit.column + 1, hit.length)),
        },
      ];
    });
    return [...mine, ...others];
  });

  /**
   * The rows on screen, kind by kind. A row that reads the same as before is
   * the same object as before, so what is drawn for it stays in place. Answers
   * arrive one after another, some of them late: without this every answer
   * would draw every row anew, and a click that began on a row would end on its
   * replacement and be lost.
   */
  const shown = createMemo<Record<Kind, Row[]>>(
    (before) => {
      const wanted = scope();
      const all: Record<Kind, Row[]> = {
        tasks: tasks().slice(0, wanted === 'tasks' ? 50 : 5),
        // Among everything else a few rows of each are shown: a task with many matches
        // of its own must not push the other projects out of sight altogether.
        files:
          wanted === 'files'
            ? [...fileRows(), ...fileRowsElsewhere()].slice(0, 100)
            : [...fileRows().slice(0, 6), ...fileRowsElsewhere().slice(0, here() ? 3 : 6)],
        chats: chatRows().slice(0, wanted === 'chats' ? 50 : 4),
        commits: commitRows().slice(0, wanted === 'commits' ? 50 : 4),
        code: codeRows(),
        commands: commands().slice(0, wanted === 'commands' ? 50 : parsed().text ? 4 : 6),
      };
      const known = new Map(KINDS.flatMap((kind) => before[kind]).map((row) => [row.key, row]));
      const same = (a: Row, b: Row) =>
        a.text === b.text &&
        a.note === b.note &&
        a.marks.length === b.marks.length &&
        a.marks.every((n, i) => n === b.marks[i]);
      for (const kind of KINDS) {
        all[kind] =
          wanted === 'all' || wanted === kind
            ? all[kind].map((row) => {
                const old = known.get(row.key);
                return old && same(old, row) ? old : row;
              })
            : [];
      }
      return all;
    },
    { tasks: [], files: [], chats: [], commits: [], code: [], commands: [] },
  );

  /**
   * Looks through the code for `text`: in the task on screen first, then,
   * with `everywhere`, in every other task. The first answer is shown at
   * once; the second is added under it when it arrives.
   */
  const lookInCode = (text: string, everywhere: boolean) => {
    // A click on the row takes the keyboard away from the search field, and the row then
    // disappears: give the keyboard back, so the arrows, Enter and Esc go on working.
    input?.focus();
    const run = ++codeRun;
    const task = here();
    const mine =
      task && code() === null
        ? searchText(task.info.id, text, 100).then(
            (result) => result.hits,
            () => [],
          )
        : null;
    setSearchingCode(mine !== null);
    setSearchingElsewhere(everywhere);
    void (async () => {
      if (mine) {
        const hits = await mine;
        if (run !== codeRun) return;
        setCode(hits);
        setFilter('code');
        setPickedKey(null);
        setSearchingCode(false);
      }
      if (!everywhere) return;
      const hits = await searchTextEverywhere(text, task?.info.id, 100).then(
        (result) => result.hits,
        () => [],
      );
      if (run !== codeRun) return;
      // Rows found in the task on screen may be there already, and one of them under the user's hand.
      if (codeRows().length > 0) holdPick();
      setCodeElsewhere(hits);
      setFilter('code');
      setSearchingElsewhere(false);
    })();
  };

  /**
   * The rows that start a search through the code: of the task on screen, and
   * of every project. They are offered once there is something to look for.
   */
  const offers = createMemo<Row[]>(() => {
    const task = here();
    // The rows are made anew whenever the query changes, so the text they hold is the text on screen.
    const text = parsed().text;
    if (text.length < 2 || parsed().forced || codeElsewhere() !== null || searchingElsewhere()) return [];
    if (scope() !== 'all' && scope() !== 'code') return [];
    const offer = (key: string, where: string, everywhere: boolean): Row => ({
      kind: 'code',
      key,
      text: `Look for “${text}” inside the code of ${where}`,
      marks: Array.from({ length: text.length }, (_, i) => 'Look for “'.length + i),
      note: 'full text',
      lead: arrow,
      run: () => lookInCode(text, everywhere),
    });
    return [
      ...(task && code() === null ? [offer('code:offer', 'this task', false)] : []),
      offer('code:offer:everywhere', 'every project', true),
    ];
  });

  const rows = createMemo(() => [...KINDS.flatMap((kind) => shown()[kind]), ...offers()]);
  const picked = createMemo(() =>
    Math.max(
      0,
      rows().findIndex((row) => row.key === pickedKey()),
    ),
  );
  const setPicked = (index: number) => setPickedKey(rows()[index]?.key ?? null);
  /**
   * Keeps the highlight on the result it is on now, whatever is added above it
   * next. Called before rows that come late are shown: the user may be about to
   * press Enter. A row that only offers a search gives way to what was found.
   */
  const holdPick = () => {
    const row = rows()[picked()];
    if (row && !offers().includes(row)) setPickedKey(row.key);
  };
  const counts = createMemo<Record<Kind, number>>(() => ({
    tasks: tasks().length,
    files: fileRows().length + fileRowsElsewhere().length,
    chats: chatRows().length,
    commits: commitRows().length,
    code: codeRows().length,
    commands: commands().length,
  }));
  const total = () => Object.values(counts()).reduce((sum, n) => sum + n, 0);

  // Ask the core as the user types, a moment after the last key.
  createEffect(
    on(
      () => [parsed().text, open()] as const,
      ([text, isOpen]) => {
        // Whatever was being looked for in the code belongs to the query that is gone.
        codeRun++;
        setCode(null);
        setCodeElsewhere(null);
        setSearchingCode(false);
        setSearchingElsewhere(false);
        setPickedKey(null);
        if (filter() === 'code') setFilter('all');
        if (!isOpen || text === '') {
          setFiles([]);
          setChats([]);
          setFoundCommits(nothing);
          setFoundElsewhere(nothing);
          elsewhere.forget();
          return;
        }
        const task = here();
        const hereId = task?.info.id;
        // "@" and ">" ask for tasks and commands alone, and those are known without the core.
        const wantsFiles = parsed().forced === undefined || parsed().forced === 'files';
        const timer = setTimeout(() => {
          if (task) {
            void searchFiles(task.info.id, text, 50)
              .then((result) => setFiles(result.hits, text))
              .catch(() => setFiles([], text));
            void searchCommits(task.info.id, text, 50).then(
              (result) => setFoundCommits({ query: text, here: hereId, hits: result.hits }),
              () => setFoundCommits({ query: text, here: hereId, hits: [] }),
            );
          }
          void searchChats(text, 50)
            .then((result) => setChats(result.hits, text))
            .catch(() => setChats([], text));
          // The other tasks last, and apart: their answer may take a while and is added when it comes.
          if (wantsFiles) elsewhere.ask({ query: text, here: hereId, session });
        }, 90);
        onCleanup(() => clearTimeout(timer));
      },
    ),
  );

  createEffect(
    on(open, (isOpen) => {
      session++;
      if (!isOpen) return;
      setQuery('');
      setFilter('all');
      queueMicrotask(() => input?.focus());
    }),
  );

  // Keep the picked row in view while moving with the arrows.
  createEffect(() => {
    const index = picked();
    list?.querySelector(`[data-index="${index}"]`)?.scrollIntoView({ block: 'nearest' });
  });

  const onKeyDown = (event: KeyboardEvent) => {
    const count = rows().length;
    if (event.key === 'Escape') {
      event.preventDefault();
      close();
    } else if (event.key === 'ArrowDown') {
      event.preventDefault();
      if (count) setPicked((picked() + 1) % count);
    } else if (event.key === 'ArrowUp') {
      event.preventDefault();
      if (count) setPicked((picked() - 1 + count) % count);
    } else if (event.key === 'Enter' && !event.isComposing) {
      event.preventDefault();
      rows()[picked()]?.run();
    }
  };

  onMount(() => {
    const onGlobalKey = (event: KeyboardEvent) => {
      // The editor takes the key first when code is selected; then it is not ours.
      if (event.defaultPrevented) return;
      // The first run covers the page: a search opened under it would take the keyboard unseen.
      if (onboardingOpen()) return;
      if ((event.metaKey || event.ctrlKey) && !event.altKey && event.key.toLowerCase() === 'k') {
        event.preventDefault();
        setOpen(!open());
      }
    };
    window.addEventListener('keydown', onGlobalKey);
    onCleanup(() => window.removeEventListener('keydown', onGlobalKey));
  });

  const filters = (): (Kind | 'all')[] => [
    'all',
    'tasks',
    'files',
    'chats',
    // Commits are those of the task on screen and of its project; without a task there are none to look through.
    ...(here() ? (['commits'] as const) : []),
    ...(code() || codeElsewhere() ? (['code'] as const) : []),
    'commands',
  ];
  const rowIndex = (row: Row) => rows().indexOf(row);

  const renderRow = (row: Row) => (
    <button
      type="button"
      class="hit"
      data-index={rowIndex(row)}
      aria-current={rowIndex(row) === picked() ? 'true' : undefined}
      onMouseMove={() => setPicked(rowIndex(row))}
      onClick={() => row.run()}
    >
      <span class="hit-lead">{row.lead()}</span>
      <span class="hit-text" classList={{ mono: row.mono }}>
        <Marked text={row.text} marks={row.marks} />
      </span>
      <span class="hit-note" title={row.note}>
        {row.note}
      </span>
      <Show when={rowIndex(row) === picked()}>
        <kbd>Enter</kbd>
      </Show>
    </button>
  );

  return (
    <Show when={open()}>
      {/* Clicking outside closes the search; with the keyboard Esc does the same. */}
      <div class="scrim" role="presentation" onClick={close} />
      <section class="palette" role="dialog" aria-label="Search everything" onKeyDown={onKeyDown}>
        <div class="palette-top">
          <Icon.Search />
          <input
            ref={input}
            type="search"
            aria-label="Search tasks, files, chats, commits and commands"
            placeholder={
              here() ? 'Search tasks, files, chats, commits and commands' : 'Search tasks, files, chats and commands'
            }
            value={query()}
            onInput={(event) => setQuery(event.currentTarget.value)}
          />
          <kbd>Esc</kbd>
        </div>

        <Show when={!parsed().forced}>
          <div class="palette-filters" role="group" aria-label="Where to look">
            <For each={filters()}>
              {(kind) => (
                <button
                  type="button"
                  class="pill"
                  aria-pressed={filter() === kind}
                  onClick={() => (setFilter(kind), setPicked(0), input?.focus())}
                >
                  {kind === 'all' ? `All ${total()}` : `${kindNames[kind]} ${counts()[kind]}`}
                </button>
              )}
            </For>
          </div>
        </Show>

        <div class="palette-list" ref={list}>
          <For each={KINDS}>
            {(kind) => (
              <Show when={shown()[kind].length > 0}>
                <h3>{kindNames[kind]}</h3>
                <For each={shown()[kind]}>{renderRow}</For>
              </Show>
            )}
          </For>
          <Show when={searchingCode()}>
            <p class="palette-empty">Looking through the code</p>
          </Show>
          <Show when={searchingElsewhere() && !searchingCode()}>
            <p class="palette-empty">Looking through every project</p>
          </Show>
          <Show when={filter() === 'code' && codeRows().length === 0 && !searchingCode() && !searchingElsewhere()}>
            <p class="palette-empty">
              {codeElsewhere() ? 'Nothing in the code matches.' : 'Nothing in the code of this task matches.'}
            </p>
          </Show>
          {/* The rows are drawn anew for every query, so none of them offers to look for an older one. */}
          <For each={offers()}>{renderRow}</For>
          <Show when={rows().length === 0 && filter() !== 'code' && !searchingCode() && !searchingElsewhere()}>
            <p class="palette-empty">{parsed().text ? 'Nothing matches.' : 'Type to search.'}</p>
          </Show>
        </div>

        <div class="palette-foot">
          <span>
            <kbd>↑</kbd>
            <kbd>↓</kbd>move
          </span>
          <span>
            <kbd>Enter</kbd>open
          </span>
          <span>
            <kbd>{modKey}K</kbd>close
          </span>
          <span class="grow" />
          <span>Start with &gt; for commands, @ for tasks, # for files</span>
        </div>
      </section>
    </Show>
  );
}
