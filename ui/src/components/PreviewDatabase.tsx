import { createEffect, createSignal, For, on, onCleanup, onMount, Show, type JSX } from 'solid-js';
import { core } from '../core/client';
import {
  dangers,
  failedStep,
  migrationWords,
  NOTHING,
  pickDatabase,
  rowsForAgent,
  selectAll,
  trialForAgent,
  trialVerdict,
  type DbRows,
  type DbState,
  type DbTable,
  type Trial,
} from '../core/database';
import { tookWords } from '../core/api';
import { createKept } from '../core/kept';
import { addShown } from '../core/previewState';
import { Marked, SETTLE } from './PreviewApi';

const said = (error: unknown) => (error instanceof Error ? error.message : String(error));

/**
 * The Preview of what a task does to a database: the migrations it added or
 * changed, read step by step; a trial that runs them, takes them back and
 * runs them again on the task's own database; the tables; and a query.
 */
export function DatabaseView(props: { taskId: string }): JSX.Element {
  // eslint-disable-next-line solid/reactivity -- made anew for each task
  const taskId = props.taskId;
  const [db, setDb] = createKept<DbState | null>(`task:${taskId}:db.state`, null);
  const [picked, setPicked] = createKept<string>(`task:${taskId}:db.picked`, '');
  const [tables, setTables] = createKept<DbTable[] | null>(`task:${taskId}:db.tables`, null);
  const [sql, setSql] = createKept<string>(`task:${taskId}:db.sql`, '');
  /** What the last query gave, with the database it was asked of: rows of one are not shown under another. */
  const [asked, setAsked] = createKept<{ database: string; sql: string; rows: DbRows } | null>(
    `task:${taskId}:db.rows`,
    null,
  );
  const rows = () => (asked() && asked()!.database === database()?.id ? asked() : null);
  const [trial, setTrial] = createKept<Trial | null>(`task:${taskId}:db.trial`, null);
  const [problem, setProblem] = createSignal<string | null>(null);
  const [tablesProblem, setTablesProblem] = createSignal<string | null>(null);
  const [queryProblem, setQueryProblem] = createSignal<string | null>(null);
  const [write, setWrite] = createSignal(false);
  const [busy, setBusy] = createSignal<'query' | 'trial' | 'save' | null>(null);
  /** The trial is asked for and waits for a second press: it changes the database. */
  const [armed, setArmed] = createSignal(false);
  const [setting, setSetting] = createSignal(false);
  const [handed, setHanded] = createSignal<string | null>(null);

  const database = () => pickDatabase(db()?.databases ?? [], picked());

  /** Counts the readings of the tables: an answer that comes after a newer question is dropped. */
  let tablesAsked = 0;
  const loadTables = async () => {
    const now = database();
    const mine = ++tablesAsked;
    if (!now?.ready) {
      setTables(null);
      setTablesProblem(null);
      return;
    }
    try {
      const found = await core.request<{ tables: DbTable[] }>('db.schema', { taskId, database: now.id });
      if (mine !== tablesAsked) return;
      setTables(found.tables);
      setTablesProblem(null);
    } catch (error) {
      if (mine === tablesAsked) setTablesProblem(said(error));
    }
  };

  // One reading at a time; one asked for meanwhile follows it.
  let reading = false;
  let again = false;
  /** `withTables` is false for a change in the files: a migration is a file, the tables are not. */
  const load = async (withTables = true) => {
    if (reading) {
      again = true;
      return;
    }
    reading = true;
    try {
      const before = database()?.id;
      const ready = database()?.ready;
      setDb(await core.request<DbState>('db.get', { taskId }));
      setProblem(null);
      if (withTables || database()?.id !== before || database()?.ready !== ready) await loadTables();
    } catch (error) {
      setProblem(said(error));
    } finally {
      reading = false;
      if (again) {
        again = false;
        void load();
      }
    }
  };

  onMount(() => {
    void load();
    let soon: ReturnType<typeof setTimeout> | undefined;
    const stop = core.onEvent((event) => {
      if (event.event === 'link.open') void load();
      if (event.data?.taskId !== taskId) return;
      // A migration is a file of the task; a database comes and goes with what the task runs.
      const files = ['files.changed', 'review.changed', 'git.changed'].includes(event.event);
      if (files || event.event === 'run.changed') {
        clearTimeout(soon);
        soon = setTimeout(() => void load(!files), SETTLE);
      }
    });
    onCleanup(() => {
      stop();
      clearTimeout(soon);
    });
  });
  // Another database picked: its tables are read, and what was read of the other one is put away.
  createEffect(
    on(
      picked,
      () => {
        setTables(null);
        setQueryProblem(null);
        void loadTables();
      },
      { defer: true },
    ),
  );

  const run = async (text = sql()) => {
    const now = database();
    if (busy() || !now || text.trim() === '') return;
    setBusy('query');
    setQueryProblem(null);
    setHanded(null);
    try {
      const got = await core.request<DbRows>('db.query', { taskId, database: now.id, sql: text, write: write() });
      setAsked({ database: now.id, sql: text, rows: got });
      // What changed rows may have changed the tables too.
      if (write()) void loadTables();
    } catch (error) {
      setQueryProblem(said(error));
    } finally {
      setBusy(null);
    }
  };

  const tryThem = async () => {
    if (busy()) return;
    setArmed(false);
    setBusy('trial');
    setProblem(null);
    setHanded(null);
    try {
      setTrial(await core.request<Trial>('db.try', { taskId, database: database()?.id }));
      await loadTables();
    } catch (error) {
      setProblem(said(error));
    } finally {
      setBusy(null);
    }
  };

  const show = (made: { title: string; text: string }) => {
    addShown(taskId, made.title, made.text);
    setHanded(made.title);
  };

  // ----- the commands of the project -----

  const [upLine, setUpLine] = createSignal('');
  const [downLine, setDownLine] = createSignal('');
  const openSettings = () => {
    setUpLine(db()?.settings.up ?? '');
    setDownLine(db()?.settings.down ?? '');
    setSetting(true);
  };
  const saveSettings = async () => {
    setBusy('save');
    setProblem(null);
    try {
      await core.request('db.set', { taskId, up: upLine(), down: downLine() });
      setSetting(false);
      await load();
    } catch (error) {
      setProblem(said(error));
    } finally {
      setBusy(null);
    }
  };

  return (
    <div class="api db">
      <div class="preview-bar">
        <Show
          when={(db()?.databases.length ?? 0) > 0}
          fallback={
            <span class="fine">
              {db() ? 'No database of this task was found.' : 'Looking for the databases of this task'}
            </span>
          }
        >
          <label class="fine" for={`db-pick-${taskId}`}>
            Database
          </label>
          <select
            id={`db-pick-${taskId}`}
            class="issue-place"
            value={database()?.id ?? ''}
            onChange={(event) => setPicked(event.currentTarget.value)}
          >
            <For each={db()?.databases ?? []}>{(one) => <option value={one.id}>{one.name}</option>}</For>
          </select>
          <Show when={database()?.note}>
            <span class="fine">{database()!.note}</span>
          </Show>
          <Show when={database()?.shared}>
            <span
              class="tag accent"
              title="The compose file names its volume itself, or its address is not on this computer"
            >
              other tasks may use it
            </span>
          </Show>
        </Show>
        <span class="grow" />
        <button
          type="button"
          class="btn small ghost"
          aria-expanded={setting()}
          onClick={() => (setting() ? setSetting(false) : openSettings())}
        >
          Set up
        </button>
      </div>
      <Show when={setting()}>
        <form
          class="api-setup"
          aria-label="How the migrations are run"
          onSubmit={(event) => {
            event.preventDefault();
            void saveSettings();
          }}
        >
          <label for={`db-up-${taskId}`}>Run the migrations</label>
          <input
            id={`db-up-${taskId}`}
            class="field mono"
            spellcheck={false}
            placeholder={db()?.tool?.up || 'a command, run in the folder of the task with its environment'}
            value={upLine()}
            onInput={(event) => setUpLine(event.currentTarget.value)}
          />
          <label for={`db-down-${taskId}`}>Take them back</label>
          <input
            id={`db-down-${taskId}`}
            class="field mono"
            spellcheck={false}
            placeholder={db()?.tool?.down.replace('{n}', '1') || 'a command that undoes what the one above did'}
            value={downLine()}
            onInput={(event) => setDownLine(event.currentTarget.value)}
          />
          <span />
          <span class="api-setup-end">
            <button type="submit" class="btn small" disabled={busy() !== null}>
              Save
            </button>
            <button type="button" class="btn small ghost" onClick={() => setSetting(false)}>
              Cancel
            </button>
            <span class="fine">Kept for the project. Leave one empty to use what the project's own tool gives.</span>
          </span>
        </form>
      </Show>
      <Show when={problem()}>
        <p class="banner danger flush" role="alert">
          <span class="grow">{problem()}</span>
          <button type="button" class="btn tiny" onClick={() => setProblem(null)}>
            Dismiss
          </button>
        </p>
      </Show>
      <Show when={db()} fallback={<p class="bottom-note">Reading the migrations of this task</p>}>
        {(now) => (
          <div class="db-body">
            <section class="db-part" aria-label="Migrations of this task">
              <div class="db-head">
                <h2 class="api-title">
                  Migrations
                  <span class="fine">
                    {' '}
                    {now().tool ? `${now().tool!.name}: ` : ''}
                    {migrationWords(now().migrations)}
                  </span>
                </h2>
                <Show when={dangers(now().migrations) > 0}>
                  <span class="tag" data-level="breaking">
                    {dangers(now().migrations)} to look at twice
                  </span>
                </Show>
                <span class="grow" />
                <Show when={now().up !== ''}>
                  <Show
                    when={armed()}
                    fallback={
                      <button type="button" class="btn small" disabled={busy() !== null} onClick={() => setArmed(true)}>
                        {busy() === 'trial' ? 'Trying the migrations' : 'Try them'}
                      </button>
                    }
                  >
                    <button type="button" class="btn small" onClick={() => void tryThem()}>
                      Run the trial
                    </button>
                    <button type="button" class="btn small ghost" onClick={() => setArmed(false)}>
                      Cancel
                    </button>
                  </Show>
                </Show>
              </div>
              <Show when={armed()}>
                <p class="banner warn db-flat" role="status">
                  <span class="grow">
                    The trial runs <code class="mono api-name">{now().up}</code>
                    <Show
                      when={now().down !== ''}
                      fallback=". No command takes the migrations back, so that is not tried."
                    >
                      , takes it back with <code class="mono api-name">{now().down}</code> and runs it again
                    </Show>
                    . The commands run with the task's environment, on the database the project's own settings point at
                    <Show when={database()}>; the tables are looked at in {database()!.name}</Show>. Rows the migrations
                    drop are lost.
                  </span>
                </p>
              </Show>
              <Show when={now().note}>
                <p class="soft">{now().note}</p>
              </Show>
              <Show when={!now().tool && now().up === ''}>
                <p class="soft">
                  No migration tool was found in this project. Under Set up, give the command that runs its migrations
                  to try them here.
                </p>
              </Show>
              <ul class="db-migrations">
                <For each={now().migrations}>
                  {(one) => (
                    <li>
                      <div class="db-file">
                        <span class="mono">{one.path}</span>
                        <span
                          class="tag"
                          classList={{ good: one.change === 'added', accent: one.change === 'modified' }}
                        >
                          {one.change === 'modified' ? 'changed' : one.change === 'deleted' ? 'removed' : 'new'}
                        </span>
                        <Show when={one.wayBack === false}>
                          <span class="tag" data-level="careful">
                            no way back is written
                          </span>
                        </Show>
                      </div>
                      <Show when={one.change === 'modified'}>
                        <p class="fine">
                          A migration that has run somewhere will not run there again: a change to it reaches only new
                          databases.
                        </p>
                      </Show>
                      <ul class="db-steps">
                        <For each={one.steps}>
                          {(step) => (
                            <li classList={{ danger: step.danger !== null }}>
                              <Marked text={step.text} />
                              <Show when={step.danger}>
                                <span class="fine"> {step.danger}</span>
                              </Show>
                            </li>
                          )}
                        </For>
                      </ul>
                      <Show when={one.steps.length === 0 && one.change !== 'deleted'}>
                        <p class="fine">Nothing in it could be read as a change to the tables.</p>
                      </Show>
                    </li>
                  )}
                </For>
              </ul>
              <Show when={trial()}>
                {(tried) => (
                  <section class="db-trial" aria-label="The trial of the migrations">
                    <div class="db-head">
                      <strong data-tone={trialVerdict(tried()).tone}>{trialVerdict(tried()).text}</strong>
                      <span class="grow" />
                      <Show when={handed() === trialForAgent(tried()).title}>
                        <span class="fine">It goes along with your next message.</span>
                      </Show>
                      <button type="button" class="btn small" onClick={() => show(trialForAgent(tried()))}>
                        Show it to the agent
                      </button>
                    </div>
                    <ol class="db-ran">
                      <For each={tried().steps}>
                        {(step) => (
                          <li>
                            <span class="tag mono" data-tone={step.ok ? 'good' : 'failed'}>
                              {step.ok ? 'ok' : 'failed'}
                            </span>
                            <span>{step.name}</span>
                            <code class="mono fine">{step.command}</code>
                            <span class="fine">{tookWords(step.took)}</span>
                            <Show when={step.output}>
                              <details class="api-more" open={!step.ok}>
                                <summary>What it printed</summary>
                                <pre class="api-body mono">{step.output}</pre>
                              </details>
                            </Show>
                          </li>
                        )}
                      </For>
                    </ol>
                    <Show when={tried().does && !failedStep(tried())}>
                      <h3 class="api-level">What the migrations did to the tables</h3>
                      <Show
                        when={tried().does!.length > 0}
                        fallback={<p class="soft">Nothing: the tables are the same before and after.</p>}
                      >
                        <ul class="db-steps">
                          <For each={tried().does!}>
                            {(line) => (
                              <li>
                                <Marked text={line} />
                              </li>
                            )}
                          </For>
                        </ul>
                      </Show>
                    </Show>
                    <Show when={tried().left.length > 0}>
                      <h3 class="api-level" data-level="breaking">
                        What taking them back left behind
                      </h3>
                      <ul class="db-steps">
                        <For each={tried().left}>
                          {(line) => (
                            <li class="danger">
                              <Marked text={line} />
                            </li>
                          )}
                        </For>
                      </ul>
                    </Show>
                  </section>
                )}
              </Show>
            </section>

            <Show when={database()}>
              <section class="db-part" aria-label="Tables">
                <h2 class="api-title">
                  Tables
                  <Show when={tables()}>
                    <span class="fine"> {tables()!.length}</span>
                  </Show>
                </h2>
                <Show when={tablesProblem()}>
                  <p class="soft">{tablesProblem()}</p>
                </Show>
                <Show when={!database()!.ready && !tablesProblem()}>
                  <p class="soft">{database()!.note ?? 'It cannot be asked right now.'}</p>
                </Show>
                <Show when={tables()?.length === 0}>
                  <p class="soft">It has no tables yet.</p>
                </Show>
                <For each={(tables() ?? []).map((table) => table.name)}>
                  {(name) => {
                    const table = () => tables()?.find((one) => one.name === name);
                    return (
                      <details class="db-table">
                        <summary>
                          <span class="mono">{name}</span>
                          <span class="fine"> {table()?.columns.length ?? 0} columns</span>
                          <button
                            type="button"
                            class="btn tiny ghost"
                            aria-label={`Look at the rows of ${name}`}
                            disabled={busy() !== null}
                            onClick={(event) => {
                              event.preventDefault();
                              const text = selectAll(database()!.engine, name);
                              setSql(text);
                              void run(text);
                            }}
                          >
                            Rows
                          </button>
                        </summary>
                        <table class="db-grid">
                          <tbody>
                            <For each={table()?.columns ?? []}>
                              {(column) => (
                                <tr>
                                  <td class="mono">{column.name}</td>
                                  <td class="mono fine">{column.kind}</td>
                                  <td class="fine">{column.nullable ? 'may be empty' : 'never empty'}</td>
                                  <td class="mono fine">{column.default}</td>
                                </tr>
                              )}
                            </For>
                          </tbody>
                        </table>
                        <For each={table()?.indexes ?? []}>
                          {(index) => (
                            <p class="fine">
                              {index.unique ? 'Unique index' : 'Index'} <span class="mono">{index.name}</span> on{' '}
                              <span class="mono">{index.on}</span>
                            </p>
                          )}
                        </For>
                      </details>
                    );
                  }}
                </For>
              </section>

              <section class="db-part" aria-label="Query">
                <h2 class="api-title">Query</h2>
                <label class="sr-only" for={`db-sql-${taskId}`}>
                  A query in SQL
                </label>
                <textarea
                  id={`db-sql-${taskId}`}
                  class="api-text mono"
                  rows={Math.min(10, Math.max(3, sql().split('\n').length + 1))}
                  spellcheck={false}
                  placeholder="select * from orders limit 50"
                  value={sql()}
                  onInput={(event) => setSql(event.currentTarget.value)}
                  onKeyDown={(event) => {
                    if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) void run();
                  }}
                />
                <div class="api-send">
                  <button
                    type="button"
                    class="btn"
                    disabled={busy() !== null || sql().trim() === '' || !database()!.ready}
                    onClick={() => void run()}
                  >
                    {busy() === 'query' ? 'Asking' : 'Run'}
                  </button>
                  <label class="db-write">
                    <input
                      type="checkbox"
                      checked={write()}
                      onChange={(event) => setWrite(event.currentTarget.checked)}
                    />
                    Allow changes
                  </label>
                  <span class="fine">
                    {write() ? 'The query may change and delete rows.' : 'The query is run for reading only.'}
                  </span>
                </div>
                <Show when={queryProblem()}>
                  <p class="banner danger db-flat" role="alert">
                    <span class="grow">{queryProblem()}</span>
                  </p>
                </Show>
                <Show when={rows()}>
                  {(got) => (
                    <section class="api-answer" aria-label="Rows">
                      <div class="api-answer-head">
                        <span class="fine">
                          {got().rows.said ??
                            `${got().rows.rows.length}${got().rows.more ? ' rows, the first of more' : got().rows.rows.length === 1 ? ' row' : ' rows'}`}{' '}
                          · {tookWords(got().rows.took)}
                        </span>
                        <span class="grow" />
                        <Show when={handed() === rowsForAgent(got().sql, got().rows, database()!.name).title}>
                          <span class="fine">It goes along with your next message.</span>
                        </Show>
                        <button
                          type="button"
                          class="btn small"
                          onClick={() => show(rowsForAgent(got().sql, got().rows, database()!.name))}
                        >
                          Show it to the agent
                        </button>
                      </div>
                      <Show when={got().rows.columns.length > 0}>
                        <div class="db-rows" tabindex="0" role="region" aria-label="The rows the query gave">
                          <table class="db-grid rows">
                            <thead>
                              <tr>
                                <For each={got().rows.columns}>{(name) => <th class="mono">{name}</th>}</For>
                              </tr>
                            </thead>
                            <tbody>
                              <For each={got().rows.rows}>
                                {(row) => (
                                  <tr>
                                    <For each={row}>
                                      {(cell) => (
                                        <td class="mono" classList={{ nothing: cell === NOTHING }}>
                                          {cell}
                                        </td>
                                      )}
                                    </For>
                                  </tr>
                                )}
                              </For>
                            </tbody>
                          </table>
                        </div>
                      </Show>
                    </section>
                  )}
                </Show>
              </section>
            </Show>
          </div>
        )}
      </Show>
    </div>
  );
}
