import { createEffect, createSignal, For, Match, on, onCleanup, Show, Switch, type JSX } from 'solid-js';
import { go } from '../app/router';
import { Chip } from '../components/bits';
import { Tour } from '../components/Tour';
import { core } from '../core/client';
import {
  agentFrom,
  agentsHave,
  finishOnboarding,
  getFirstRun,
  glanceAt,
  offersFor,
  onboardingOpen,
  projectHave,
  setOnboardingOpen,
  type Found,
  type Glance,
  type Offer,
} from '../core/onboarding';
import { saveAgent, tryAgent, type Tried } from '../core/settings';
import { chooseFolder, shellMark } from '../core/shell';
import {
  addProject,
  integrationsStatus,
  linearAccount,
  reloadAgents,
  setLinearKey,
  state,
  type AgentDef,
  type Integrations,
  type ToolState,
} from '../core/store';
import type { TourStep } from '../core/tour';

/**
 * The chapters of the first run, in the order they are gone through. Each is
 * a dialog that connects something, then a tour of hints on the interface
 * itself that shows where the connected thing now is.
 */
const CHAPTERS = ['Trackers', 'Agents', 'Code'] as const;
const TRACKERS = 0;
const AGENTS = 1;
const CODE = 2;

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

/**
 * The first run. Three chapters (trackers, agents, code), each a dialog
 * followed by hints on the interface. A chapter with nothing connected has
 * nothing to show and goes straight on to the next. Closing the dialog puts
 * the whole first run aside for good; Settings brings it back.
 */
export function Onboarding(): JSX.Element {
  const [chapter, setChapter] = createSignal(0);
  /** Whether the chapter is at its dialog or at its hints. */
  const [touring, setTouring] = createSignal(false);
  const [problem, setProblem] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal(false);
  let dialog: HTMLElement | undefined;

  // The first time the editor is opened, the first run starts by itself.
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

  // ----- trackers and code hosts -----

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

  const connected = (tool: ToolState) => tool.installed && tool.signedIn;
  const hosts = () => {
    const known = services();
    if (!known) return [];
    return [connected(known.github) ? 'GitHub' : null, connected(known.gitlab) ? 'GitLab' : null].filter(
      (name): name is string => name !== null,
    );
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

  const agentTag = (agent: AgentDef) => {
    if (!agent.available) return 'Not installed';
    if (agent.speaks === 'terminal') return 'In a terminal';
    // That its program is here does not say that it answers: only a try does.
    const result = tried()[agent.id];
    return result && result !== 'trying' && result.ok ? 'Works' : 'Installed';
  };

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

  // ----- what each chapter has to show -----

  /** Whether the chapter connected anything: without that there is nothing on the interface to point at. */
  const hasSomething = () => {
    if (chapter() === TRACKERS) return services()?.linear.hasKey === true;
    if (chapter() === AGENTS) return agents().some((agent) => agent.available);
    return state.projects.length > 0;
  };

  /** The hints of the chapter on screen, written for what was connected. */
  const steps = (): TourStep[] => {
    if (chapter() === TRACKERS) {
      return [
        {
          target: '.header-nav a[href="#/issues"]',
          route: '#/',
          title: 'Your Linear issues are in Issues',
          text: 'Press Issues here, or Next, to look at them.',
          press: true,
        },
        {
          target: 'section.issues-linear',
          route: '#/issues',
          title: 'The issues assigned to you',
          text: 'They are read from Linear each time you open this page. “Start” turns one into a task: its title names the task, and its text is the first message to the agent.',
        },
        {
          target: '.header-nav a[href="#/"]',
          route: '#/issues',
          title: 'A task from an issue lives on Home',
          text: 'Starting one moves the issue on in Linear. When the task’s request is merged, the issue is closed by itself.',
        },
      ];
    }
    if (chapter() === AGENTS) {
      const several = agents().filter((agent) => agent.available).length > 1;
      return [
        {
          target: '.new-task select[aria-label="Agent"]',
          route: '#/',
          title: 'Every task goes to one agent',
          text: 'Pick it here when you start a task. A task can be handed to another agent later, in the head of its conversation.',
          lines: agentsHave(agents()),
        },
        ...(several
          ? [
              {
                target: '.new-task .race-toggle',
                route: '#/',
                title: 'Or to several at once',
                text: 'Race sends the same task to two or more agents, each in its own copy of the code. You compare the results and keep one.',
              },
            ]
          : []),
        {
          target: '.header-nav a[href="#/agents"]',
          route: '#/',
          title: 'Who is doing what',
          text: 'Agents shows what each agent is working on, what each task cost, and the budget for a day.',
        },
        {
          target: 'a[href="#/settings"]',
          route: '#/',
          title: 'Commands, keys and more agents',
          text: 'Settings changes how an agent is started and adds agents of your own. This setup can be opened again from there.',
        },
      ];
    }
    const from = hosts();
    return [
      {
        target: '.new-task select[aria-label="Project"]',
        route: '#/',
        title: 'A task starts in a project',
        text: 'Pick the project here. “Another folder” adds one more.',
        lines: glance() ? projectHave(glance()!) : undefined,
      },
      {
        target: '#task-what',
        route: '#/',
        title: 'Say what to do',
        text: 'Describe the task and press Start. The agent works in the task’s own copy; you read what it changed, line by line, and commit from the Changes view.',
      },
      {
        target: 'section.rail',
        route: '#/',
        title: 'Every project and its tasks',
        text: 'Each task has a mark here that says what it waits for: the agent, your answer, checks, or a merge.',
      },
      ...(from.length > 0
        ? [
            {
              target: '.header-nav a[href="#/issues"]',
              route: '#/',
              title: `Issues from ${from.join(' and ')}`,
              text: 'The open issues of your projects are listed in Issues. A task pushes its branch and opens its request from Changes, and “Merge now” merges, cleans up and closes the issue.',
            },
          ]
        : []),
    ];
  };

  // ----- moving through the chapters -----

  const close = () => void finishOnboarding().catch((error) => setProblem(said(error)));

  /** The chapter after this one, or the end of the first run. */
  const onward = () => {
    setTouring(false);
    if (chapter() === CHAPTERS.length - 1) {
      go('#/');
      close();
      return;
    }
    setChapter(chapter() + 1);
  };
  /** From a dialog: on to its hints when it has something to show, else to the next chapter. */
  const next = () => (hasSomething() ? setTouring(true) : onward());

  // What a dialog shows is read as it opens, and again when the first run is opened anew.
  createEffect(
    on([onboardingOpen, chapter, touring], ([open, at, hints]) => {
      if (!open || hints) return;
      setProblem(null);
      if (at === TRACKERS || at === CODE) void lookAtServices();
      if (at === AGENTS) {
        void getFirstRun().then(
          (first) => setFound(first.programs),
          (error) => setProblem(said(error)),
        );
      }
      if (at === CODE && glance() === null && state.projects[0]) void lookAtProject(state.projects[0].id);
      // Each dialog starts at its heading, for the eye and for a screen reader alike.
      queueMicrotask(() => dialog?.querySelector<HTMLElement>('[data-step-title]')?.focus());
    }),
  );
  createEffect(
    on(onboardingOpen, (open) => {
      if (!open) return;
      setChapter(0);
      setTouring(false);
    }),
  );

  /** Keeps the keyboard inside the dialog, and lets Escape put the first run aside. */
  const onKeyDown = (event: KeyboardEvent) => {
    if (event.key === 'Escape') {
      event.preventDefault();
      close();
      return;
    }
    if (event.key !== 'Tab' || !dialog) return;
    const inside = [...dialog.querySelectorAll<HTMLElement>('button, [href], input, select, textarea')].filter(
      (element) => !element.hasAttribute('disabled') && element.tabIndex !== -1,
    );
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

  // A press beside the dialog leaves the keyboard on nothing at all: Escape and Tab still mean the dialog.
  // While the hints are up the dialog is not there, and they look after their own keys.
  const outside = (event: KeyboardEvent) => {
    // A key the hints or the dialog itself already took is not taken twice: Escape that ended the hints
    // has just brought the dialog back, and must not put it aside in the same breath.
    if (event.defaultPrevented) return;
    if (!onboardingOpen() || touring() || !dialog || dialog.contains(event.target as Node)) return;
    onKeyDown(event);
  };
  document.addEventListener('keydown', outside);
  onCleanup(() => document.removeEventListener('keydown', outside));

  const service = (name: string, tool: ToolState, gives: string) => (
    <div class="first-row" role="group" aria-label={name}>
      <div class="first-what">
        <span class="first-name">
          <span class="name">{name}</span>
          <span class="tag" classList={{ good: connected(tool) }}>
            {connected(tool) ? 'Connected' : 'Not connected'}
          </span>
        </span>
        <span class="fine">{connected(tool) ? gives : (tool.hint ?? gives)}</span>
      </div>
    </div>
  );

  return (
    <Show when={onboardingOpen()}>
      <Show
        when={!touring()}
        fallback={
          <Tour
            name={CHAPTERS[chapter()]!}
            steps={steps()}
            lastLabel={chapter() === CHAPTERS.length - 1 ? 'Start using the editor' : 'Got it'}
            onDone={onward}
          />
        }
      >
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
              <For each={CHAPTERS}>
                {(name, index) => (
                  <li
                    classList={{ on: index() === chapter(), done: index() < chapter() }}
                    aria-current={index() === chapter() ? 'step' : undefined}
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
              <Match when={chapter() === TRACKERS}>
                <h2 id="first-run-title" class="first-title" tabindex="-1" data-step-title>
                  Connect your tracker
                </h2>
                <p class="soft">
                  A task can start from an issue: its title names the task and its text is what the agent is told.
                  Linear is connected here. Issues on GitHub and GitLab come with your code, two steps on.
                </p>
                <Show when={services()} fallback={<p class="soft">Looking at what is connected</p>}>
                  {(known) => (
                    <div class="first-list">
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
                              <Show when={known().linear.readOnly}>
                                {' '}
                                The key may only read, so issues are not moved when a task starts or merges.
                              </Show>
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
                            <span class="fine">
                              The key is kept by the core on this computer, in its own database, as it was typed.
                            </span>
                            <div class="actions">
                              <button type="submit" class="btn primary" disabled={busy()}>
                                Connect Linear
                              </button>
                            </div>
                          </form>
                        </Show>
                      </div>
                    </div>
                  )}
                </Show>
              </Match>

              <Match when={chapter() === AGENTS}>
                <h2 id="first-run-title" class="first-title" tabindex="-1" data-step-title>
                  Connect your agents
                </h2>
                <p class="soft">
                  The editor writes no code itself: agents do, and it gives each task to one of them. An agent is a
                  program on this computer. It signs in, or takes a key, in its own way; the editor only starts it.
                </p>
                <div class="first-list" role="group" aria-label="Agents the editor knows">
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
                                  aria-expanded={
                                    offer.preset.asks.length > 0 ? adding() === offer.preset.id : undefined
                                  }
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
                                      The key is kept by the core on this computer, in its own database, as it was
                                      typed. It is given to this agent and to nothing else.
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
              </Match>

              <Match when={chapter() === CODE}>
                <h2 id="first-run-title" class="first-title" tabindex="-1" data-step-title>
                  Connect your code
                </h2>
                <p class="soft">
                  The editor reaches GitHub and GitLab through their own programs, so it keeps no password of yours: you
                  sign in once, in a terminal, and it uses that. Then add a project: a folder with your code.
                </p>
                <Show when={services()} fallback={<p class="soft">Looking at what is connected</p>}>
                  {(known) => (
                    <>
                      <div class="first-list">
                        {service('GitHub', known().github, 'Issues, pull requests and their checks.')}
                        {service('GitLab', known().gitlab, 'Issues, merge requests and their pipelines.')}
                        {service('Docker', known().docker, 'Services of a compose file, started for each task.')}
                      </div>
                      <p>
                        <button type="button" class="btn small" disabled={busy()} onClick={() => void lookAtServices()}>
                          Check again
                        </button>{' '}
                        <span class="fine">After you signed in or started Docker in another window.</span>
                      </p>
                    </>
                  )}
                </Show>
                <form
                  class="first-form plain"
                  aria-label="A new project"
                  onSubmit={(event) => {
                    event.preventDefault();
                    void addFolder();
                  }}
                >
                  <label for="first-folder">Folder of a project</label>
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
                    <button type="submit" class="btn" disabled={busy()}>
                      Add the project
                    </button>
                  </div>
                </form>
                <Show when={state.projects.length > 0}>
                  <div class="first-list" role="group" aria-label="Your projects">
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
              </Match>
            </Switch>
          </div>

          <footer class="first-foot">
            <Show when={chapter() > 0}>
              <button type="button" class="btn ghost" onClick={() => setChapter(chapter() - 1)}>
                Back
              </button>
            </Show>
            <span class="grow" />
            <Show when={hasSomething()}>
              <button type="button" class="btn ghost" onClick={onward}>
                Skip the hints
              </button>
            </Show>
            <button type="button" class="btn primary" onClick={next}>
              {hasSomething() ? 'Show me where it is' : chapter() === CHAPTERS.length - 1 ? 'Finish' : 'Not now'}
            </button>
          </footer>
        </section>
      </Show>
    </Show>
  );
}
