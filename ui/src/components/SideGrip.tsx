import { createSignal, type JSX } from 'solid-js';
import { setSideWidth, SIDES, sideWidth, type Side } from '../core/layout';

/** How far one press of an arrow key moves the edge. */
const STEP = 16;

/**
 * The edge between a side pane and the middle of a task. Drag it to make the
 * pane wider or narrower; the arrow keys do the same, and a double click puts
 * the pane back to the width it had at first.
 */
export function SideGrip(props: { side: Side; label: string; hidden?: boolean }): JSX.Element {
  const [dragging, setDragging] = createSignal(false);
  /** The row of panes this edge stands in: its width is the room there is to share. */
  const row = (grip: HTMLElement) => grip.parentElement?.getBoundingClientRect();

  const drag = (event: PointerEvent) => {
    if (event.button !== 0) return;
    const grip = event.currentTarget as HTMLElement;
    grip.setPointerCapture(event.pointerId);
    setDragging(true);
    const move = (moved: PointerEvent) => {
      const box = row(grip);
      if (!box) return;
      setSideWidth(
        props.side,
        props.side === 'files' ? moved.clientX - box.left : box.right - moved.clientX,
        box.width,
      );
    };
    const stop = () => {
      setDragging(false);
      grip.removeEventListener('pointermove', move);
      grip.removeEventListener('pointerup', stop);
      grip.removeEventListener('pointercancel', stop);
    };
    grip.addEventListener('pointermove', move);
    grip.addEventListener('pointerup', stop);
    grip.addEventListener('pointercancel', stop);
    // No text gets selected while the edge is being dragged.
    event.preventDefault();
  };

  const nudge = (event: KeyboardEvent) => {
    if (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight') return;
    event.preventDefault();
    // The arrow moves the edge the way it points; for the pane on the right that is the other way round.
    const wider = (event.key === 'ArrowRight') === (props.side === 'files');
    const room = row(event.currentTarget as HTMLElement)?.width;
    setSideWidth(props.side, sideWidth(props.side) + (wider ? STEP : -STEP), room);
  };

  return (
    <div
      class="side-grip"
      classList={{ dragging: dragging() }}
      style={{ display: props.hidden ? 'none' : undefined }}
      role="separator"
      aria-orientation="vertical"
      aria-label={props.label}
      aria-valuenow={sideWidth(props.side)}
      aria-valuemin={SIDES[props.side].min}
      aria-valuemax={SIDES[props.side].max}
      tabindex="0"
      title="Drag to resize. Double click to reset."
      onPointerDown={drag}
      onKeyDown={nudge}
      onDblClick={() => setSideWidth(props.side, SIDES[props.side].initial)}
    />
  );
}
