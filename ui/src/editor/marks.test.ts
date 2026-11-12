import { EditorState } from '@codemirror/state';
import { describe, expect, it } from 'vitest';
import { languageOf } from './languages';
import { authorsField, authorsFromRuns, fromOutside, setAuthors } from './marks';

/** A document whose lines 2 and 3 are the agent's. */
function start(text = 'one\ntwo\nthree\nfour') {
  return EditorState.create({
    doc: text,
    extensions: [authorsField.init((state) => authorsFromRuns(state.doc.lines, [{ from: 2, to: 3, who: 'agent' }]))],
  });
}
const authors = (state: EditorState) => state.field(authorsField).join('');

describe('who wrote which line, as the user types', () => {
  it('starts from what the core says', () => {
    expect(authors(start())).toBe('baab');
    expect(authorsFromRuns(3, [{ from: 2, to: 9, who: 'you' }])).toEqual(['b', 'y', 'y']);
  });

  it('makes a line the user types in theirs and leaves the others alone', () => {
    const state = start();
    const line = state.doc.line(2);
    const after = state.update({ changes: { from: line.to, insert: '!' } }).state;
    expect(authors(after)).toBe('byab');
  });

  it('marks only the new line when Enter is pressed at the end of a line', () => {
    const state = start();
    const after = state.update({ changes: { from: state.doc.line(2).to, insert: '\n' } }).state;
    expect(after.doc.lines).toBe(5);
    expect(authors(after)).toBe('bayab');
  });

  it('marks nothing when a whole line is deleted', () => {
    const state = start();
    const after = state.update({ changes: { from: state.doc.line(2).from, to: state.doc.line(3).from } }).state;
    expect(after.doc.toString()).toBe('one\nthree\nfour');
    expect(authors(after)).toBe('bab');
  });

  it('handles several edits at once, also two on one line', () => {
    const state = start();
    const first = state.doc.line(1);
    const last = state.doc.line(4);
    const after = state.update({
      changes: [
        { from: first.from, insert: '>' },
        { from: first.to, insert: '<' },
        { from: last.from, insert: 'x\n' },
      ],
    }).state;
    expect(after.doc.toString()).toBe('>one<\ntwo\nthree\nx\nfour');
    expect(authors(after)).toBe('yaayb');
  });

  it('marks pasted lines as the user’s and keeps the lines around them', () => {
    const state = start();
    const after = state.update({ changes: { from: state.doc.line(3).from, insert: 'a\nb\n' } }).state;
    expect(authors(after)).toBe('bayyab');
  });

  it('does not credit the user with a reload from disk', () => {
    const state = start();
    const after = state.update({
      changes: { from: 0, to: state.doc.length, insert: 'entirely\nnew' },
      annotations: fromOutside.of(true),
      effects: setAuthors.of(['a', 'b']),
    }).state;
    expect(authors(after)).toBe('ab');
  });

  it('always has one author per line', () => {
    let state = start('a');
    state = state.update({ changes: { from: 0, to: 1, insert: '1\n2\n3' } }).state;
    expect(state.field(authorsField)).toHaveLength(state.doc.lines);
    state = state.update({ changes: { from: 0, to: state.doc.length } }).state;
    expect(state.field(authorsField)).toHaveLength(1);
  });
});

describe('the language of a file', () => {
  it('is judged by the name', () => {
    expect(languageOf('src/middleware/rateLimit.ts').name).toBe('TypeScript');
    expect(languageOf('App.tsx').name).toBe('TypeScript JSX');
    expect(languageOf('Dockerfile').name).toBe('Dockerfile');
    expect(languageOf('Dockerfile.dev').name).toBe('Dockerfile');
    expect(languageOf('.env.local').name).toBe('Env');
    expect(languageOf('README.MD').name).toBe('Markdown');
    expect(languageOf('LICENSE').name).toBe('Plain text');
    expect(languageOf('data.unknownext').name).toBe('Plain text');
  });
});
