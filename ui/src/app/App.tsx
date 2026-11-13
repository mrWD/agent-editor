import { createMemo, ErrorBoundary, lazy, Match, Suspense, Switch, type JSX } from 'solid-js';
import { Header } from '../components/Header';
import { ProjectsRail } from '../components/ProjectsRail';
import { SearchPalette } from '../components/SearchPalette';
import { Agents } from '../screens/Agents';
import { Automations } from '../screens/Automations';
import { Health } from '../screens/Health';
import { Home } from '../screens/Home';
import { Issues } from '../screens/Issues';
import { Settings } from '../screens/Settings';
import { route, type View } from './router';

// The editor is the heavy part of the interface. Home opens without it; it is fetched with the first task.
const TaskScreen = lazy(() => import('../screens/Task').then((module) => ({ default: module.TaskScreen })));

export function App(): JSX.Element {
  // The task last shown. It keeps its value while the task screen is being taken
  // down, so a part of that screen that reads it at that moment still finds it.
  // (An accessor handed out by <Match> throws when read after its condition turned false.)
  const shownTask = createMemo<{ id: string; view: View } | undefined>(
    (last) => {
      const current = route();
      return current.page === 'task' ? { id: current.id, view: current.view } : last;
    },
    undefined,
    { equals: (a, b) => a?.id === b?.id && a?.view === b?.view },
  );

  return (
    <div class="app">
      <Header />
      <div class="app-body">
        <Switch>
          <Match when={route().page === 'home'}>
            <Home />
          </Match>
          <Match when={route().page === 'task'}>
            {/* The task screen is fetched when first needed. If that fails, say so: a blank screen says nothing. */}
            <ErrorBoundary
              fallback={(error) => (
                <main class="page">
                  <div class="page-inner">
                    <h1 class="headline">This screen could not be loaded</h1>
                    <p class="soft">{error instanceof Error ? error.message : String(error)}</p>
                    <p>
                      <button type="button" class="btn" onClick={() => window.location.reload()}>
                        Reload the window
                      </button>
                    </p>
                  </div>
                </main>
              )}
            >
              <Suspense>
                <TaskScreen id={shownTask()!.id} view={shownTask()!.view} />
              </Suspense>
            </ErrorBoundary>
          </Match>
          <Match when={route().page === 'issues'}>
            <Issues />
          </Match>
          <Match when={route().page === 'automations'}>
            <Automations />
          </Match>
          <Match when={route().page === 'agents'}>
            <Agents />
          </Match>
          <Match when={route().page === 'health'}>
            <Health />
          </Match>
          <Match when={route().page === 'settings'}>
            <Settings />
          </Match>
        </Switch>
      </div>
      <ProjectsRail />
      <SearchPalette />
    </div>
  );
}
