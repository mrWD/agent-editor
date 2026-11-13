import { describe, expect, it } from 'vitest';
import { layoutFlow } from './flow';
import {
  countsLine,
  describeBlock,
  hasStateLine,
  layoutPlan,
  movedBy,
  stateWord,
  type Block,
  type PlanArrow,
} from './plan';

const block = (id: string, text: string, more: Partial<Block> = {}): Block => ({
  id,
  kind: 'step',
  text,
  details: '',
  state: 'planned',
  by: null,
  byName: null,
  at: null,
  files: [],
  from: null,
  ...more,
});
const arrow = (from: string, to: string, label: string | null = null): PlanArrow => ({ from, to, label });

/** The plan of the design: a request, a question, and what follows each answer. */
const blocks = [
  block('b1', 'Request to POST /login', { state: 'before' }),
  block('b2', 'Over the limit?', { kind: 'ask', state: 'built' }),
  block('b3', 'Respond 429', { state: 'built' }),
  block('b4', 'Check the password', { state: 'before' }),
];
const arrows = [arrow('b1', 'b2'), arrow('b2', 'b3', 'yes'), arrow('b2', 'b4', 'no')];

describe('the words of a plan', () => {
  it('counts what is built, and leaves out what there is none of', () => {
    expect(countsLine({ built: 2, total: 5, building: 1, planned: 1, proposed: 1 })).toBe(
      '2 of 5 built · 1 being built · 1 planned · 1 proposed',
    );
    expect(countsLine({ built: 3, total: 3, building: 0, planned: 0, proposed: 0 })).toBe('3 of 3 built');
    expect(countsLine({ built: 0, total: 2, building: 0, planned: 2, proposed: 0 })).toBe('0 of 2 built · 2 planned');
    expect(countsLine({ built: 0, total: 0, building: 0, planned: 0, proposed: 0 })).toBe('Nothing to build yet');
  });

  it('names a block for someone who cannot see the chart: what it is, how it stands, where it leads', () => {
    const plan = { blocks, arrows };
    expect(describeBlock(blocks[1], plan)).toBe(
      'Question: Over the limit? Built. Then yes: Respond 429; no: Check the password.',
    );
    expect(describeBlock(blocks[0], plan)).toBe(
      'Step: Request to POST /login. Was in the code before. Then Over the limit?.',
    );
    expect(describeBlock(blocks[2], plan)).toBe('Step: Respond 429. Built.');
    expect(describeBlock(block('n', 'Never hardcode the limit', { kind: 'note' }), plan)).toBe(
      'Note: Never hardcode the limit.',
    );
    expect(stateWord('yours')).toBe('Planned, yours');
    expect(stateWord('proposed')).toBe('Proposed');
  });
});

describe('laying a plan out', () => {
  it('lays blocks that hang together out as the Map view lays out a flow', () => {
    const layout = layoutPlan({ blocks, arrows });
    const flow = layoutFlow({
      nodes: blocks.map((one) => ({
        id: one.id,
        kind: one.kind === 'ask' ? 'ask' : 'step',
        text: one.text,
        changed: false,
        path: null,
        line: null,
      })),
      arrows,
    });
    expect(layout.boxes).toEqual(flow.boxes.map(({ id, x, y, w, h }) => ({ id, x, y, w, h })));
    expect(layout.lines.map((line) => line.d)).toEqual(flow.lines.map((line) => line.d));
    // Each line knows its arrow, so an arrow to a proposed block can be drawn apart.
    expect(layout.lines.map((line) => [line.from, line.to, line.label])).toEqual([
      ['b1', 'b2', null],
      ['b2', 'b3', 'yes'],
      ['b2', 'b4', 'no'],
    ]);
    expect(layout.width).toBe(flow.width);
    // The two ends of the flow get a button to go on from, and the chart has room for the lower one.
    expect(layout.plus.map((plus) => plus.after)).toEqual(['b3', 'b4']);
    const last = layout.boxes.find((box) => box.id === 'b4')!;
    expect(layout.plus[1]).toEqual({ after: 'b4', x: last.x + last.w / 2 - 12, y: last.y + last.h + 8 });
    expect(layout.height).toBe(last.y + last.h + 8 + 24 + 8);
  });

  it('puts loose blocks and notes under the flow, each in a row of its own', () => {
    const loose = block('b5', 'Log the blocked attempt');
    const note = block('b6', 'Never hardcode the limit', { kind: 'note' });
    const alone = layoutPlan({ blocks, arrows });
    const layout = layoutPlan({ blocks: [...blocks, loose, note], arrows });
    const box = (id: string) => layout.boxes.find((entry) => entry.id === id)!;
    expect(layout.boxes.slice(0, 4)).toEqual(alone.boxes);
    expect(box('b5').y).toBeGreaterThanOrEqual(alone.height);
    expect(box('b6').y).toBeGreaterThan(box('b5').y + box('b5').h);
    // Each stands in the line of the flow's first block.
    expect(box('b5').x + box('b5').w / 2).toBe(box('b1').x + box('b1').w / 2);
    expect(box('b6').x).toBe(box('b5').x);
    // A loose step can be built on; a note cannot.
    expect(layout.plus.map((plus) => plus.after)).toEqual(['b3', 'b4', 'b5']);
    // Nothing overlaps: every box lies under the one before it or beside it.
    for (const [index, one] of layout.boxes.entries()) {
      for (const other of layout.boxes.slice(index + 1)) {
        const apart =
          one.x + one.w <= other.x ||
          other.x + other.w <= one.x ||
          one.y + one.h <= other.y ||
          other.y + other.h <= one.y;
        expect(apart, `${one.id} and ${other.id}`).toBe(true);
      }
    }
  });

  it('starts a group at the block nothing leads to, wherever it stands in the list', () => {
    const turned = [block('b2', 'Second'), block('b1', 'First')];
    const layout = layoutPlan({ blocks: turned, arrows: [arrow('b1', 'b2')] });
    const box = (id: string) => layout.boxes.find((entry) => entry.id === id)!;
    expect(box('b1').y).toBeLessThan(box('b2').y);
    // An arrow to a block that is gone is not drawn, and breaks nothing.
    expect(layoutPlan({ blocks: turned, arrows: [arrow('b1', 'b2'), arrow('b2', 'b9')] }).lines).toHaveLength(1);
    expect(layoutPlan({ blocks: [], arrows: [] })).toEqual({ width: 0, height: 0, boxes: [], lines: [], plus: [] });
  });

  it('gives a block that shows a line under its name the room for it', () => {
    expect(hasStateLine(block('a', 'One'))).toBe(false);
    expect(hasStateLine(block('a', 'One', { state: 'built' }))).toBe(false);
    expect(hasStateLine(block('a', 'One', { state: 'building' }))).toBe(true);
    expect(hasStateLine(block('a', 'One', { state: 'yours' }))).toBe(true);
    expect(hasStateLine(block('a', 'One', { state: 'proposed', kind: 'ask' }))).toBe(true);
    expect(hasStateLine(block('a', 'One', { state: 'proposed', kind: 'note' }))).toBe(false);

    const heightOf = (one: Block) => layoutPlan({ blocks: [one], arrows: [] }).boxes[0];
    expect(heightOf(block('a', 'Log the blocked attempt')).h).toBe(44);
    expect(heightOf(block('a', 'Log the blocked attempt', { state: 'yours' })).h).toBe(60);
    expect(heightOf(block('a', 'Over the limit?', { kind: 'ask' }))).toMatchObject({ w: 220, h: 88 });
    expect(heightOf(block('a', 'Over the limit?', { kind: 'ask', state: 'building' }))).toMatchObject({
      w: 260,
      h: 104,
    });
    // What waits for a yes cannot be built on yet: it has no button under it.
    expect(layoutPlan({ blocks: [block('a', 'One', { state: 'proposed' })], arrows: [] }).plus).toEqual([]);
  });

  it('moves the paths of the flow layout to the right and down', () => {
    expect(movedBy('M 110 56 V 91', 0, 100)).toBe('M 110 156 V 191');
    expect(movedBy('M 220 140 H 275', 0, 100)).toBe('M 220 240 H 275');
    expect(movedBy('M 110 56 V 76 H 300 V 91', 20, 10)).toBe('M 130 66 V 86 H 320 V 101');
    expect(movedBy('M 200 34 H 416 V 180 H 205', 5, 6)).toBe('M 205 40 H 421 V 186 H 210');
  });
});
