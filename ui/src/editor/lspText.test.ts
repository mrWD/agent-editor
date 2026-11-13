import { Text } from '@codemirror/state';
import { describe, expect, it } from 'vitest';
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

const doc = Text.of(['const a = 1;', '', '  return a;']);

/** The text after the changes, applied the way the editor applies them: all against the old text. */
function apply(text: Text, changes: { from: number; to: number; insert: string }[]): string {
  let result = text.toString();
  for (const change of [...changes].sort((a, b) => b.from - a.from)) {
    result = result.slice(0, change.from) + change.insert + result.slice(change.to);
  }
  return result;
}

describe('places in a file', () => {
  it('turns an offset into a line and a character, and back', () => {
    expect(positionAt(doc, 0)).toEqual({ line: 0, character: 0 });
    expect(positionAt(doc, 6)).toEqual({ line: 0, character: 6 });
    expect(positionAt(doc, 13)).toEqual({ line: 1, character: 0 });
    expect(positionAt(doc, doc.length)).toEqual({ line: 2, character: 11 });
    for (const offset of [0, 6, 12, 13, 14, 20, doc.length]) {
      expect(offsetAt(doc, positionAt(doc, offset))).toBe(offset);
    }
  });

  it('pulls a place of an older, longer text back into the file', () => {
    expect(offsetAt(doc, { line: 0, character: 99 })).toBe(12);
    expect(offsetAt(doc, { line: 99, character: 0 })).toBe(doc.length);
    expect(offsetAt(doc, { line: -1, character: -5 })).toBe(0);
    expect(offsetAt(doc, undefined)).toBe(0);
  });
});

describe('addresses of files', () => {
  const root = 'file:///work/my%20shop';

  it('names a file of the task by the address of its folder', () => {
    expect(uriOf(root, 'src/app.ts')).toBe('file:///work/my%20shop/src/app.ts');
    expect(uriOf(`${root}/`, 'src/a b/[id].ts')).toBe('file:///work/my%20shop/src/a%20b/%5Bid%5D.ts');
  });

  it('reads an address back as a path from the folder, however the server spells it', () => {
    expect(pathInRoot(root, 'file:///work/my%20shop/src/app.ts')).toBe('src/app.ts');
    expect(pathInRoot(root, uriOf(root, 'src/a b/[id].ts'))).toBe('src/a b/[id].ts');
    // The same place with other escapes, and a Windows drive in either case.
    expect(pathInRoot('file:///work/(shop)', 'file:///work/%28shop%29/a.ts')).toBe('a.ts');
    expect(pathInRoot('file:///C:/work', 'file:///c%3A/work/src/a.ts')).toBe('src/a.ts');
  });

  it('says no to every place outside the folder', () => {
    expect(pathInRoot(root, 'file:///work/other/app.ts')).toBeNull();
    expect(pathInRoot(root, 'file:///work/my%20shop-two/app.ts')).toBeNull();
    expect(pathInRoot(root, 'file:///work/my%20shop/../secret.txt')).toBeNull();
    expect(pathInRoot(root, 'file:///work/my%20shop')).toBeNull();
    expect(pathInRoot(root, 'https://example.com/work/my%20shop/a.ts')).toBeNull();
    expect(pathInRoot(root, 'untitled:1')).toBeNull();
  });
});

describe('what a server says', () => {
  it('reads how bad a problem is', () => {
    expect([1, 2, 3, 4, undefined].map(severityOf)).toEqual(['error', 'warning', 'info', 'hint', 'error']);
  });

  it('reads the kind of a completion', () => {
    expect([3, 6, 7, 14, 99, undefined].map(completionType)).toEqual([
      'function',
      'variable',
      'class',
      'keyword',
      undefined,
      undefined,
    ]);
  });

  it('reads hover text in each of the shapes servers send', () => {
    expect(hoverText({ kind: 'markdown', value: '**limits**' })).toEqual({ text: '**limits**', markdown: true });
    expect(hoverText({ kind: 'plaintext', value: 'a < b' })).toEqual({ text: 'a < b', markdown: false });
    expect(hoverText('plain words')).toEqual({ text: 'plain words', markdown: true });
    expect(hoverText([{ language: 'ts', value: 'const a: number' }, 'The first one.'])).toEqual({
      text: '```ts\nconst a: number\n```\n\nThe first one.',
      markdown: true,
    });
    expect(hoverText({ kind: 'markdown', value: '  ' })).toBeNull();
    expect(hoverText(null)).toBeNull();
    expect(hoverText([])).toBeNull();
  });

  it('finds where a definition is in each of the shapes servers send', () => {
    const range = { start: { line: 4, character: 13 }, end: { line: 4, character: 19 } };
    expect(firstLocation({ uri: 'file:///a.ts', range })).toEqual({ uri: 'file:///a.ts', line: 4, character: 13 });
    expect(
      firstLocation([
        { uri: 'file:///b.ts', range },
        { uri: 'file:///c.ts', range },
      ])?.uri,
    ).toBe('file:///b.ts');
    expect(
      firstLocation([
        { targetUri: 'file:///d.ts', targetRange: range, targetSelectionRange: range, originSelectionRange: range },
      ]),
    ).toEqual({ uri: 'file:///d.ts', line: 4, character: 13 });
    expect(firstLocation(null)).toBeNull();
    expect(firstLocation([])).toBeNull();
    expect(firstLocation({ range })).toBeNull();
  });
});

describe('edits of a formatter', () => {
  it('become changes to the text they were made for', () => {
    const text = Text.of(['a  ', 'b', 'c \t']);
    const edits = [
      { range: { start: { line: 0, character: 1 }, end: { line: 0, character: 3 } }, newText: '' },
      { range: { start: { line: 2, character: 1 }, end: { line: 2, character: 3 } }, newText: '' },
    ];
    const changes = changesOf(text, edits);
    expect(changes).toEqual([
      { from: 1, to: 3, insert: '' },
      { from: 7, to: 9, insert: '' },
    ]);
    expect(apply(text, changes)).toBe('a\nb\nc');
  });

  it('cut a whole file sent back down to what really changed', () => {
    const text = Text.of(['const a = 1;', 'const  b = 2;', 'const c = 3;']);
    const whole = { start: { line: 0, character: 0 }, end: { line: 99, character: 0 } };
    const changes = changesOf(text, [{ range: whole, newText: 'const a = 1;\r\nconst b = 2;\r\nconst c = 3;' }]);
    // One space goes; the caret, wherever it is outside that space, stays where it was.
    expect(changes).toHaveLength(1);
    expect(changes[0]!.to - changes[0]!.from).toBe(1);
    expect(changes[0]!.insert).toBe('');
    expect(apply(text, changes)).toBe('const a = 1;\nconst b = 2;\nconst c = 3;');
  });

  it('leave out edits that change nothing, and answers that are not edits', () => {
    const text = Text.of(['same']);
    const all = { start: { line: 0, character: 0 }, end: { line: 0, character: 4 } };
    expect(changesOf(text, [{ range: all, newText: 'same' }])).toEqual([]);
    expect(changesOf(text, null)).toEqual([]);
    expect(changesOf(text, [{ newText: 'x' }, { range: all }, null])).toEqual([]);
  });
});

describe('what was typed against a completion', () => {
  it('matches letters in order, in either case', () => {
    expect(couldMean('practiceAlpha', 'prac')).toBe(true);
    expect(couldMean('practiceAlpha', 'pA')).toBe(true);
    expect(couldMean('practiceAlpha', '')).toBe(true);
    expect(couldMean('practiceAlpha', 'ord')).toBe(false);
    expect(couldMean('abc', 'cba')).toBe(false);
  });
});
