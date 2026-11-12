import { createSignal } from 'solid-js';
import { createStore } from 'solid-js/store';

/** A piece of code the user wants to ask the agent about. */
export type Quote = { path: string; fromLine: number; toLine: number; text: string };

/**
 * What the user is doing by hand in a task right now. The code view writes
 * it, the agent pane reads it, so the conversation knows about the editing.
 */
export type Work = {
  /** The file and line the caret is in, while that file has unsaved edits. */
  editing: { path: string; line: number } | null;
  /** How many open files have unsaved edits. */
  unsaved: number;
  /** The agent's turn was stopped because the user started typing in the code. */
  pausedByTyping: boolean;
  /** Code picked in the editor to send along with the next message. */
  quote: Quote | null;
};

const empty: Work = { editing: null, unsaved: 0, pausedByTyping: false, quote: null };

const [work, setWorkStore] = createStore<Record<string, Work>>({});

export function workOf(taskId: string): Work {
  return work[taskId] ?? empty;
}

export function setWork(taskId: string, change: Partial<Work>) {
  if (!work[taskId]) setWorkStore(taskId, { ...empty });
  setWorkStore(taskId, change);
}

/** Saves every unsaved file of a task. Set by the code view while it is on screen. */
const savers = new Map<string, () => Promise<void>>();

export function registerSaver(taskId: string, save: () => Promise<void>): () => void {
  savers.set(taskId, save);
  return () => {
    if (savers.get(taskId) === save) savers.delete(taskId);
  };
}

/** Saves what the user has typed, so that the agent sees it. */
export async function saveAll(taskId: string): Promise<void> {
  await savers.get(taskId)?.();
}

/** A request to show a file in the code view, from anywhere in the interface. */
export type OpenRequest = {
  taskId: string;
  path: string;
  line?: number;
  column?: number;
  length?: number;
  nonce: number;
};

const [openRequest, setOpenRequest] = createSignal<OpenRequest | null>(null);
export { openRequest };

let nonce = 0;
export function openInEditor(taskId: string, path: string, line?: number, column?: number, length?: number) {
  setOpenRequest({ taskId, path, line, column, length, nonce: ++nonce });
}

/** Whether the panel under the code (terminal and friends) is open. Remembered between visits. */
const stored = (() => {
  try {
    return JSON.parse(localStorage.getItem('ae.bottom') ?? '{}') as { open?: boolean; height?: number };
  } catch {
    return {};
  }
})();
export const [bottomOpen, setBottomOpenSignal] = createSignal(stored.open ?? false);
export const [bottomHeight, setBottomHeightSignal] = createSignal(stored.height ?? 220);

function remember() {
  try {
    localStorage.setItem('ae.bottom', JSON.stringify({ open: bottomOpen(), height: bottomHeight() }));
  } catch {
    // Private windows may refuse storage; the panel still works for this visit.
  }
}
export function setBottomOpen(open: boolean) {
  setBottomOpenSignal(open);
  remember();
}
export function setBottomHeight(height: number) {
  setBottomHeightSignal(Math.round(Math.min(600, Math.max(120, height))));
  remember();
}
