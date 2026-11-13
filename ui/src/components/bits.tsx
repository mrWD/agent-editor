import { createSignal, type JSX } from 'solid-js';
import { stageOf, stageWord, type Stage } from '../core/stages';
import type { TaskState } from '../core/store';

export { stageOf, type Stage };

/** The key that goes with S, K and friends on this machine. */
export const modKey = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform) ? '⌘' : 'Ctrl+';

/** A clock that moves twice a minute, so "4 min ago" stays true without a timer per row. */
const [now, setNow] = createSignal(Date.now());
setInterval(() => setNow(Date.now()), 30_000);

/** How long ago `at` was, in plain words. */
export function ago(at: number): string {
  const seconds = Math.max(0, Math.round((now() - at) / 1000));
  if (seconds < 45) return 'just now';
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes} min ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours} h ago`;
  const days = Math.round(hours / 24);
  return days === 1 ? 'yesterday' : `${days} days ago`;
}

const two = (n: number) => String(n).padStart(2, '0');

/** A time of day, as "14:02". */
export function clock(at: number): string {
  const date = new Date(at);
  return `${two(date.getHours())}:${two(date.getMinutes())}`;
}

/** A moment in plain words: "today, 14:02", "yesterday, 18:40", "12 Nov, 09:15". */
export function whenLabel(at: number): string {
  const date = new Date(at);
  const startOf = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const days = Math.round((startOf(new Date(now())) - startOf(date)) / 86_400_000);
  if (days === 0) return `today, ${clock(at)}`;
  if (days === 1) return `yesterday, ${clock(at)}`;
  const month = date.toLocaleString('en-US', { month: 'short' });
  return `${date.getDate()} ${month}, ${clock(at)}`;
}

/** A length of time in plain words: "38 min", "3 h", "2 days". */
export function span(ms: number): string {
  const minutes = Math.max(1, Math.round(ms / 60_000));
  if (minutes < 60) return `${minutes} min`;
  const hours = Math.round(minutes / 60);
  if (hours < 48) return `${hours} h`;
  return `${Math.round(hours / 24)} days`;
}

/** The two-letter mark of whoever wrote or did something. */
export function Chip(props: { agentId: string; short: string; flat?: boolean; label?: string }): JSX.Element {
  const tone = () => (['claude', 'codex', 'gemini', 'you'].includes(props.agentId) ? props.agentId : '');
  return (
    <span
      class={`chip ${tone()} ${props.flat ? 'flat' : ''}`}
      role={props.label ? 'img' : undefined}
      aria-label={props.label}
      aria-hidden={props.label ? undefined : 'true'}
    >
      {props.short}
    </span>
  );
}

/** What a task's stage is called when only its state is known. */
export function stateLabel(state: TaskState): string {
  return stageWord(stageOf(state));
}

/** The 12-pixel mark of a task's stage. Shape and colour both carry the meaning. */
export function StageGlyph(props: { stage: Stage }): JSX.Element {
  return (
    <svg class="glyph" width="12" height="12" viewBox="0 0 12 12" aria-hidden="true">
      {props.stage === 'needs' && <circle cx="6" cy="6" r="5" fill="var(--accent)" />}
      {props.stage === 'ready' && (
        <path
          d="M2.5 6.5l2.5 2.5 4.5-5"
          fill="none"
          stroke="var(--green)"
          stroke-width="1.8"
          stroke-linecap="round"
          stroke-linejoin="round"
        />
      )}
      {props.stage === 'checks' && (
        <>
          <circle cx="6" cy="6" r="4.5" fill="none" stroke="var(--text-2)" stroke-width="1.5" />
          <circle cx="6" cy="6" r="1.8" fill="var(--text-2)" />
        </>
      )}
      {props.stage === 'merged' && (
        <rect x="2" y="2" width="8" height="8" rx="1.5" fill="none" stroke="var(--text-3)" stroke-width="1.5" />
      )}
      {props.stage === 'turn' && <circle cx="6" cy="6" r="4.5" fill="none" stroke="var(--text)" stroke-width="1.5" />}
      {props.stage === 'running' && <circle cx="6" cy="6" r="2.5" fill="var(--text-3)" />}
    </svg>
  );
}

export function StateGlyph(props: { state: TaskState }): JSX.Element {
  return <StageGlyph stage={stageOf(props.state)} />;
}

export function Check(): JSX.Element {
  return (
    <svg class="glyph" width="12" height="12" viewBox="0 0 12 12" aria-hidden="true">
      <path
        d="M2.5 6.5l2.5 2.5 4.5-5"
        fill="none"
        stroke="var(--green)"
        stroke-width="1.8"
        stroke-linecap="round"
        stroke-linejoin="round"
      />
    </svg>
  );
}

export function Ring(): JSX.Element {
  return (
    <svg class="glyph" width="12" height="12" viewBox="0 0 12 12" aria-hidden="true">
      <circle cx="6" cy="6" r="4.5" fill="none" stroke="var(--text-2)" stroke-width="1.5" />
    </svg>
  );
}

export function RingDot(): JSX.Element {
  return (
    <svg class="glyph" width="12" height="12" viewBox="0 0 12 12" aria-hidden="true">
      <circle cx="6" cy="6" r="4.5" fill="none" stroke="var(--text-2)" stroke-width="1.5" />
      <circle cx="6" cy="6" r="1.8" fill="var(--text-2)" />
    </svg>
  );
}

const stroke = {
  fill: 'none',
  stroke: 'currentColor',
  'stroke-linecap': 'round',
  'stroke-linejoin': 'round',
} as const;

export const Icon = {
  Inbox: () => (
    <svg width="16" height="16" viewBox="0 0 16 16" {...stroke} stroke-width="1.5" aria-hidden="true">
      <path d="M2 9l2-6h8l2 6v4H2V9z" />
      <path d="M2 9h4l1 2h2l1-2h4" />
    </svg>
  ),
  Plus: () => (
    <svg width="14" height="14" viewBox="0 0 16 16" {...stroke} stroke-width="1.6" aria-hidden="true">
      <path d="M8 3v10M3 8h10" />
    </svg>
  ),
  Search: () => (
    <svg width="14" height="14" viewBox="0 0 16 16" {...stroke} stroke-width="1.6" aria-hidden="true">
      <circle cx="7" cy="7" r="4.5" />
      <path d="M10.5 10.5L14 14" />
    </svg>
  ),
  Settings: () => (
    <svg width="16" height="16" viewBox="0 0 16 16" {...stroke} stroke-width="1.5" aria-hidden="true">
      <path d="M3 5h10M3 11h10" />
      <circle cx="6" cy="5" r="1.5" />
      <circle cx="10" cy="11" r="1.5" />
    </svg>
  ),
  Close: () => (
    <svg width="10" height="10" viewBox="0 0 10 10" {...stroke} stroke-width="1.5" aria-hidden="true">
      <path d="M2 2l6 6M8 2l-6 6" />
    </svg>
  ),
  ChevronUp: () => (
    <svg width="14" height="14" viewBox="0 0 16 16" {...stroke} stroke-width="1.5" aria-hidden="true">
      <path d="M4 10l4-4 4 4" />
    </svg>
  ),
};
