import { ToolServers } from '../components/ToolServers';
import { createSignal, For, onCleanup, onMount, Show, type JSX } from 'solid-js';
import { createStore, reconcile } from 'solid-js/store';
import { recallCopy, rememberCopy } from '../core/kept';
import { Chip } from '../components/bits';
import { core } from '../core/client';
import { askPermission, permission, refusal, type Permission } from '../core/notify';
import { showOnboardingAgain } from '../core/onboarding';
import { chooseTheme, themeChoice, type ThemeChoice } from '../core/theme';
import {
  commandLine,
  envText,
  getSettings,
  joinArgs,
  NOTICE_KINDS,
  parseEnv,
  removeAgent,
  resetAgent,
  saveAgent,
  setSettings,
  shortFor,
  splitArgs,
  tryAgent,
  type AgentSetting,
  type NoticeKind,
  type Settings as SettingsState,
  type Tried,
} from '../core/settings';
import { state } from '../core/store';

const message = (error: unknown) => (error instanceof Error ? error.message : String(error));

/** What each notification is about, as the row of its switch says it. */
const NOTICE_WORDS: Record<NoticeKind, string> = {
  question: 'An agent asks a question',
  turnEnd: 'An agent finished its turn',
  checksFailed: 'The checks of a merge request failed',
  readyToMerge: 'A merge request became ready to merge',
};

/** Where an agent comes from, as the small tag next to its name says it. */
function originOf(agent: Pick<AgentSetting, 'own' | 'changed'>): string {
  if (agent.own) return 'Yours';
  return agent.changed ? 'Command changed' : 'Preset';
}

const THEMES: { value: ThemeChoice; name: string }[] = [
  { value: 'dark', name: 'Dark' },
  { value: 'light', name: 'Light' },
  { value: 'system', name: 'As the computer' },
];

/** The value of `editing` that means "the form for a new agent is open". */
const NEW = '__new__';

/** An on-off button, the same as the ones of Automations. */
function Switch(props: {
  on: boolean;
  label: string;
  disabled?: boolean;
  onChange: (on: boolean) => void;
}): JSX.Element {
  return (
    <button
      type="button"
      class="btn small"
      classList={{ primary: props.on }}
      aria-pressed={props.on}
      aria-label={`${props.label}: ${props.on ? 'on' : 'off'}`}
      disabled={props.disabled}
      onClick={() => props.onChange(!props.on)}
    >
      {props.on ? 'On' : 'Off'}
    </button>
  );
}

/** What "Try it" found, under the agent's command. */
function TriedLine(props: { name: string; found: Tried | 'trying' }): JSX.Element {
  const failed = () => props.found !== 'trying' && !props.found.ok;
  const words = () => {
    const found = props.found;
    if (found === 'trying') return `Starting ${props.name}. An agent that is downloaded first takes a while.`;
    return found.ok ? found.says : found.problem;
  };
  return (
    <span class="setting-tried" role="status" classList={{ 'text-bad': failed() }}>
      {words()}
    </span>
  );
}

/** What holds for the whole editor: the agents, notifications, sleep, how a new project starts. */
export function Settings(): JSX.Element {
  // A store, so that reading the settings again leaves every row that did not change as it is:
  // a form that is being typed into must not be drawn anew because something else changed.
  // It opens on what it showed last time, and reads again behind that.
  const [kept, setKept] = createStore<{ settings: SettingsState | null }>({
    settings: recallCopy<SettingsState>('settings'),
  });
  const settings = () => kept.settings;
  const setKnown = (now: SettingsState) => setKept('settings', reconcile(rememberCopy('settings', now), { key: 'id' }));
  const [problem, setProblem] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal(false);
  /** The agent whose form is open, `NEW` for the form of a new one, `null` for none. */
  const [editing, setEditing] = createSignal<string | null>(null);
  const [name, setName] = createSignal('');
  const [short, setShort] = createSignal('');
  const [program, setProgram] = createSignal('');
  const [args, setArgs] = createSignal('');
  const [env, setEnv] = createSignal('');
  /** Whether the agent in the form runs in a terminal because it speaks no protocol. */
  const [inTerminal, setInTerminal] = createSignal(false);
  /** What "Try it" found for each agent it was pressed on; "trying" while it runs. */
  const [tried, setTried] = createSignal<Record<string, Tried | 'trying'>>({});
  /** What this browser says about showing notifications. */
  const [allowed, setAllowed] = createSignal<Permission>(permission());
  /** Why the browser did not let notifications be turned on, when it did not. */
  const [refused, setRefused] = createSignal<string | null>(null);

  const load = async () => {
    try {
      setKnown(await getSettings());
    } catch (error) {
      setProblem(message(error));
    }
  };
  onMount(() => {
    void load();
    const stop = core.onEvent((event) => {
      // Another window changed something, or the helper that keeps the computer awake started or ended.
      if (event.event === 'settings.changed' || event.event === 'link.open') void load();
    });
    onCleanup(stop);
  });

  /** Does one thing at a time and says what went wrong. The work answers the settings as they are afterwards. */
  const act = async (work: () => Promise<SettingsState | void>) => {
    if (busy()) return;
    setBusy(true);
    setProblem(null);
    try {
      const now = await work();
      if (now) setKnown(now);
    } catch (error) {
      setProblem(message(error));
    } finally {
      setBusy(false);
    }
  };

  // ----- agents -----

  const openForm = (agent: AgentSetting | null) => {
    setName(agent?.name ?? '');
    setShort(agent?.short ?? '');
    setProgram(agent?.program ?? '');
    setArgs(joinArgs(agent?.args ?? []));
    setEnv(envText(agent?.env ?? []));
    setInTerminal(agent?.speaks === 'terminal');
    setEditing(agent?.id ?? NEW);
    setProblem(null);
  };

  const save = (agent: AgentSetting | null) =>
    // eslint-disable-next-line solid/reactivity -- called later, as an event handler: it reads what is current then
    act(async () => {
      const read = parseEnv(env());
      if (read.problem) throw new Error(read.problem);
      const saved = await saveAgent({
        id: agent?.id,
        name: name().trim(),
        // Left empty, the mark is made from the name.
        short: short().trim() || shortFor(name()),
        program: program().trim(),
        args: splitArgs(args()),
        env: read.env,
        terminal: inTerminal(),
      });
      setEditing(null);
      forgetTry(saved.saved);
      return saved;
    });

  /** What "Try it" found for an agent says nothing about a command it no longer has. */
  const forgetTry = (agentId: string) =>
    setTried((now) => Object.fromEntries(Object.entries(now).filter(([id]) => id !== agentId)));

  const putBack = (agent: AgentSetting) =>
    act(async () => {
      const now = await resetAgent(agent.id);
      forgetTry(agent.id);
      return now;
    });

  const tryIt = async (agent: AgentSetting) => {
    setTried((now) => ({ ...now, [agent.id]: 'trying' }));
    try {
      const found = await tryAgent(agent.id);
      setTried((now) => ({ ...now, [agent.id]: found }));
    } catch (error) {
      setTried((now) => ({ ...now, [agent.id]: { ok: false, problem: message(error) } }));
    }
  };

  // ----- notifications -----

  const turnNotifications = (on: boolean) =>
    // eslint-disable-next-line solid/reactivity -- called later, as an event handler: it reads what is current then
    act(async () => {
      setRefused(null);
      // In a browser the browser has to agree first. The app shows them itself and asks nobody here.
      if (on && !settings()?.notifications.byApp) {
        const answer = await askPermission();
        setAllowed(answer);
        const why = refusal(answer);
        if (why) {
          setRefused(why);
          return;
        }
      }
      return setSettings({ notifications: { on } });
    });

  /** The fields of the form, for a new agent or for the command of one that is there. */
  const form = (agent: AgentSetting | null) => (
    <form
      class="card auto-form setting-form"
      aria-label={agent ? `The command of ${agent.name}` : 'A new agent'}
      onSubmit={(event) => {
        event.preventDefault();
        void save(agent);
      }}
    >
      <Show when={!agent || agent.own}>
        <label for="agent-name">Name</label>
        <input
          id="agent-name"
          class="field"
          placeholder="My agent"
          value={name()}
          onInput={(event) => setName(event.currentTarget.value)}
        />
        <label for="agent-short">Short name: one or two letters for the mark next to its lines</label>
        <input
          id="agent-short"
          class="field narrow"
          maxlength="2"
          placeholder={shortFor(name()) || 'My'}
          value={short()}
          onInput={(event) => setShort(event.currentTarget.value)}
        />
      </Show>
      <label for="agent-program">Program</label>
      <input
        id="agent-program"
        class="field mono"
        placeholder="npx"
        spellcheck={false}
        value={program()}
        onInput={(event) => setProgram(event.currentTarget.value)}
      />
      <label for="agent-args">Arguments, as on a command line</label>
      <input
        id="agent-args"
        class="field mono"
        placeholder="-y @agentclientprotocol/claude-agent-acp"
        spellcheck={false}
        value={args()}
        onInput={(event) => setArgs(event.currentTarget.value)}
      />
      <label for="agent-env">Environment: one NAME=value on a line</label>
      <textarea
        id="agent-env"
        class="git-message mono"
        rows="2"
        spellcheck={false}
        value={env()}
        onInput={(event) => setEnv(event.currentTarget.value)}
      />
      <Show when={!agent || agent.own}>
        <label class="setting-check">
          <input
            type="checkbox"
            checked={inTerminal()}
            onChange={(event) => setInTerminal(event.currentTarget.checked)}
          />
          It runs in a terminal: it does not speak the Agent Client Protocol
        </label>
      </Show>
      <span class="fine">
        <Show
          when={inTerminal()}
          fallback="The command has to speak the Agent Client Protocol on its input and output."
        >
          The program is started in a terminal in the task's folder, and your messages are typed into it. Write{' '}
          <code>{'{message}'}</code> among the arguments where the program takes a first message: it is then started
          with yours, and a question it opens with (whether to trust the folder, say) waits for you instead of being
          answered by what was typed. Its turn is taken to be over when it has printed nothing for a moment. It gets no
          pictures and none of the editor's own tools, and what it does is shown in its terminal, not as a conversation.
        </Show>{' '}
        It is used from the agent's next start; an agent that is running goes on as it was started.
      </span>
      <div class="actions">
        <button type="submit" class="btn primary" disabled={busy()}>
          {agent ? 'Save the command' : 'Add the agent'}
        </button>
        <button type="button" class="btn ghost" onClick={() => setEditing(null)}>
          Cancel
        </button>
      </div>
    </form>
  );

  return (
    <main class="page">
      <div class="page-inner wide">
        <h1 class="headline">Settings</h1>
        <p class="soft">
          What holds for the whole editor. Every change is kept by the core and holds from the moment you make it.
        </p>
        <Show when={problem()}>
          <p class="banner danger flush" role="alert">
            <span class="grow">{problem()}</span>
            <button type="button" class="btn tiny" onClick={() => setProblem(null)}>
              Dismiss
            </button>
          </p>
        </Show>
        <Show when={!settings() && !problem()}>
          <p class="soft">Reading the settings</p>
        </Show>

        <Show when={settings()}>
          {(known) => (
            <>
              <section class="stage" aria-label="Agents">
                <div class="stage-head">
                  <h2 class="stage-title">Agents</h2>
                  <span class="stage-note">
                    Each agent is a command line that speaks the Agent Client Protocol, or a program that runs in a
                    terminal. The editor starts it once for every task.
                  </span>
                </div>
                <For each={known().agents}>
                  {(agent) => (
                    <div class="stage-row setting-row" role="group" aria-label={agent.name}>
                      <div class="setting-what">
                        <span class="setting-head">
                          <Chip agentId={agent.id} short={agent.short} flat />
                          <span class="name">{agent.name}</span>
                          <span class="tag">{originOf(agent)}</span>
                          <Show when={agent.speaks === 'terminal'}>
                            <span class="tag">In a terminal</span>
                          </Show>
                          <Show when={!agent.available}>
                            <span class="fine text-bad">Not installed</span>
                          </Show>
                        </span>
                        <Show when={agent.about}>
                          <span class="fine">{agent.about}</span>
                        </Show>
                        <code class="setting-command">{commandLine(agent)}</code>
                        <Show when={agent.env.length > 0}>
                          <span class="fine">With {agent.env.map(([variable]) => variable).join(', ')} set.</span>
                        </Show>
                        <Show when={agent.note}>
                          <span class="fine">{agent.note}</span>
                        </Show>
                        <Show when={tried()[agent.id]}>
                          {(found) => <TriedLine name={agent.name} found={found()} />}
                        </Show>
                      </div>
                      <div class="actions">
                        <button
                          type="button"
                          class="btn small"
                          disabled={tried()[agent.id] === 'trying'}
                          onClick={() => void tryIt(agent)}
                        >
                          Try it
                        </button>
                        <button
                          type="button"
                          class="btn small"
                          aria-expanded={editing() === agent.id}
                          onClick={() => (editing() === agent.id ? setEditing(null) : openForm(agent))}
                        >
                          Change
                        </button>
                        <Show when={agent.changed}>
                          <button
                            type="button"
                            class="btn small ghost"
                            disabled={busy()}
                            title="Go back to the command the editor comes with"
                            onClick={() => putBack(agent)}
                          >
                            Put back
                          </button>
                        </Show>
                        <Show when={agent.own}>
                          <button
                            type="button"
                            class="btn small ghost"
                            disabled={busy()}
                            onClick={() => act(() => removeAgent(agent.id))}
                          >
                            Remove
                          </button>
                        </Show>
                      </div>
                      <Show when={editing() === agent.id}>{form(agent)}</Show>
                    </div>
                  )}
                </For>
                <div class="stage-row setting-row">
                  <label class="name" for="default-agent">
                    A new task starts with
                  </label>
                  <select
                    id="default-agent"
                    class="select-button"
                    disabled={busy()}
                    onChange={(event) => {
                      const id = event.currentTarget.value || null;
                      void act(() => setSettings({ defaultAgent: id }));
                    }}
                  >
                    <option value="" selected={known().defaultAgent === null}>
                      The agent you used last, or the first that is installed
                    </option>
                    <For each={known().agents}>
                      {(agent) => (
                        <option value={agent.id} selected={agent.isDefault} disabled={!agent.available}>
                          {agent.name}
                          {agent.available ? '' : ' (not installed)'}
                        </option>
                      )}
                    </For>
                  </select>
                  <button
                    type="button"
                    class="btn small"
                    aria-expanded={editing() === NEW}
                    onClick={() => (editing() === NEW ? setEditing(null) : openForm(null))}
                  >
                    Add an agent
                  </button>
                  <Show when={editing() === NEW}>{form(null)}</Show>
                </div>
              </section>

              <ToolServers />

              <section class="stage" aria-label="Notifications">
                <div class="stage-head">
                  <h2 class="stage-title">Notifications</h2>
                  <span class="stage-note">What you are told while the editor is not in front.</span>
                </div>
                <div class="stage-row setting-row">
                  <div class="setting-what">
                    <span class="name">Tell me what happens while I am not looking</span>
                    <span class="soft">
                      {known().notifications.byApp
                        ? 'The app shows them as notifications of the system, with its window closed too.'
                        : 'This browser shows them. Turning them on asks it for permission; a click on one brings this window forward and opens the task.'}
                    </span>
                  </div>
                  <Switch
                    on={known().notifications.on}
                    label="Notifications"
                    disabled={busy()}
                    onChange={turnNotifications}
                  />
                </div>
                <Show when={refused()}>
                  <p class="stage-row plain danger" role="alert">
                    {refused()}
                  </p>
                </Show>
                <Show when={known().notifications.on && !known().notifications.byApp && allowed() !== 'granted'}>
                  <div class="stage-row setting-row" role="status">
                    <span class="setting-what soft">
                      They are on, but this browser has not been allowed to show them, so nothing is shown here.
                    </span>
                    <button type="button" class="btn small" disabled={busy()} onClick={() => turnNotifications(true)}>
                      Allow in this browser
                    </button>
                  </div>
                </Show>
                <For each={NOTICE_KINDS}>
                  {(kind) => (
                    <div class="stage-row setting-row">
                      <span class="setting-what">{NOTICE_WORDS[kind]}</span>
                      <Switch
                        on={known().notifications.on && known().notifications[kind]}
                        label={NOTICE_WORDS[kind]}
                        disabled={busy() || !known().notifications.on}
                        onChange={(on) => act(() => setSettings({ notifications: { [kind]: on } }))}
                      />
                    </div>
                  )}
                </For>
                <p class="stage-row plain">
                  Nothing is told while a window of the editor is in front, and each thing is told once. While either of
                  the two about merge requests is on, the core asks GitHub or GitLab how they stand about once a minute.
                </p>
              </section>

              <section class="stage" aria-label="The computer">
                <div class="stage-head">
                  <h2 class="stage-title">The computer</h2>
                </div>
                <div class="stage-row setting-row">
                  <div class="setting-what">
                    <span class="name">Keep the computer awake while an agent works</span>
                    <span class="soft">
                      {known().keepAwake.available
                        ? `Runs ${known().keepAwake.how} for as long as an agent is working and ends it when the last one stops. Closing the lid still puts the computer to sleep.`
                        : known().keepAwake.note}
                    </span>
                    <Show when={known().keepAwake.active}>
                      <span class="fine" role="status">
                        An agent is working: the computer is being kept awake right now.
                      </span>
                    </Show>
                  </div>
                  <Switch
                    on={known().keepAwake.on}
                    label="Keep the computer awake while an agent works"
                    // One that was on before the helper went missing can still be turned off.
                    disabled={busy() || (!known().keepAwake.available && !known().keepAwake.on)}
                    onChange={(on) => act(() => setSettings({ keepAwake: on }))}
                  />
                </div>
              </section>

              <section class="stage" aria-label="The editor">
                <div class="stage-head">
                  <h2 class="stage-title">The editor</h2>
                </div>
                <div class="stage-row setting-row">
                  <div class="setting-what">
                    <label class="name" for="theme-choice">
                      Theme
                    </label>
                    <span class="soft">
                      Dark is the editor as it was designed. "As the computer" follows the system and changes with it.
                      This is kept in this browser, so another browser, or the app, has a choice of its own.
                    </span>
                  </div>
                  <select
                    id="theme-choice"
                    class="select-button"
                    onChange={(event) => chooseTheme(event.currentTarget.value as ThemeChoice)}
                  >
                    <For each={THEMES}>
                      {(theme) => (
                        <option value={theme.value} selected={themeChoice() === theme.value}>
                          {theme.name}
                        </option>
                      )}
                    </For>
                  </select>
                </div>
                <div class="stage-row setting-row">
                  <div class="setting-what">
                    <span class="name">Format files on save in new projects</span>
                    <span class="soft">
                      Saving tidies the file first, where a language server can. This is what a new project starts with:
                      each project has its own switch under its code, and a project you have already keeps what it has.
                    </span>
                  </div>
                  <Switch
                    on={known().formatOnSave}
                    label="Format files on save in new projects"
                    disabled={busy()}
                    onChange={(on) => act(() => setSettings({ formatOnSave: on }))}
                  />
                </div>
                <div class="stage-row setting-row">
                  <div class="setting-what">
                    <span class="name">Offer a rule after a correction</span>
                    <span class="soft">
                      When you rewrite a line an agent wrote, the conversation asks whether to make a rule of it. Off,
                      nobody is asked. On, a project where you pressed “Stop asking here” stays quiet all the same.
                    </span>
                  </div>
                  <Switch
                    on={known().ruleOffers}
                    label="Offer a rule after a correction"
                    disabled={busy()}
                    onChange={(on) => act(() => setSettings({ ruleOffers: on }))}
                  />
                </div>
              </section>

              <section class="stage" aria-label="About">
                <div class="stage-head">
                  <h2 class="stage-title">About</h2>
                </div>
                <div class="stage-row setting-row">
                  <span class="setting-what">Agent Editor</span>
                  <span class="mono">version {state.app?.version ?? 'unknown'}</span>
                </div>
                <div class="stage-row setting-row">
                  <div class="setting-what">
                    <span class="name">The first-run setup</span>
                    <span class="soft">
                      Connecting agents and services and adding a project, step by step, as on the first day.
                    </span>
                  </div>
                  <button type="button" class="btn small" onClick={() => void showOnboardingAgain()}>
                    Open it again
                  </button>
                </div>
                <div class="stage-row setting-row">
                  <div class="setting-what">
                    <span class="name">Where the data lives</span>
                    <code class="setting-command">{state.app?.dataDir}</code>
                    <span class="soft">
                      Projects, tasks, conversations, these settings and the working copies of tasks are in this folder,
                      on this computer.
                    </span>
                  </div>
                </div>
                <div class="stage-row setting-row">
                  <div class="setting-what">
                    <span class="name">Daily budget</span>
                    <span class="soft">How much the agents may spend in a day is set next to what they cost.</span>
                  </div>
                  <a class="btn small" href="#/agents">
                    Open Agents
                  </a>
                </div>
              </section>
            </>
          )}
        </Show>
      </div>
    </main>
  );
}
