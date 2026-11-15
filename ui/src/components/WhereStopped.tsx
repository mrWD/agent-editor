import { For, Show, type JSX } from 'solid-js';
import { taskHref } from '../app/router';
import { requests, stageOfTask } from '../core/requests';
import { agentOf, type Since, type Task } from '../core/store';
import { setWork } from '../core/work';
import { ago, Chip, span, whenLabel } from './bits';

/**
 * What the user sees on coming back to a task: what they did last, and what
 * happened while they were away. Every word comes from what the core wrote
 * down as it happened; no model is asked, so it costs nothing and invents nothing.
 */
export function WhereStopped(props: { task: Task; since: Since; onHide: () => void }): JSX.Element {
  const agent = () => agentOf(props.task);
  const name = () => agent()?.name ?? props.task.info.agentId;
  const id = () => props.task.info.id;
  const hasRequest = () => Boolean(requests[id()]);

  /** One line on what the task waits for now. */
  const nowLine = () => {
    switch (stageOfTask(props.task)) {
      case 'needs':
        return props.task.info.state === 'waiting_permission'
          ? `${name()} is waiting for your answer.`
          : 'The checks of the merge request failed.';
      case 'running':
        return `${name()} is still working.`;
      case 'ready':
        return 'Nothing is waiting for you except the merge.';
      case 'checks':
        return 'The checks of the merge request are still running.';
      case 'merged':
        return 'The request is merged. What is left is to clean up.';
      case 'turn':
        return hasRequest()
          ? `${name()} is paused. It is your turn.`
          : `${name()} is paused, there is no merge request yet.`;
    }
  };

  return (
    <section class="stopped" aria-label="Where you stopped">
      <div class="stopped-head">
        <h2>Where you stopped · {whenLabel(props.since.leftAt)}</h2>
        <span class="grow" />
        <button type="button" class="btn tiny ghost" onClick={() => props.onHide()}>
          Hide
        </button>
      </div>
      <Show when={props.since.stopped}>
        <p class="stopped-you">{props.since.stopped}</p>
      </Show>
      <Show when={!props.since.quiet} fallback={<p class="soft">Since then: nothing. {nowLine()}</p>}>
        <div class="stopped-away">
          <Chip agentId={props.task.info.agentId} short={agent()?.short ?? '··'} />
          <span class="who">While you were away</span>
          <span class="muted">{span(props.since.awayMs)}</span>
        </div>
        <ul class="stopped-list">
          <For each={props.since.happened}>
            {(line) => (
              <li>
                {line.text}
                <Show when={line.short}>
                  {' '}
                  <span class="mono muted">{line.short}</span>
                </Show>
                <Show when={line.at}>
                  {' '}
                  <span class="muted">{ago(line.at!)}</span>
                </Show>
              </li>
            )}
          </For>
          <Show when={props.since.happened.length === 0 && props.since.files.length > 0}>
            <li>{props.since.files.length === 1 ? 'One file' : `${props.since.files.length} files`} changed on disk</li>
          </Show>
        </ul>
        <Show when={props.since.lastWords}>
          <blockquote class="stopped-words">
            {name()} said: {props.since.lastWords}
          </blockquote>
        </Show>
        <p class="soft">{nowLine()}</p>
        <div class="actions">
          <Show when={props.since.canCompare && props.since.files.length > 0}>
            <a class="btn small" href={taskHref(id(), 'changes')} onClick={() => setWork(id(), { changes: 'visit' })}>
              See what changed since
            </a>
          </Show>
          <Show when={hasRequest()}>
            <a
              class="btn small ghost"
              href={taskHref(id(), 'changes')}
              onClick={() => setWork(id(), { changes: 'request' })}
            >
              Open the merge request
            </a>
          </Show>
        </div>
      </Show>
    </section>
  );
}
