import { createSignal } from 'solid-js';

/**
 * The look of the editor: dark, as it was designed, or light.
 *
 * The choice is "dark", "light", or "system" (whichever the computer is set
 * to, changing with it). It is kept in this browser, not in the core: how a
 * screen should look belongs to the screen. The page's first lines set the
 * theme before anything is drawn (see `index.html`); this file keeps it right
 * afterwards and lets Settings change it.
 */

export type ThemeChoice = 'dark' | 'light' | 'system';
export type Theme = 'dark' | 'light';

/** Where the choice is kept. The page's first lines read the same key. */
export const THEME_KEY = 'ae.theme';

type Media = { matches: boolean };

/** What was chosen, as it is written down. Anything else is "dark": the design's own. */
export function readChoice(written: string | null): ThemeChoice {
  return written === 'light' || written === 'system' ? written : 'dark';
}

/** The theme a choice comes to on a computer that is, or is not, set to light. */
export function resolve(choice: ThemeChoice, systemIsLight: boolean): Theme {
  if (choice === 'system') return systemIsLight ? 'light' : 'dark';
  return choice;
}

function stored(): string | null {
  try {
    return localStorage.getItem(THEME_KEY);
  } catch {
    // Storage may be refused; the editor is then dark, as designed.
    return null;
  }
}

const system: Media & { addEventListener?: (kind: 'change', heard: () => void) => void } =
  typeof matchMedia === 'function' ? matchMedia('(prefers-color-scheme: light)') : { matches: false };

const atStart = readChoice(stored());
const [choice, setChoice] = createSignal<ThemeChoice>(atStart);
const [theme, setTheme] = createSignal<Theme>(resolve(atStart, system.matches));

/** What the user chose. */
export const themeChoice = choice;
/** The theme on screen right now. What draws its own colours (the terminal) follows this. */
export const currentTheme = theme;

function apply() {
  const now = resolve(choice(), system.matches);
  document.documentElement.dataset.theme = now;
  document.querySelector('meta[name="color-scheme"]')?.setAttribute('content', now);
  setTheme(now);
}

/** Chooses a theme, from now on and for the next visits of this browser. */
export function chooseTheme(next: ThemeChoice) {
  setChoice(next);
  try {
    if (next === 'dark') localStorage.removeItem(THEME_KEY);
    else localStorage.setItem(THEME_KEY, next);
  } catch {
    // It holds for this visit at least.
  }
  apply();
}

/** Puts the chosen theme on the page and keeps "system" in step with the computer. Call once, at the start. */
export function startTheme() {
  apply();
  system.addEventListener?.('change', apply);
  // Another window of this browser chose: this one follows.
  window.addEventListener('storage', (event) => {
    if (event.key !== THEME_KEY) return;
    setChoice(readChoice(event.newValue));
    apply();
  });
}

/** The value of a colour of the style sheet as it is right now, such as `--term-bg`. */
export function colourOf(name: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}
