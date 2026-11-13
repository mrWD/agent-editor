import { describe, expect, it } from 'vitest';
import { place, ringOf } from './tour';

const view = { width: 1000, height: 700 };
const card = { width: 320, height: 160 };

describe('where the card of a hint goes', () => {
  it('stands under what it points at, centred on it', () => {
    expect(place({ top: 20, left: 400, width: 100, height: 30 }, card, view)).toEqual({
      top: 62,
      left: 290,
      side: 'below',
    });
  });

  it('goes above when there is no room under', () => {
    const placed = place({ top: 640, left: 400, width: 100, height: 40 }, card, view);
    expect(placed).toEqual({ top: 468, left: 290, side: 'above' });
  });

  it('goes beside something as tall as the window', () => {
    expect(place({ top: 0, left: 0, width: 200, height: 700 }, card, view).side).toBe('right');
    expect(place({ top: 0, left: 800, width: 200, height: 700 }, card, view).side).toBe('left');
  });

  it('never leaves the window, however near the edge its target is', () => {
    const atTheEdge = place({ top: 20, left: 980, width: 16, height: 16 }, card, view);
    expect(atTheEdge.left).toBe(1000 - 320 - 8);
    expect(place({ top: 20, left: 0, width: 16, height: 16 }, card, view).left).toBe(8);
  });

  it('stands in the middle when there is nothing to point at, or nowhere to stand', () => {
    expect(place(null, card, view)).toEqual({ top: 270, left: 340, side: 'middle' });
    expect(place({ top: 0, left: 0, width: 1000, height: 700 }, card, view).side).toBe('middle');
    // A window smaller than the card: as far up and left as the edge allows, not off screen.
    expect(place(null, card, { width: 300, height: 100 })).toEqual({ top: 8, left: 8, side: 'middle' });
  });

  it('draws the ring a little wider than the thing', () => {
    expect(ringOf({ top: 10, left: 20, width: 100, height: 30 })).toEqual({ top: 6, left: 16, width: 108, height: 38 });
  });
});
