import { createMemo, ErrorBoundary, lazy, Match, Suspense, Switch, type JSX } from 'solid-js';
import { Header } from '../components/Header';
import { ProjectsRail } from '../components/ProjectsRail';
import { SearchPalette } from '../components/SearchPalette';
import { Home } from '../screens/Home';
import { Issues } from '../screens/Issues';
import { route, type View } from './router';

// The editor is the heavy part of the interface. Home opens without it; it is fetched with the first task.
const TaskScreen = lazy(() => import('../screens/Task').then((module) => ({ default: module.TaskScreen })));

const sections = {
  health: { title: 'Health', what: 'What each project is missing, and the checks every task must pass.' },
  automations: { title: 'Automations', what: 'What happens by itself, such as the clean-up after a merge.' },
  agents: { title: 'Agents', what: 'Limits, cost and who is doing what.' },
} as const;

/** A section that is drawn in the design but not built yet says so plainly. */
function NotBuilt(props: { page: keyof typeof sections }): JSX.Element {
  return (
    <main class="page">
      <div class="page-inner">
        <h1 class="headline">{sections[props.page].title} is not built yet</h1>
        <p class="soft">{sections[props.page].what} It arrives with milestone M8.</p>
      </div>
    </main>
  );
}

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
          <Match when={route().page in sections}>
            <NotBuilt page={route().page as keyof typeof sections} />
          </Match>
        </Switch>
      </div>
      <ProjectsRail />
      <SearchPalette />
    </div>
  );
}
