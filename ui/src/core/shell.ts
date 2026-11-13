import { core } from './client';

/**
 * The desktop app, seen from the interface.
 *
 * The interface is the same page in a browser and in the app's window. The
 * app leaves a mark on its window before the page loads; that is how the page
 * knows it may ask for things only an app can do, such as the system's folder
 * dialog. In a browser there is no mark, and the user types the path instead.
 */
export type ShellMark = {
  /** Goes with every request for something only the app can do, so the core knows the app's own window asks. */
  key: string;
};

/** The mark the desktop app left on this window. `null` in a browser. */
export function shellMark(scope: unknown = globalThis): ShellMark | null {
  const mark = (scope as { __AE_SHELL__?: unknown } | null | undefined)?.__AE_SHELL__;
  if (typeof mark !== 'object' || mark === null) return null;
  const key = (mark as { key?: unknown }).key;
  return typeof key === 'string' && key !== '' ? { key } : null;
}

type Request = <T>(method: string, params: Record<string, unknown>) => Promise<T>;

/**
 * Lets the user choose a folder in the system's own dialog.
 *
 * Gives the folder's path, or `null` when there is nothing to take: the user
 * closed the dialog, or this is a browser and there is no dialog to show.
 * Fails only when the app was asked and could not show its dialog.
 */
export async function chooseFolder(
  mark: ShellMark | null = shellMark(),
  request: Request = (method, params) => core.request(method, params),
): Promise<string | null> {
  if (!mark) return null;
  const answer = await request<{ path: string | null }>('dialogs.pickFolder', { key: mark.key });
  return typeof answer?.path === 'string' && answer.path !== '' ? answer.path : null;
}
