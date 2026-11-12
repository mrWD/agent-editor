import { createMemo, For, Show, type JSX } from 'solid-js';
import { route } from '../app/router';
import { core } from '../core/client';
import { projectFilter, setProjectFilter, state } from '../core/store';
import { Icon, modKey } from './bits';
import { openSearch } from './SearchPalette';

/** The top row: the sections of the app, the same on every screen. */
export function Header(): JSX.Element {
  const needsYou = createMemo(
    () => Object.values(state.tasks).filter((task) => task.info.state === 'waiting_permission').length,
  );
  const current = (page: string) => (route().page === page ? 'page' : undefined);
  const taskCount = (projectId: string | null) =>
    Object.values(state.tasks).filter((task) => projectId === null || task.info.projectId === projectId).length;

  return (
    <>
      <header class="header">
        <nav class="header-nav" aria-label="Sections">
          <a class="nav-link" href="#/" aria-current={current('home')}>
            <Icon.Inbox />
            Home
            <Show when={needsYou() > 0}>
              <span class="badge" aria-label={`${needsYou()} need you`}>
                {needsYou()}
              </span>
            </Show>
          </a>
          <a class="nav-link" href="#/issues" aria-current={current('issues')}>
            Issues
          </a>
          <a class="nav-link" href="#/health" aria-current={current('health')}>
            Health
          </a>
          <a class="nav-link" href="#/automations" aria-current={current('automations')}>
            Automations
          </a>
          <a class="nav-link" href="#/agents" aria-current={current('agents')}>
            Agents
          </a>
          <a class="nav-link" href="#/">
            <Icon.Plus />
            New task
          </a>
        </nav>
        <Show when={route().page === 'home' && state.projects.length > 1}>
          <div class="switcher" role="group" aria-label="Show tasks of">
            <button type="button" aria-pressed={projectFilter() === null} onClick={() => setProjectFilter(null)}>
              All <span class="mono">{taskCount(null)}</span>
            </button>
            <For each={state.projects}>
              {(project) => (
                <button
                  type="button"
                  aria-pressed={projectFilter() === project.id}
                  onClick={() => setProjectFilter(project.id)}
                >
                  {project.name} <span class="mono">{taskCount(project.id)}</span>
                </button>
              )}
            </For>
          </div>
        </Show>
        <button type="button" class="search-link" onClick={openSearch}>
          <Icon.Search />
          Search
          <kbd>{modKey}K</kbd>
        </button>
        <button type="button" class="icon-button" aria-label="Settings" disabled title="Settings are not built yet">
          <Icon.Settings />
        </button>
      </header>
      <Show when={core.status() === 'lost'}>
        <div class="link-lost" role="status">
          The core is not answering. Agents keep working if it is still running; this page reconnects by itself.
        </div>
      </Show>
    </>
  );
}
