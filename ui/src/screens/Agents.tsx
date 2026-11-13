import { createSignal, For, onCleanup, onMount, Show, type JSX } from 'solid-js';
import { taskHref } from '../app/router';
import { ago, Chip, money, whenLabel } from '../components/bits';
import { core } from '../core/client';
import {
  addRule,
  ALL_PROJECTS,
  listRules,
  projectName,
  removeRule,
  setBudget,
  state,
  usageSummary,
  type AgentUsage,
  type Rule,
  type UsageSummary,
} from '../core/store';

const message = (error: unknown) => (error instanceof Error ? error.message : String(error));

/** How a limit's window is called in plain words. */
function windowName(key: string): string {
  switch (key) {
    case 'five_hour':
      return '5-hour window';
    case 'seven_day':
      return 'This week';
    case 'seven_day_opus':
    case 'seven_day_sonnet':
      return `This week, ${key.split('_').pop()}`;
    default:
      return key.replace(/_/g, ' ');
  }
}

type Window = { key: string; used: number | null; resetsAt: number | null; status: string | null };

/** The limits an agent last told about, one row per window it has. Agents that tell nothing give none. */
export function windowsOf(limit: AgentUsage['limit']): Window[] {
  const info = limit?.info;
  if (!info) return [];
  const windows = info.unifiedWindows;
  const read = (key: string, value: Record<string, unknown>): Window => ({
    key,
    used: typeof value.utilization === 'number' ? Math.round(value.utilization * 100) : null,
    // Agents give the moment in seconds.
    resetsAt: typeof value.resetsAt === 'number' ? value.resetsAt * 1000 : null,
    status: typeof value.status === 'string' ? value.status : null,
  });
  if (windows && typeof windows === 'object') {
    return Object.entries(windows as Record<string, Record<string, unknown>>).map(([key, value]) => read(key, value));
  }
  return typeof info.rateLimitType === 'string' ? [read(info.rateLimitType, info)] : [];
}

/** The headline: who is out of room, or how much was spent. */
function headline(summary: UsageSummary): string {
  const ready = summary.agents.filter((agent) => agent.available);
  const out = ready.filter((agent) =>
    windowsOf(agent.limit).some((one) => one.status !== null && one.status !== 'allowed'),
  );
  if (out.length > 0) {
    return `${out.map((agent) => agent.name).join(' and ')} ${out.length === 1 ? 'is' : 'are'} out of room`;
  }
  if (ready.length === 0) return 'No agent is installed yet';
  const working = ready.filter((agent) => agent.working > 0).length;
  const spent = summary.today > 0 ? `${money(summary.today, summary.currency)} spent today` : 'nothing spent today';
  return `${working === 0 ? 'No agent is working' : `${working} ${working === 1 ? 'agent is' : 'agents are'} working`}, ${spent}`;
}

/** Limits, cost and who is doing what, across all projects. */
export function Agents(): JSX.Element {
  const [summary, setSummary] = createSignal<UsageSummary | null>(null);
  const [problem, setProblem] = createSignal<string | null>(null);
  const [daily, setDaily] = createSignal('');
  const [rules, setRules] = createSignal<{ scope: string; rules: Rule[] }[]>([]);
  const [ruleText, setRuleText] = createSignal('');
  const [ruleScope, setRuleScope] = createSignal(ALL_PROJECTS);
  const loadRules = async () => {
    try {
      setRules((await listRules()).lists);
    } catch (error) {
      setProblem(message(error));
    }
  };
  const saveRule = async () => {
    try {
      await addRule(ruleScope(), ruleText());
      setRuleText('');
      await loadRules();
    } catch (error) {
      setProblem(message(error));
    }
  };

  const load = async () => {
    try {
      const got = await usageSummary();
      setSummary(got);
      setProblem(null);
      // What is being typed is left alone; otherwise the box shows what is stored.
      if (document.activeElement?.id !== 'budget-daily')
        setDaily(got.budget.daily === null ? '' : String(got.budget.daily));
    } catch (error) {
      setProblem(message(error));
    }
  };

  onMount(() => {
    void load();
    void loadRules();
    const stop = core.onEvent((event) => {
      if (event.event === 'rules.changed') void loadRules();
      if (event.event === 'usage.changed' || event.event === 'task.created' || event.event === 'task.archived')
        void load();
      // A turn that ended may have cost something; the state says who is working, and it changes after
      // the turn's end is told.
      if (event.event === 'task.event' && (event.data.type === 'turn_end' || event.data.type === 'state'))
        void load();
    });
    onCleanup(stop);
  });

  const saveDaily = () => {
    const text = daily().trim();
    const amount = text === '' ? null : Number(text.replace(',', '.'));
    if (amount !== null && (!Number.isFinite(amount) || amount < 0)) {
      setProblem('A budget is a number of dollars, zero or more.');
      return;
    }
    if (amount === (summary()?.budget.daily ?? null)) return;
    void setBudget({ daily: amount })
      .then(load)
      .catch((error) => setProblem(message(error)));
  };

  const stateOf = (agent: AgentUsage) => {
    if (!agent.available) return 'Not installed';
    if (agent.working > 0) return `Working on ${agent.working === 1 ? 'one task' : `${agent.working} tasks`}`;
    return agent.tasks > 0 ? `Idle, ${agent.tasks === 1 ? 'one open task' : `${agent.tasks} open tasks`}` : 'Idle';
  };

  return (
    <main class="page">
      <div class="page-inner wide">
        <h1 class="headline">{summary() ? headline(summary()!) : 'Reading what the agents cost'}</h1>
        <p class="soft">Limits, cost and who is doing what, across all projects.</p>
        <Show when={problem()}>
          <p class="banner danger flush" role="alert">
            <span class="grow">{problem()}</span>
            <button type="button" class="btn tiny" onClick={() => setProblem(null)}>
              Dismiss
            </button>
          </p>
        </Show>

        <Show when={summary()}>
          {(known) => (
            <>
              <div class="agent-cards">
                <For each={known().agents}>
                  {(agent) => (
                    <section class="card agent-card" aria-label={agent.name}>
                      <div class="agent-card-head">
                        <Chip agentId={agent.id} short={agent.short} />
                        <h2>{agent.name}</h2>
                        <span class="grow" />
                        <span class="fine">{stateOf(agent)}</span>
                      </div>
                      <Show when={!agent.available && agent.note}>
                        <p class="fine">{agent.note}</p>
                      </Show>
                      <For each={windowsOf(agent.limit)}>
                        {(one) => (
                          <div class="agent-row">
                            <span>{windowName(one.key)}</span>
                            <span class="grow" />
                            <span classList={{ 'text-bad': one.status !== null && one.status !== 'allowed' }}>
                              {one.status !== null && one.status !== 'allowed' ? 'limit reached' : ''}
                              {one.used !== null ? ` ${one.used}%` : ''}
                              {one.resetsAt ? `, resets ${whenLabel(one.resetsAt)}` : ''}
                            </span>
                          </div>
                        )}
                      </For>
                      <Show when={agent.available && agent.limit}>
                        <span class="fine">
                          As {agent.name} told it {ago(agent.limit!.at)}.
                        </span>
                      </Show>
                      <Show when={agent.available && !agent.limit && agent.spendsTokens}>
                        <span class="fine">
                          {agent.name} has not told its limits yet. It does with its first answer.
                        </span>
                      </Show>
                      <div class="agent-row">
                        <span>Today</span>
                        <span class="grow" />
                        <span>{agent.spendsTokens ? money(agent.today, known().currency) : 'costs nothing'}</span>
                      </div>
                      <Show when={agent.spendsTokens}>
                        <div class="agent-row">
                          <span>This week</span>
                          <span class="grow" />
                          <span>{money(agent.week, known().currency)}</span>
                        </div>
                      </Show>
                    </section>
                  )}
                </For>
              </div>

              <section class="stage">
                <div class="stage-head">
                  <h2 class="stage-title">Daily budget</h2>
                  <span class="stage-note">{money(known().today, known().currency)} spent today</span>
                </div>
                <div class="stage-row budget-row">
                  <label for="budget-daily">Daily budget, in dollars</label>
                  <input
                    id="budget-daily"
                    class="field narrow"
                    inputmode="decimal"
                    placeholder="none"
                    value={daily()}
                    onInput={(event) => setDaily(event.currentTarget.value)}
                    onBlur={saveDaily}
                    onKeyDown={(event) => event.key === 'Enter' && saveDaily()}
                  />
                  <label class="check">
                    <input
                      type="checkbox"
                      checked={known().budget.ask}
                      onChange={(event) => void setBudget({ ask: event.currentTarget.checked }).then(load)}
                    />
                    Ask before starting a new task once it is reached
                  </label>
                </div>
              </section>

              <section class="stage">
                <div class="stage-head">
                  <h2 class="stage-title">Cost by task, today</h2>
                </div>
                <Show when={known().tasks.length === 0}>
                  <p class="stage-row plain">No task has cost anything today.</p>
                </Show>
                <For each={known().tasks}>
                  {(task) => (
                    <div class="stage-row cost-row">
                      <Show when={!task.archived} fallback={<span class="name">{task.title}</span>}>
                        <a class="name" href={taskHref(task.taskId)}>
                          {task.title}
                        </a>
                      </Show>
                      <span class="fine">
                        {projectName(task.projectId)}
                        {task.archived ? ' · archived' : ''}
                      </span>
                      <span class="cost-agents">
                        <For each={task.agents}>
                          {(id) => (
                            <Chip
                              agentId={id}
                              short={known().agents.find((one) => one.id === id)?.short ?? '··'}
                              flat
                            />
                          )}
                        </For>
                      </span>
                      <span class="mono">{money(task.today, known().currency)}</span>
                    </div>
                  )}
                </For>
                <Show when={known().tasks.length > 0}>
                  <div class="stage-row cost-row total">
                    <span class="name">All tasks today</span>
                    <span class="mono">{money(known().today, known().currency)}</span>
                  </div>
                </Show>
              </section>
              <section class="stage" aria-label="Rules given to the agents">
                <div class="stage-head">
                  <h2 class="stage-title">Rules given to the agents</h2>
                  <span class="stage-note">
                    Each agent is told them with the first message of a session. Nothing is written into your projects.
                  </span>
                </div>
                <form
                  class="stage-row rule-add"
                  onSubmit={(event) => {
                    event.preventDefault();
                    void saveRule();
                  }}
                >
                  <input
                    class="field grow"
                    aria-label="A new rule"
                    placeholder="A new rule, in one sentence"
                    value={ruleText()}
                    onInput={(event) => setRuleText(event.currentTarget.value)}
                  />
                  <select
                    class="select-button"
                    aria-label="Where the rule applies"
                    value={ruleScope()}
                    onChange={(event) => setRuleScope(event.currentTarget.value)}
                  >
                    <option value={ALL_PROJECTS}>All my projects</option>
                    <For each={state.projects}>{(project) => <option value={project.id}>{project.name}</option>}</For>
                  </select>
                  <button type="submit" class="btn small" disabled={ruleText().trim() === ''}>
                    Add the rule
                  </button>
                </form>
                <For each={rules()}>
                  {(list) => (
                    <For each={list.rules}>
                      {(rule) => (
                        <div class="stage-row rule-row">
                          <span class="project-tag">
                            {list.scope === ALL_PROJECTS ? 'All projects' : projectName(list.scope)}
                          </span>
                          <span class="name">{rule.text}</span>
                          <span class="fine">
                            {rule.from === 'edit' ? 'From your correction' : 'Written by hand'}, {ago(rule.at)}
                            {rule.agents.length > 0
                              ? ` · only for ${rule.agents.map((id) => known().agents.find((one) => one.id === id)?.name ?? id).join(', ')}`
                              : ''}
                          </span>
                          <button
                            type="button"
                            class="btn small ghost"
                            aria-label={`Remove the rule: ${rule.text}`}
                            onClick={() =>
                              void removeRule(list.scope, rule.id)
                                .then(loadRules)
                                .catch((error) => setProblem(message(error)))
                            }
                          >
                            Remove
                          </button>
                        </div>
                      )}
                    </For>
                  )}
                </For>
                <Show when={rules().every((list) => list.rules.length === 0)}>
                  <p class="stage-row plain">
                    No rules yet. One is offered when you rewrite a line an agent wrote, or write one above.
                  </p>
                </Show>
              </section>
              <p class="fine">
                The numbers are what each agent reports with its answers; nothing is estimated. Inside a task the same
                numbers sit in the agent panel, next to the agent's name.
              </p>
            </>
          )}
        </Show>
      </div>
    </main>
  );
}
