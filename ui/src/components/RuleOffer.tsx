import { createEffect, createSignal, For, Show, type JSX } from 'solid-js';
import {
  addRule,
  agentOf,
  ALL_PROJECTS,
  projectName,
  sendPrompt,
  setRuleOffers,
  state,
  type Task,
} from '../core/store';
import { setWork, workOf } from '../core/work';

const message = (error: unknown) => (error instanceof Error ? error.message : String(error));

/**
 * "Make this a rule?" It is asked only when the user has rewritten a line an
 * agent wrote, never for the user's own code: a correction is the moment a
 * rule is worth keeping. The user writes the rule, or lets the agent word it.
 */
export function RuleOffer(props: { task: Task }): JSX.Element {
  const taskId = () => props.task.info.id;
  const offer = () => workOf(taskId()).ruleOffer;
  const agentName = () => agentOf(props.task)?.name ?? 'the agent';
  const canAsk = () => props.task.info.state === 'idle' || props.task.info.state === 'asleep';
  const [text, setText] = createSignal('');
  const [everywhere, setEverywhere] = createSignal(false);
  const [given, setGiven] = createSignal<string[] | null>(null);
  const [problem, setProblem] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal(false);
  /** The length of the conversation when the agent was asked to word the rule. */
  const [awaited, setAwaited] = createSignal<number | null>(null);

  const agents = () => state.agents.filter((agent) => agent.available);
  const chosen = () => given() ?? agents().map((agent) => agent.id);
  const toggle = (id: string) =>
    setGiven(chosen().includes(id) ? chosen().filter((one) => one !== id) : [...chosen(), id]);
  const close = () => {
    setWork(taskId(), { ruleOffer: null });
    setText('');
    setProblem(null);
    setAwaited(null);
  };

  const act = async (work: () => Promise<unknown>) => {
    if (busy()) return;
    setBusy(true);
    setProblem(null);
    try {
      await work();
    } catch (error) {
      setProblem(message(error));
    } finally {
      setBusy(false);
    }
  };

  const save = () =>
    // eslint-disable-next-line solid/reactivity -- called later, as an event handler: it reads what is current then
    act(async () => {
      const all = chosen().length === agents().length;
      await addRule(everywhere() ? ALL_PROJECTS : props.task.info.projectId, text(), {
        from: 'edit',
        // Given to every agent: also to one that is installed later.
        agents: all ? [] : chosen(),
      });
      close();
    });

  const word = () =>
    // eslint-disable-next-line solid/reactivity -- called later, as an event handler: it reads what is current then
    act(async () => {
      const now = offer()!;
      setAwaited(props.task.feed.length);
      try {
        await sendPrompt(
          taskId(),
          `In \`${now.path}\` the user rewrote what you wrote.\n\nYou wrote:\n\n\`\`\`\n${now.was.join('\n')}\n\`\`\`\n\nThe user made it:\n\n\`\`\`\n${now.now.join('\n')}\n\`\`\`\n\n` +
            'Say in one sentence the rule to follow from now on, so that this correction is not needed again. Answer with that one sentence only, and leave every file as it is.',
        );
      } catch (error) {
        setAwaited(null);
        throw error;
      }
    });
  // When that turn has ended, what the agent said last is the rule, for the user to change or keep.
  createEffect(() => {
    const from = awaited();
    if (from === null) return;
    const since = props.task.feed.slice(from);
    const asked = since.findIndex((entry) => entry.kind === 'user');
    if (asked < 0) return;
    const after = since.slice(asked + 1);
    const atRest = props.task.info.state === 'idle' || props.task.info.state === 'asleep';
    const said = after.findLast((entry) => entry.kind === 'agent');
    if (!atRest || (!said && !after.some((entry) => entry.kind === 'notice'))) return;
    if (said && said.kind === 'agent' && said.text.trim()) setText(said.text.trim().split('\n\n')[0]!.trim());
    setAwaited(null);
  });

  return (
    <Show when={offer()}>
      {(now) => (
        <section class="card pinned rule-offer" aria-label="Make this a rule?">
          <h2>Make this a rule?</h2>
          <p>
            You rewrote what {agentName()} wrote in <span class="mono">{now().path.split('/').pop()}</span>. Saved as a
            rule, no agent repeats it.
          </p>
          <label class="fine" for={`rule-${taskId()}`}>
            The rule, in one sentence
          </label>
          <textarea
            id={`rule-${taskId()}`}
            class="git-message"
            rows="2"
            placeholder="Never hardcode limits. Read them from config."
            value={text()}
            onInput={(event) => setText(event.currentTarget.value)}
          />
          <div class="git-head">
            <span class="grow fine">{awaited() !== null ? `${agentName()} is wording it` : ''}</span>
            <button
              type="button"
              class="btn tiny ghost"
              disabled={busy() || !canAsk() || awaited() !== null}
              title={`${agentName()} is shown your correction and writes the rule. It is a turn of the conversation.`}
              onClick={() => void word()}
            >
              Ask {agentName()} to word it
            </button>
          </div>
          <fieldset class="rule-scope">
            <legend>Applies to</legend>
            <label class="check">
              <input
                type="radio"
                name={`scope-${taskId()}`}
                checked={!everywhere()}
                onChange={() => setEverywhere(false)}
              />
              This project, {projectName(props.task.info.projectId)}
            </label>
            <label class="check">
              <input
                type="radio"
                name={`scope-${taskId()}`}
                checked={everywhere()}
                onChange={() => setEverywhere(true)}
              />
              All my projects
            </label>
          </fieldset>
          <Show when={agents().length > 1}>
            <fieldset class="rule-scope">
              <legend>Given to</legend>
              <For each={agents()}>
                {(agent) => (
                  <label class="check">
                    <input type="checkbox" checked={chosen().includes(agent.id)} onChange={() => toggle(agent.id)} />
                    {agent.name}
                  </label>
                )}
              </For>
            </fieldset>
          </Show>
          <Show when={problem()}>
            <p class="notice danger" role="alert">
              {problem()}
            </p>
          </Show>
          <div class="actions">
            <button
              type="button"
              class="btn primary"
              disabled={busy() || text().trim() === '' || chosen().length === 0}
              onClick={() => void save()}
            >
              Save rule
            </button>
            <button type="button" class="btn" onClick={close}>
              Not this time
            </button>
            <button
              type="button"
              class="btn ghost"
              disabled={busy()}
              onClick={() => act(() => setRuleOffers(props.task.info.projectId, false).then(close))}
            >
              Stop asking here
            </button>
          </div>
          <span class="fine">
            The question appears only when you rewrite a line an agent wrote, never for your own code.
          </span>
        </section>
      )}
    </Show>
  );
}
