import { autocompletion, closeBrackets, closeBracketsKeymap, completionKeymap } from '@codemirror/autocomplete';
import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands';
import { bracketMatching, indentOnInput, indentUnit } from '@codemirror/language';
import { highlightSelectionMatches, search, searchKeymap } from '@codemirror/search';
import {
  Compartment,
  EditorSelection,
  EditorState,
  Text,
  type Extension,
  type TransactionSpec,
} from '@codemirror/state';
import {
  drawSelection,
  dropCursor,
  EditorView,
  highlightActiveLine,
  highlightActiveLineGutter,
  keymap,
  lineNumbers,
} from '@codemirror/view';
import type { Run } from '../core/store';
import type { Language } from './languages';
import { agentLineCount, authorsField, authorsFromRuns, fromOutside, marks, setAuthors } from './marks';
import { graphite } from './theme';

/** What the rest of the interface wants to know about an open file as it changes. */
export type FileMeta = {
  dirty: boolean;
  line: number;
  column: number;
  /** Lines of the selection when something is selected. */
  selection: { fromLine: number; toLine: number } | null;
  /** How many lines of the agent are not reviewed yet. */
  agentLines: number;
};

export type HostOptions = {
  /** The agent's two letters, shown on its lines. */
  short: string;
  onMeta: (path: string, meta: FileMeta) => void;
  onSave: (path: string) => void;
  /** The user changed the text with their own hands. */
  onUserEdit: (path: string) => void;
  /** The user pressed the ask key with code selected. */
  onAsk: (path: string, fromLine: number, toLine: number, text: string) => void;
  /** A file is no longer held. */
  onClosed?: (path: string) => void;
};

/**
 * One editor on screen and the state of every open file behind it.
 * Switching files swaps the state, so each file keeps its own undo history,
 * caret and scroll position while it is open.
 */
export class EditorHost {
  readonly view: EditorView;
  private states = new Map<string, EditorState>();
  private saved = new Map<string, Text>();
  private scrolls = new Map<string, number>();
  private current: string | null = null;
  private language = new Compartment();
  private base: Extension;

  constructor(
    parent: HTMLElement,
    private options: HostOptions,
  ) {
    this.base = [
      marks(options.short),
      lineNumbers(),
      highlightActiveLineGutter(),
      history(),
      drawSelection(),
      dropCursor(),
      EditorState.allowMultipleSelections.of(true),
      indentOnInput(),
      bracketMatching(),
      closeBrackets(),
      autocompletion(),
      highlightActiveLine(),
      highlightSelectionMatches(),
      // Long lines wrap: the design has no horizontal scrolling.
      EditorView.lineWrapping,
      search({ top: false }),
      keymap.of([
        {
          key: 'Mod-s',
          preventDefault: true,
          run: () => {
            if (this.current) this.options.onSave(this.current);
            return true;
          },
        },
        {
          // With code selected the key asks the agent about it; without, it falls through to the search.
          key: 'Mod-k',
          run: (view) => {
            const range = view.state.selection.main;
            if (range.empty || !this.current) return false;
            const doc = view.state.doc;
            const end = doc.lineAt(range.to);
            // A selection that ends at the very start of a line does not include that line.
            const toLine =
              end.from === range.to && end.number > doc.lineAt(range.from).number ? end.number - 1 : end.number;
            this.options.onAsk(
              this.current,
              doc.lineAt(range.from).number,
              toLine,
              doc.sliceString(range.from, range.to),
            );
            return true;
          },
        },
        ...closeBracketsKeymap,
        ...defaultKeymap,
        ...searchKeymap,
        ...historyKeymap,
        ...completionKeymap,
        indentWithTab,
      ]),
      graphite,
      EditorView.contentAttributes.of({ 'aria-label': 'Code' }),
      EditorView.updateListener.of((update) => {
        if (!this.current) return;
        if (update.docChanged && update.transactions.some((tr) => tr.docChanged && !tr.annotation(fromOutside))) {
          this.options.onUserEdit(this.current);
        }
        if (
          update.docChanged ||
          update.selectionSet ||
          update.startState.field(authorsField) !== update.state.field(authorsField)
        ) {
          this.options.onMeta(this.current, this.metaOf(this.current, update.state));
        }
      }),
    ];
    this.view = new EditorView({ parent, state: this.idleState() });
  }

  /**
   * What the editor holds while no file is on screen. The completion extension
   * is part of it for one reason: after a keystroke it keeps a timer that reads
   * its own field from whatever state the editor has when the timer fires. A
   * tab closed right after typing would otherwise end in an error nobody catches.
   */
  private idleState(): EditorState {
    return EditorState.create({ doc: '', extensions: [EditorState.readOnly.of(true), graphite, autocompletion()] });
  }

  private metaOf(path: string, state: EditorState): FileMeta {
    const range = state.selection.main;
    const line = state.doc.lineAt(range.head);
    const saved = this.saved.get(path);
    return {
      dirty: saved ? !state.doc.eq(saved) : false,
      line: line.number,
      column: range.head - line.from + 1,
      selection: range.empty
        ? null
        : { fromLine: state.doc.lineAt(range.from).number, toLine: state.doc.lineAt(range.to).number },
      agentLines: agentLineCount(state),
    };
  }

  /** The state of a file, whether it is on screen or behind. */
  stateOf(path: string): EditorState | undefined {
    return path === this.current ? this.view.state : this.states.get(path);
  }

  /** Applies a change to a file wherever its state lives, and reports the result. */
  change(path: string, spec: TransactionSpec) {
    if (path === this.current) {
      this.view.dispatch(spec);
    } else {
      const state = this.states.get(path);
      if (!state) return;
      const next = state.update(spec).state;
      this.states.set(path, next);
      this.options.onMeta(path, this.metaOf(path, next));
    }
  }

  has(path: string): boolean {
    return this.states.has(path) || this.current === path;
  }

  /** Starts holding a file. Its language is fetched in the background. */
  open(path: string, text: string, runs: Run[], language: Language) {
    const usesTabs = /^\t/m.test(text);
    const state = EditorState.create({
      doc: text,
      extensions: [
        this.base,
        this.language.of([]),
        indentUnit.of(usesTabs ? '\t' : '  '),
        EditorState.tabSize.of(usesTabs ? 4 : 2),
        authorsField.init((created) => authorsFromRuns(created.doc.lines, runs)),
      ],
    });
    this.states.set(path, state);
    this.saved.set(path, state.doc);
    void language.load().then(
      (extension) => {
        if (this.has(path)) this.change(path, { effects: this.language.reconfigure(extension) });
      },
      () => {
        // Without its language a file is still editable, only not coloured.
      },
    );
  }

  /** Puts a held file on screen. */
  show(path: string) {
    if (this.current === path) return;
    if (this.current) {
      this.states.set(this.current, this.view.state);
      this.scrolls.set(this.current, this.view.scrollDOM.scrollTop);
    }
    const state = this.states.get(path);
    if (!state) return;
    this.current = path;
    this.view.setState(state);
    this.view.scrollDOM.scrollTop = this.scrolls.get(path) ?? 0;
    this.options.onMeta(path, this.metaOf(path, state));
  }

  /** Stops holding a file. */
  close(path: string) {
    if (this.current === path) {
      this.current = null;
      this.view.setState(this.idleState());
    }
    this.states.delete(path);
    this.saved.delete(path);
    this.scrolls.delete(path);
    this.options.onClosed?.(path);
  }

  /** The text of a file as it stands in the editor, and a handle to tell later if it changed. */
  snapshot(path: string): { text: string; doc: Text } | null {
    const state = this.stateOf(path);
    return state ? { text: state.doc.toString(), doc: state.doc } : null;
  }

  /** Records that `doc` is what is on disk now. `runs` are the marks the core computed for it. */
  markSaved(path: string, doc: Text, runs: Run[]) {
    const state = this.stateOf(path);
    if (!state) return;
    this.saved.set(path, doc);
    if (state.doc === doc) {
      // Nothing was typed while the save was under way, so the core's marks fit the text.
      this.change(path, { effects: setAuthors.of(authorsFromRuns(doc.lines, runs)) });
    } else {
      this.options.onMeta(path, this.metaOf(path, state));
    }
  }

  /** Replaces a file's text with what is on disk now. The caret stays where it was when it can. */
  replaceFromDisk(path: string, text: string, runs: Run[]) {
    const state = this.stateOf(path);
    if (!state) return;
    const head = Math.min(state.selection.main.head, text.length);
    const next = Text.of(text.split('\n'));
    this.saved.set(path, next);
    this.change(path, {
      changes: { from: 0, to: state.doc.length, insert: next },
      selection: EditorSelection.cursor(head),
      annotations: fromOutside.of(true),
      effects: setAuthors.of(authorsFromRuns(next.lines, runs)),
    });
    // The replacement made a new document object; remember that one as saved.
    const after = this.stateOf(path);
    if (after) {
      this.saved.set(path, after.doc);
      this.options.onMeta(path, this.metaOf(path, after));
    }
  }

  /** Sets the marks of a file to what the core says, when the text on screen is the saved text. */
  setRuns(path: string, runs: Run[]) {
    const state = this.stateOf(path);
    const saved = this.saved.get(path);
    if (!state || !saved || !state.doc.eq(saved)) return;
    this.change(path, { effects: setAuthors.of(authorsFromRuns(state.doc.lines, runs)) });
  }

  /** Stops marking the agent's lines of a file: the user has accepted them. */
  clearAgentMarks(path: string) {
    const state = this.stateOf(path);
    if (!state) return;
    const authors = state.field(authorsField).map((author) => (author === 'a' ? 'b' : author));
    this.change(path, { effects: setAuthors.of(authors) });
  }

  /** Moves the caret to a place in the file on screen and brings it into view. */
  reveal(line: number, column = 1, length = 0) {
    const doc = this.view.state.doc;
    const target = doc.line(Math.min(Math.max(1, line), doc.lines));
    const from = Math.min(target.from + Math.max(0, column - 1), target.to);
    const to = Math.min(from + length, target.to);
    this.view.dispatch({
      selection: EditorSelection.range(from, to),
      effects: EditorView.scrollIntoView(from, { y: 'center' }),
    });
    this.view.focus();
  }

  focus() {
    this.view.focus();
  }

  destroy() {
    this.view.destroy();
  }
}
