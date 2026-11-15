import { describe, expect, it } from 'vitest';
import { fitSize, readSizes, SIZES } from './sizes';

describe('the sizes of what can be dragged', () => {
  it('stay within their own limits', () => {
    expect(fitSize('composer', 10)).toBe(SIZES.composer.min);
    expect(fitSize('composer', 5000)).toBe(SIZES.composer.max);
    expect(fitSize('changes', 400.4)).toBe(400);
    expect(fitSize('changes', Number.NaN)).toBe(SIZES.changes.initial);
  });

  it('never take more than the place they stand in can give, but keep their least', () => {
    expect(fitSize('issue', 800, 600)).toBe(600);
    expect(fitSize('issue', 800, 100)).toBe(SIZES.issue.min);
    expect(fitSize('composer', 300, 180)).toBe(180);
  });

  it('are read from what was kept, and what is not a size is passed over', () => {
    const read = readSizes(JSON.stringify({ composer: 200, changes: 'wide', list: 5, unknown: 3 }));
    expect(read.composer).toBe(200);
    expect(read.changes).toBe(SIZES.changes.initial);
    expect(read.list).toBe(SIZES.list.min);
    expect(read.request).toBe(SIZES.request.initial);
    expect(Object.keys(read).sort()).toEqual(['changes', 'composer', 'issue', 'list', 'request']);
    expect(readSizes('not json')).toEqual(readSizes(null));
    expect(readSizes('[1,2]').composer).toBe(SIZES.composer.initial);
  });
});
