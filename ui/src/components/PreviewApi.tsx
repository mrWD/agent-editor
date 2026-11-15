import { sizeOf } from '../core/sizes';
import { Grip } from './Grip';
import { createEffect, createMemo, createSignal, For, on, onCleanup, onMount, Show, type JSX } from 'solid-js';
import {
  ANY,
  answerForAgent,
  bodyProblem,
  contractWords,
  draftOf,
  guessKind,
  levelWord,
  matching,
  requestHeaders,
  requestPath,
  shownBody,
  sizeWords,
  statusTone,
  tookWords,
  valueKey,
  type ApiAnswer,
  type ApiDraft,
  type ApiOperation,
  type ApiState,
  type ContractChange,
} from '../core/api';
import { core } from '../core/client';
import { createKept } from '../core/kept';
import { addShown } from '../core/previewState';
import { headersText, parseHeaders } from '../core/toolservers';

const said = (error: unknown) => (error instanceof Error ? error.message : String(error));

/** A sentence of the core with the names it put in code marks shown as code. */
export function Marked(props: { text: string }): JSX.Element {
  return (
    <span>
      <For each={props.text.split('`')}>
        {(part, at) => (
          <Show when={at() % 2 === 1} fallback={part}>
            <code class="mono api-name">{part}</code>
          </Show>
        )}
      </For>
    </span>
  );
}

/** How long the files of a task are left to settle before the view reads again: an agent at work changes them in bursts. */
export const SETTLE = 1500;

const METHODS = ['GET', 'POST', 'PUT', 'PATCH', 'DELETE', 'HEAD', 'OPTIONS'];
const emptyDraft = (): ApiDraft => ({ values: {}, headers: '', body: '' });

/**
 * The Preview of a service that answers in data, not in pages: what its API
 * offers, a request sent to the task's own running service with the answer
 * and the time it took, and what the task changed in the contract against
 * the base. `guess` is where the task's services run, to ask for a
 * description when the user gave no address.
 */
export function ApiView(props: { taskId: string; guess: string[] }): JSX.Element {
  // eslint-disable-next-line solid/reactivity -- made anew for each task
  const taskId = props.taskId;
  const [api, setApi] = createKept<ApiState | null>(`task:${taskId}:api.state`, null);
  const [picked, setPicked] = createKept<string>(`task:${taskId}:api.picked`, '');
  const [drafts, setDrafts] = createKept<Record<string, ApiDraft>>(`task:${taskId}:api.drafts`, {});
  const [answers, setAnswers] = createKept<Record<string, ApiAnswer>>(`task:${taskId}:api.answers`, {});
  /** What each answer was the answer to: the draft may have been changed since. */
  const [sentWith, setSentWith] = createKept<Record<string, { headers: [string, string][]; body: string }>>(
    `task:${taskId}:api.sent`,
    {},
  );
  /** The request that is not in the description. */
  const [free, setFree] = createKept<{ method: string; path: string }>(`task:${taskId}:api.free`, {
    method: 'GET',
    path: '/',
  });
  const [view, setView] = createKept<'request' | 'contract'>(`task:${taskId}:api.view`, 'request');
  const [filter, setFilter] = createSignal('');
  /** Which operations the list holds. `null` until the user says: then what the task touched, if anything. */
  const [scope, setScope] = createKept<'touched' | 'all' | null>(`task:${taskId}:api.scope`, null);
  const [problem, setProblem] = createSignal<string | null>(null);
  const [sendProblem, setSendProblem] = createSignal<string | null>(null);
  const [sending, setSending] = createSignal(false);
  const [setting, setSetting] = createSignal(false);
  const [saving, setSaving] = createSignal(false);
  /** The title of what was last put before the agent, to say so next to the button. */
  const [handed, setHanded] = createSignal<string | null>(null);

  /** Where the task's services run, as last said from above. */
  let guess: string[] = [];
  // One reading at a time; one asked for meanwhile follows it.
  let reading = false;
  let again = false;
  const load = async () => {
    if (reading) {
      again = true;
      return;
    }
    reading = true;
    try {
      setApi(await core.request<ApiState>('api.get', { taskId, guess }));
      setProblem(null);
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
    let soon: ReturnType<typeof setTimeout> | undefined;
    const stop = core.onEvent((event) => {
      if (event.event === 'link.open') void load();
      if (event.data?.taskId !== taskId) return;
      // A description kept as a file changes with the files; one a service gives, when the service starts anew.
      if (['files.changed', 'review.changed', 'git.changed', 'run.changed'].includes(event.event)) {
        clearTimeout(soon);
        soon = setTimeout(() => void load(), SETTLE);
      }
    });
    onCleanup(() => {
      stop();
      clearTimeout(soon);
    });
  });
  // Asked at once, and again when another service of the task comes up or goes.
  createEffect(
    on(
      () => props.guess.join(' '),
      (joined) => {
        guess = joined === '' ? [] : joined.split(' ');
        void load();
      },
    ),
  );

  const operations = () => api()?.operations ?? [];
  // The first operation is on show until another is picked. It is picked for good once it is shown:
  // the list is put in another order when the task touches an operation, and what is being filled
  // in must not change under the user's hands.
  createEffect(() => {
    const first = operations()[0];
    if (picked() === '' && first) setPicked(first.id);
  });
  const byId = createMemo(() => new Map(operations().map((operation) => [operation.id, operation])));
  /** The operations the task added or changed, when the contract was compared with the base. */
  const touched = createMemo(() => operations().filter((operation) => operation.change !== null));
  /** Only what the task touched is listed: as chosen, else whenever the task touched something. */
  const onlyTouched = () =>
    Boolean(api()?.contract) && (scope() ?? (touched().length > 0 ? 'touched' : 'all')) === 'touched';
  /** The rows of the list, by name: a row stays the same row when the list is read again. */
  const shownIds = createMemo(() =>
    matching(onlyTouched() ? touched() : operations(), filter()).map((operation) => operation.id),
  );
  /** The operation on show: the one picked, else the first, so the view never opens on nothing. */
  const operation = (): ApiOperation | null =>
    picked() === ANY ? null : (byId().get(picked()) ?? operations()[0] ?? null);
  const key = () => operation()?.id ?? ANY;
  const draft = (): ApiDraft => drafts()[key()] ?? (operation() ? draftOf(operation()!) : emptyDraft());
  const change = (next: Partial<ApiDraft>) => {
    const [at, whole] = [key(), { ...draft(), ...next }];
    setDrafts((before) => ({ ...before, [at]: whole }));
  };
  const setValue = (name: string, value: string) => change({ values: { ...draft().values, [name]: value } });
  const answer = (): ApiAnswer | null => answers()[key()] ?? null;
  const changesOf = (id: string): ContractChange[] =>
    api()?.contract?.changes.filter((one) => one.operation === id) ?? [];

  const request = createMemo(() => {
    const one = operation();
    if (!one) return { method: free().method, path: free().path.trim(), missing: [] as string[] };
    return { method: one.method, ...requestPath(api()?.prefix ?? '', one, draft()) };
  });
  const takesBody = () => (operation() ? operation()!.body !== null : !['GET', 'HEAD'].includes(free().method));
  /** Why the request cannot be sent yet, in words. `null` when it can. */
  const refusal = (): string | null => {
    if (!api()?.address) return 'Give the address of the service first';
    if (request().missing.length > 0) return `Fill in ${request().missing.join(', ')} in the path`;
    if (!request().path.startsWith('/')) return 'The path starts with /';
    const made = requestHeaders(operation(), draft());
    if (made.problem) return made.problem;
    const kind = made.headers.find(([name]) => name.toLowerCase() === 'content-type')?.[1] ?? guessKind(draft().body);
    return takesBody() ? bodyProblem(kind, draft().body) : null;
  };

  const send = async () => {
    const address = api()?.address;
    if (sending() || refusal() || !address) return;
    const at = key();
    const { method, path } = request();
    const body = takesBody() ? draft().body : '';
    const { headers } = requestHeaders(operation(), { ...draft(), body });
    setSending(true);
    setSendProblem(null);
    setHanded(null);
    try {
      const got = await core.request<ApiAnswer>('api.send', { taskId, address, method, path, headers, body });
      setAnswers((before) => ({ ...before, [at]: got }));
      setSentWith((before) => ({ ...before, [at]: { headers, body } }));
    } catch (error) {
      setSendProblem(said(error));
    } finally {
      setSending(false);
    }
  };

  const showAgent = () => {
    const got = answer();
    if (!got) return;
    const made = answerForAgent(got, sentWith()[key()] ?? { headers: [], body: '' }, api()?.settings.headers ?? []);
    addShown(taskId, made.title, made.text);
    setHanded(made.title);
  };

  const keep = async (what: Record<string, unknown>) => {
    setProblem(null);
    try {
      await core.request('api.set', { taskId, ...what });
      await load();
      return true;
    } catch (error) {
      setProblem(said(error));
      return false;
    }
  };

  // The address on show follows what the core says, but never under the user's hands:
  // an answer that was on its way must not wipe what is being typed.
  let addressInput: HTMLInputElement | undefined;
  createEffect(() => {
    const now = api()?.address ?? '';
    if (addressInput && document.activeElement !== addressInput) addressInput.value = now;
  });

  // ----- the settings -----

  const [specAt, setSpecAt] = createSignal('');
  const [baseAt, setBaseAt] = createSignal('');
  const [everyHeader, setEveryHeader] = createSignal('');
  const openSettings = () => {
    const now = api()?.settings;
    setSpecAt(now?.spec ?? '');
    setBaseAt(now?.base ?? '');
    setEveryHeader(headersText(now?.headers ?? []));
    setSetting(true);
  };
  const saveSettings = async () => {
    const parsed = parseHeaders(everyHeader());
    if (parsed.problem) {
      setProblem(parsed.problem);
      return;
    }
    setSaving(true);
    const kept = await keep({ spec: specAt(), base: baseAt(), headers: parsed.headers });
    setSaving(false);
    if (kept) setSetting(false);
  };

  const sourceWords = () => {
    const source = api()?.source;
    if (!source) return null;
    const named = [api()!.title, api()!.version].filter(Boolean).join(' ');
    const from = source.kind === 'file' ? `read from ${source.where}` : `as the service gives it at ${source.where}`;
    return `${named || 'The API'}, ${from}`;
  };

  const Row = (row: { id: string }) => {
    const one = () => byId().get(row.id);
    return (
      <Show when={one()}>
        {(shown) => (
          <li>
            <button
              type="button"
              class="api-row"
              aria-current={operation()?.id === row.id && view() === 'request' ? 'true' : undefined}
              onClick={() => {
                setPicked(row.id);
                setView('request');
                setSendProblem(null);
                setHanded(null);
              }}
            >
              <span class="api-method mono" data-method={shown().method}>
                {shown().method}
              </span>
              <span class="api-path mono" classList={{ old: shown().deprecated }}>
                {shown().path}
              </span>
              <Show when={shown().change}>
                {(mark) => (
                  <span class="tag" classList={{ accent: mark() === 'changed', good: mark() === 'new' }}>
                    {mark() === 'new' ? 'new' : 'changed'}
                  </span>
                )}
              </Show>
            </button>
          </li>
        )}
      </Show>
    );
  };

  const Contract = () => (
    <section class="api-one" aria-label="What the task changes in the contract">
      <Show
        when={api()?.contract}
        fallback={<p class="soft">{api()?.contractNote ?? 'There is no description of the API to compare.'}</p>}
      >
        {(contract) => (
          <>
            <h2 class="api-title">
              {contractWords(contract().changes)}
              <span class="fine"> against {contract().against}</span>
            </h2>
            <Show when={contract().changes.length === 0}>
              <p class="soft">A client written against {contract().against} sees the same API in this task.</p>
            </Show>
            <For each={['breaking', 'careful', 'safe'] as const}>
              {(level) => (
                <Show when={contract().changes.some((one) => one.level === level)}>
                  <h3 class="api-level" data-level={level}>
                    {levelWord[level]}
                  </h3>
                  <ul class="api-changes">
                    <For each={contract().changes.filter((one) => one.level === level)}>
                      {(one) => (
                        <li>
                          <Show
                            when={byId().has(one.operation)}
                            fallback={<span class="mono api-change-op">{one.operation}</span>}
                          >
                            <button
                              type="button"
                              class="link mono api-change-op"
                              onClick={() => {
                                setPicked(one.operation);
                                setView('request');
                              }}
                            >
                              {one.operation}
                            </button>
                          </Show>
                          <Marked text={one.text} />
                        </li>
                      )}
                    </For>
                  </ul>
                </Show>
              )}
            </For>
          </>
        )}
      </Show>
    </section>
  );

  const Request = () => (
    <section class="api-one" aria-label="Request">
      <Show
        when={operation()}
        fallback={
          <>
            <h2 class="api-title">Any request</h2>
            <div class="api-free">
              <label class="sr-only" for={`api-method-${taskId}`}>
                Method
              </label>
              <select
                id={`api-method-${taskId}`}
                class="issue-place mono"
                value={free().method}
                onChange={(event) => setFree({ ...free(), method: event.currentTarget.value })}
              >
                <For each={METHODS}>{(method) => <option value={method}>{method}</option>}</For>
              </select>
              <label class="sr-only" for={`api-path-${taskId}`}>
                Path of the request
              </label>
              <input
                id={`api-path-${taskId}`}
                class="field mono grow"
                spellcheck={false}
                placeholder="/health"
                value={free().path}
                onInput={(event) => setFree({ ...free(), path: event.currentTarget.value })}
                onKeyDown={(event) => event.key === 'Enter' && void send()}
              />
            </div>
          </>
        }
      >
        {(one) => (
          <>
            <h2 class="api-title">
              <span class="api-method mono" data-method={one().method}>
                {one().method}
              </span>
              <span class="mono">{one().path}</span>
              <Show when={one().deprecated}>
                <span class="tag">deprecated</span>
              </Show>
            </h2>
            <Show when={one().summary}>
              <p class="soft">{one().summary}</p>
            </Show>
            <Show when={changesOf(one().id).length > 0}>
              <ul class="api-changes of-one" aria-label="What the task changed here">
                <For each={changesOf(one().id)}>
                  {(change) => (
                    <li>
                      <span class="tag" data-level={change.level}>
                        {levelWord[change.level]}
                      </span>
                      <Marked text={change.text} />
                    </li>
                  )}
                </For>
              </ul>
            </Show>
            <Show when={one().parameters.length > 0}>
              <div class="api-params">
                <For each={one().parameters}>
                  {(parameter) => (
                    <>
                      <label
                        for={`api-${taskId}-${one().id}-${valueKey(parameter)}`}
                        title={parameter.about || undefined}
                      >
                        <span class="mono">{parameter.name}</span>
                        <span class="fine">
                          {parameter.in}
                          {parameter.required ? ', needed' : ''}
                        </span>
                      </label>
                      <input
                        id={`api-${taskId}-${one().id}-${valueKey(parameter)}`}
                        class="field mono"
                        spellcheck={false}
                        placeholder={parameter.kind}
                        value={draft().values[valueKey(parameter)] ?? ''}
                        onInput={(event) => setValue(valueKey(parameter), event.currentTarget.value)}
                        onKeyDown={(event) => event.key === 'Enter' && void send()}
                      />
                    </>
                  )}
                </For>
              </div>
            </Show>
          </>
        )}
      </Show>

      <Show when={takesBody()}>
        <label class="api-label" for={`api-body-${taskId}`}>
          Body
          <span class="fine"> {operation()?.body?.kind ?? ''}</span>
        </label>
        <textarea
          id={`api-body-${taskId}`}
          class="api-text mono"
          rows={Math.min(14, Math.max(4, draft().body.split('\n').length + 1))}
          spellcheck={false}
          value={draft().body}
          onInput={(event) => change({ body: event.currentTarget.value })}
        />
      </Show>
      <details class="api-more">
        <summary>Headers of this request{draft().headers.trim() ? ' (some are set)' : ''}</summary>
        <label class="sr-only" for={`api-headers-${taskId}`}>
          Headers of this request, one on a line
        </label>
        <textarea
          id={`api-headers-${taskId}`}
          class="api-text mono"
          rows={3}
          spellcheck={false}
          placeholder="X-Request-Id: try-1"
          value={draft().headers}
          onInput={(event) => change({ headers: event.currentTarget.value })}
        />
        <Show when={(api()?.settings.headers.length ?? 0) > 0}>
          <p class="fine">
            Sent with every request: {(api()?.settings.headers ?? []).map(([name]) => name).join(', ')}.
          </p>
        </Show>
      </details>

      <div class="api-send">
        <button type="button" class="btn" disabled={sending() || refusal() !== null} onClick={() => void send()}>
          {sending() ? 'Waiting for the answer' : 'Send'}
        </button>
        <span class="mono api-url" aria-label="Where the request goes">
          {request().method} {api()?.address ? `http://${api()!.address}` : ''}
          {request().path}
        </span>
      </div>
      <Show when={refusal() && api()?.address}>
        <p class="fine">{refusal()}.</p>
      </Show>
      <Show when={sendProblem()}>
        <p class="banner danger" role="alert">
          <span class="grow">{sendProblem()}</span>
        </p>
      </Show>

      <Show when={answer()}>
        {(got) => (
          <section class="api-answer" aria-label="Answer">
            <div class="api-answer-head">
              <span class="tag mono" data-tone={statusTone(got().status)}>
                {got().status} {got().reason}
              </span>
              <span class="fine">
                {tookWords(got().took)} · {sizeWords(got().size)}
                {got().cut ? ', the start of it' : ''}
              </span>
              <span class="grow" />
              <Show when={handed()}>
                <span class="fine">It goes along with your next message.</span>
              </Show>
              <button type="button" class="btn small" onClick={showAgent}>
                Show it to the agent
              </button>
            </div>
            <Show
              when={got().kind !== 'binary'}
              fallback={<p class="soft">The answer is {sizeWords(got().size)} that are not text.</p>}
            >
              <Show when={got().body !== ''} fallback={<p class="soft">The answer has no body.</p>}>
                <pre class="api-body mono" tabindex="0" aria-label="Body of the answer">
                  {shownBody(got())}
                </pre>
              </Show>
            </Show>
            <details class="api-more">
              <summary>Headers of the answer</summary>
              <dl class="api-headers mono">
                <For each={got().headers}>
                  {([name, value]) => (
                    <>
                      <dt>{name}</dt>
                      <dd>{value}</dd>
                    </>
                  )}
                </For>
              </dl>
            </details>
          </section>
        )}
      </Show>
    </section>
  );

  return (
    <div class="api">
      <div class="preview-bar">
        <span class="preview-address">
          <input
            class="mono"
            type="text"
            aria-label="Address of the service"
            placeholder="localhost:8000"
            spellcheck={false}
            ref={addressInput}
            onChange={(event) => void keep({ address: event.currentTarget.value })}
          />
        </span>
        <Show when={sourceWords()}>
          <span class="fine api-source">{sourceWords()}</span>
        </Show>
        <span class="grow" />
        <Show when={api()?.source}>
          <button
            type="button"
            class="btn small"
            classList={{ ghost: view() !== 'contract' }}
            aria-pressed={view() === 'contract'}
            onClick={() => setView(view() === 'contract' ? 'request' : 'contract')}
          >
            {api()?.contract ? contractWords(api()!.contract!.changes) : 'Contract'}
          </button>
        </Show>
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
          aria-label="How the API is read"
          onSubmit={(event) => {
            event.preventDefault();
            void saveSettings();
          }}
        >
          <label for={`api-spec-${taskId}`}>Where the description is</label>
          <input
            id={`api-spec-${taskId}`}
            class="field mono"
            spellcheck={false}
            placeholder="found by itself: openapi.json in the project, /openapi.json on the service"
            value={specAt()}
            onInput={(event) => setSpecAt(event.currentTarget.value)}
          />
          <label for={`api-base-${taskId}`}>A service that runs the base branch</label>
          <input
            id={`api-base-${taskId}`}
            class="field mono"
            spellcheck={false}
            placeholder="https://staging.example.com, to compare the contract with"
            value={baseAt()}
            onInput={(event) => setBaseAt(event.currentTarget.value)}
          />
          <label for={`api-every-${taskId}`}>Headers for every request</label>
          <textarea
            id={`api-every-${taskId}`}
            class="api-text mono"
            rows={2}
            spellcheck={false}
            placeholder="Authorization: Bearer …"
            value={everyHeader()}
            onInput={(event) => setEveryHeader(event.currentTarget.value)}
          />
          <span />
          <span class="api-setup-end">
            <button type="submit" class="btn small" disabled={saving()}>
              Save
            </button>
            <button type="button" class="btn small ghost" onClick={() => setSetting(false)}>
              Cancel
            </button>
            <span class="fine">Kept for the project, on this computer.</span>
          </span>
        </form>
      </Show>
      <Show when={problem() ?? api()?.note}>
        <p class="banner flush" classList={{ danger: problem() !== null }} role={problem() ? 'alert' : 'status'}>
          <span class="grow">{problem() ?? api()?.note}</span>
        </p>
      </Show>
      <Show when={api()} fallback={<p class="bottom-note">Reading the API of this task</p>}>
        <div class="api-split">
          <nav class="api-list" aria-label="What the API offers" style={{ flex: `0 0 ${sizeOf('list')}px` }}>
            <Show when={api()?.contract}>
              <div class="switcher api-scope" role="group" aria-label="Which operations to list">
                <button
                  type="button"
                  aria-pressed={onlyTouched()}
                  title={`The operations this task added or changed, against ${api()!.contract!.against}`}
                  onClick={() => setScope('touched')}
                >
                  Changed <span class="count">{touched().length}</span>
                </button>
                <button type="button" aria-pressed={!onlyTouched()} onClick={() => setScope('all')}>
                  All <span class="count">{operations().length}</span>
                </button>
              </div>
            </Show>
            <Show when={operations().length > 6}>
              <label class="sr-only" for={`api-filter-${taskId}`}>
                Find an operation
              </label>
              <input
                id={`api-filter-${taskId}`}
                class="field"
                type="search"
                placeholder="Find"
                value={filter()}
                onInput={(event) => setFilter(event.currentTarget.value)}
              />
            </Show>
            <Show when={onlyTouched() && touched().length === 0}>
              <p class="fine api-none">This task changes no operation of the API.</p>
            </Show>
            <ul>
              <For each={shownIds()}>{(id) => <Row id={id} />}</For>
              <li>
                <button
                  type="button"
                  class="api-row"
                  aria-current={!operation() && view() === 'request' ? 'true' : undefined}
                  onClick={() => {
                    setPicked(ANY);
                    setView('request');
                  }}
                >
                  <span class="api-path">Any request</span>
                </button>
              </li>
            </ul>
          </nav>
          <Grip name="list" edge="right" label="Drag to make the list of operations wider or narrower" />
          <Show when={view() === 'contract'} fallback={<Request />}>
            <Contract />
          </Show>
        </div>
      </Show>
    </div>
  );
}
