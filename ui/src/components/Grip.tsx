import { createSignal, type JSX } from 'solid-js';
import { setSize, SIZES, sizeOf, type SizeName } from '../core/sizes';

/** How far one press of an arrow key moves the edge. */
const STEP = 16;

/**
 * An edge that can be dragged to make a part of the screen larger or smaller.
 *
 * `edge` is the side of the part the edge is on: dragging away from the part
 * makes it larger. The arrow keys do the same, and a double click puts the
 * part back to the size it had at first. `room` says how large the part may
 * get in the place it stands in right now.
 */
export function Grip(props: {
  name: SizeName;
  edge: 'top' | 'left' | 'right';
  label: string;
  room?: () => number;
}): JSX.Element {
  const [dragging, setDragging] = createSignal(false);
  const upright = () => props.edge !== 'top';
  /** How much larger the part gets for a move of the pointer by `dx`, `dy`. */
  const grown = (dx: number, dy: number) => (props.edge === 'top' ? -dy : props.edge === 'left' ? -dx : dx);

  const drag = (event: PointerEvent) => {
    if (event.button !== 0) return;
    const grip = event.currentTarget as HTMLElement;
    grip.setPointerCapture(event.pointerId);
    setDragging(true);
    const [fromX, fromY, was] = [event.clientX, event.clientY, sizeOf(props.name)];
    const move = (moved: PointerEvent) =>
      setSize(props.name, was + grown(moved.clientX - fromX, moved.clientY - fromY), props.room?.());
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
    const moves: Record<string, [number, number]> = {
      ArrowLeft: [-STEP, 0],
      ArrowRight: [STEP, 0],
      ArrowUp: [0, -STEP],
      ArrowDown: [0, STEP],
    };
    const move = moves[event.key];
    // Only the arrows along the way the edge moves.
    if (!move || (move[0] !== 0) !== upright()) return;
    event.preventDefault();
    setSize(props.name, sizeOf(props.name) + grown(move[0], move[1]), props.room?.());
  };

  return (
    <div
      class={`grip grip-${props.edge}`}
      classList={{ dragging: dragging() }}
      role="separator"
      aria-orientation={upright() ? 'vertical' : 'horizontal'}
      aria-label={props.label}
      aria-valuenow={sizeOf(props.name)}
      aria-valuemin={SIZES[props.name].min}
      aria-valuemax={SIZES[props.name].max}
      tabindex="0"
      title="Drag to resize. Double click to reset."
      onPointerDown={drag}
      onKeyDown={nudge}
      onDblClick={() => setSize(props.name, SIZES[props.name].initial)}
    />
  );
}
