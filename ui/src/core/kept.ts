import { createSignal, type Accessor } from 'solid-js';
import { unwrap } from 'solid-js/store';

/**
 * What a screen read last, kept after the screen is gone.
 *
 * A section reads what it shows when it is opened. Without this, leaving it
 * and coming back means an empty screen first and the same content a moment
 * later. With it, the screen opens on what it showed last time and reads
 * again behind that: what is on screen may be a moment old, never missing.
 *
 * Nothing here outlives the window: a reload starts clean.
 */

const last = new Map<string, unknown>();

/** What was kept under a name, or `null`. */
export function recall<T>(key: string): T | null {
  return last.has(key) ? (last.get(key) as T) : null;
}

/** Keeps a value under a name, and gives it back. */
export function remember<T>(key: string, value: T): T {
  last.set(key, value);
  return value;
}

/** A copy of what was kept, for a store: a store changes what it is given, and the kept value must stay as it was. */
export function recallCopy<T>(key: string): T | null {
  const kept = recall<T>(key);
  return kept === null ? null : structuredClone(kept);
}

/**
 * Keeps a copy of a value a store is about to take in. The value may hold pieces of the store
 * itself (what was known before, carried over into a new answer): those are taken out of their
 * wrapping first, since a wrapped piece cannot be copied.
 */
export function rememberCopy<T>(key: string, value: T): T {
  last.set(key, structuredClone(unwrap(value)));
  return value;
}

/** Forgets everything kept under names that start so: for a task that is gone. */
export function forget(prefix: string) {
  for (const key of [...last.keys()]) if (key.startsWith(prefix)) last.delete(key);
}

type Setter<T> = (next: T | ((before: T) => T)) => T;

/**
 * A signal whose value is kept after the component is gone: the next
 * component made with the same name starts with it. `start` is what it holds
 * the very first time.
 */
export function createKept<T>(key: string, start: T): [Accessor<T>, Setter<T>] {
  const [value, setValue] = createSignal<T>(last.has(key) ? (last.get(key) as T) : start);
  const set: Setter<T> = (next) => {
    const now = typeof next === 'function' ? (next as (before: T) => T)(value()) : next;
    last.set(key, now);
    setValue(() => now);
    return now;
  };
  return [value, set];
}
