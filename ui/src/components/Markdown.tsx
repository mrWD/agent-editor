import DOMPurify from 'dompurify';
import { Marked } from 'marked';
import { createMemo, type JSX } from 'solid-js';

/** Endings that make a bare name, with no folder in front, read as a file. */
const KNOWN =
  'md|txt|json|jsonc|toml|yaml|yml|lock|ts|tsx|js|jsx|mjs|cjs|css|scss|html|vue|svelte|rs|go|py|rb|java|kt|swift|c|h|cpp|cs|php|sh|sql|xml|env|ini|conf|gradle';
const FILE = new RegExp(
  String.raw`^(?:\./)?((?:[\w@.-]+/)+[\w@.-]*\w|[\w@-][\w@.-]*\.(?:${KNOWN}))(?::(\d+))?(?::\d+)?$`,
);

/** A file an agent named in its answer: the path from the task's folder and, if given, the line. */
export function fileIn(text: string): { path: string; line?: number } | null {
  const found = FILE.exec(text.trim());
  if (!found) return null;
  return { path: found[1], line: found[2] ? Number(found[2]) : undefined };
}

const escape = (text: string) =>
  text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');

const marked = new Marked({
  gfm: true,
  breaks: true,
  renderer: {
    // A file named in backticks becomes a link that opens it in the editor.
    codespan({ text }) {
      const file = fileIn(text.replace(/&lt;|&gt;|&amp;|&quot;/g, ''));
      const code = `<code>${text}</code>`;
      if (!file) return code;
      const where = file.line ? `${file.path}:${file.line}` : file.path;
      return `<a class="file-link" href="${escape(where)}">${code}</a>`;
    },
  },
});

DOMPurify.addHook('afterSanitizeAttributes', (node) => {
  if (node.tagName !== 'A') return;
  const href = node.getAttribute('href') ?? '';
  // A link out of the app opens in the browser and learns nothing about where it was clicked.
  if (/^[a-z][a-z0-9+.-]*:/i.test(href)) {
    node.setAttribute('target', '_blank');
    node.setAttribute('rel', 'noreferrer noopener');
  }
});

/**
 * Markdown as safe HTML. What an agent writes is not trusted: scripts, styles,
 * forms and pictures are taken out. A picture would be fetched from wherever
 * the text says, which tells that place the text was read.
 */
export function toHtml(text: string): string {
  const html = marked.parse(text, { async: false });
  return DOMPurify.sanitize(html, {
    USE_PROFILES: { html: true },
    FORBID_TAGS: ['img', 'picture', 'video', 'audio', 'style', 'form', 'input', 'button', 'textarea', 'select'],
    FORBID_ATTR: ['style', 'srcset'],
  });
}

/** An agent's message, with its Markdown shown as it is meant. Files it names open in the editor. */
export function Markdown(props: { text: string; onFile?: (path: string, line?: number) => void }): JSX.Element {
  const html = createMemo(() => toHtml(props.text));

  const onClick = (event: MouseEvent) => {
    const link = (event.target as HTMLElement).closest('a');
    if (!link) return;
    const href = link.getAttribute('href') ?? '';
    if (/^[a-z][a-z0-9+.-]*:/i.test(href)) return;
    // Anything else would take the whole app to another address.
    event.preventDefault();
    let where = href;
    try {
      where = decodeURI(href);
    } catch {
      // Not a well-formed address: take it as it is written.
    }
    const file = fileIn(where);
    if (file) props.onFile?.(file.path, file.line);
  };

  // The HTML comes from `toHtml`, which takes out everything that could run.
  // eslint-disable-next-line solid/no-innerhtml
  return <div class="md" innerHTML={html()} onClick={onClick} role="presentation" />;
}
