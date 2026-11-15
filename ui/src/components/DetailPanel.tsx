import { createEffect, createSignal, For, on, onCleanup, onMount, Show, type Accessor, type JSX } from 'solid-js';
import { noteAbout, notesWords, peopleLines, pictureFrom, trackerName, type Detail } from '../core/details';
import { recall, remember } from '../core/kept';
import { ago } from './bits';
import { Markdown } from './Markdown';

/** What a panel was opened on: what to call it meanwhile, whose it is, and how the tracker is asked for it. */
export type Opened = {
  /** A name of its own among everything that can be opened: what was read last is kept under it. */
  id: string;
  title: string;
  projectId: string;
  ask: () => Promise<Detail>;
  /** What can be done with it, shown at the foot of the panel. */
  foot?: () => JSX.Element;
};

export type Details = {
  opened: Accessor<Opened | null>;
  detail: Accessor<Detail | null>;
  loading: Accessor<boolean>;
  problem: Accessor<string | null>;
  open: (what: Opened) => void;
  close: () => void;
  again: () => void;
};

/**
 * What a screen needs to show one issue or request at its side: which is open, what the tracker
 * said about it, and whether it is being asked. What was read last time is shown at once and
 * read again behind that; an answer for something that is no longer open is dropped.
 */
export function createDetails(): Details {
  const [opened, setOpened] = createSignal<Opened | null>(null);
  const [detail, setDetail] = createSignal<Detail | null>(null);
  const [loading, setLoading] = createSignal(false);
  const [problem, setProblem] = createSignal<string | null>(null);
  let asked = 0;

  const read = async (what: Opened) => {
    const mine = ++asked;
    setLoading(true);
    setProblem(null);
    try {
      const found = await what.ask();
      remember(`detail:${what.id}`, found);
      if (mine === asked) setDetail(found);
    } catch (error) {
      if (mine === asked) setProblem(error instanceof Error ? error.message : String(error));
    } finally {
      if (mine === asked) setLoading(false);
    }
  };

  return {
    opened,
    detail,
    loading,
    problem,
    open: (what) => {
      setOpened(what);
      setDetail(recall<Detail>(`detail:${what.id}`));
      void read(what);
    },
    close: () => {
      asked += 1;
      setOpened(null);
      setLoading(false);
    },
    again: () => {
      const what = opened();
      if (what) void read(what);
    },
  };
}

/** The panel of whatever a screen has open, or nothing. */
export function DetailSide(props: { details: Details }): JSX.Element {
  return (
    <Show when={props.details.opened()}>
      {(what) => (
        <DetailPanel
          title={what().title}
          detail={props.details.detail()}
          loading={props.details.loading()}
          problem={props.details.problem()}
          projectId={what().projectId}
          onClose={props.details.close}
          onAgain={props.details.again}
        >
          {what().foot?.()}
        </DetailPanel>
      )}
    </Show>
  );
}

/**
 * An issue or a merge request in full, in a panel at the side of the screen:
 * what it says, who is on it, and what was written under it, with the
 * pictures. The list or the card it was opened from stays where it is.
 *
 * `detail` is what the tracker said last; while it is read for the first time
 * `title` stands in. `children` are what can be done with it, such as
 * starting a task. Escape closes the panel.
 */
export function DetailPanel(props: {
  /** What to call it before it is read: its name in the list. */
  title: string;
  detail: Detail | null;
  loading: boolean;
  problem: string | null;
  /** The project it belongs to: its pictures are fetched with that project's sign-in. */
  projectId: string;
  onClose: () => void;
  onAgain: () => void;
  children?: JSX.Element;
}): JSX.Element {
  let closeButton: HTMLButtonElement | undefined;

  onMount(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== 'Escape' || event.defaultPrevented) return;
      event.preventDefault();
      props.onClose();
    };
    document.addEventListener('keydown', onKey);
    onCleanup(() => document.removeEventListener('keydown', onKey));
    closeButton?.focus();
  });
  // Another issue in the same panel starts at its top.
  let body: HTMLDivElement | undefined;
  createEffect(
    on(
      () => props.title,
      () => body?.scrollTo({ top: 0 }),
    ),
  );

  const pictures = () => pictureFrom(props.projectId);

  return (
    <aside class="detail" aria-label={`Details of ${props.detail?.key ?? props.title}`}>
      <div class="detail-head">
        <Show when={props.detail} fallback={<span class="grow fine">{props.loading ? 'Asking the tracker' : ''}</span>}>
          {(detail) => (
            <>
              <a class="issue-key" href={detail().url} target="_blank" rel="noreferrer">
                {detail().key}
              </a>
              <span class="tag">{detail().state}</span>
              <Show when={detail().draft}>
                <span class="tag">draft</span>
              </Show>
              <span class="grow" />
              <a class="btn tiny ghost" href={detail().url} target="_blank" rel="noreferrer">
                Open in {trackerName[detail().source]}
              </a>
            </>
          )}
        </Show>
        <button type="button" class="btn tiny ghost" disabled={props.loading} onClick={() => props.onAgain()}>
          {props.loading ? 'Reading' : 'Read again'}
        </button>
        <button type="button" class="btn tiny" ref={closeButton} onClick={() => props.onClose()}>
          Close
        </button>
      </div>
      <div class="detail-body" ref={body}>
        <h2 class="detail-title">{props.detail?.title ?? props.title}</h2>
        <Show when={props.problem}>
          <p class="banner danger detail-flat" role="alert">
            <span class="grow">{props.problem}</span>
          </p>
        </Show>
        <Show when={props.detail}>
          {(detail) => (
            <>
              <p class="fine detail-people">
                <For each={peopleLines(detail())}>{(line) => <span>{line}</span>}</For>
                <Show when={detail().updatedAt > 0}>
                  <span>Changed {ago(detail().updatedAt)}</span>
                </Show>
              </p>
              <Show when={detail().branch}>
                <p class="fine mono detail-branch">
                  {detail().branch} → {detail().base}
                </p>
              </Show>
              <Show when={detail().labels.length > 0}>
                <p class="detail-labels">
                  <For each={detail().labels}>{(label) => <span class="tag">{label}</span>}</For>
                </p>
              </Show>
              <Show when={detail().body.trim() !== ''} fallback={<p class="soft">It has no description.</p>}>
                <section class="detail-text" aria-label="Description">
                  <Markdown text={detail().body} pictures={pictures()} />
                </section>
              </Show>
              <Show when={detail().links.length > 0}>
                <ul class="detail-links" aria-label="Attached">
                  <For each={detail().links}>
                    {(link) => (
                      <li>
                        <a href={link.url} target="_blank" rel="noreferrer">
                          {link.title || link.url}
                        </a>
                      </li>
                    )}
                  </For>
                </ul>
              </Show>
              <section class="detail-notes" aria-label="Comments">
                <h3 class="detail-sub">{notesWords(detail().notes)}</h3>
                <For each={detail().notes}>
                  {(note) => (
                    <article class="detail-note" classList={{ review: note.kind === 'review' }}>
                      <p class="detail-note-head">
                        <span class="who">{note.author || 'Somebody'}</span>
                        <Show when={noteAbout(note)}>
                          <span class="tag" classList={{ mono: note.kind === 'comment' }}>
                            {noteAbout(note)}
                          </span>
                        </Show>
                        <span class="fine">{ago(note.at)}</span>
                      </p>
                      <Show when={note.body.trim() !== ''}>
                        <Markdown text={note.body} pictures={pictures()} />
                      </Show>
                    </article>
                  )}
                </For>
              </section>
            </>
          )}
        </Show>
      </div>
      <Show when={props.children}>
        <div class="detail-foot">{props.children}</div>
      </Show>
    </aside>
  );
}
