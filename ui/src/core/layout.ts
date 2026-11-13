import { createSignal } from 'solid-js';

/**
 * How wide the user made the panes at the sides of a task: the files on the
 * left, the conversation on the right. Remembered between visits, and the same
 * for every task.
 */
export type Side = 'files' | 'agent';

export type SideLimits = { min: number; max: number; initial: number };

export const SIDES: Record<Side, SideLimits> = {
  files: { min: 160, max: 560, initial: 230 },
  agent: { min: 280, max: 760, initial: 380 },
};

/** The least room the code in the middle keeps, whatever the sides are dragged to. */
export const MIDDLE_MIN = 320;

/**
 * The width a side gets when it is asked to be `wanted` wide.
 *
 * It stays between its own limits, and it never takes the middle's last room:
 * `room` is how wide the three panes are together, `other` how wide the other
 * side is. In a window too narrow for all of that, the side's own least width wins.
 */
export function fitSide(side: Side, wanted: number, room = Infinity, other = 0): number {
  const { min, max, initial } = SIDES[side];
  if (!Number.isFinite(wanted)) return initial;
  const most = Math.max(min, Math.min(max, room - other - MIDDLE_MIN));
  return Math.round(Math.max(min, Math.min(most, wanted)));
}

/** Reads what was kept. Anything that is not a width is passed over. */
export function readSides(kept: string | null): Record<Side, number> {
  let found: Partial<Record<Side, unknown>> = {};
  try {
    const parsed: unknown = JSON.parse(kept ?? '{}');
    if (typeof parsed === 'object' && parsed !== null) found = parsed;
  } catch {
    // Damaged: as good as nothing kept.
  }
  const one = (side: Side) =>
    typeof found[side] === 'number' ? fitSide(side, found[side] as number) : SIDES[side].initial;
  return { files: one('files'), agent: one('agent') };
}

const KEY = 'ae.sides';

const stored = (() => {
  try {
    return readSides(localStorage.getItem(KEY));
  } catch {
    return readSides(null);
  }
})();

const [files, setFiles] = createSignal(stored.files);
const [agent, setAgent] = createSignal(stored.agent);

export const sideWidth = (side: Side): number => (side === 'files' ? files() : agent());

export function setSideWidth(side: Side, wanted: number, room?: number) {
  const other = side === 'files' ? agent() : files();
  (side === 'files' ? setFiles : setAgent)(fitSide(side, wanted, room, other));
  try {
    localStorage.setItem(KEY, JSON.stringify({ files: files(), agent: agent() }));
  } catch {
    // Private windows may refuse storage; the width still holds for this visit.
  }
}
