import { describe, expect, it } from 'vitest';
import {
  gateOf,
  listed,
  noteAbout,
  notesWords,
  peopleLines,
  pictureFrom,
  requestName,
  type DetailNote,
} from './details';

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

describe('a request and its merge', () => {
  const checks = (state: 'none' | 'pending' | 'passed' | 'failed', passed = 0, failed = 0, pending = 0) => ({
    state,
    passed,
    failed,
    pending,
  });

  it('says what stands before the merge, in three lines', () => {
    const ready = gateOf({ mergeable: true, checks: checks('passed', 4), review: 'approved' }, 'GitHub');
    expect(ready.map((gate) => [gate.title, gate.state, gate.note])).toEqual([
      ['Conflicts', 'good', 'None'],
      ['Checks', 'good', '4 passed'],
      ['Review', 'good', 'Approved'],
    ]);
    const stuck = gateOf(
      { mergeable: false, checks: checks('failed', 2, 1, 3), review: 'changes_requested' },
      'GitLab',
    );
    expect(stuck.map((gate) => [gate.state, gate.note])).toEqual([
      ['bad', 'It conflicts with its target. Update the branch.'],
      ['bad', '2 passed, 1 failed, 3 running'],
      ['bad', 'Changes were asked for'],
    ]);
    const waiting = gateOf({ mergeable: null, checks: checks('none'), review: 'review_required' }, 'GitLab');
    expect(waiting.map((gate) => [gate.state, gate.note])).toEqual([
      ['wait', 'GitLab is still working it out'],
      ['good', 'This project runs no checks on requests'],
      ['wait', 'Waiting for a review'],
    ]);
  });

  it('is called the way its host calls it', () => {
    expect(requestName({ number: 128, url: 'https://github.com/x/shop/pull/128' })).toBe('PR #128');
    expect(requestName({ number: 48, url: 'https://gitlab.example.com/g/shop/-/merge_requests/48' })).toBe('MR !48');
  });
});
