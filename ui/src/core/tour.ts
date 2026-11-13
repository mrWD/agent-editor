/**
 * Hints on the interface itself: a ring around one thing on screen and a card
 * beside it that says what it is. This file is the part that draws nothing:
 * what a step is, and where its card goes.
 */

/** One hint of a tour. */
export type TourStep = {
  /** What the hint is about, as a selector of the element on screen. */
  target: string;
  /** The page the element is on, such as `#/issues`. The tour goes there first. */
  route?: string;
  title: string;
  text: string;
  /** More to say, as a short list under the text. */
  lines?: string[];
  /**
   * True when the thing itself may be pressed: pressing it is the way on, the
   * same as "Next". Anything else on screen cannot be pressed during a tour.
   */
  press?: boolean;
};

export type Box = { top: number; left: number; width: number; height: number };
export type Size = { width: number; height: number };
export type Placed = { top: number; left: number; side: 'below' | 'above' | 'right' | 'left' | 'middle' };

/** How far the card stands from what it points at, and from the edge of the window. */
const GAP = 12;
const EDGE = 8;

const within = (value: number, least: number, most: number) => Math.min(Math.max(value, least), Math.max(least, most));

/**
 * Where the card goes: under what it points at when there is room, else
 * above, else beside it, and in the middle of the window when nothing fits or
 * there is nothing to point at. It never leaves the window.
 */
export function place(target: Box | null, card: Size, view: Size): Placed {
  const middle: Placed = {
    top: within((view.height - card.height) / 2, EDGE, view.height - card.height - EDGE),
    left: within((view.width - card.width) / 2, EDGE, view.width - card.width - EDGE),
    side: 'middle',
  };
  if (!target) return middle;
  const across = within(target.left + target.width / 2 - card.width / 2, EDGE, view.width - card.width - EDGE);
  const down = within(target.top + target.height / 2 - card.height / 2, EDGE, view.height - card.height - EDGE);
  const bottom = target.top + target.height;
  const right = target.left + target.width;
  if (bottom + GAP + card.height + EDGE <= view.height) return { top: bottom + GAP, left: across, side: 'below' };
  if (target.top - GAP - card.height >= EDGE)
    return { top: target.top - GAP - card.height, left: across, side: 'above' };
  if (right + GAP + card.width + EDGE <= view.width) return { top: down, left: right + GAP, side: 'right' };
  if (target.left - GAP - card.width >= EDGE) return { top: down, left: target.left - GAP - card.width, side: 'left' };
  return middle;
}

/** The ring around what a hint points at: the element's own box, a little wider. */
export function ringOf(target: Box, pad = 4): Box {
  return {
    top: target.top - pad,
    left: target.left - pad,
    width: target.width + pad * 2,
    height: target.height + pad * 2,
  };
}
