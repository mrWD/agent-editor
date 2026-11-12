import { describe, expect, it } from 'vitest';
import { applyEvent, inTask, type Task, type TaskInfo } from './store';

function task(): Task {
  const info: TaskInfo = {
    id: 't1',
    projectId: 'p1',
    title: 'A task',
    agentId: 'mock',
    cwd: '/tmp',
    branch: null,
    createdAt: 0,
    seenSeq: -1,
    state: 'idle',
  };
  return { info, feed: [], lastSeq: -1, lastAt: 0, changed: [] };
}

let seq = 0;
/** Wraps an event the way the core does, with the next number. */
function numbered(body: Record<string, unknown>) {
  return { ...body, seq: seq++, at: 1000 + seq };
}
function update(body: Record<string, unknown>) {
  return numbered({ type: 'update', update: body });
}
function chunk(text: string) {
  return update({ sessionUpdate: 'agent_message_chunk', content: { type: 'text', text } });
}

describe('applying core events to a task', () => {
  it('glues answer chunks into one message and starts a new one after anything else', () => {
    const t = task();
    seq = 0;
    for (const event of [
      numbered({ type: 'user_message', text: 'Hi' }),
      chunk('Hello'),
      chunk(', world'),
      update({
        sessionUpdate: 'tool_call',
        toolCallId: 'tool-1',
        title: 'Read a.ts',
        kind: 'read',
        status: 'in_progress',
      }),
      chunk('Done.'),
    ]) {
      applyEvent(t, event);
    }
    expect(t.feed.map((item) => item.kind)).toEqual(['user', 'agent', 'tool', 'agent']);
    expect(t.feed[1]).toMatchObject({ kind: 'agent', text: 'Hello, world' });
    expect(t.feed[3]).toMatchObject({ kind: 'agent', text: 'Done.' });
  });

  it('applies each event once, even when it is delivered twice', () => {
    const t = task();
    seq = 0;
    const first = chunk('once');
    applyEvent(t, first);
    applyEvent(t, first);
    expect(t.feed).toHaveLength(1);
    expect(t.feed[0]).toMatchObject({ text: 'once' });
    expect(t.lastSeq).toBe(0);
  });

  it('updates the newest tool call when an id is reused by a restarted agent', () => {
    const t = task();
    seq = 0;
    const call = () =>
      update({
        sessionUpdate: 'tool_call',
        toolCallId: 'tool-1',
        title: 'Read a.ts',
        kind: 'read',
        status: 'in_progress',
      });
    applyEvent(t, call());
    applyEvent(t, update({ sessionUpdate: 'tool_call_update', toolCallId: 'tool-1', status: 'failed' }));
    applyEvent(t, call());
    applyEvent(t, update({ sessionUpdate: 'tool_call_update', toolCallId: 'tool-1', status: 'completed' }));
    expect(t.feed.map((item) => (item.kind === 'tool' ? item.status : null))).toEqual(['failed', 'completed']);
  });

  it('keeps the file and the change a tool call carries', () => {
    const t = task();
    seq = 0;
    applyEvent(
      t,
      update({
        sessionUpdate: 'tool_call',
        toolCallId: 'tool-2',
        title: 'Edit a.ts',
        kind: 'edit',
        status: 'pending',
        locations: [{ path: 'src/a.ts', line: 3 }],
        content: [{ type: 'diff', path: 'src/a.ts', oldText: 'a\n', newText: 'b\n' }],
      }),
    );
    expect(t.feed[0]).toMatchObject({ kind: 'tool', path: 'src/a.ts', diff: { oldText: 'a\n', newText: 'b\n' } });
  });

  it('shows one plan per turn: a newer plan replaces it, the next turn gets its own', () => {
    const t = task();
    seq = 0;
    const plan = (status: string) => update({ sessionUpdate: 'plan', entries: [{ content: 'Read', status }] });
    applyEvent(t, numbered({ type: 'user_message', text: 'one' }));
    applyEvent(t, plan('pending'));
    applyEvent(t, plan('completed'));
    applyEvent(t, numbered({ type: 'user_message', text: 'two' }));
    applyEvent(t, plan('pending'));
    const plans = t.feed.filter((item) => item.kind === 'plan');
    expect(plans).toHaveLength(2);
    expect(plans[0]).toMatchObject({ entries: [{ content: 'Read', status: 'completed' }] });
  });

  it('marks a question answered with the option that was chosen', () => {
    const t = task();
    seq = 0;
    applyEvent(
      t,
      numbered({
        type: 'permission_request',
        requestId: 'r1',
        toolCall: { title: 'Edit a.ts', kind: 'edit' },
        options: [{ optionId: 'allow-once', name: 'Allow', kind: 'allow_once' }],
      }),
    );
    expect(t.feed[0]).toMatchObject({ kind: 'permission', answered: false, title: 'Edit a.ts' });
    applyEvent(t, numbered({ type: 'permission_resolved', requestId: 'r1', optionId: 'allow-once' }));
    expect(t.feed[0]).toMatchObject({ answered: true, chosen: 'allow-once' });
  });

  it('follows the state and says why a turn ended only when that is worth saying', () => {
    const t = task();
    seq = 0;
    applyEvent(t, numbered({ type: 'state', state: 'running' }));
    expect(t.info.state).toBe('running');
    applyEvent(t, numbered({ type: 'turn_end', stopReason: 'end_turn' }));
    expect(t.feed).toHaveLength(0);
    applyEvent(t, numbered({ type: 'turn_end', stopReason: 'cancelled' }));
    expect(t.feed[0]).toMatchObject({ kind: 'notice', text: 'You stopped this turn.' });
    applyEvent(t, numbered({ type: 'error', message: 'It broke' }));
    expect(t.feed[1]).toMatchObject({ kind: 'notice', tone: 'danger', text: 'It broke' });
  });

  it('shows files by their path inside the task, however the agent names them', () => {
    const t = task();
    seq = 0;
    applyEvent(
      t,
      update({
        sessionUpdate: 'tool_call',
        toolCallId: 'tool-9',
        title: 'Edit /tmp/src/a.ts',
        kind: 'edit',
        status: 'pending',
        locations: [{ path: '/tmp/src/a.ts' }],
      }),
    );
    expect(t.feed[0]).toMatchObject({ kind: 'tool', path: 'src/a.ts', title: 'Edit src/a.ts' });
    expect(inTask('/tmp', '/tmpfile/a.ts')).toBe('/tmpfile/a.ts');
    expect(inTask('C:\\work\\shop', 'C:\\work\\shop\\src\\a.ts')).toBe('src/a.ts');
  });

  it('keeps what the user changed by hand: a note per file, and the list sent with a message', () => {
    const t = task();
    seq = 0;
    applyEvent(t, numbered({ type: 'user_edit', path: 'src/a.ts' }));
    applyEvent(t, numbered({ type: 'user_message', text: 'Go on', edited: ['src/a.ts'] }));
    expect(t.feed[0]).toMatchObject({ kind: 'edit', path: 'src/a.ts' });
    expect(t.feed[1]).toMatchObject({ kind: 'user', text: 'Go on', edited: ['src/a.ts'] });
  });

  it('shows what was done with git in the conversation', () => {
    const t = task();
    seq = 0;
    applyEvent(t, numbered({ type: 'git', action: 'committed', subject: 'Add a limit', short: '9f8e7d6' }));
    expect(t.feed[0]).toMatchObject({ kind: 'git', action: 'committed', subject: 'Add a limit', short: '9f8e7d6' });
  });

  it('notes a restarted agent, but not the first start', () => {
    const t = task();
    seq = 0;
    applyEvent(t, numbered({ type: 'agent_started', resumed: false }));
    expect(t.feed).toHaveLength(0);
    applyEvent(t, numbered({ type: 'agent_started', resumed: true }));
    expect(t.feed[0]).toMatchObject({ kind: 'notice', tone: 'muted' });
  });
});
