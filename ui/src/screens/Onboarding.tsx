import { createEffect, createSignal, For, Match, on, onCleanup, Show, Switch, type JSX } from 'solid-js';
import { go, taskHref } from '../app/router';
import { core } from '../core/client';
import { askPermission, refusal } from '../core/notify';
import {
  agentFrom,
  agentsHave,
  finishOnboarding,
  getFirstRun,
  glanceAt,
  offersFor,
  onboardingOpen,
  projectHave,
  servicesHave,
  setOnboardingOpen,
  type Found,
  type Glance,
  type Offer,
} from '../core/onboarding';
import { getSettings, saveAgent, setSettings, tryAgent, type Tried } from '../core/settings';
import { chooseFolder, shellMark } from '../core/shell';
import {
  addProject,
  createTask,
  integrationsStatus,
  linearAccount,
  reloadAgents,
  setLinearKey,
  state,
  type AgentDef,
  type Integrations,
  type ToolState,
} from '../core/store';
import { Chip } from '../components/bits';

const STEPS = ['Agents', 'Services', 'A project', 'Ready'] as const;
const GROUPS: { id: Offer['preset']['group']; title: string; about?: string }[] = [
  {
    id: 'terminal',
    title: 'Programs found on this computer',
    about: 'Command-line agents you already have. Each runs in a terminal inside the task, as you use it yourself.',
  },
  {
    id: 'protocol',
    title: 'More agents that speak the protocol',
    about:
      'Their edits, questions and cost show in the conversation, as with the agents above. From the registry of the Agent Client Protocol.',
  },
  { id: 'key', title: 'With an API key' },
  {
    id: 'models',
    title: 'Models of other makers',
    about: 'DeepSeek, Kimi, GLM, Qwen, MiniMax and whatever OpenRouter passes on to, each with a key of yours.',
  },
  { id: 'local', title: 'A model of your own' },
];
/** How many agents of a group are listed before "Show all". */
const SHOWN_AT_FIRST = 4;

const said = (error: unknown) => (error instanceof Error ? error.message : String(error));

/** "This is what you have now": the pay-off of a step, said in sentences. */
function Have(props: { lines: string[]; label: string }): JSX.Element {
  return (
    <section class="first-have" aria-label={props.label}>
      <h3>What you have now</h3>
      <ul>
        <For each={props.lines}>{(line) => <li>{line}</li>}</For>
      </ul>
    </section>
  );
}

/**
 * The dialog of the first run. Four steps: connect agents, connect services,
 * add a project, and a last one that sums up. Every step can be skipped, and
 * closing the dialog puts it aside for good; Settings brings it back.
 */
export function Onboarding(): JSX.Element {
  const [step, setStep] = createSignal(0);
  const [problem, setProblem] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal(false);
  let dialog: HTMLElement | undefined;

  // The first time the editor is opened, the dialog opens by itself.
  createEffect(() => {
    if (state.ready && state.app && state.app.onboarded === false) setOnboardingOpen(true);
  });
  // Another window finished it, or brought it back.
  const stop = core.onEvent((event) => {
    if (event.event === 'onboarding.changed') setOnboardingOpen(event.data.done !== true);
  });
  onCleanup(stop);

  /** Does one thing at a time and says what went wrong. */
  const act = async (work: () => Promise<void>) => {
    if (busy()) return;
    setBusy(true);
    setProblem(null);
    try {
      await work();
    } catch (error) {
      setProblem(said(error));
    } finally {
      setBusy(false);
    }
  };

  // ----- agents -----

  const [found, setFound] = createSignal<Found | null>(null);
  /** The ready-made agent whose form is open. */
  const [adding, setAdding] = createSignal<string | null>(null);
  const [typed, setTyped] = createSignal<Record<string, string>>({});
  const [tried, setTried] = createSignal<Record<string, Tried | 'trying'>>({});
  /** The agents added here, by the ready-made one they came from. */
  const [added, setAdded] = createSignal<Record<string, string>>({});
  const offers = () => (found() ? offersFor(found()!) : []);
  /** The groups whose every agent is listed, not only the first few. */
  const [unfolded, setUnfolded] = createSignal<string[]>([]);
  const inGroup = (group: string) => offers().filter((offer) => offer.preset.group === group);
  const listed = (group: string) => {
    const all = inGroup(group);
    // One that was added, or whose form is open, stays in view whatever the fold.
    return unfolded().includes(group)
      ? all
      : all.filter(
          (offer, index) => index < SHOWN_AT_FIRST || added()[offer.preset.id] || adding() === offer.preset.id,
        );
  };
  /** Only agents with a model behind them are worth trying: the practice agent always starts. */
  const agents = () => state.agents;

  const openForm = (offer: Offer) => {
    setProblem(null);
    setTyped(Object.fromEntries(offer.preset.asks.map((ask) => [ask.id, ask.value ?? ''])));
    setAdding(offer.preset.id);
  };

  const add = (offer: Offer) =>
    // eslint-disable-next-line solid/reactivity -- called later, as an event handler: it reads what is current then
    act(async () => {
      const made = agentFrom(offer.preset, typed(), offer.program ?? undefined);
      if ('problem' in made) throw new Error(made.problem);
      const saved = await saveAgent(made.agent);
      await reloadAgents();
      setAdded((now) => ({ ...now, [offer.preset.id]: saved.saved }));
      setAdding(null);
    });

  const tryIt = async (agent: AgentDef) => {
    setTried((now) => ({ ...now, [agent.id]: 'trying' }));
    try {
      const result = await tryAgent(agent.id);
      setTried((now) => ({ ...now, [agent.id]: result }));
    } catch (error) {
      setTried((now) => ({ ...now, [agent.id]: { ok: false, problem: said(error) } }));
    }
  };

  // ----- services -----

  const [services, setServices] = createSignal<Integrations | null>(null);
  const [linearKey, setLinearKeyText] = createSignal('');
  const [linearName, setLinearName] = createSignal<string | null>(null);
  const lookAtServices = () =>
    // eslint-disable-next-line solid/reactivity -- called later, as an event handler: it reads what is current then
    act(async () => {
      const now = await integrationsStatus();
      setServices(now);
      if (now.linear.hasKey && linearName() === null) {
        // Says whose key it is, which is also the proof that the key works.
        const account = await linearAccount().catch(() => null);
        setLinearName(account?.name ?? account?.workspace ?? null);
      }
    });

  const connectLinear = () =>
    // eslint-disable-next-line solid/reactivity -- called later, as an event handler: it reads what is current then
    act(async () => {
      const key = linearKey().trim();
      if (key === '') throw new Error('Paste a Linear API key first');
      await setLinearKey(key);
      try {
        const account = await linearAccount();
        setLinearName(account.name ?? account.workspace ?? 'your account');
      } catch (error) {
        // A key Linear does not take is not kept.
        await setLinearKey(null);
        throw new Error(`Linear did not take the key: ${said(error)}`, { cause: error });
      }
      setLinearKeyText('');
      setServices(await integrationsStatus());
    });

  const stateOf = (tool: ToolState) => (tool.installed && tool.signedIn ? 'Connected' : 'Not connected');

  // ----- a project -----

  const [folder, setFolder] = createSignal('');
  const [glance, setGlance] = createSignal<Glance | null>(null);
  const inApp = () => shellMark() !== null;

  const lookAtProject = (projectId: string) => glanceAt(projectId).then(setGlance, () => setGlance(null));

  const addFolder = () =>
    // eslint-disable-next-line solid/reactivity -- called later, as an event handler: it reads what is current then
    act(async () => {
      const path = folder().trim();
      if (path === '') throw new Error('Give the folder of a project');
      const project = await addProject(path);
      setFolder('');
      await lookAtProject(project.id);
    });

  const pick = () =>
    act(async () => {
      const path = await chooseFolder();
      if (path) setFolder(path);
    });

  // ----- the last step -----

  const [notices, setNotices] = createSignal<{ on: boolean; byApp: boolean } | null>(null);
  const [refused, setRefused] = createSignal<string | null>(null);

  const turnNotices = (turnOn: boolean) =>
    // eslint-disable-next-line solid/reactivity -- called later, as an event handler: it reads what is current then
    act(async () => {
      setRefused(null);
      if (turnOn && !notices()?.byApp) {
        const why = refusal(await askPermission());
        if (why) {
          setRefused(why);
          return;
        }
      }
      const now = await setSettings({ notifications: { on: turnOn } });
      setNotices(now.notifications);
    });

  const practice = () => state.agents.find((agent) => agent.id === 'mock' && agent.available);
  /** The project the last step offers a first task in: the one that was just looked at. */
  const firstProject = () => state.projects.find((project) => project.path === glance()?.path) ?? state.projects[0];

  const tryATask = () =>
    // eslint-disable-next-line solid/reactivity -- called later, as an event handler: it reads what is current then
    act(async () => {
      const project = firstProject();
      const agent = practice();
      if (!project || !agent) return;
      const taskId = await createTask(project.id, agent.id, 'Describe what this project is and how it is laid out');
      await finishOnboarding();
      go(taskHref(taskId, 'code'));
    });

  // ----- moving through the steps -----

  // What a step shows is read as the step opens, and again when the dialog is opened anew.
  createEffect(
    on([onboardingOpen, step], ([open, at]) => {
      if (!open) return;
      setProblem(null);
      if (at === 0)
        void getFirstRun().then(
          (first) => setFound(first.programs),
          (error) => setProblem(said(error)),
        );
      if (at === 1) void lookAtServices();
      if (at === 2 && glance() === null && state.projects[0]) void lookAtProject(state.projects[0].id);
      if (at === 3)
        void getSettings().then(
          (now) => setNotices(now.notifications),
          () => {},
        );
      // Each step starts at its heading, for the eye and for a screen reader alike.
      queueMicrotask(() => dialog?.querySelector<HTMLElement>('[data-step-title]')?.focus());
    }),
  );
  createEffect(
    on(onboardingOpen, (open) => {
      if (open) setStep(0);
    }),
  );

  const close = () => void finishOnboarding().catch((error) => setProblem(said(error)));
  const next = () => (step() < STEPS.length - 1 ? setStep(step() + 1) : close());

  /** Keeps the keyboard inside the dialog, and lets Escape put the dialog aside. */
  const onKeyDown = (event: KeyboardEvent) => {
    if (event.key === 'Escape') {
      event.preventDefault();
      close();
      return;
    }
    if (event.key !== 'Tab' || !dialog) return;
    const inside = [
      ...dialog.querySelectorAll<HTMLElement>(
        'button, [href], input, select, textarea, [tabindex="-1"][data-step-title]',
      ),
    ].filter((element) => !element.hasAttribute('disabled') && element.tabIndex !== -1);
    const [first, last] = [inside[0], inside[inside.length - 1]];
    if (!first || !last) return;
    const at = document.activeElement;
    if (event.shiftKey && (at === first || !dialog.contains(at))) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && (at === last || !dialog.contains(at))) {
      event.preventDefault();
      first.focus();
    }
  };

  const agentTag = (agent: AgentDef) => {
    if (!agent.available) return 'Not installed';
    if (agent.speaks === 'terminal') return 'In a terminal';
    // That its program is here does not say that it answers: only a try does.
    const result = tried()[agent.id];
    return result && result !== 'trying' && result.ok ? 'Works' : 'Installed';
  };

  const service = (name: string, tool: ToolState, gives: string) => (
    <div class="first-row" role="group" aria-label={name}>
      <div class="first-what">
        <span class="first-name">
          <span class="name">{name}</span>
          <span class="tag" classList={{ good: tool.installed && tool.signedIn }}>
            {stateOf(tool)}
          </span>
        </span>
        <span class="fine">{tool.installed && tool.signedIn ? gives : (tool.hint ?? gives)}</span>
      </div>
    </div>
  );

  return (
    <Show when={onboardingOpen()}>
      <div class="scrim first-scrim" />
      <section
        class="first-run"
        role="dialog"
        aria-modal="true"
        aria-labelledby="first-run-title"
        ref={dialog}
        onKeyDown={onKeyDown}
      >
        <header class="first-head">
          <ol class="first-steps" aria-label="Steps of the setup">
            <For each={STEPS}>
              {(name, index) => (
                <li
                  classList={{ on: index() === step(), done: index() < step() }}
                  aria-current={index() === step() ? 'step' : undefined}
                >
                  <span class="first-dot">{index() + 1}</span>
                  {name}
                </li>
              )}
            </For>
          </ol>
          <button type="button" class="btn small ghost" onClick={close}>
            Skip the setup
          </button>
        </header>

        <div class="first-body">
          <Show when={problem()}>
            <p class="banner danger flush" role="alert">
              <span class="grow">{problem()}</span>
              <button type="button" class="btn tiny" onClick={() => setProblem(null)}>
                Dismiss
              </button>
            </p>
          </Show>

          <Switch>
            <Match when={step() === 0}>
              <h2 id="first-run-title" class="first-title" tabindex="-1" data-step-title>
                Connect your agents
              </h2>
              <p class="soft">
                The editor writes no code itself: agents do, and it gives each task to one of them. An agent is a
                program on this computer. It signs in, or takes a key, in its own way; the editor only starts it.
              </p>
              <div class="first-list" aria-label="Agents the editor knows">
                <For each={agents()}>
                  {(agent) => (
                    <div class="first-row" role="group" aria-label={agent.name}>
                      <div class="first-what">
                        <span class="first-name">
                          <Chip agentId={agent.id} short={agent.short} flat />
                          <span class="name">{agent.name}</span>
                          <span class="tag" classList={{ good: agent.available }}>
                            {agentTag(agent)}
                          </span>
                        </span>
                        <Show when={agent.note}>
                          <span class="fine">{agent.note}</span>
                        </Show>
                        <Show when={tried()[agent.id]}>
                          {(result) => (
                            <span
                              class="fine"
                              role="status"
                              classList={{ 'text-bad': result() !== 'trying' && !(result() as Tried).ok }}
                            >
                              {result() === 'trying'
                                ? `Starting ${agent.name}. An agent that is downloaded first takes a while.`
                                : (result() as Tried).ok
                                  ? (result() as Extract<Tried, { ok: true }>).says
                                  : (result() as Extract<Tried, { ok: false }>).problem}
                            </span>
                          )}
                        </Show>
                      </div>
                      <Show when={agent.available && agent.spendsTokens}>
                        <button
                          type="button"
                          class="btn small"
                          disabled={tried()[agent.id] === 'trying'}
                          onClick={() => void tryIt(agent)}
                        >
                          Try it
                        </button>
                      </Show>
                    </div>
                  )}
                </For>
              </div>

              <For each={GROUPS}>
                {(group) => (
                  <Show when={inGroup(group.id).length > 0}>
                    <h3 class="first-group">{group.title}</h3>
                    <Show when={group.about}>
                      <p class="fine first-about">{group.about}</p>
                    </Show>
                    <div class="first-list" role="group" aria-label={group.title}>
                      <For each={listed(group.id)}>
                        {(offer) => (
                          <div class="first-row" role="group" aria-label={offer.preset.title}>
                            <div class="first-what">
                              <span class="first-name">
                                <span class="name">{offer.preset.title}</span>
                                <Show when={added()[offer.preset.id]}>
                                  <span class="tag good">Added</span>
                                </Show>
                              </span>
                              <span class="fine">{offer.missing ?? offer.preset.about}</span>
                            </div>
                            <Show when={!added()[offer.preset.id]}>
                              <button
                                type="button"
                                class="btn small"
                                disabled={!offer.ready || busy()}
                                aria-expanded={offer.preset.asks.length > 0 ? adding() === offer.preset.id : undefined}
                                onClick={() => (offer.preset.asks.length === 0 ? void add(offer) : openForm(offer))}
                              >
                                {offer.preset.asks.length === 0 ? 'Add' : 'Set up'}
                              </button>
                            </Show>
                            <Show when={adding() === offer.preset.id}>
                              <form
                                class="first-form"
                                aria-label={offer.preset.title}
                                onSubmit={(event) => {
                                  event.preventDefault();
                                  void add(offer);
                                }}
                              >
                                <For each={offer.preset.asks}>
                                  {(ask) => (
                                    <>
                                      <label for={`first-${offer.preset.id}-${ask.id}`}>{ask.label}</label>
                                      <input
                                        id={`first-${offer.preset.id}-${ask.id}`}
                                        class="field mono"
                                        type={ask.secret ? 'password' : 'text'}
                                        autocomplete="off"
                                        spellcheck={false}
                                        placeholder={ask.placeholder}
                                        value={typed()[ask.id] ?? ''}
                                        onInput={(event) =>
                                          setTyped((now) => ({ ...now, [ask.id]: event.currentTarget.value }))
                                        }
                                      />
                                    </>
                                  )}
                                </For>
                                <Show when={offer.preset.asks.some((ask) => ask.secret)}>
                                  <span class="fine">
                                    The key is kept by the core on this computer, in its own database, as it was typed.
                                    It is given to this agent and to nothing else.
                                  </span>
                                </Show>
                                <div class="actions">
                                  <button type="submit" class="btn primary" disabled={busy()}>
                                    Add the agent
                                  </button>
                                  <button type="button" class="btn ghost" onClick={() => setAdding(null)}>
                                    Cancel
                                  </button>
                                </div>
                              </form>
                            </Show>
                          </div>
                        )}
                      </For>
                      <Show when={inGroup(group.id).length > listed(group.id).length}>
                        <div class="first-row">
                          <button
                            type="button"
                            class="btn small ghost"
                            onClick={() => setUnfolded((now) => [...now, group.id])}
                          >
                            Show all {inGroup(group.id).length}
                          </button>
                        </div>
                      </Show>
                    </div>
                  </Show>
                )}
              </For>
              <Have lines={agentsHave(agents())} label="What your agents give you" />
            </Match>

            <Match when={step() === 1}>
              <h2 id="first-run-title" class="first-title" tabindex="-1" data-step-title>
                Connect your services
              </h2>
              <p class="soft">
                The editor reaches GitHub and GitLab through their own programs, so it keeps no password of yours: you
                sign in once, in a terminal, and it uses that.
              </p>
              <Show when={services()} fallback={<p class="soft">Looking at what is connected</p>}>
                {(known) => (
                  <>
                    <div class="first-list">
                      {service('GitHub', known().github, 'Issues, pull requests and their checks.')}
                      {service('GitLab', known().gitlab, 'Issues, merge requests and their pipelines.')}
                      <div class="first-row" role="group" aria-label="Linear">
                        <div class="first-what">
                          <span class="first-name">
                            <span class="name">Linear</span>
                            <span class="tag" classList={{ good: known().linear.hasKey }}>
                              {known().linear.hasKey ? 'Connected' : 'Not connected'}
                            </span>
                          </span>
                          <span class="fine">
                            <Show
                              when={known().linear.hasKey}
                              fallback="Paste an API key of your own: Linear, Settings, Security and access, Personal API keys."
                            >
                              {linearName() ? `Signed in as ${linearName()}.` : 'A key is kept.'} Issues assigned to you
                              start tasks.
                            </Show>
                          </span>
                        </div>
                        <Show when={!known().linear.hasKey}>
                          <form
                            class="first-form"
                            aria-label="Linear"
                            onSubmit={(event) => {
                              event.preventDefault();
                              void connectLinear();
                            }}
                          >
                            <label for="first-linear-key">Linear API key</label>
                            <input
                              id="first-linear-key"
                              class="field mono"
                              type="password"
                              autocomplete="off"
                              spellcheck={false}
                              placeholder="lin_api_…"
                              value={linearKey()}
                              onInput={(event) => setLinearKeyText(event.currentTarget.value)}
                            />
                            <div class="actions">
                              <button type="submit" class="btn primary" disabled={busy()}>
                                Connect Linear
                              </button>
                            </div>
                          </form>
                        </Show>
                      </div>
                      {service('Docker', known().docker, 'Services of a compose file, started for each task.')}
                    </div>
                    <p>
                      <button type="button" class="btn small" disabled={busy()} onClick={() => void lookAtServices()}>
                        Check again
                      </button>{' '}
                      <span class="fine">After you signed in or started Docker in another window.</span>
                    </p>
                    <Have lines={servicesHave(known())} label="What your services give you" />
                  </>
                )}
              </Show>
            </Match>

            <Match when={step() === 2}>
              <h2 id="first-run-title" class="first-title" tabindex="-1" data-step-title>
                Add a project
              </h2>
              <p class="soft">
                A project is a folder with code. Agents work on tasks inside it; in a git repository each task gets a
                copy of its own, so nothing touches the files you have open elsewhere.
              </p>
              <form
                class="first-form plain"
                aria-label="A new project"
                onSubmit={(event) => {
                  event.preventDefault();
                  void addFolder();
                }}
              >
                <label for="first-folder">Folder of the project</label>
                <div class="first-inline">
                  <input
                    id="first-folder"
                    class="field mono grow"
                    spellcheck={false}
                    placeholder={state.app?.cwd ?? '/path/to/your/project'}
                    value={folder()}
                    onInput={(event) => setFolder(event.currentTarget.value)}
                  />
                  <Show when={inApp()}>
                    <button type="button" class="btn" disabled={busy()} onClick={() => void pick()}>
                      Choose a folder
                    </button>
                  </Show>
                  <button type="submit" class="btn primary" disabled={busy()}>
                    Add the project
                  </button>
                </div>
              </form>
              <Show when={state.projects.length > 0}>
                <div class="first-list" aria-label="Your projects">
                  <For each={state.projects}>
                    {(project) => (
                      <div class="first-row" role="group" aria-label={project.name}>
                        <div class="first-what">
                          <span class="first-name">
                            <span class="name">{project.name}</span>
                            <span class="tag good">Added</span>
                          </span>
                          <span class="fine mono">{project.path}</span>
                        </div>
                      </div>
                    )}
                  </For>
                </div>
              </Show>
              <Show when={glance()}>
                {(seen) => <Have lines={projectHave(seen())} label="What the project gives you" />}
              </Show>
            </Match>

            <Match when={step() === 3}>
              <h2 id="first-run-title" class="first-title" tabindex="-1" data-step-title>
                You are set up
              </h2>
              <p class="soft">
                Describe a task on Home, pick an agent, and it starts working. Everything here can be changed later in
                Settings, and this setup can be opened again from there.
              </p>
              <Have
                lines={[
                  ...agentsHave(agents()),
                  ...(services() ? servicesHave(services()!) : []),
                  ...(glance() ? projectHave(glance()!) : []),
                ]}
                label="Everything you have now"
              />
              <Show when={notices()}>
                {(known) => (
                  <div class="first-row" role="group" aria-label="Notifications">
                    <div class="first-what">
                      <span class="first-name">
                        <span class="name">Tell me when an agent needs me</span>
                      </span>
                      <span class="fine">
                        {refused() ??
                          'A notification when an agent asks a question or finishes its turn, only while the editor is not in front.'}
                      </span>
                    </div>
                    <button
                      type="button"
                      class="btn small"
                      classList={{ primary: known().on }}
                      aria-pressed={known().on}
                      aria-label={`Notifications: ${known().on ? 'on' : 'off'}`}
                      disabled={busy()}
                      onClick={() => void turnNotices(!known().on)}
                    >
                      {known().on ? 'On' : 'Off'}
                    </button>
                  </div>
                )}
              </Show>
            </Match>
          </Switch>
        </div>

        <footer class="first-foot">
          <Show when={step() > 0}>
            <button type="button" class="btn ghost" onClick={() => setStep(step() - 1)}>
              Back
            </button>
          </Show>
          <span class="grow" />
          <Show when={step() === 3 && practice() && state.projects.length > 0}>
            <button type="button" class="btn" disabled={busy()} onClick={() => void tryATask()}>
              Try a first task with the practice agent
            </button>
          </Show>
          <button type="button" class="btn primary" onClick={next}>
            {step() === STEPS.length - 1 ? 'Start using the editor' : 'Continue'}
          </button>
        </footer>
      </section>
    </Show>
  );
}
