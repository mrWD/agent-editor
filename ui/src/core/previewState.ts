import { createSignal } from 'solid-js';
import { createStore } from 'solid-js/store';
import { describePointed, type Drawing, type Picked, type PickedElement } from './preview';

/**
 * What the user pointed at in the Preview of a task: an element they picked
 * and marks they drew. The Preview view writes it, the conversation pane shows
 * it and sends it along with the next message, the way it does a quote from the code.
 */
export type Pointed = {
  picked: Picked | null;
  drawings: Drawing[];
  /** While the Preview shows a page: whether the frames are reloaded for it, or the page reloads itself. */
  live: 'reloaded' | 'itself' | null;
};

const empty: Pointed = { picked: null, drawings: [], live: null };

const [pointed, setPointed] = createStore<Record<string, Pointed>>({});

export function pointedOf(taskId: string): Pointed {
  return pointed[taskId] ?? empty;
}

function change(taskId: string, next: Partial<Pointed>) {
  if (!pointed[taskId]) setPointed(taskId, { ...empty, drawings: [] });
  setPointed(taskId, next);
}

export function setPicked(taskId: string, picked: Picked | null) {
  change(taskId, { picked });
}

/** Keeps a new drawing under the next free number and returns that number. */
export function addDrawing(taskId: string, width: number, d: string, element: PickedElement | null): number {
  const drawings = pointedOf(taskId).drawings;
  const n = drawings.reduce((most, drawing) => Math.max(most, drawing.n), 0) + 1;
  change(taskId, { drawings: [...drawings, { n, width, d, element, note: '' }] });
  return n;
}

export function setDrawingNote(taskId: string, n: number, note: string) {
  setPointed(taskId, 'drawings', (drawing) => drawing.n === n, 'note', note);
}

export function removeDrawing(taskId: string, n: number) {
  change(taskId, { drawings: pointedOf(taskId).drawings.filter((drawing) => drawing.n !== n) });
}

/** Forgets the pick and the drawings: they were sent, or the user left them out. */
export function clearPointed(taskId: string) {
  if (!pointed[taskId]) return;
  change(taskId, { picked: null, drawings: [] });
}

export function setLive(taskId: string, live: Pointed['live']) {
  if (pointedOf(taskId).live !== live) change(taskId, { live });
}

/** What goes in front of the next message to the agent. Empty when nothing was pointed at. */
export function pointedText(taskId: string, cwd: string): string {
  const now = pointedOf(taskId);
  return describePointed(now.picked, now.drawings, cwd);
}

/** A request from the pane to the Preview view: pick the parent of the picked element. */
const [parentAsked, setParentAsked] = createSignal<{ taskId: string; nonce: number } | null>(null);
export { parentAsked };

let nonce = 0;
export function askForParent(taskId: string) {
  setParentAsked({ taskId, nonce: ++nonce });
}
