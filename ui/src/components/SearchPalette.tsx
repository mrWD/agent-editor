import { createEffect, createMemo, createSignal, For, on, onCleanup, onMount, Show, type JSX } from 'solid-js';
import { go, route, taskHref, VIEWS } from '../app/router';
import {
  agentOf,
  projectName,
  searchChats,
  searchFiles,
  searchText,
  state,
  tasksNewestFirst,
  type ChatHit,
  type FileHit,
  type Task,
  type TextHit,
} from '../core/store';
import { openInEditor, setBottomOpen } from '../core/work';
import { ago, Chip, Icon, modKey, StateGlyph, stateLabel } from './bits';

const [open, setOpen] = createSignal(false);

/** Opens the search over everything. */
export function openSearch() {
  setOpen(true);
}

type Kind = 'tasks' | 'files' | 'chats' | 'code' | 'commands';
const kindNames: Record<Kind, string> = {
  tasks: 'Tasks',
  files: 'Files',
  chats: 'Chats',
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

/** Where `needle` sits in `text`, as character positions, ignoring case. Empty when it does not. */
function positionsOf(text: string, needle: string): number[] | null {
  if (!needle) return [];
  const at = text.toLowerCase().indexOf(needle.toLowerCase());
  if (at < 0) return null;
  return Array.from({ length: needle.length }, (_, i) => at + i);
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
 * The search over everything: tasks, the files and code of the task on
 * screen, things said in any task, and commands. Opens with the search key.
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
  const [picked, setPicked] = createSignal(0);
  let input: HTMLInputElement | undefined;
  let list: HTMLDivElement | undefined;

  const here = (): Task | undefined => {
    const current = route();
    return current.page === 'task' ? state.tasks[current.id] : undefined;
  };

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

  const fileRows = createMemo<Row[]>(() => {
    const task = here();
    if (!task) return [];
    return files().map((hit) => ({
      kind: 'files' as const,
      key: `file:${hit.path}`,
      text: hit.path,
      marks: hit.positions,
      note: task.changed.some((file) => file.path === hit.path)
        ? 'changed in this task'
        : projectName(task.info.projectId),
      mono: true,
      lead: fileGlyph,
      run: finish(() => showFile(task, hit.path)),
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
    const hits = code();
    if (!task || !hits) return [];
    return hits.map((hit) => ({
      kind: 'code' as const,
      key: `code:${hit.path}:${hit.line}:${hit.column}`,
      text: hit.text,
      marks: Array.from({ length: hit.length }, (_, i) => hit.column + i),
      note: `${hit.path}:${hit.line}`,
      mono: true,
      lead: fileGlyph,
      run: finish(() => showFile(task, hit.path, hit.line, hit.column + 1, hit.length)),
    }));
  });

  const groups = createMemo(() => {
    const all: { kind: Kind; rows: Row[] }[] = [
      { kind: 'tasks', rows: tasks().slice(0, scope() === 'tasks' ? 50 : 5) },
      { kind: 'files', rows: fileRows().slice(0, scope() === 'files' ? 50 : 6) },
      { kind: 'chats', rows: chatRows().slice(0, scope() === 'chats' ? 50 : 4) },
      { kind: 'code', rows: codeRows() },
      { kind: 'commands', rows: commands().slice(0, scope() === 'commands' ? 50 : parsed().text ? 4 : 6) },
    ];
    const wanted = scope();
    return all.filter((group) => group.rows.length > 0 && (wanted === 'all' || wanted === group.kind));
  });

  /** The row that starts a search through the code. It is offered once there is something to look for. */
  const codeOffer = createMemo<Row | null>(() => {
    const task = here();
    // The row is made anew whenever the query changes, so the text it holds is the text on screen.
    // eslint-disable-next-line solid/reactivity -- see above
    const text = parsed().text;
    if (!task || text.length < 2 || code() !== null || parsed().forced) return null;
    if (scope() !== 'all' && scope() !== 'code') return null;
    return {
      kind: 'code',
      key: 'code:offer',
      text: `Look for “${text}” inside the code of this task`,
      marks: Array.from({ length: text.length }, (_, i) => 'Look for “'.length + i),
      note: 'full text',
      lead: arrow,
      run: () => {
        // A click on this row takes the keyboard away from the search field, and the row then
        // disappears: give the keyboard back, so the arrows, Enter and Esc go on working.
        input?.focus();
        setSearchingCode(true);
        void searchText(task.info.id, text, 100)
          .then((result) => {
            if (parsed().text === text) {
              setCode(result.hits);
              setFilter('code');
              setPicked(0);
            }
          })
          .catch(() => setCode([]))
          .finally(() => setSearchingCode(false));
      },
    };
  });

  const rows = createMemo(() => {
    const flat = groups().flatMap((group) => group.rows);
    const offer = codeOffer();
    return offer ? [...flat, offer] : flat;
  });
  const counts = createMemo(() => ({
    tasks: tasks().length,
    files: fileRows().length,
    chats: chatRows().length,
    code: codeRows().length,
    commands: commands().length,
  }));
  const total = () => Object.values(counts()).reduce((sum, n) => sum + n, 0);

  // Ask the core as the user types, a moment after the last key.
  createEffect(
    on(
      () => [parsed().text, open()] as const,
      ([text, isOpen]) => {
        setCode(null);
        setPicked(0);
        if (filter() === 'code') setFilter('all');
        if (!isOpen || text === '') {
          setFiles([]);
          setChats([]);
          return;
        }
        const task = here();
        const timer = setTimeout(() => {
          if (task) {
            void searchFiles(task.info.id, text, 50)
              .then((result) => setFiles(result.hits, text))
              .catch(() => setFiles([], text));
          }
          void searchChats(text, 50)
            .then((result) => setChats(result.hits, text))
            .catch(() => setChats([], text));
        }, 90);
        onCleanup(() => clearTimeout(timer));
      },
    ),
  );

  createEffect(
    on(open, (isOpen) => {
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
    ...(code() ? (['code'] as const) : []),
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
      <span class="hit-note">{row.note}</span>
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
            aria-label="Search tasks, files, chats and commands"
            placeholder={here() ? 'Search tasks, files, chats and commands' : 'Search tasks, chats and commands'}
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
          <For each={groups()}>
            {(group) => (
              <>
                <h3>{kindNames[group.kind]}</h3>
                <For each={group.rows}>{renderRow}</For>
              </>
            )}
          </For>
          {/* Keyed: the row is drawn anew for every query, so it never offers to look for an older one. */}
          <Show when={codeOffer()} keyed>
            {(offer) => renderRow(offer)}
          </Show>
          <Show when={searchingCode()}>
            <p class="palette-empty">Looking through the code</p>
          </Show>
          <Show when={rows().length === 0 && !searchingCode()}>
            <p class="palette-empty">
              {code() && filter() === 'code'
                ? 'Nothing in the code matches.'
                : parsed().text
                  ? 'Nothing matches.'
                  : 'Type to search.'}
            </p>
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
