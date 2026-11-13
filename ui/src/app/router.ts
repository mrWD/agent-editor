import { createSignal } from 'solid-js';

/** The six views of one task, in the order of the switcher. */
export const VIEWS = ['plan', 'code', 'changes', 'preview', 'map', 'checks'] as const;
/** A seventh view that only the entrants of a race have: the results side by side. */
export const RACE_VIEW = 'compare' as const;
export type View = (typeof VIEWS)[number] | typeof RACE_VIEW;

export type Route =
  | { page: 'home' }
  | { page: 'issues' | 'health' | 'automations' | 'agents' | 'settings' }
  | { page: 'task'; id: string; view: View };

function parse(hash: string): Route {
  const parts = hash.replace(/^#\/?/, '').split('/').filter(Boolean);
  switch (parts[0]) {
    case 'issues':
    case 'health':
    case 'automations':
    case 'agents':
    case 'settings':
      return { page: parts[0] };
    case 'task': {
      if (!parts[1]) return { page: 'home' };
      const view = [RACE_VIEW, ...VIEWS].includes(parts[2] as View) ? (parts[2] as View) : 'code';
      return { page: 'task', id: parts[1], view };
    }
    default:
      return { page: 'home' };
  }
}

// The address after "#" is the whole route. It survives a reload and needs
// nothing from the server, so it works the same in a browser and in the shell.
const [route, setRoute] = createSignal<Route>(parse(location.hash));
window.addEventListener('hashchange', () => setRoute(parse(location.hash)));

export { route };

export function taskHref(id: string, view: View = 'code'): string {
  return `#/task/${id}/${view}`;
}

export function go(href: string) {
  location.hash = href.replace(/^#/, '');
}
