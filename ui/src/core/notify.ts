import { core } from './client';

/**
 * Notifications: what the user is told while the editor is not in front.
 *
 * The core decides what is worth telling and when; a page does two things.
 * It says whether it is in front, so the core knows when nobody is looking.
 * And, in a browser, it shows what the core sends with the browser's own
 * notifications. Inside the desktop app the app shows them itself, also with
 * its window closed, and the page shows nothing.
 */

/** One thing the core wants the user to be told. */
export type Notice = {
  /** The same in every window, so two tabs of one browser show it once. */
  id: string;
  kind: string;
  taskId: string;
  /** The view of the task a click opens. */
  view: string;
  title: string;
  body: string;
  /** True when the desktop app has shown it already. */
  byApp: boolean;
};

/** What a browser says about showing notifications. "unsupported": it has none. */
export type Permission = 'granted' | 'denied' | 'default' | 'unsupported';

type Scope = { Notification?: typeof Notification };

/** Whether this browser may show notifications right now. */
export function permission(scope: Scope = globalThis): Permission {
  const api = scope.Notification;
  if (typeof api !== 'function') return 'unsupported';
  return api.permission === 'granted' || api.permission === 'denied' ? api.permission : 'default';
}

/** Asks the browser for permission. Must follow a press of the user, or browsers refuse without asking. */
export async function askPermission(scope: Scope = globalThis): Promise<Permission> {
  const api = scope.Notification;
  if (typeof api !== 'function') return 'unsupported';
  try {
    await api.requestPermission();
  } catch {
    // An old browser that wants a callback: what it decided is read below either way.
  }
  return permission(scope);
}

/** What to tell a user whose browser did not give permission, in plain words. `null` when it did. */
export function refusal(answer: Permission): string | null {
  switch (answer) {
    case 'granted':
      return null;
    case 'denied':
      return 'This browser does not allow notifications from this address. Allow them in the browser’s settings for this page, then turn them on again.';
    case 'default':
      return 'The browser’s question was closed without an answer, so notifications stay off.';
    case 'unsupported':
      return 'This browser cannot show notifications.';
  }
}

/**
 * True when this window is the one the user is looking at: on screen, and the
 * one that takes the keys. A frame inside the page that has the keys (the
 * Preview) still counts: the browser says the page has the focus then.
 */
export function inFront(page: Pick<Document, 'visibilityState' | 'hasFocus'> = document): boolean {
  return page.visibilityState === 'visible' && page.hasFocus();
}

/**
 * Where a click on a notice leads: the task, on the view the notice names.
 * The router reads a view it does not know as the conversation.
 */
export function hrefOf(notice: Pick<Notice, 'taskId' | 'view'>): string {
  return `#/task/${encodeURIComponent(notice.taskId)}/${encodeURIComponent(notice.view)}`;
}

/**
 * Shows one notice with the browser's notifications. Returns whether it was
 * shown: not when the app has shown it, and not without permission. A click
 * on it brings the window forward and opens the task with `open`.
 */
export function show(
  notice: Notice,
  scope: Scope & { focus?: () => void } = globalThis,
  open: (href: string) => void = (href) => {
    location.hash = href.replace(/^#/, '');
  },
): boolean {
  if (notice.byApp || permission(scope) !== 'granted') return false;
  try {
    // The tag makes a second tab's copy replace this one instead of standing next to it.
    const shown = new scope.Notification!(notice.title, { body: notice.body, tag: notice.id });
    shown.onclick = () => {
      scope.focus?.();
      open(hrefOf(notice));
      shown.close();
    };
    return true;
  } catch {
    // Some browsers only let a service worker show notifications; there is nothing to show with here.
    return false;
  }
}

let watching = false;

/** Tells the core when this window comes to the front or leaves it, and shows what the core sends. Call once. */
export function watchNotifications() {
  if (watching) return;
  watching = true;
  /** What the core was last told; `null` when it has to be told again. */
  let told: boolean | null = null;
  const report = () => {
    const front = inFront();
    if (front === told) return;
    told = front;
    core.request('notifications.attention', { front }).catch(() => {
      told = null;
    });
  };
  // A moment later: while "blur" is being handled some browsers still say the page has the focus.
  const soon = () => setTimeout(report, 0);
  window.addEventListener('focus', soon);
  window.addEventListener('blur', soon);
  document.addEventListener('visibilitychange', soon);
  // Not every way of leaving a window is announced (another desktop, a screen that locks).
  setInterval(report, 5000);

  core.onEvent((event) => {
    if (event.event === 'link.open') {
      // A new connection is a new window to the core: it knows nothing about this one yet.
      told = null;
      report();
    } else if (event.event === 'notification') {
      show(event.data as Notice, window);
    }
  });
}
