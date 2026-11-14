import { For, Show, type JSX } from 'solid-js';
import { route } from '../app/router';
import {
  canOpen,
  componentLabel,
  elementName,
  frameLabel,
  sizeLabel,
  sourceLabel,
  sourcePath,
  type PickedElement,
} from '../core/preview';
import {
  askForParent,
  clearPointed,
  pointedOf,
  removeDrawing,
  removeShown,
  setDrawingNote,
  setShownNote,
} from '../core/previewState';
import type { Task } from '../core/store';

/** `<p> “Too many attempts.”`: an element named by what it is and what it reads. */
function plainName(element: PickedElement): string {
  const name = elementName(element);
  return element.text ? `${name} “${element.text}”` : name;
}

/**
 * What the user pointed at in the Preview, at the top of the conversation: the
 * picked element with its component, file and size, and each drawing with a
 * note. `onOpen` shows a file of the task in the editor.
 */
export function PreviewPointed(props: {
  task: Task;
  agentName: string;
  onOpen: (path: string, line?: number) => void;
}): JSX.Element {
  const taskId = () => props.task.info.id;
  const now = () => pointedOf(taskId());
  const cwd = () => props.task.info.cwd;
  const onPreview = () => {
    const current = route();
    return current.page === 'task' && current.view === 'preview';
  };

  return (
    <>
      <Show
        when={onPreview() && now().live && !now().picked && now().drawings.length === 0 && now().shown.length === 0}
      >
        <section class="card pinned" aria-label="The preview is live">
          <h2>The preview is live</h2>
          <p>
            {now().live === 'itself'
              ? `The page reloads itself when you or ${props.agentName} change the code.`
              : `It reloads when you or ${props.agentName} change the code.`}
          </p>
        </section>
      </Show>
      <Show when={now().picked}>
        {(picked) => {
          const element = () => picked().element;
          const path = () => (element().file ? sourcePath(element().file!, cwd()) : null);
          return (
            <section class="card pinned picked" aria-label="Picked element">
              <div class="picked-head">
                <h2>{plainName(element())}</h2>
                <span class="fine">picked on {frameLabel(picked().width)}</span>
              </div>
              <dl class="picked-facts">
                <Show when={componentLabel(element())}>
                  <dt>Component</dt>
                  <dd>{componentLabel(element())}</dd>
                </Show>
                <dt>Source</dt>
                <dd>
                  <Show when={path()} fallback="No file is known">
                    <Show when={element().nearby}>around it: </Show>
                    <Show
                      when={canOpen(path()!)}
                      fallback={<span class="mono picked-source">{sourceLabel(element(), cwd())}</span>}
                    >
                      <button
                        type="button"
                        class="link mono picked-source"
                        title="Open it in the editor"
                        onClick={() => props.onOpen(path()!, element().line ?? undefined)}
                      >
                        {sourceLabel(element(), cwd())}
                      </button>
                    </Show>
                  </Show>
                </dd>
                <dt>Here</dt>
                <dd>{sizeLabel(element())}</dd>
              </dl>
              <div class="actions">
                <Show when={path() && canOpen(path()!)}>
                  <button
                    type="button"
                    class="btn small"
                    onClick={() => props.onOpen(path()!, element().line ?? undefined)}
                  >
                    Open in code
                  </button>
                </Show>
                <Show when={onPreview() && element().tag !== 'html'}>
                  <button type="button" class="btn small ghost" onClick={() => askForParent(taskId())}>
                    Pick its parent
                  </button>
                </Show>
              </div>
            </section>
          );
        }}
      </Show>
      <For each={now().drawings}>
        {(drawing) => (
          <section class="drawn" aria-label={`Drawing ${drawing.n}`}>
            <label for={`draw-note-${taskId()}-${drawing.n}`}>
              <span class="drawn-badge" aria-hidden="true">
                {drawing.n}
              </span>
              <span class="grow">
                Your drawing on {frameLabel(drawing.width)}
                {drawing.element ? `, around ${plainName(drawing.element)}` : ''}
              </span>
              <button
                type="button"
                class="btn tiny ghost"
                aria-label={`Remove drawing ${drawing.n}`}
                onClick={() => removeDrawing(taskId(), drawing.n)}
              >
                Remove
              </button>
            </label>
            <input
              id={`draw-note-${taskId()}-${drawing.n}`}
              type="text"
              class="field"
              placeholder="Say what is wrong here"
              value={drawing.note}
              onInput={(event) => setDrawingNote(taskId(), drawing.n, event.currentTarget.value)}
            />
          </section>
        )}
      </For>
      <For each={now().shown}>
        {(shown) => (
          <section class="card pinned shown" aria-label={`From the preview: ${shown.title}`}>
            <label for={`shown-note-${taskId()}-${shown.n}`}>
              <span class="grow">{shown.title}</span>
              <button
                type="button"
                class="btn tiny ghost"
                aria-label={`Leave out ${shown.title}`}
                onClick={() => removeShown(taskId(), shown.n)}
              >
                Remove
              </button>
            </label>
            <pre class="shown-text mono">{shown.text}</pre>
            <input
              id={`shown-note-${taskId()}-${shown.n}`}
              type="text"
              class="field"
              placeholder="Say what is wrong here"
              value={shown.note}
              onInput={(event) => setShownNote(taskId(), shown.n, event.currentTarget.value)}
            />
          </section>
        )}
      </For>
    </>
  );
}

/** The line above the message box that names what goes along with the message. */
export function PointedChips(props: { taskId: string }): JSX.Element {
  const now = () => pointedOf(props.taskId);
  /** The frames the pick and the drawings are from, each named once. */
  const frames = () => {
    const widths = [...(now().picked ? [now().picked!.width] : []), ...now().drawings.map((drawing) => drawing.width)];
    return [...new Set(widths)].sort((a, b) => a - b);
  };
  return (
    <Show when={now().picked || now().drawings.length > 0 || now().shown.length > 0}>
      <div class="quote pointed" aria-label="Goes along with your message">
        <span class="grow pointed-chips">
          <Show when={now().picked}>{(picked) => <span class="tag mono">{elementName(picked().element)}</span>}</Show>
          <For each={now().drawings}>{(drawing) => <span class="tag mono">drawing {drawing.n}</span>}</For>
          <For each={frames()}>{(width) => <span class="tag mono">{frameLabel(width)}</span>}</For>
          <For each={now().shown}>{(shown) => <span class="tag mono">{shown.title}</span>}</For>
        </span>
        <button type="button" class="btn tiny" onClick={() => clearPointed(props.taskId)}>
          Leave it out
        </button>
      </div>
    </Show>
  );
}
