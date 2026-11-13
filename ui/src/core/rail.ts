/**
 * What the projects rail has to work out when there are many projects: which
 * folder each is in, which ones stay on the line when not all fit, and what
 * the Find box is looking for. No drawing here, so it can be tested as it is.
 */

/** What deciding who stays on the line needs to know of a project. */
export type Foldable = { id: string; needs: boolean; current: boolean; lastActive: number };

/**
 * The order of projects when not all of them fit: the ones that need the
 * user, the one on screen, then the ones worked in most recently. What falls
 * off the end is what folds into "N more".
 */
export function foldOrder<T extends Foldable>(items: T[]): T[] {
  const rank = (item: T) => (item.needs ? 0 : item.current ? 1 : 2);
  return [...items].sort((a, b) => rank(a) - rank(b) || b.lastActive - a.lastActive);
}

/**
 * How many projects are never folded away, counted from the start of
 * `foldOrder`: every one that needs the user, the one on screen, and the one
 * worked in last.
 */
export function alwaysShown(ordered: Foldable[]): number {
  const fixed = ordered.filter((item) => item.needs || item.current).length;
  return Math.min(ordered.length, fixed + 1);
}

/** How many of the ordered widths stay on a line with `room` on it. Never fewer than `keep`. */
export function fitCount(widths: number[], keep: number, room: number): number {
  let used = 0;
  let count = 0;
  for (const width of widths) {
    if (used + width > room) break;
    used += width;
    count++;
  }
  return Math.min(widths.length, Math.max(count, keep));
}

/** Projects by the folder they are in. Folders go by name; a project without a folder stands on its own. */
export function byFolder<T extends { folder: string | null }>(
  projects: T[],
): { folders: { name: string; projects: T[] }[]; loose: T[] } {
  const folders = new Map<string, T[]>();
  const loose: T[] = [];
  for (const project of projects) {
    const name = project.folder?.trim();
    if (!name) loose.push(project);
    else folders.set(name, [...(folders.get(name) ?? []), project]);
  }
  return {
    folders: [...folders.entries()]
      .sort(([a], [b]) => a.localeCompare(b, undefined, { sensitivity: 'base' }))
      .map(([name, list]) => ({ name, projects: list })),
    loose,
  };
}

/** Whether `text` is what is being looked for: every word typed is in it, in any order and any case. */
export function finds(query: string, text: string): boolean {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  const where = text.toLowerCase();
  return words.every((word) => where.includes(word));
}

/** "86 tasks in 12 projects", with the words right for one of each. */
export function totals(tasks: number, projects: number): string {
  return `${tasks} ${tasks === 1 ? 'task' : 'tasks'} in ${projects} ${projects === 1 ? 'project' : 'projects'}`;
}
