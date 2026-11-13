import { createRoot } from 'solid-js';
import { createStore } from 'solid-js/store';
import { describe, expect, it } from 'vitest';
import { createKept, forget, recall, recallCopy, remember, rememberCopy } from './kept';

describe('what a screen read last', () => {
  it('is there for the next screen of the same name, and not for another', () => {
    createRoot((dispose) => {
      const [issues, setIssues] = createKept<string[] | null>('test.issues', null);
      expect(issues()).toBeNull();
      setIssues(['APP-1']);
      expect(issues()).toEqual(['APP-1']);
      dispose();
    });
    // The first screen is gone. The next one opens on what it showed.
    createRoot((dispose) => {
      const [issues] = createKept<string[] | null>('test.issues', null);
      expect(issues()).toEqual(['APP-1']);
      const [other] = createKept<string[] | null>('test.other', null);
      expect(other()).toBeNull();
      dispose();
    });
  });

  it('takes a value or a way to make the next one from the last', () => {
    createRoot((dispose) => {
      const [count, setCount] = createKept('test.count', 1);
      setCount((before) => before + 1);
      expect(count()).toBe(2);
      // A value that is a function of nothing in particular is still a value to keep: `null` is kept too.
      const [maybe, setMaybe] = createKept<number | null>('test.maybe', 5);
      setMaybe(null);
      expect(maybe()).toBeNull();
      dispose();
    });
    createRoot((dispose) => {
      expect(createKept('test.count', 1)[0]()).toBe(2);
      expect(createKept<number | null>('test.maybe', 5)[0]()).toBeNull();
      dispose();
    });
  });

  it('keeps a copy for a store, which changes what it is given', () => {
    const read = { services: [{ name: 'web', state: 'running' }] };
    rememberCopy('test.run', read);
    read.services[0]!.state = 'stopped';
    const first = recallCopy<typeof read>('test.run')!;
    expect(first.services[0]!.state).toBe('running');
    first.services[0]!.state = 'changed by the store';
    expect(recallCopy<typeof read>('test.run')!.services[0]!.state).toBe('running');
    expect(recallCopy('test.nothing')).toBeNull();
  });

  it('forgets what belonged to a task that is gone', () => {
    remember('task-1:map', 'a');
    remember('task-1:run', 'b');
    remember('task-12:map', 'c');
    forget('task-1:');
    expect([recall('task-1:map'), recall('task-1:run'), recall('task-12:map')]).toEqual([null, null, 'c']);
  });

  it('takes a copy of an answer that carries pieces of the store over', () => {
    // An answer without the disk keeps what the store knew of it: that piece is the store's own.
    const [held, setHeld] = createStore<{ use: { others: string[]; disk?: { images: number } } | null }>({
      use: { others: ['a'], disk: { images: 2 } },
    });
    const next = { others: [], disk: held.use?.disk };
    expect(() => setHeld('use', rememberCopy('test.use', next))).not.toThrow();
    expect(recall('test.use')).toEqual({ others: [], disk: { images: 2 } });
    // What is kept is a copy: the store changing later does not change it.
    setHeld('use', 'disk', 'images', 5);
    expect(recall('test.use')).toEqual({ others: [], disk: { images: 2 } });
  });
});
