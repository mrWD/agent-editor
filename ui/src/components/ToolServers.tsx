import { createSignal, For, onCleanup, onMount, Show, type JSX } from 'solid-js';
import { core } from '../core/client';
import { createKept } from '../core/kept';
import { envText, joinArgs, parseEnv, splitArgs } from '../core/settings';
import {
  headersText,
  listToolServers,
  parseHeaders,
  removeToolServer,
  saveToolServer,
  TOOL_PRESETS,
  toolHanded,
  toolKindWords,
  toolLine,
  type ToolDraft,
  type ToolKind,
  type ToolPreset,
  type ToolServer,
} from '../core/toolservers';
import { Sure } from '../screens/Changes';

const said = (error: unknown) => (error instanceof Error ? error.message : String(error));

/** The id the form has while it holds a server that is not in the list yet. */
const NEW = '__new__';

/**
 * The part of Settings about tool servers: the list, a form to add or change
 * one, and servers that are ready to be filled in. Every agent that speaks the
 * protocol is named these when a session opens.
 */
export function ToolServers(): JSX.Element {
  const [servers, setServers] = createKept<ToolServer[] | null>('settings.tools', null);
  const [problem, setProblem] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal(false);
  /** The server the form holds: its id, `NEW`, or `null` when the form is closed. */
  const [editing, setEditing] = createSignal<string | null>(null);
  const [picking, setPicking] = createSignal(false);
  /** What is left for the user to give, said under the form of a server that was picked ready-made. */
  const [needs, setNeeds] = createSignal<string | null>(null);

  const [name, setName] = createSignal('');
  const [kind, setKind] = createSignal<ToolKind>('stdio');
  const [program, setProgram] = createSignal('');
  const [args, setArgs] = createSignal('');
  const [env, setEnv] = createSignal('');
  const [url, setUrl] = createSignal('');
  const [headers, setHeaders] = createSignal('');

  const load = async () => {
    try {
      setServers(await listToolServers());
    } catch (error) {
      setProblem(said(error));
    }
  };

  onMount(() => {
    void load();
    const stop = core.onEvent((event) => {
      // Another window changed the list, or the core was reached anew.
      if (event.event === 'tools.changed' || event.event === 'link.open') void load();
    });
    onCleanup(stop);
  });

  const fill = (server: Omit<ToolDraft, 'on' | 'id'> | null) => {
    setName(server?.name ?? '');
    setKind(server?.kind ?? 'stdio');
    setProgram(server?.program ?? '');
    setArgs(joinArgs(server?.args ?? []));
    setEnv(envText(server?.env ?? []));
    setUrl(server?.url ?? '');
    setHeaders(headersText(server?.headers ?? []));
    setProblem(null);
  };

  const open = (server: ToolServer | null) => {
    fill(server);
    setNeeds(null);
    setPicking(false);
    setEditing(server?.id ?? NEW);
  };

  const pick = (preset: ToolPreset) => {
    fill(preset.draft);
    setNeeds(preset.needs);
    setPicking(false);
    setEditing(NEW);
  };

  const act = async (work: () => Promise<ToolServer[]>) => {
    if (busy()) return false;
    setBusy(true);
    setProblem(null);
    try {
      setServers(await work());
      return true;
    } catch (error) {
      setProblem(said(error));
      return false;
    } finally {
      setBusy(false);
    }
  };

  const save = async (known: ToolServer | null) => {
    const [readEnv, readHeaders] = [parseEnv(env()), parseHeaders(headers())];
    const wrong = kind() === 'stdio' ? readEnv.problem : readHeaders.problem;
    if (wrong) {
      setProblem(wrong);
      return;
    }
    // The form as it stands at this press.
    const draft: ToolDraft = {
      id: known?.id,
      name: name().trim(),
      kind: kind(),
      program: program().trim(),
      args: splitArgs(args()),
      env: readEnv.env,
      url: url().trim(),
      headers: readHeaders.headers,
      on: known?.on ?? true,
    };
    const saved = await act(() => saveToolServer(draft));
    if (saved) setEditing(null);
  };

  const form = (known: ToolServer | null) => (
    <form
      class="card auto-form setting-form"
      aria-label={known ? `The tool server ${known.name}` : 'A new tool server'}
      onSubmit={(event) => {
        event.preventDefault();
        void save(known);
      }}
    >
      <label for="tool-name">Name: what agents see it as</label>
      <input
        id="tool-name"
        class="field"
        placeholder="Sentry"
        value={name()}
        onInput={(event) => setName(event.currentTarget.value)}
      />
      <label for="tool-kind">How it is reached</label>
      <select
        id="tool-kind"
        class="select-button"
        value={kind()}
        onChange={(event) => setKind(event.currentTarget.value as ToolKind)}
      >
        <option value="stdio">A program the agent starts on this computer</option>
        <option value="http">An address, over HTTP</option>
        <option value="sse">An address, over SSE (the older way)</option>
      </select>
      <Show
        when={kind() === 'stdio'}
        fallback={
          <>
            <label for="tool-url">Address</label>
            <input
              id="tool-url"
              class="field mono"
              placeholder="https://mcp.example.com/mcp"
              spellcheck={false}
              value={url()}
              onInput={(event) => setUrl(event.currentTarget.value)}
            />
            <label for="tool-headers">Headers sent with every request: one "Name: value" on a line</label>
            <textarea
              id="tool-headers"
              class="git-message mono"
              rows="2"
              placeholder="Authorization: Bearer …"
              spellcheck={false}
              value={headers()}
              onInput={(event) => setHeaders(event.currentTarget.value)}
            />
          </>
        }
      >
        <label for="tool-program">Program</label>
        <input
          id="tool-program"
          class="field mono"
          placeholder="npx"
          spellcheck={false}
          value={program()}
          onInput={(event) => setProgram(event.currentTarget.value)}
        />
        <label for="tool-args">Arguments, as on a command line</label>
        <input
          id="tool-args"
          class="field mono"
          placeholder="-y @sentry/mcp-server@latest"
          spellcheck={false}
          value={args()}
          onInput={(event) => setArgs(event.currentTarget.value)}
        />
        <label for="tool-env">Environment: one NAME=value on a line</label>
        <textarea
          id="tool-env"
          class="git-message mono"
          rows="2"
          spellcheck={false}
          value={env()}
          onInput={(event) => setEnv(event.currentTarget.value)}
        />
      </Show>
      <Show when={needs()}>
        <span class="fine">{needs()}</span>
      </Show>
      <span class="fine">
        A key you give here is kept on this computer with the other settings and goes to the agents you start, to nobody
        else. The server is named to sessions that open from now on; an agent that is running goes on with what it was
        given.
      </span>
      <div class="actions">
        <button type="submit" class="btn primary" disabled={busy()}>
          {known ? 'Save the server' : 'Add the server'}
        </button>
        <button type="button" class="btn ghost" onClick={() => setEditing(null)}>
          Cancel
        </button>
      </div>
    </form>
  );

  return (
    <section class="stage" aria-label="Tool servers">
      <div class="stage-head">
        <h2 class="stage-title">Tool servers</h2>
        <span class="stage-note">
          Servers that speak the Model Context Protocol: a tracker of errors, a design, a wiki, a database. Every agent
          that speaks the protocol is named these when it starts, next to the editor's own tools. A program in a
          terminal keeps its own list, in its own settings.
        </span>
      </div>
      <Show when={problem()}>
        <p class="banner danger flush" role="alert">
          <span class="grow">{problem()}</span>
          <button type="button" class="btn tiny" onClick={() => setProblem(null)}>
            Dismiss
          </button>
        </p>
      </Show>
      <Show when={servers()} fallback={<p class="stage-row fine">Reading the tool servers</p>}>
        <Show when={servers()!.length === 0}>
          <p class="stage-row fine">None yet. Pick one that is ready, or add your own.</p>
        </Show>
        <For each={servers()!}>
          {(server) => (
            <>
              <div class="stage-row setting-row" role="group" aria-label={server.name}>
                <div class="setting-what">
                  <span class="setting-head">
                    <span class="name">{server.name}</span>
                    <span class="tag">{toolKindWords(server.kind)}</span>
                    <Show when={!server.on}>
                      <span class="tag">Off</span>
                    </Show>
                  </span>
                  <code class="setting-command">{toolLine(server)}</code>
                  <Show when={toolHanded(server).names}>
                    <span class="fine">With {toolHanded(server).names} set.</span>
                  </Show>
                  <Show when={toolHanded(server).empty.length > 0}>
                    <span class="fine text-bad">
                      {toolHanded(server).empty.join(', ')} {toolHanded(server).empty.length === 1 ? 'is' : 'are'} still
                      empty: it will not work until you fill it in.
                    </span>
                  </Show>
                </div>
                <div class="actions">
                  <button
                    type="button"
                    class="btn small ghost"
                    aria-pressed={server.on}
                    aria-label={`Hand ${server.name} to agents`}
                    disabled={busy()}
                    onClick={() => void act(() => saveToolServer({ ...server, on: !server.on }))}
                  >
                    {server.on ? 'On' : 'Off'}
                  </button>
                  <button
                    type="button"
                    class="btn small"
                    aria-expanded={editing() === server.id}
                    aria-label={`Change ${server.name}`}
                    onClick={() => (editing() === server.id ? setEditing(null) : open(server))}
                  >
                    Change
                  </button>
                  <Sure
                    class="btn small ghost"
                    label="Remove"
                    sure="Press again to remove"
                    about={server.id}
                    onSure={() => void act(() => removeToolServer(server.id))}
                  />
                </div>
              </div>
              <Show when={editing() === server.id}>{form(server)}</Show>
            </>
          )}
        </For>
      </Show>
      <div class="stage-row setting-add">
        <div class="actions">
          <button
            type="button"
            class="btn small"
            aria-expanded={picking()}
            onClick={() => {
              setPicking(!picking());
              setEditing(null);
            }}
          >
            Pick one that is ready
          </button>
          <button
            type="button"
            class="btn small ghost"
            aria-expanded={editing() === NEW}
            onClick={() => (editing() === NEW ? setEditing(null) : open(null))}
          >
            Add your own
          </button>
        </div>
        <Show when={picking()}>
          <ul class="tool-presets" aria-label="Tool servers that are ready to be filled in">
            <For each={TOOL_PRESETS}>
              {(preset) => (
                <li>
                  <span class="tool-preset-what">
                    <span class="name">{preset.name}</span>
                    <span class="fine">{preset.gives}</span>
                  </span>
                  <button
                    type="button"
                    class="btn small"
                    aria-label={`Fill in ${preset.name}`}
                    onClick={() => pick(preset)}
                  >
                    Fill in
                  </button>
                </li>
              )}
            </For>
          </ul>
        </Show>
        <Show when={editing() === NEW}>{form(null)}</Show>
      </div>
    </section>
  );
}
