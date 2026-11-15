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

/** For the text being made safe right now: where its pictures are shown from. `null` shows none. */
let picturesThrough: ((src: string) => string) | null = null;

DOMPurify.addHook('afterSanitizeAttributes', (node) => {
  // A picture is never asked of the address the text names: it is shown through the core, or not at all.
  if (node.tagName === 'IMG') {
    const from = node.getAttribute('src') ?? '';
    if (!picturesThrough || from === '') {
      node.remove();
      return;
    }
    node.setAttribute('data-from', from);
    node.setAttribute('src', picturesThrough(from));
    node.setAttribute('loading', 'lazy');
    node.removeAttribute('width');
    node.removeAttribute('height');
    return;
  }
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
 *
 * The text of an issue or a request is the user's own tracker speaking, and
 * its pictures are part of what it says. With `pictures`, each is shown from
 * the address that function gives (the core, which fetches it from the
 * tracker), never from the one the text names.
 */
export function toHtml(text: string, pictures?: (src: string) => string): string {
  const html = marked.parse(text, { async: false });
  const without = ['picture', 'video', 'audio', 'style', 'form', 'input', 'button', 'textarea', 'select'];
  picturesThrough = pictures ?? null;
  try {
    return DOMPurify.sanitize(html, {
      USE_PROFILES: { html: true },
      FORBID_TAGS: pictures ? without : ['img', ...without],
      FORBID_ATTR: ['style', 'srcset'],
    });
  } finally {
    picturesThrough = null;
  }
}

/**
 * A text in Markdown, shown as it is meant. Files it names open in the editor. With `pictures`
 * its pictures are shown too, from where that function says; one that does not come becomes a
 * link to where the text said it is.
 */
export function Markdown(props: {
  text: string;
  onFile?: (path: string, line?: number) => void;
  pictures?: (src: string) => string;
}): JSX.Element {
  const html = createMemo(() => toHtml(props.text, props.pictures));

  // A picture the core could not fetch: what stands in its place says so and links to the tracker's own.
  const onLost = (event: Event) => {
    const picture = event.target as HTMLElement;
    if (picture.tagName !== 'IMG') return;
    const from = picture.getAttribute('data-from') ?? '';
    const link = document.createElement(/^https?:/i.test(from) ? 'a' : 'span');
    link.className = 'md-lost';
    link.textContent = `Picture not shown: ${picture.getAttribute('alt') || from}`;
    if (link instanceof HTMLAnchorElement) {
      link.href = from;
      link.target = '_blank';
      link.rel = 'noreferrer noopener';
    }
    picture.replaceWith(link);
  };

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

  return (
    <div
      class="md"
      // The HTML comes from `toHtml`, which takes out everything that could run.
      // eslint-disable-next-line solid/no-innerhtml
      innerHTML={html()}
      onClick={onClick}
      role="presentation"
      // The loss of a picture does not rise through the page like a press does: it is caught on its way down.
      ref={(el) => el.addEventListener('error', onLost, true)}
    />
  );
}
