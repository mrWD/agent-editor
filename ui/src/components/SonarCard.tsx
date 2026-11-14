import { createSignal, For, Show, type JSX } from 'solid-js';
import { core } from '../core/client';
import { projectName, type SonarStatus } from '../core/store';
import { whenLabel } from './bits';

const said = (error: unknown) => (error instanceof Error ? error.message : String(error));

/**
 * The part of Health about SonarQube: connecting it, and which project of
 * SonarQube each project is. What it found is among the findings above, read
 * anew with "Check now".
 */
export function SonarCard(props: {
  status: SonarStatus;
  /** Called when something here changed what Health shows. */
  onChanged: () => void;
}): JSX.Element {
  const [url, setUrl] = createSignal('');
  const [token, setToken] = createSignal('');
  const [busy, setBusy] = createSignal(false);
  const [problem, setProblem] = createSignal<string | null>(null);
  /** The project whose key is being typed, and what was typed. */
  const [keyOf, setKeyOf] = createSignal<string | null>(null);
  const [key, setKey] = createSignal('');

  const act = async (work: () => Promise<unknown>) => {
    if (busy()) return;
    setBusy(true);
    setProblem(null);
    try {
      await work();
      props.onChanged();
    } catch (error) {
      setProblem(said(error));
    } finally {
      setBusy(false);
    }
  };

  const connect = () => {
    const given = { url: url().trim(), token: token().trim() };
    return act(async () => {
      await core.request('sonar.connect', given);
      setToken('');
    });
  };

  const saveKey = (projectId: string) => {
    const given = key().trim();
    return act(async () => {
      await core.request('sonar.key', { projectId, key: given });
      setKeyOf(null);
    });
  };

  return (
    <section class="stage" aria-label="SonarQube">
      <div class="stage-head">
        <h2 class="stage-title">SonarQube</h2>
        <span class="stage-note">
          <Show
            when={props.status.connected}
            fallback="What your SonarQube or SonarCloud found in each project, shown above with the rest."
          >
            Connected to {props.status.url}.{' '}
            <Show when={props.status.checkedAt} fallback="Press Check now to read what it found.">
              {(at) => <>Last read {whenLabel(at())}.</>}
            </Show>
          </Show>
        </span>
      </div>
      <Show when={problem()}>
        <p class="stage-row plain danger" role="alert">
          {problem()}
        </p>
      </Show>
      <Show
        when={props.status.connected}
        fallback={
          <div class="stage-row setting-row">
            <span class="setting-what soft">
              Give its address and a token of a user who may see the projects. The token is kept on this computer.
            </span>
            <label class="sr-only" for="sonar-url">
              Address of SonarQube
            </label>
            <input
              id="sonar-url"
              class="field"
              placeholder="https://sonarcloud.io"
              autocomplete="off"
              value={url()}
              onInput={(event) => setUrl(event.currentTarget.value)}
            />
            <label class="sr-only" for="sonar-token">
              SonarQube token
            </label>
            <input
              id="sonar-token"
              class="field"
              type="password"
              autocomplete="off"
              placeholder="squ_…"
              value={token()}
              onInput={(event) => setToken(event.currentTarget.value)}
              onKeyDown={(event) => event.key === 'Enter' && void connect()}
            />
            <button
              type="button"
              class="btn small"
              disabled={busy() || url().trim() === '' || token().trim() === ''}
              onClick={() => void connect()}
            >
              {busy() ? 'Asking SonarQube' : 'Connect SonarQube'}
            </button>
          </div>
        }
      >
        <For each={props.status.projects}>
          {(project) => (
            <div class="stage-row health-check">
              <span class="project-tag">{projectName(project.projectId)}</span>
              <Show
                when={keyOf() === project.projectId}
                fallback={
                  <>
                    <span class="name">
                      <Show when={project.key} fallback="Not a project of SonarQube yet">
                        <code class="mono">{project.key}</code>
                      </Show>
                    </span>
                    <span class="fine">
                      {project.from === 'file'
                        ? 'from its sonar-project.properties'
                        : project.from === 'set'
                          ? 'given here'
                          : 'no key in its files'}
                    </span>
                    <span class="grow" />
                    <button
                      type="button"
                      class="btn tiny ghost"
                      aria-label={`Change the SonarQube key of ${projectName(project.projectId)}`}
                      onClick={() => {
                        setKey(project.from === 'set' ? (project.key ?? '') : '');
                        setKeyOf(project.projectId);
                      }}
                    >
                      {project.key ? 'Change the key' : 'Give its key'}
                    </button>
                  </>
                }
              >
                <input
                  class="field"
                  aria-label={`SonarQube project key for ${projectName(project.projectId)}`}
                  placeholder="Project key, such as acme:shop-web"
                  value={key()}
                  onInput={(event) => setKey(event.currentTarget.value)}
                  onKeyDown={(event) => event.key === 'Enter' && void saveKey(project.projectId)}
                />
                <span class="grow" />
                <button
                  type="button"
                  class="btn tiny"
                  disabled={busy()}
                  onClick={() => void saveKey(project.projectId)}
                >
                  {key().trim() === '' ? "Use the files' key" : 'Save'}
                </button>
                <button type="button" class="btn tiny ghost" onClick={() => setKeyOf(null)}>
                  Cancel
                </button>
              </Show>
            </div>
          )}
        </For>
        <p class="stage-row plain">
          <span class="grow">
            SonarQube is only read. It closes a finding itself, with its next analysis of the fixed code.
          </span>
          <button
            type="button"
            class="btn tiny ghost"
            disabled={busy()}
            onClick={() => void act(() => core.request('sonar.connect', { token: '' }))}
          >
            Forget the token
          </button>
        </p>
      </Show>
    </section>
  );
}
