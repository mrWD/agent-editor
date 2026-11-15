import { describe, expect, it } from 'vitest';
import { listed, noteAbout, notesWords, peopleLines, pictureFrom, type DetailNote } from './details';

const note = (kind: DetailNote['kind'], about = ''): DetailNote => ({ author: 'Ann', at: 1, body: '', kind, about });

describe('an issue or a request in full', () => {
  it('lists who is on it, a line for each kind', () => {
    expect(listed([])).toBe('');
    expect(listed(['Ann'])).toBe('Ann');
    expect(listed(['Ann', 'Bob'])).toBe('Ann and Bob');
    expect(listed(['Ann', 'Bob', 'Cy'])).toBe('Ann, Bob and Cy');
    expect(peopleLines({ author: 'Ann', assignees: ['Bob'], reviewers: ['Cy', 'Dee'] })).toEqual([
      'Opened by Ann',
      'Assigned to Bob',
      'Review asked of Cy and Dee',
    ]);
    expect(peopleLines({ author: '', assignees: [], reviewers: [] })).toEqual([]);
  });

  it('counts what was written under it', () => {
    expect(notesWords([])).toBe('Nobody has written under it');
    expect(notesWords([note('comment')])).toBe('1 comment');
    expect(notesWords([note('comment'), note('comment'), note('review', 'approved')])).toBe('2 comments and 1 review');
    expect(noteAbout(note('review', 'asked for changes'))).toBe('asked for changes');
    expect(noteAbout(note('comment', 'src/limit.ts'))).toBe('on src/limit.ts');
    expect(noteAbout(note('comment'))).toBe('');
  });

  it('shows a picture through the core, with the address written out', () => {
    expect(pictureFrom('p 1')('/uploads/abc/shot 1.png?x=1&y=2')).toBe(
      '/api/tracker/picture?project=p%201&src=%2Fuploads%2Fabc%2Fshot%201.png%3Fx%3D1%26y%3D2',
    );
  });
});
