import { For, Show, type JSX } from 'solid-js';
import { problemsOf } from '../core/problems';

/**
 * The Problems tab under the code: what the checks and the language servers
 * found wrong, each with its file and line. A press opens the file there.
 */
export function ProblemsTab(props: {
  taskId: string;
  onOpen: (path: string, line: number, column?: number) => void;
}): JSX.Element {
  const list = () => problemsOf(props.taskId);
  return (
    <div class="problems" aria-label="Problems">
      <Show when={list().length === 0}>
        <p class="bottom-note">
          No problems are known. They show here when a check on the Checks view fails, or when a language server finds
          one in an open file.
        </p>
      </Show>
      <For each={list()}>
        {(problem) => (
          <button
            type="button"
            class="problem"
            onClick={() => props.onOpen(problem.path, problem.line, problem.column)}
          >
            <span class={`problem-mark ${problem.severity}`} aria-hidden="true">
              {problem.severity === 'error' ? '!' : problem.severity === 'warning' ? '?' : 'i'}
            </span>
            <span class="problem-text">
              <span class="sr-only">{problem.severity}: </span>
              {problem.message}
            </span>
            <span class="mono muted">
              {problem.path}:{problem.line}
            </span>
            <span class="fine">{problem.from}</span>
          </button>
        )}
      </For>
    </div>
  );
}
