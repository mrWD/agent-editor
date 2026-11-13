import { createEffect, createSignal, For, on, onCleanup, onMount, Show, type JSX } from 'solid-js';
import { go } from '../app/router';
import { place, ringOf, type Box, type TourStep } from '../core/tour';

/** How long a step waits for what it points at to be on screen, after going to its page. */
const WAIT_FOR_TARGET = 3000;

const boxOf = (element: Element): Box => {
  const rect = element.getBoundingClientRect();
  return { top: rect.top, left: rect.left, width: rect.width, height: rect.height };
};
const same = (a: Box | null, b: Box | null) =>
  a === b ||
  (a !== null && b !== null && a.top === b.top && a.left === b.left && a.width === b.width && a.height === b.height);

/**
 * A tour of hints over the interface as it is: each step rings one thing on
 * screen and says what it is, with "Back" and "Next". Nothing else can be
 * pressed meanwhile, except the ringed thing itself where the step says so.
 * `onDone` is called when the last step is left, or the tour is skipped.
 */
export function Tour(props: { name: string; steps: TourStep[]; lastLabel: string; onDone: () => void }): JSX.Element {
  const [at, setAt] = createSignal(0);
  const [box, setBox] = createSignal<Box | null>(null);
  const [placed, setPlaced] = createSignal({ top: 0, left: 0 });
  const [shown, setShown] = createSignal(false);
  let card: HTMLElement | undefined;
  let target: Element | null = null;
  let waiting: ReturnType<typeof setInterval> | undefined;

  const step = () => props.steps[at()]!;
  const last = () => at() === props.steps.length - 1;
  const next = () => (last() ? props.onDone() : setAt(at() + 1));
  const back = () => setAt(Math.max(0, at() - 1));

  /** Puts the ring and the card where the thing is right now. */
  const measure = () => {
    const now = target && target.isConnected ? boxOf(target) : null;
    if (!same(now, box())) setBox(now);
    if (!card) return;
    const view = { width: window.innerWidth, height: window.innerHeight };
    const size = { width: card.offsetWidth, height: card.offsetHeight };
    const where = place(now ? ringOf(now) : null, size, view);
    if (where.top !== placed().top || where.left !== placed().left) setPlaced({ top: where.top, left: where.left });
  };

  /** Pressing the ringed thing is the way on, where the step says it may be pressed. */
  const pressed = (event: MouseEvent) => {
    if (step().press && target && event.target instanceof Node && target.contains(event.target)) {
      // The press does what it does (a link goes to its page); the tour only moves on.
      queueMicrotask(next);
    }
  };

  createEffect(
    on(at, () => {
      clearInterval(waiting);
      target = null;
      setShown(false);
      const wanted = step();
      if (wanted.route && location.hash !== wanted.route) go(wanted.route);
      const since = Date.now();
      const look = () => {
        target = document.querySelector(wanted.target);
        // A thing that is not there after a while is talked about without a ring, in the middle.
        if (!target && Date.now() - since < WAIT_FOR_TARGET) return;
        clearInterval(waiting);
        target?.scrollIntoView({ block: 'nearest', inline: 'nearest' });
        setShown(true);
        measure();
        requestAnimationFrame(() => {
          measure();
          card?.querySelector<HTMLElement>('[data-tour-next]')?.focus();
        });
      };
      waiting = setInterval(look, 60);
      look();
    }),
  );

  onMount(() => {
    // The page under the tour moves by itself (lists load, the window is resized): the ring follows.
    const follow = setInterval(measure, 200);
    window.addEventListener('resize', measure);
    window.addEventListener('scroll', measure, true);
    document.addEventListener('click', pressed, true);
    // A press on a shade leaves the keyboard on nothing at all: Escape and Tab still mean the tour.
    const outside = (event: KeyboardEvent) => {
      if (event.defaultPrevented || !card || card.contains(event.target as Node)) return;
      onKeyDown(event);
    };
    document.addEventListener('keydown', outside);
    onCleanup(() => {
      clearInterval(follow);
      clearInterval(waiting);
      window.removeEventListener('resize', measure);
      window.removeEventListener('scroll', measure, true);
      document.removeEventListener('click', pressed, true);
      document.removeEventListener('keydown', outside);
    });
  });

  const onKeyDown = (event: KeyboardEvent) => {
    if (event.key === 'Escape') {
      event.preventDefault();
      props.onDone();
      return;
    }
    if (event.key !== 'Tab' || !card) return;
    // The keyboard stays on the card: what is under the tour cannot be reached while it is up.
    const inside = [...card.querySelectorAll<HTMLElement>('button')];
    const [first, final] = [inside[0], inside[inside.length - 1]];
    if (!first || !final) return;
    const focused = document.activeElement;
    if (event.shiftKey && (focused === first || !card.contains(focused))) {
      event.preventDefault();
      final.focus();
    } else if (!event.shiftKey && (focused === final || !card.contains(focused))) {
      event.preventDefault();
      first.focus();
    }
  };

  /** The four sides around the ring, which take every press that is not meant for the ringed thing. */
  const shades = () => {
    const ring = box() ? ringOf(box()!) : null;
    if (!ring) return [{ top: '0', left: '0', width: '100%', height: '100%' }];
    const px = (value: number) => `${Math.max(0, value)}px`;
    const bottom = ring.top + ring.height;
    return [
      { top: '0', left: '0', width: '100%', height: px(ring.top) },
      { top: px(bottom), left: '0', width: '100%', height: `calc(100% - ${px(bottom)})` },
      { top: px(ring.top), left: '0', width: px(ring.left), height: px(ring.height) },
      {
        top: px(ring.top),
        left: px(ring.left + ring.width),
        width: `calc(100% - ${px(ring.left + ring.width)})`,
        height: px(ring.height),
      },
    ];
  };

  return (
    <div class="tour" classList={{ shown: shown() }}>
      <For each={shades()}>{(shade) => <div class="tour-shade" style={shade} />}</For>
      <Show when={box()}>
        {(found) => {
          const ring = () => ringOf(found());
          return (
            <div
              class="tour-ring"
              classList={{ press: step().press === true }}
              style={{
                top: `${ring().top}px`,
                left: `${ring().left}px`,
                width: `${ring().width}px`,
                height: `${ring().height}px`,
              }}
            />
          );
        }}
      </Show>
      <section
        class="tour-card"
        role="dialog"
        aria-label={`${props.name}: hint ${at() + 1} of ${props.steps.length}`}
        ref={card}
        style={{ top: `${placed().top}px`, left: `${placed().left}px` }}
        onKeyDown={onKeyDown}
      >
        <header class="tour-head">
          <h2 class="tour-title">{step().title}</h2>
          {/* Said to a screen reader by the card's own name; here it is for the eye. */}
          <span class="fine tour-count" aria-hidden="true">
            {at() + 1} of {props.steps.length}
          </span>
        </header>
        <p>{step().text}</p>
        <Show when={step().lines?.length}>
          <ul class="tour-lines">
            <For each={step().lines}>{(line) => <li>{line}</li>}</For>
          </ul>
        </Show>
        <footer class="tour-foot">
          <button type="button" class="btn small ghost" onClick={() => props.onDone()}>
            Skip these hints
          </button>
          <span class="grow" />
          <Show when={at() > 0}>
            <button type="button" class="btn small" onClick={back}>
              Back
            </button>
          </Show>
          <button type="button" class="btn small primary" data-tour-next onClick={next}>
            {last() ? props.lastLabel : 'Next'}
          </button>
        </footer>
      </section>
    </div>
  );
}
