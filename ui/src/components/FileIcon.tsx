import { Match, Switch, type JSX } from 'solid-js';
import { iconOf } from '../core/fileIcons';

/**
 * The icon in front of a file or a folder in the tree.
 *
 * It is drawn, not read aloud: every row already says its name, and a folder
 * says whether it is open. The shape tells code from data, words, a picture;
 * the colour tells the language. Both come from the name alone.
 */
export function FileIcon(props: { path: string; isDir?: boolean; open?: boolean }): JSX.Element {
  const icon = () => iconOf(props.path, Boolean(props.isDir), Boolean(props.open));
  const shape = () => icon().shape;
  return (
    // The tooltip sits on a wrapper, not inside the drawing: an SVG <title> would count as the row's text,
    // and every row already reads as its file's name.
    <span class="file-icon-wrap" aria-hidden="true" title={icon().what}>
      <svg
        class="file-icon"
        data-tone={icon().tone}
        width="14"
        height="14"
        viewBox="0 0 16 16"
        fill="none"
        stroke="currentColor"
        stroke-width="1.3"
        stroke-linecap="round"
        stroke-linejoin="round"
        aria-hidden="true"
      >
        <Switch>
          <Match when={shape() === 'folder'}>
            <path d="M2 4.5a1 1 0 0 1 1-1h3L7.5 5H13a1 1 0 0 1 1 1v6a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1z" />
          </Match>
          <Match when={shape() === 'folderOpen'}>
            <path d="M2 12.5V4.5a1 1 0 0 1 1-1h3L7.5 5H13a1 1 0 0 1 1 1v1" />
            <path d="M2.2 12.8 4 7.8a1 1 0 0 1 .95-.8H15l-1.9 5.2a1 1 0 0 1-.95.8H3a1 1 0 0 1-.8-.4z" />
          </Match>
          <Match when={shape() === 'lock'}>
            <rect x="3.5" y="7" width="9" height="6.5" rx="1.2" />
            <path d="M5.75 7V5.25a2.25 2.25 0 0 1 4.5 0V7" />
          </Match>
          <Match when={shape() === 'git'}>
            <circle cx="4.5" cy="4" r="1.6" />
            <circle cx="4.5" cy="12" r="1.6" />
            <circle cx="11.5" cy="7.5" r="1.6" />
            <path d="M4.5 5.6v4.8M6.1 7.5h2.3a1.5 1.5 0 0 0 1.5-1.5v-.1" />
          </Match>
          <Match when={shape() === 'image'}>
            <rect x="2.5" y="3.5" width="11" height="9" rx="1.2" />
            <circle cx="6" cy="6.5" r="1" />
            <path d="M3 11.2 6 8.6l2.2 1.8L10.4 8l2.6 2.4" />
          </Match>
          <Match when={shape() === 'shell'}>
            <rect x="2.5" y="3" width="11" height="10" rx="1.2" />
            <path d="M5 6.5 7 8.2 5 9.9M8.6 10.2h2.6" />
          </Match>
          {/* Everything else is a page, with a mark inside that says what is written on it. */}
          <Match when={true}>
            <path d="M4 2.5h4.6L12 6v7.5H4z" />
            <path d="M8.4 2.6V6H12" />
            <Switch>
              <Match when={shape() === 'code'}>
                <path d="M6.6 8.6 5.4 9.8l1.2 1.2M9.4 8.6l1.2 1.2-1.2 1.2" />
              </Match>
              <Match when={shape() === 'data'}>
                <path d="M5.6 8.7h4.8M5.6 10.6h3.2" />
                <circle cx="5.6" cy="8.7" r="0.1" />
              </Match>
              <Match when={shape() === 'words'}>
                <path d="M5.6 8.4h4.8M5.6 10.1h4.8M5.6 11.8h3" />
              </Match>
            </Switch>
          </Match>
        </Switch>
      </svg>
    </span>
  );
}
