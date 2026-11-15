import { createSignal } from 'solid-js';

/**
 * How large the user made the parts of the screen that can be dragged larger
 * or smaller, by name. Kept in this browser, the same for every task.
 *
 * The two panes at the sides of a task and the panels at the bottom keep
 * their sizes in files of their own (`layout`, `work`, the projects rail);
 * these are the rest: the box a message is typed in, the column of Changes,
 * the lists of the Preview, and the panels of an issue and a request.
 */
export type SizeName = 'composer' | 'changes' | 'list' | 'issue' | 'request';

export type Limits = { min: number; max: number; initial: number };

export const SIZES: Record<SizeName, Limits> = {
  /** The height of the box a message is typed in. */
  composer: { min: 44, max: 560, initial: 56 },
  /** The width of the column of Changes: what goes in, the commits, the request. */
  changes: { min: 240, max: 640, initial: 320 },
  /** The width of a list in the Preview: the operations of an API, the jobs. */
  list: { min: 180, max: 560, initial: 300 },
  /** The width of the panel of an issue, and of a request. */
  issue: { min: 320, max: 900, initial: 480 },
  request: { min: 320, max: 900, initial: 480 },
};

/**
 * The size a part gets when it is asked to be `wanted` large: within its own limits, and never
 * more than `room`, which is what the place it stands in can give. In a place too small for its
 * least size, the least size wins.
 */
export function fitSize(name: SizeName, wanted: number, room = Infinity): number {
  const { min, max, initial } = SIZES[name];
  if (!Number.isFinite(wanted)) return initial;
  const most = Math.max(min, Math.min(max, room));
  return Math.round(Math.max(min, Math.min(most, wanted)));
}

/** Reads what was kept. Anything that is not a size is passed over. */
export function readSizes(kept: string | null): Record<SizeName, number> {
  let found: Partial<Record<SizeName, unknown>> = {};
  try {
    const parsed: unknown = JSON.parse(kept ?? '{}');
    if (typeof parsed === 'object' && parsed !== null) found = parsed;
  } catch {
    // Damaged: as good as nothing kept.
  }
  const names = Object.keys(SIZES) as SizeName[];
  return Object.fromEntries(
    names.map((name) => [
      name,
      typeof found[name] === 'number' ? fitSize(name, found[name] as number) : SIZES[name].initial,
    ]),
  ) as Record<SizeName, number>;
}

const KEY = 'ae.sizes';

const [sizes, setSizes] = createSignal(
  (() => {
    try {
      return readSizes(localStorage.getItem(KEY));
    } catch {
      return readSizes(null);
    }
  })(),
);

export const sizeOf = (name: SizeName): number => sizes()[name];

export function setSize(name: SizeName, wanted: number, room?: number) {
  setSizes({ ...sizes(), [name]: fitSize(name, wanted, room) });
  try {
    localStorage.setItem(KEY, JSON.stringify(sizes()));
  } catch {
    // Private windows may refuse storage; the size still holds for this visit.
  }
}
