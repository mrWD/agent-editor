import { describe, expect, it } from 'vitest';
import { fitSide, MIDDLE_MIN, readSides, SIDES } from './layout';

describe('how wide a side pane may be', () => {
  it('stays between its own limits', () => {
    expect(fitSide('files', 10)).toBe(SIDES.files.min);
    expect(fitSide('files', 5000)).toBe(SIDES.files.max);
    expect(fitSide('agent', 412.6)).toBe(413);
  });

  it('leaves the middle its room', () => {
    // 1200 wide, the conversation takes 380: the files may have what is left after the middle.
    expect(fitSide('files', 560, 1200, 380)).toBe(1200 - 380 - MIDDLE_MIN);
    expect(fitSide('agent', 760, 1200, 230)).toBe(1200 - 230 - MIDDLE_MIN);
  });

  it('keeps its least width in a window too narrow for everything', () => {
    expect(fitSide('files', 300, 700, 380)).toBe(SIDES.files.min);
  });

  it('falls back to the first width when asked for something that is not a number', () => {
    expect(fitSide('files', Number.NaN)).toBe(SIDES.files.initial);
  });
});

describe('what was kept between visits', () => {
  it('is read back', () => {
    expect(readSides('{"files":300,"agent":500}')).toEqual({ files: 300, agent: 500 });
  });

  it('starts as new when nothing or something damaged was kept', () => {
    const fresh = { files: SIDES.files.initial, agent: SIDES.agent.initial };
    expect(readSides(null)).toEqual(fresh);
    expect(readSides('not json')).toEqual(fresh);
    expect(readSides('[1,2]')).toEqual(fresh);
    expect(readSides('{"files":"wide"}')).toEqual(fresh);
  });

  it('brings a kept width back inside the limits', () => {
    expect(readSides('{"files":9999,"agent":1}')).toEqual({ files: SIDES.files.max, agent: SIDES.agent.min });
  });
});
