import { createSignal } from 'solid-js';
import { createStore } from 'solid-js/store';
import { describePointed, type Drawing, type Picked, type PickedElement, type Shown } from './preview';

/**
 * What the user pointed at in the Preview of a task: an element they picked
 * and marks they drew. The Preview view writes it, the conversation pane shows
 * it and sends it along with the next message, the way it does a quote from the code.
 */
export type Pointed = {
  picked: Picked | null;
  drawings: Drawing[];
  /** Answers, rows and outputs the user wants the agent to see. */
  shown: Shown[];
  /** While the Preview shows a page: whether the frames are reloaded for it, or the page reloads itself. */
  live: 'reloaded' | 'itself' | null;
};

const empty: Pointed = { picked: null, drawings: [], shown: [], live: null };

const [pointed, setPointed] = createStore<Record<string, Pointed>>({});

export function pointedOf(taskId: string): Pointed {
  return pointed[taskId] ?? empty;
}

function change(taskId: string, next: Partial<Pointed>) {
  if (!pointed[taskId]) setPointed(taskId, { ...empty, drawings: [], shown: [] });
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

/** Puts a text of the Preview before the agent, to go with the next message. The same text is not put twice. */
export function addShown(taskId: string, title: string, text: string) {
  const shown = pointedOf(taskId).shown;
  if (shown.some((one) => one.title === title && one.text === text)) return;
  const n = shown.reduce((most, one) => Math.max(most, one.n), 0) + 1;
  change(taskId, { shown: [...shown, { n, title, text, note: '' }] });
}

export function setShownNote(taskId: string, n: number, note: string) {
  setPointed(taskId, 'shown', (one) => one.n === n, 'note', note);
}

export function removeShown(taskId: string, n: number) {
  change(taskId, { shown: pointedOf(taskId).shown.filter((one) => one.n !== n) });
}

/** Forgets the pick, the drawings and what was shown: they were sent, or the user left them out. */
export function clearPointed(taskId: string) {
  if (!pointed[taskId]) return;
  change(taskId, { picked: null, drawings: [], shown: [] });
}

export function setLive(taskId: string, live: Pointed['live']) {
  if (pointedOf(taskId).live !== live) change(taskId, { live });
}

/** What goes in front of the next message to the agent. Empty when nothing was pointed at. */
export function pointedText(taskId: string, cwd: string): string {
  const now = pointedOf(taskId);
  return describePointed(now.picked, now.drawings, cwd, now.shown);
}

/** A request from the pane to the Preview view: pick the parent of the picked element. */
const [parentAsked, setParentAsked] = createSignal<{ taskId: string; nonce: number } | null>(null);
export { parentAsked };

let nonce = 0;
export function askForParent(taskId: string) {
  setParentAsked({ taskId, nonce: ++nonce });
}

/**
 * What the Preview view lends to the conversation while a page is on show:
 * a way to take a picture of each frame at the moment a message is sent.
 */
export type Shooter = {
  /** Why no screenshot can be taken right now, in words for the user. `null` when one can. */
  refusal: () => string | null;
  /** What a screenshot taken now would be of, and what it is not. */
  words: () => string;
  /** Takes a picture of each frame and puts them into the unsent message. */
  take: () => Promise<void>;
};

const [shooters, setShooters] = createSignal<Record<string, Shooter>>({});

/** The Preview of a task says that screenshots can be asked of it, or, with `null`, that it is gone. */
export function setShooter(taskId: string, shooter: Shooter | null) {
  setShooters((before) => {
    const next = { ...before };
    if (shooter) next[taskId] = shooter;
    else delete next[taskId];
    return next;
  });
}

/** The way to a screenshot of the task's Preview, while that Preview is on screen and shows a page. */
export function shooterOf(taskId: string): Shooter | null {
  return shooters()[taskId] ?? null;
}

// "Send a screenshot too", per task. Off to begin with: a picture costs the agent's tokens, so it is the user's to ask for.
const [shotToo, setShotToo] = createStore<Record<string, boolean>>({});

export function sendsShot(taskId: string): boolean {
  return shotToo[taskId] === true;
}

export function setSendsShot(taskId: string, on: boolean) {
  setShotToo(taskId, on);
}
