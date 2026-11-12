import {
  Annotation,
  Facet,
  RangeSetBuilder,
  StateEffect,
  StateField,
  type EditorState,
  type Extension,
  type Text,
  type Transaction,
} from '@codemirror/state';
import {
  Decoration,
  type EditorView,
  gutter,
  GutterMarker,
  ViewPlugin,
  type DecorationSet,
  type ViewUpdate,
} from '@codemirror/view';
import type { Run } from '../core/store';

/**
 * Who wrote each line since the last review: `b` as it was, `a` the agent, `y` you.
 * One letter per line, in the order of the lines.
 */
export type Authors = readonly string[];

/** Replaces the authors, for example with what the core says after a save. */
export const setAuthors = StateEffect.define<Authors>();

/** Marks a change that did not come from the user's hands, such as a reload from disk. */
export const fromOutside = Annotation.define<boolean>();

/** The two letters shown for the agent's lines. */
export const agentShort = Facet.define<string, string>({ combine: (values) => values[0] ?? 'Ag' });

export function authorsFromRuns(lineCount: number, runs: Run[]): string[] {
  const authors = new Array<string>(lineCount).fill('b');
  for (const run of runs) {
    for (let line = run.from; line <= Math.min(run.to, lineCount); line++) {
      authors[line - 1] = run.who === 'agent' ? 'a' : 'y';
    }
  }
  return authors;
}

function lineTexts(doc: Text, from: number, to: number): string[] {
  const texts: string[] = [];
  for (let line = from; line <= to; line++) texts.push(doc.line(line).text);
  return texts;
}

/**
 * The authors after an edit by the user. Lines the edit left alone keep their
 * author; lines it wrote or rewrote become the user's. Exported for tests.
 */
export function authorsAfter(before: Authors, oldDoc: Text, newDoc: Text, tr: Pick<Transaction, 'changes'>): string[] {
  // Each changed stretch as whole lines: old lines [a1, a2] became new lines [b1, b2].
  const stretches: { a1: number; a2: number; b1: number; b2: number }[] = [];
  tr.changes.iterChangedRanges((fromA, toA, fromB, toB) => {
    const next = {
      a1: oldDoc.lineAt(fromA).number,
      a2: oldDoc.lineAt(toA).number,
      b1: newDoc.lineAt(fromB).number,
      b2: newDoc.lineAt(toB).number,
    };
    const last = stretches[stretches.length - 1];
    // Two edits on one line are one stretch.
    if (last && next.a1 <= last.a2) {
      last.a2 = next.a2;
      last.b2 = next.b2;
    } else {
      stretches.push(next);
    }
  });

  const after: string[] = [];
  let copied = 0; // how many old lines are already carried over
  for (const { a1, a2, b1, b2 } of stretches) {
    for (let line = copied; line < a1 - 1; line++) after.push(before[line] ?? 'b');

    const oldTexts = lineTexts(oldDoc, a1, a2);
    const newTexts = lineTexts(newDoc, b1, b2);
    // Lines at either end that read the same were not really touched.
    let same = 0;
    while (same < oldTexts.length && same < newTexts.length && oldTexts[same] === newTexts[same]) same++;
    let sameEnd = 0;
    while (
      sameEnd < oldTexts.length - same &&
      sameEnd < newTexts.length - same &&
      oldTexts[oldTexts.length - 1 - sameEnd] === newTexts[newTexts.length - 1 - sameEnd]
    ) {
      sameEnd++;
    }
    for (let i = 0; i < same; i++) after.push(before[a1 - 1 + i] ?? 'b');
    for (let i = same; i < newTexts.length - sameEnd; i++) after.push('y');
    for (let i = sameEnd; i > 0; i--) after.push(before[a2 - i] ?? 'b');
    copied = a2;
  }
  for (let line = copied; line < before.length; line++) after.push(before[line] ?? 'b');

  // Keep one author per line whatever happened above.
  while (after.length < newDoc.lines) after.push('b');
  after.length = newDoc.lines;
  return after;
}

export const authorsField = StateField.define<Authors>({
  create: (state) => new Array<string>(state.doc.lines).fill('b'),
  update(authors, tr) {
    for (const effect of tr.effects) {
      if (effect.is(setAuthors)) return effect.value;
    }
    if (!tr.docChanged) return authors;
    if (tr.annotation(fromOutside)) {
      // The text came from elsewhere; the authors follow in a `setAuthors` effect.
      return new Array<string>(tr.newDoc.lines).fill('b');
    }
    return authorsAfter(authors, tr.startState.doc, tr.newDoc, tr);
  },
});

/** How many of the lines are the agent's and still not reviewed. */
export function agentLineCount(state: EditorState): number {
  let count = 0;
  for (const author of state.field(authorsField)) if (author === 'a') count++;
  return count;
}

class Badge extends GutterMarker {
  constructor(
    readonly who: 'a' | 'y',
    readonly label: string,
  ) {
    super();
  }
  eq(other: Badge) {
    return other.who === this.who && other.label === this.label;
  }
  toDOM() {
    const badge = document.createElement('span');
    badge.className = this.who === 'a' ? 'cm-badge cm-badge-agent' : 'cm-badge cm-badge-you';
    badge.textContent = this.label;
    badge.setAttribute('role', 'img');
    badge.setAttribute(
      'aria-label',
      this.who === 'a' ? 'Lines the agent wrote start here' : 'Lines you wrote start here',
    );
    return badge;
  }
}

// The badges live in the gutter, which screen readers skip, so the line itself says whose it is.
const agentLine = Decoration.line({
  class: 'cm-line-agent',
  attributes: { 'aria-description': 'written by the agent, not reviewed yet' },
});
const yourLine = Decoration.line({ class: 'cm-line-you', attributes: { 'aria-description': 'written by you' } });

/** Tints the lines on screen by who wrote them. */
const tint = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;
    constructor(view: EditorView) {
      this.decorations = this.build(view);
    }
    update(update: ViewUpdate) {
      if (
        update.docChanged ||
        update.viewportChanged ||
        update.startState.field(authorsField) !== update.state.field(authorsField)
      ) {
        this.decorations = this.build(update.view);
      }
    }
    build(view: EditorView): DecorationSet {
      const authors = view.state.field(authorsField);
      const builder = new RangeSetBuilder<Decoration>();
      for (const { from, to } of view.visibleRanges) {
        let line = view.state.doc.lineAt(from);
        for (;;) {
          const author = authors[line.number - 1];
          if (author === 'a') builder.add(line.from, line.from, agentLine);
          else if (author === 'y') builder.add(line.from, line.from, yourLine);
          if (line.to >= to || line.number >= view.state.doc.lines) break;
          line = view.state.doc.line(line.number + 1);
        }
      }
      return builder.finish();
    }
  },
  { decorations: (plugin) => plugin.decorations },
);

/** The column left of the line numbers: a badge where a stretch of someone's lines begins. */
const badges = gutter({
  class: 'cm-marks',
  lineMarker(view, line) {
    const authors = view.state.field(authorsField);
    const number = view.state.doc.lineAt(line.from).number;
    const author = authors[number - 1];
    if ((author !== 'a' && author !== 'y') || authors[number - 2] === author) return null;
    return new Badge(author, author === 'a' ? view.state.facet(agentShort) : 'You');
  },
  lineMarkerChange: (update) => update.startState.field(authorsField) !== update.state.field(authorsField),
  initialSpacer: () => new Badge('y', 'You'),
});

/** Everything that shows who wrote which line. */
export function marks(short: string): Extension {
  return [agentShort.of(short), authorsField, badges, tint];
}
