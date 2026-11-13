import { Show, type JSX } from 'solid-js';
import type { LspSession } from '../editor/lsp';

/**
 * A few quiet words next to the language of the file on screen: whether a
 * language server stands behind it. Nothing is shown for a file of a language
 * no server is known for.
 */
export function LspBadge(props: { lsp: LspSession; path: string }): JSX.Element {
  const status = () => props.lsp.statusOf(props.path);
  const words = () => {
    switch (status()?.state) {
      case 'running':
        return 'Language server on';
      case 'missing':
        return 'No language server';
      case 'failed':
        return 'Language server did not start';
      case 'stopped':
        return 'Language server stopped';
      default:
        return null;
    }
  };
  return (
    <Show when={words()}>
      <span class="tab-note lsp-state" title={status()?.hint}>
        {words()}
      </span>
    </Show>
  );
}

/**
 * What the status line under the code adds for language servers: whether
 * saving formats the file, and the one thing the user should hear, such as
 * how to install a server that is missing.
 */
export function LspStatus(props: { lsp: LspSession; path: string }): JSX.Element {
  const status = () => props.lsp.statusOf(props.path);
  const formats = () => props.lsp.formatOnSave();
  return (
    <>
      <Show when={status()?.state === 'running' && status()?.formats}>
        <button
          type="button"
          class="lsp-toggle"
          aria-pressed={formats()}
          title={
            formats()
              ? 'Saving tidies the file first. Press to turn that off for this project.'
              : 'Saving leaves the file as typed. Press to let saving tidy it, for this project.'
          }
          onClick={() => void props.lsp.setFormatOnSave(!formats())}
        >
          Format on save: {formats() ? 'on' : 'off'}
        </button>
      </Show>
      <Show when={props.lsp.note()}>
        {(note) => (
          <span class="lsp-note" role="status">
            <span>{note().text}</span>
            <Show when={note().command}>
              <code>{note().command}</code>
            </Show>
            <button type="button" class="lsp-hide" onClick={() => props.lsp.hideNote()}>
              Hide this note
            </button>
          </span>
        )}
      </Show>
    </>
  );
}
