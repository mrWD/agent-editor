import { describe, expect, it } from 'vitest';
import { askPermission, hrefOf, inFront, permission, refusal, show, type Notice } from './notify';

/** A stand-in for the browser's notifications: it notes what was shown and answers a request as told. */
function browser(start: string, answer = start) {
  const shown: { title: string; body?: string; tag?: string; onclick: (() => void) | null; closed: boolean }[] = [];
  let asked = 0;
  class Fake {
    static permission = start;
    static requestPermission() {
      asked += 1;
      Fake.permission = answer;
      return Promise.resolve(answer);
    }
    onclick: (() => void) | null = null;
    closed = false;
    title: string;
    body?: string;
    tag?: string;
    constructor(title: string, options?: { body?: string; tag?: string }) {
      this.title = title;
      this.body = options?.body;
      this.tag = options?.tag;
      shown.push(this);
    }
    close() {
      this.closed = true;
    }
  }
  let focused = 0;
  const scope = { Notification: Fake as unknown as typeof Notification, focus: () => void (focused += 1) };
  return { scope, shown, asked: () => asked, focused: () => focused };
}

const notice: Notice = {
  id: 'task-1:42',
  kind: 'checksFailed',
  taskId: 'task-1',
  view: 'changes',
  title: 'The checks of pull request 128 failed',
  body: 'Take the limit from config · shop',
  byApp: false,
};

describe('what the browser says about notifications', () => {
  it('is read as it stands, and a browser without them says so', () => {
    expect(permission({})).toBe('unsupported');
    expect(permission(browser('granted').scope)).toBe('granted');
    expect(permission(browser('denied').scope)).toBe('denied');
    expect(permission(browser('default').scope)).toBe('default');
    // The tests themselves run where there are no notifications.
    expect(permission()).toBe('unsupported');
  });

  it('is asked for once, and the answer is what counts', async () => {
    const agrees = browser('default', 'granted');
    expect(await askPermission(agrees.scope)).toBe('granted');
    expect(agrees.asked()).toBe(1);
    expect(await askPermission(browser('default', 'denied').scope)).toBe('denied');
    expect(await askPermission(browser('default').scope)).toBe('default');
    expect(await askPermission({})).toBe('unsupported');
  });

  it('is read even when asking failed', async () => {
    const old = browser('denied');
    old.scope.Notification.requestPermission = () => Promise.reject(new Error('wants a callback'));
    expect(await askPermission(old.scope)).toBe('denied');
  });

  it('is put in plain words when the answer is no', () => {
    expect(refusal('granted')).toBeNull();
    expect(refusal('denied')).toContain('does not allow notifications from this address');
    expect(refusal('default')).toContain('closed without an answer');
    expect(refusal('unsupported')).toBe('This browser cannot show notifications.');
  });
});

describe('whether this window is in front', () => {
  it('takes both: on screen, and the one that has the keys', () => {
    const page = (visibilityState: DocumentVisibilityState, focus: boolean) => ({
      visibilityState,
      hasFocus: () => focus,
    });
    expect(inFront(page('visible', true))).toBe(true);
    // Another program is in front of a window that is still on screen.
    expect(inFront(page('visible', false))).toBe(false);
    // Another tab of the same browser window.
    expect(inFront(page('hidden', true))).toBe(false);
    expect(inFront(page('hidden', false))).toBe(false);
  });
});

describe('showing a notice in a browser', () => {
  it('shows it once, with a tag that is the same in every window', () => {
    const chrome = browser('granted');
    expect(show(notice, chrome.scope, () => {})).toBe(true);
    expect(chrome.shown).toHaveLength(1);
    expect(chrome.shown[0]).toMatchObject({
      title: 'The checks of pull request 128 failed',
      body: 'Take the limit from config · shop',
      tag: 'task-1:42',
    });
  });

  it('shows nothing without permission, and nothing the app has shown already', () => {
    for (const state of ['default', 'denied']) {
      const refusing = browser(state);
      expect(show(notice, refusing.scope, () => {})).toBe(false);
      expect(refusing.shown).toEqual([]);
    }
    const inApp = browser('granted');
    expect(show({ ...notice, byApp: true }, inApp.scope, () => {})).toBe(false);
    expect(inApp.shown).toEqual([]);
    expect(show(notice, {}, () => {})).toBe(false);
  });

  it('a click brings the window forward, opens the task on the right view and takes the notice away', () => {
    const chrome = browser('granted');
    const opened: string[] = [];
    show(notice, chrome.scope, (href) => opened.push(href));
    expect(opened).toEqual([]);
    chrome.shown[0]!.onclick!();
    expect(chrome.focused()).toBe(1);
    expect(opened).toEqual(['#/task/task-1/changes']);
    expect(chrome.shown[0]!.closed).toBe(true);
  });

  it('a browser that refuses to make one is not an error', () => {
    const mobile = browser('granted');
    mobile.scope.Notification = class {
      static permission = 'granted';
      constructor() {
        throw new TypeError('Illegal constructor');
      }
    } as unknown as typeof Notification;
    expect(show(notice, mobile.scope, () => {})).toBe(false);
  });

  it('leads to the task and the view the notice names', () => {
    expect(hrefOf({ taskId: 'abc', view: 'code' })).toBe('#/task/abc/code');
    expect(hrefOf({ taskId: 'a b', view: 'changes' })).toBe('#/task/a%20b/changes');
  });
});
