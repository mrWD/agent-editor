import { createEffect, createSignal, For, Show, type JSX } from 'solid-js';
import { go, taskHref } from '../app/router';
import { ago } from '../components/bits';
import { matching, placeOf, type ClaudeSession } from '../core/imports';
import { bringIn, claudeSessions, state } from '../core/store';

/** How many conversations are listed before "Show more". */
const AT_FIRST = 12;

const said = (error: unknown) => (error instanceof Error ? error.message : String(error));

/**
 * Brings a conversation of Claude Code into the editor as a task: the words
 * and tool calls of the conversation, in the folder it worked in. A button on
 * the home screen that opens into a list of the conversations Claude Code had
 * in a project's folder, newest first. One that is a task already is opened,
 * not brought in twice.
 */
export function BringIn(): JSX.Element {
  const [open, setOpen] = createSignal(false);
  const [projectId, setProjectId] = createSignal('');
  const [found, setFound] = createSignal<ClaudeSession[] | null>(null);
  const [typed, setTyped] = createSignal('');
  const [all, setAll] = createSignal(false);
  const [busy, setBusy] = createSignal<string | null>(null);
  const [problem, setProblem] = createSignal<string | null>(null);

  createEffect(() => {
    if (!state.ready) return;
    if (!state.projects.some((project) => project.id === projectId())) setProjectId(state.projects[0]?.id ?? '');
  });

  const project = () => state.projects.find((one) => one.id === projectId());

  // The list is read when the card opens and again for another project. An answer for a project
  // that is no longer the one picked is not shown.
  let asked = 0;
  createEffect(() => {
    const id = projectId();
    if (!open() || !id) return;
    const mine = ++asked;
    setFound(null);
    setProblem(null);
    setAll(false);
    claudeSessions(id)
      .then((sessions) => mine === asked && setFound(sessions))
      .catch((error) => {
        if (mine !== asked) return;
        setFound([]);
        setProblem(said(error));
      });
  });

  const shown = () => matching(found() ?? [], typed());
  const listed = () => (all() ? shown() : shown().slice(0, AT_FIRST));

  const take = async (session: ClaudeSession) => {
    if (busy()) return;
    if (session.taskId) {
      go(taskHref(session.taskId));
      return;
    }
    setBusy(session.id);
    setProblem(null);
    try {
      const id = await bringIn(projectId(), session.id);
      setOpen(false);
      go(taskHref(id));
    } catch (error) {
      setProblem(said(error));
    } finally {
      setBusy(null);
    }
  };

  return (
    <Show
      when={open()}
      fallback={
        <p class="soft setup-offer">
          <button type="button" class="btn small ghost" onClick={() => setOpen(true)}>
            Bring in a conversation from Claude Code
          </button>{' '}
          Go on here with what you began in Claude Code: its words, its tool calls, its folder.
        </p>
      }
    >
      <section class="card setup bring-in" aria-label="Bring in a conversation from Claude Code">
        <h2>Bring in a conversation from Claude Code</h2>
        <Show
          when={state.projects.length > 0}
          fallback={
            <p>
              Conversations are found by the folder they worked in. Add that folder as a project first, then come back
              here.
            </p>
          }
        >
          <p>
            These are the conversations Claude Code had in the project's folder and in folders inside it. One becomes a
            task that sleeps: your next message there wakes Claude with what was said. Claude Code's own files are only
            read.
          </p>
          <div class="setup-row">
            <select
              class="select-button"
              aria-label="Project whose conversations to list"
              value={projectId()}
              onChange={(event) => setProjectId(event.currentTarget.value)}
            >
              <For each={state.projects}>{(one) => <option value={one.id}>{one.name}</option>}</For>
            </select>
            <input
              class="field where"
              type="search"
              aria-label="Find a conversation"
              placeholder="Find by its first words or its branch"
              value={typed()}
              onInput={(event) => setTyped(event.currentTarget.value)}
            />
          </div>
          <Show when={found()} fallback={<p class="fine">Reading the conversations</p>}>
            <Show
              when={shown().length > 0}
              fallback={
                <p class="fine">
                  {(found() ?? []).length === 0
                    ? `Claude Code has no conversation in ${project()?.path ?? 'this folder'}.`
                    : 'No conversation has those words.'}
                </p>
              }
            >
              <ul class="bring-list" aria-label="Conversations of Claude Code">
                <For each={listed()}>
                  {(session) => (
                    <li>
                      <div class="bring-what">
                        <span class="bring-title">{session.title}</span>
                        <span class="fine">
                          {ago(session.lastAt)}
                          {session.branch ? ` · ${session.branch}` : ''}
                          {' · '}
                          {session.folderThere
                            ? placeOf(session, project()?.path ?? '')
                            : 'its folder is gone, so the task gets a working copy of its own'}
                        </span>
                      </div>
                      <button
                        type="button"
                        class="btn small"
                        disabled={busy() !== null}
                        aria-label={`${session.taskId ? 'Open the task of' : 'Bring in'}: ${session.title}`}
                        onClick={() => void take(session)}
                      >
                        {busy() === session.id ? 'Bringing in' : session.taskId ? 'Open its task' : 'Bring in'}
                      </button>
                    </li>
                  )}
                </For>
              </ul>
              <Show when={!all() && shown().length > AT_FIRST}>
                <button type="button" class="btn small ghost" onClick={() => setAll(true)}>
                  Show {shown().length - AT_FIRST} more
                </button>
              </Show>
            </Show>
          </Show>
        </Show>
        <Show when={problem()}>
          <p class="form-error" role="alert">
            {problem()}
          </p>
        </Show>
        <div class="setup-row">
          <span class="grow" />
          <button type="button" class="btn tall ghost" onClick={() => setOpen(false)}>
            Not now
          </button>
        </div>
      </section>
    </Show>
  );
}
