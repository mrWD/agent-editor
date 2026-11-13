import {
  autocompletion,
  completeFromList,
  insertCompletionText,
  type Completion,
  type CompletionContext,
  type CompletionResult,
  type CompletionSource,
} from '@codemirror/autocomplete';
import { indentUnit } from '@codemirror/language';
import { linter, lintGutter, setDiagnostics, type Diagnostic } from '@codemirror/lint';
import {
  Compartment,
  StateEffect,
  type EditorState,
  type Extension,
  type Text,
  type TransactionSpec,
} from '@codemirror/state';
import { EditorView, hoverTooltip, keymap, type Tooltip } from '@codemirror/view';
import { createSignal, type Accessor } from 'solid-js';
import { createStore } from 'solid-js/store';
import { toHtml } from '../components/Markdown';
import { core, type CoreEvent } from '../core/client';
import { setProblems, type Problem } from '../core/problems';
import {
  changesOf,
  completionType,
  couldMean,
  firstLocation,
  hoverText,
  offsetAt,
  pathInRoot,
  positionAt,
  severityOf,
  uriOf,
} from './lspText';

/**
 * A small client for language servers, for the editor of one task.
 *
 * The servers run in the core, one per task and language. This side keeps
 * each open file in step with its server and turns what the server knows into
 * what the editor shows: problems under the code and in the Problems tab,
 * completions, hover text, "go to definition" and formatting on save.
 *
 * It is never in the way. Nothing here is waited for while the user types,
 * and a file whose server is missing, slow or gone is edited as before.
 */

/** How long after the last key press the server is given the new text. */
const SYNC_DELAY = 150;
/** How long formatting may take before the file is saved as typed. */
const FORMAT_WITHIN = 1000;
const COMPLETE_WITHIN = 1500;
const HOVER_WITHIN = 2000;
const DEFINITION_WITHIN = 3000;
/** A longer file is edited without a server: sending it whole after every pause would cost more than it gives. */
const LONGEST_FILE = 2_000_000;
/** How long a passing remark stays in the status line. */
const REMARK_FOR = 8000;

const onMac = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform);

/** What the editor of a task lets this client reach. `EditorHost` is one. */
export type LspHost = {
  stateOf(path: string): EditorState | undefined;
  change(path: string, spec: TransactionSpec): void;
};

export type LspOptions = {
  taskId: string;
  projectId: string;
  host: () => LspHost | undefined;
  /** Shows a place in a file of the task, opening the file when it is not open. */
  open: (path: string, line: number, column: number) => void;
};

/** Where a file stands with its language server, for the few words shown about it. */
export type LspStatus = {
  state: 'running' | 'missing' | 'failed' | 'stopped';
  /** The language as the user reads it: "TypeScript and JavaScript". */
  name: string;
  /** Whether the server can format the file. */
  formats: boolean;
  /** One more sentence for whoever wants to know: where the help comes from, or how to get it. */
  hint: string;
};

/** One thing said to the user in the status line. */
export type LspNote = { text: string; command?: string };

/** What the core answers when asked for the server of a file. */
type StartAnswer =
  | { state: 'none' }
  | { state: 'missing'; language: string; name: string; command: string | null; install: string }
  | { state: 'failed'; language: string; name: string; message: string }
  | {
      state: 'running';
      language: string;
      name: string;
      languageId: string;
      run: number;
      root: string;
      capabilities: any;
      formatOnSave: boolean;
    };

/** A language server of the task as this window knows it. */
type Server = {
  language: string;
  name: string;
  /** Changes when the core starts the server again. */
  run: number;
  /** The address of the task's folder, as the server knows it. */
  root: string;
  capabilities: any;
  alive: boolean;
  /** The problems of each open file, by its path. */
  problems: Map<string, Problem[]>;
};

/** An open file of the task. */
type FileLink = {
  path: string;
  /** The language the core named for the file; `null` when it has no known language. */
  language: string | null;
  /** The server the file is open in. */
  server: Server | null;
  uri: string;
  /** The text the server was last given. */
  synced: Text | null;
  timer: ReturnType<typeof setTimeout> | undefined;
  /** Whether the editor's state of this file has a place for what a server adds. */
  armed: boolean;
};

type Waiting = {
  server: Server;
  resolve: (value: any) => void;
  reject: (reason: Error) => void;
  timer: ReturnType<typeof setTimeout>;
};

/** What was said once already in this browser, so it is not said at every visit. */
const TOLD_KEY = 'ae.lsp.told';

function alreadyTold(key: string): boolean {
  try {
    return (JSON.parse(localStorage.getItem(TOLD_KEY) ?? '[]') as string[]).includes(key);
  } catch {
    return false;
  }
}

function rememberTold(key: string) {
  try {
    const told = JSON.parse(localStorage.getItem(TOLD_KEY) ?? '[]') as string[];
    if (!told.includes(key)) localStorage.setItem(TOLD_KEY, JSON.stringify([...told, key]));
  } catch {
    // Storage may be refused; the note is then shown again next time.
  }
}

/** The documentation of a completion or a hover as an element. What a server writes is not trusted. */
function proseElement(text: string, markdown: boolean, className: string): HTMLElement {
  const element = document.createElement('div');
  element.className = className;
  if (markdown) {
    // `toHtml` takes out everything that could run.
    element.innerHTML = toHtml(text);
    element.addEventListener('click', (event) => {
      const link = (event.target as HTMLElement).closest('a');
      // A link that is not an address of its own would take the whole app somewhere else.
      if (link && !/^[a-z][a-z0-9+.-]*:/i.test(link.getAttribute('href') ?? '')) event.preventDefault();
    });
  } else {
    element.textContent = text;
    element.style.whiteSpace = 'pre-wrap';
  }
  return element;
}

/** A word no other window has. The browser's own source of chance is asked first; it is there on localhost. */
function ownTag(): string {
  try {
    const bytes = new Uint8Array(4);
    crypto.getRandomValues(bytes);
    return [...bytes].map((byte) => byte.toString(16).padStart(2, '0')).join('');
  } catch {
    // Older browsers, and pages that are not served from a safe address.
    return Date.now().toString(36).slice(-6);
  }
}

export class LspSession {
  private files = new Map<string, FileLink>();
  private servers = new Map<string, Server>();
  private waiting = new Map<string, Waiting>();
  /** The one place in a file's editor state for everything a server adds. */
  private part = new Compartment();
  /** Servers the core said have ended. An answer that names one of them comes too late. */
  private endedRuns = new Set<number>();
  /** Every window has ids of its own, so an answer meant for another window is told apart. */
  private tag = ownTag();
  private nextId = 1;
  private saidHere = new Set<string>();
  private noteTimer: ReturnType<typeof setTimeout> | undefined;
  private disposed = false;
  /** Whether the connection to the core has been made at least once. */
  private connected = core.status() === 'open';
  private stopEvents: () => void;

  private statuses;
  private setStatuses;
  /** The one thing being said in the status line, if any. */
  readonly note: Accessor<LspNote | null>;
  private setNote;
  /** Whether files of the task's project are formatted when they are saved. */
  readonly formatOnSave: Accessor<boolean>;
  private setFormat;

  constructor(private options: LspOptions) {
    const [statuses, setStatuses] = createStore<Record<string, LspStatus | undefined>>({});
    this.statuses = statuses;
    this.setStatuses = setStatuses;
    const [note, setNote] = createSignal<LspNote | null>(null);
    this.note = note;
    this.setNote = setNote;
    const [formatOnSave, setFormat] = createSignal(true);
    this.formatOnSave = formatOnSave;
    this.setFormat = setFormat;
    this.stopEvents = core.onEvent((event) => this.onEvent(event));
  }

  /** Where a file stands with its server. Reactive. */
  statusOf(path: string): LspStatus | undefined {
    return this.statuses[path];
  }

  hideNote() {
    clearTimeout(this.noteTimer);
    this.setNote(null);
  }

  /** Turns formatting on save on or off for the task's project, in every window. */
  async setFormatOnSave(on: boolean): Promise<void> {
    this.setFormat(on);
    try {
      await core.request('lsp.setFormatOnSave', { projectId: this.options.projectId, on });
    } catch {
      // The core did not take it: show what is still true.
      this.setFormat(!on);
    }
  }

  // ----- the life of a file -----

  /** The editor started holding a file. */
  opened(path: string) {
    const file: FileLink = {
      path,
      language: null,
      server: null,
      uri: '',
      synced: null,
      timer: undefined,
      armed: false,
    };
    this.files.set(path, file);
    void this.attach(file);
  }

  /** The text of a file may have changed. Called often; the server hears of it after a pause. */
  changed(path: string) {
    const file = this.files.get(path);
    if (!file?.server?.alive) return;
    if (this.options.host()?.stateOf(path)?.doc === file.synced) return;
    clearTimeout(file.timer);
    file.timer = setTimeout(() => this.flush(file), SYNC_DELAY);
  }

  /** The file was written to disk. */
  saved(path: string) {
    const file = this.files.get(path);
    if (!file?.server?.alive) return;
    this.flush(file);
    this.send(file.server, { method: 'textDocument/didSave', params: { textDocument: { uri: file.uri } } });
  }

  /** The editor stopped holding a file. */
  closed(path: string) {
    const file = this.files.get(path);
    if (!file) return;
    this.files.delete(path);
    clearTimeout(file.timer);
    this.setStatuses(path, undefined);
    const server = file.server;
    file.server = null;
    if (!server?.alive) return;
    this.send(server, { method: 'textDocument/didClose', params: { textDocument: { uri: file.uri } } });
    // The problems of a file that is not open are nobody's business any more.
    server.problems.delete(path);
    this.publish(server);
  }

  /** The task's editor is going away: every file is closed in its server. */
  dispose() {
    for (const path of [...this.files.keys()]) this.closed(path);
    this.disposed = true;
    this.stopEvents();
    clearTimeout(this.noteTimer);
    for (const waiter of this.waiting.values()) clearTimeout(waiter.timer);
    this.waiting.clear();
    for (const server of this.servers.values()) setProblems(this.options.taskId, `lsp:${server.language}`, []);
    this.servers.clear();
  }

  /**
   * Lets the server tidy a file before it is saved. Never fails and never
   * takes long: when the server cannot do it in about a second, or the text
   * changed meanwhile, the file is left as typed.
   */
  async format(path: string): Promise<void> {
    const file = this.files.get(path);
    const server = file?.server;
    const before = this.options.host()?.stateOf(path);
    if (!file || !server?.alive || !before || !this.formatOnSave()) return;
    if (!server.capabilities?.documentFormattingProvider) return;
    this.flush(file);
    let edits: unknown;
    try {
      edits = await this.request(
        server,
        'textDocument/formatting',
        {
          textDocument: { uri: file.uri },
          options: { tabSize: before.tabSize, insertSpaces: !before.facet(indentUnit).includes('\t') },
        },
        FORMAT_WITHIN,
      );
    } catch {
      return;
    }
    const host = this.options.host();
    const now = host?.stateOf(path);
    // The edits were made for the text as it was when the server was asked.
    if (!host || !now || now.doc !== before.doc || this.files.get(path) !== file) return;
    const changes = changesOf(now.doc, edits);
    if (changes.length > 0) host.change(path, { changes, userEvent: 'format' });
  }

  // ----- servers -----

  private async attach(file: FileLink) {
    const { taskId } = this.options;
    const state = this.options.host()?.stateOf(file.path);
    if (!state || state.doc.length > LONGEST_FILE) return;
    let answer: StartAnswer;
    try {
      answer = await core.request<StartAnswer>('lsp.start', { taskId, path: file.path });
    } catch {
      // Without a server the file is edited as it always was.
      return;
    }
    if (this.disposed || this.files.get(file.path) !== file || answer.state === 'none') return;
    file.language = answer.language;

    if (answer.state === 'missing') {
      this.setStatuses(file.path, {
        state: 'missing',
        name: answer.name,
        formats: false,
        hint: `Errors and completions for ${answer.name} need ${answer.command ?? 'a language server'}. To install it: ${answer.install}`,
      });
      this.sayOnce(`missing:${answer.language}`, true, {
        text: `Errors and completions for ${answer.name} are off: ${answer.command ?? 'its language server'} is not installed. To install it:`,
        command: answer.install,
      });
      return;
    }
    if (answer.state === 'failed') {
      this.setStatuses(file.path, { state: 'failed', name: answer.name, formats: false, hint: answer.message });
      this.sayOnce(`failed:${answer.language}`, false, {
        text: `The ${answer.name} language server did not start, so errors and completions are off. ${answer.message}`,
      });
      return;
    }

    this.setFormat(answer.formatOnSave !== false);
    let server = this.servers.get(answer.language);
    if (!server || server.run !== answer.run) {
      if (this.endedRuns.has(answer.run)) return;
      // The core started the server anew without this window hearing that the old one ended.
      if (server) this.ended(server);
      server = {
        language: answer.language,
        name: answer.name,
        run: answer.run,
        root: answer.root,
        capabilities: answer.capabilities ?? {},
        alive: true,
        problems: new Map(),
      };
      this.servers.set(answer.language, server);
      // A server that was started again knows none of the files: the other open ones of its language are given to it too.
      for (const other of this.files.values()) {
        if (other !== file && other.language === answer.language) void this.attach(other);
      }
    }
    if (server.alive) this.link(file, server, answer.languageId);
  }

  /** Opens a file in a server and gives the file's editor what the server can do. */
  private link(file: FileLink, server: Server, languageId: string) {
    if (file.server === server) return;
    const host = this.options.host();
    const state = host?.stateOf(file.path);
    if (!host || !state) return;
    file.server = server;
    file.uri = uriOf(server.root, file.path);
    file.synced = state.doc;
    this.send(server, {
      method: 'textDocument/didOpen',
      params: { textDocument: { uri: file.uri, languageId, version: 1, text: state.doc.toString() } },
    });
    const added = this.extensionsFor(file, server);
    host.change(file.path, {
      effects: file.armed ? this.part.reconfigure(added) : StateEffect.appendConfig.of(this.part.of(added)),
    });
    file.armed = true;
    this.setStatuses(file.path, {
      state: 'running',
      name: server.name,
      formats: Boolean(server.capabilities?.documentFormattingProvider),
      hint: `Errors and completions come from the ${server.name} language server.`,
    });
  }

  /** A server is gone: its files are edited as files without a server. */
  private ended(server: Server) {
    if (!server.alive) return;
    server.alive = false;
    const host = this.options.host();
    for (const file of this.files.values()) {
      if (file.server !== server) continue;
      clearTimeout(file.timer);
      file.server = null;
      file.synced = null;
      // Taking the server's part out of the editor takes its marks and popups with it.
      if (file.armed && host?.stateOf(file.path)) host.change(file.path, { effects: this.part.reconfigure([]) });
      this.setStatuses(file.path, {
        state: 'stopped',
        name: server.name,
        formats: false,
        hint: `The ${server.name} language server stopped. It starts again with the next file you open.`,
      });
    }
    for (const [id, waiter] of this.waiting) {
      if (waiter.server !== server) continue;
      clearTimeout(waiter.timer);
      this.waiting.delete(id);
      waiter.reject(new Error('The language server stopped'));
    }
    server.problems.clear();
    this.publish(server);
  }

  private onEvent(event: CoreEvent) {
    const { taskId, projectId } = this.options;
    switch (event.event) {
      case 'lsp.message': {
        if (event.data.taskId !== taskId) return;
        const server = this.servers.get(event.data.language);
        if (server?.alive) this.took(server, event.data.message);
        return;
      }
      case 'lsp.stopped': {
        if (event.data.taskId !== taskId) return;
        this.endedRuns.add(Number(event.data.run));
        const server = this.servers.get(event.data.language);
        if (!server || server.run !== event.data.run || !server.alive) return;
        const held = [...this.files.values()].some((file) => file.server === server);
        this.ended(server);
        if (event.data.reason === 'crashed' && held) {
          this.sayOnce(`stopped:${server.language}:${server.run}`, false, {
            text: `The ${server.name} language server stopped. It starts again with the next file you open.`,
          });
        }
        return;
      }
      case 'lsp.settings':
        if (event.data.projectId === projectId) this.setFormat(event.data.formatOnSave !== false);
        return;
      case 'link.open':
        // The first time is the connection being made. Later it was lost and found again:
        // the core closed this window's files, or is a new core with no servers at all.
        if (this.connected) this.startOver();
        this.connected = true;
        return;
    }
  }

  private startOver() {
    for (const server of this.servers.values()) this.ended(server);
    this.servers.clear();
    for (const file of this.files.values()) void this.attach(file);
  }

  /** One message from a server: an answer to a request of this window, or news about a file. */
  private took(server: Server, message: any) {
    if (typeof message?.method === 'string') {
      if (message.method === 'textDocument/publishDiagnostics') this.gotProblems(server, message.params);
      return;
    }
    const waiter = this.waiting.get(String(message?.id));
    if (!waiter || waiter.server !== server) return;
    this.waiting.delete(String(message.id));
    clearTimeout(waiter.timer);
    if (message.error) waiter.reject(new Error(String(message.error.message ?? 'The language server refused')));
    else waiter.resolve(message.result ?? null);
  }

  private send(server: Server, message: Record<string, unknown>) {
    const { taskId } = this.options;
    core.request('lsp.send', { taskId, language: server.language, message }).catch(() => {
      // The core has no such server any more, or cannot be reached.
      this.ended(server);
    });
  }

  /** Asks a server something and waits for its answer, but not for long. */
  private request(server: Server, method: string, params: unknown, within: number): Promise<any> {
    const id = `${this.tag}:${this.nextId++}`;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.waiting.delete(id);
        // The server may stop working on what nobody waits for.
        if (server.alive) this.send(server, { method: '$/cancelRequest', params: { id } });
        reject(new Error('The language server took too long'));
      }, within);
      this.waiting.set(id, { server, resolve, reject, timer });
      this.send(server, { id, method, params });
    });
  }

  /** Gives the server the text of a file as it is now, if it does not have it yet. */
  private flush(file: FileLink) {
    clearTimeout(file.timer);
    const server = file.server;
    const doc = this.options.host()?.stateOf(file.path)?.doc;
    if (!server?.alive || !doc || doc === file.synced) return;
    file.synced = doc;
    this.send(server, {
      method: 'textDocument/didChange',
      // The whole text every time: simple, and right whatever happened to the text in between.
      params: { textDocument: { uri: file.uri, version: 0 }, contentChanges: [{ text: doc.toString() }] },
    });
  }

  // ----- problems -----

  private gotProblems(server: Server, params: any) {
    const path = pathInRoot(server.root, String(params?.uri ?? ''));
    const file = path === null ? undefined : this.files.get(path);
    const host = this.options.host();
    const state = file ? host?.stateOf(file.path) : undefined;
    // Servers also speak of files nobody has open here; those are not shown.
    if (!file || file.server !== server || !host || !state) return;
    const found: any[] = Array.isArray(params.diagnostics) ? params.diagnostics : [];
    const doc = state.doc;
    const marks: Diagnostic[] = [];
    const problems: Problem[] = [];
    for (const one of found) {
      const from = offsetAt(doc, one?.range?.start);
      const to = Math.max(from, offsetAt(doc, one?.range?.end));
      const severity = severityOf(one?.severity);
      const message = String(one?.message ?? '');
      const source = typeof one?.source === 'string' && one.source ? one.source : server.name;
      marks.push({ from, to, severity, message, source });
      // A hint is a suggestion, not a problem: it shows in the code and stays out of the list.
      if (severity === 'hint') continue;
      const line = doc.lineAt(from);
      problems.push({
        path: file.path,
        line: line.number,
        column: from - line.from + 1,
        severity,
        message,
        from: source,
      });
    }
    host.change(file.path, setDiagnostics(state, marks));
    server.problems.set(file.path, problems);
    this.publish(server);
  }

  /** Says what this server knows of the task's problems now. */
  private publish(server: Server) {
    if (this.disposed) return;
    setProblems(this.options.taskId, `lsp:${server.language}`, [...server.problems.values()].flat());
  }

  // ----- what a server adds to the editor of a file -----

  private extensionsFor(file: FileLink, server: Server): Extension {
    const can = server.capabilities ?? {};
    const added: Extension[] = [
      linter(null),
      lintGutter({ markerFilter: (found) => found.filter((one) => one.severity !== 'hint') }),
    ];
    if (can.completionProvider) {
      // The server's completions come first; what the editor offered before is what is left when it has none.
      added.push(autocompletion({ override: [(context) => this.complete(file, server, context)] }));
    }
    if (can.hoverProvider) {
      added.push(
        hoverTooltip((view, pos) => this.hover(file, server, view, pos), { hoverTime: 350, hideOnChange: true }),
      );
    }
    if (can.definitionProvider) {
      added.push(
        keymap.of([
          {
            key: 'F12',
            run: (view) => {
              void this.goToDefinition(file, server, view.state.selection.main.head);
              return true;
            },
          },
        ]),
        EditorView.domEventHandlers({
          mousedown: (event, view) => {
            if (event.button !== 0 || !(onMac ? event.metaKey : event.ctrlKey)) return false;
            const pos = view.posAtCoords({ x: event.clientX, y: event.clientY });
            if (pos === null) return false;
            event.preventDefault();
            void this.goToDefinition(file, server, pos);
            return true;
          },
        }),
      );
    }
    return added;
  }

  private async complete(file: FileLink, server: Server, context: CompletionContext): Promise<CompletionResult | null> {
    const fromServer = await this.completeFromServer(file, server, context).catch(() => null);
    if (context.aborted) return null;
    return fromServer ?? completeAsBefore(context);
  }

  private async completeFromServer(
    file: FileLink,
    server: Server,
    context: CompletionContext,
  ): Promise<CompletionResult | null> {
    if (!server.alive || file.server !== server) return null;
    const { state, pos } = context;
    const word = context.matchBefore(/[\w$]+/);
    const typed = pos > 0 ? state.sliceDoc(pos - 1, pos) : '';
    const triggers: string[] = server.capabilities?.completionProvider?.triggerCharacters ?? [];
    const triggered = triggers.includes(typed);
    if (!context.explicit && !word && !triggered) return null;

    this.flush(file);
    const answer = await this.request(
      server,
      'textDocument/completion',
      {
        textDocument: { uri: file.uri },
        position: positionAt(state.doc, pos),
        context: triggered && !word ? { triggerKind: 2, triggerCharacter: typed } : { triggerKind: 1 },
      },
      COMPLETE_WITHIN,
    );
    const items: any[] = Array.isArray(answer) ? answer : Array.isArray(answer?.items) ? answer.items : [];
    if (items.length === 0) return null;

    const from = word ? word.from : pos;
    const prefix = state.sliceDoc(from, pos);
    const options: Completion[] = [];
    for (const item of items) {
      const label = String(item?.label ?? '');
      if (label === '') continue;
      const filter = typeof item.filterText === 'string' && item.filterText ? item.filterText : label;
      if (!couldMean(filter, prefix)) continue;
      const edit = item.textEdit;
      const text = String(edit?.newText ?? item.insertText ?? label);
      const editRange = edit?.range ?? edit?.replace;
      const editFrom = editRange ? offsetAt(state.doc, editRange.start) : from;
      const option: Completion = {
        label: filter,
        displayLabel: filter === label ? undefined : label,
        detail: item.labelDetails?.detail ?? (typeof item.detail === 'string' ? item.detail : undefined),
        type: completionType(item.kind),
        sortText: typeof item.sortText === 'string' ? item.sortText : undefined,
        boost: item.preselect ? 10 : undefined,
        // An edit that starts somewhere else than the typed word replaces from its own start.
        apply:
          editFrom === from
            ? text
            : (view, _completion, _from, to) => view.dispatch(insertCompletionText(view.state, text, editFrom, to)),
      };
      const about = hoverText(item.documentation);
      if (about) option.info = () => proseElement(about.text, about.markdown, 'cm-lsp-info md');
      options.push(option);
    }
    // Nothing of the server's fits what was typed: the editor's own completions get their turn.
    if (options.length === 0) return null;
    return { from, options, validFor: answer?.isIncomplete ? undefined : /^[\w$]*$/ };
  }

  private async hover(file: FileLink, server: Server, view: EditorView, pos: number): Promise<Tooltip | null> {
    if (!server.alive || file.server !== server) return null;
    const doc = view.state.doc;
    this.flush(file);
    let answer: any;
    try {
      answer = await this.request(
        server,
        'textDocument/hover',
        { textDocument: { uri: file.uri }, position: positionAt(doc, pos) },
        HOVER_WITHIN,
      );
    } catch {
      return null;
    }
    const about = hoverText(answer?.contents);
    // An answer for a text that has changed since would point at the wrong place.
    if (!about || view.state.doc !== doc) return null;
    const from = answer.range ? offsetAt(doc, answer.range.start) : pos;
    const to = answer.range ? Math.max(from, offsetAt(doc, answer.range.end)) : pos;
    return {
      pos: from,
      end: to,
      above: true,
      create: () => ({ dom: proseElement(about.text, about.markdown, 'cm-lsp-hover md') }),
    };
  }

  private async goToDefinition(file: FileLink, server: Server, pos: number): Promise<void> {
    const state = this.options.host()?.stateOf(file.path);
    if (!state || !server.alive || file.server !== server) return;
    this.flush(file);
    let answer: unknown;
    try {
      answer = await this.request(
        server,
        'textDocument/definition',
        { textDocument: { uri: file.uri }, position: positionAt(state.doc, pos) },
        DEFINITION_WITHIN,
      );
    } catch {
      return;
    }
    const place = firstLocation(answer);
    if (!place || this.disposed) return;
    const path = pathInRoot(server.root, place.uri);
    if (path === null) {
      this.say({ text: 'That is defined outside this task’s folder, so it is not opened here.' }, REMARK_FOR);
      return;
    }
    this.options.open(path, place.line + 1, place.character + 1);
  }

  // ----- talking to the user -----

  private say(note: LspNote, forMs?: number) {
    clearTimeout(this.noteTimer);
    this.setNote(note);
    if (forMs) this.noteTimer = setTimeout(() => this.setNote(null), forMs);
  }

  /** Says something once. `everywhere` keeps it to once in this browser, not just in this visit. */
  private sayOnce(key: string, everywhere: boolean, note: LspNote) {
    if (this.saidHere.has(key) || (everywhere && alreadyTold(key))) return;
    this.saidHere.add(key);
    if (everywhere) rememberTold(key);
    this.say(note);
  }
}

/**
 * The completions the editor offered before a server was there: what the
 * file's language pack knows, such as the words of the file itself. They are
 * asked when the server has nothing for what was typed.
 */
async function completeAsBefore(context: CompletionContext): Promise<CompletionResult | null> {
  const known = context.state.languageDataAt<CompletionSource | readonly (string | Completion)[]>(
    'autocomplete',
    context.pos,
  );
  const sources = known.map((one) => (typeof one === 'function' ? one : completeFromList(one)));
  const results = (
    await Promise.all(sources.map((source) => Promise.resolve(source(context)).catch(() => null)))
  ).filter((result) => result !== null);
  const first = results[0];
  if (!first || results.length === 1) return first ?? null;
  // Several lists that start at the same place are shown as one, as the editor does by itself.
  const together = results.filter((result) => result.from === first.from);
  return { from: first.from, options: together.flatMap((result) => [...result.options]) };
}
