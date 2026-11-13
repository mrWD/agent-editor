import { describe, expect, it } from 'vitest';
import { asksFlowOf, flowRequest, layoutFlow, parseFlow, type Flow } from './flow';

/** The flow of the design, as the practice agent answers it. */
const ANSWER = [
  'Here is the flow.',
  '',
  '```flow',
  'step a: Request to POST /login',
  'ask b: Over the limit? @ src/middleware/rateLimit.ts:3',
  'step* c: Respond 429 @ src/middleware/rateLimit.ts:3',
  'step d: Check the password',
  'a -> b',
  'b -> c: yes',
  'b -> d: no',
  '```',
  'Tell me if a step is missing.',
].join('\n');

const flowOf = (answer: string): Flow => {
  const read = parseFlow(answer);
  if (!read.ok) throw new Error(read.problem);
  return read.flow;
};
const problemOf = (lines: string[]): string => {
  const read = parseFlow(['```flow', ...lines, '```'].join('\n'));
  return read.ok ? 'no problem' : read.problem;
};

describe('reading a flow out of an answer', () => {
  it('takes the block and leaves the words around it', () => {
    const flow = flowOf(ANSWER);
    expect(flow.nodes).toEqual([
      { id: 'a', kind: 'step', text: 'Request to POST /login', changed: false, path: null, line: null },
      { id: 'b', kind: 'ask', text: 'Over the limit?', changed: false, path: 'src/middleware/rateLimit.ts', line: 3 },
      { id: 'c', kind: 'step', text: 'Respond 429', changed: true, path: 'src/middleware/rateLimit.ts', line: 3 },
      { id: 'd', kind: 'step', text: 'Check the password', changed: false, path: null, line: null },
    ]);
    expect(flow.arrows).toEqual([
      { from: 'a', to: 'b', label: null },
      { from: 'b', to: 'c', label: 'yes' },
      { from: 'b', to: 'd', label: 'no' },
    ]);
  });

  it('reads Windows line ends, blank lines and a text that holds a colon or an at sign', () => {
    const flow = flowOf('```flow\r\nstep a: Mail it to: ops@example.com\r\n\r\nstep b: Done\r\na -> b\r\n```\r\n');
    expect(flow.nodes[0].text).toBe('Mail it to: ops@example.com');
    expect(flow.nodes[0].path).toBeNull();
  });

  it('says in plain words what is wrong', () => {
    expect(parseFlow('The limiter allows five attempts a minute.')).toEqual({
      ok: false,
      problem: 'The answer has no flow block.',
    });
    expect(parseFlow('```flow\nstep a: One')).toEqual({ ok: false, problem: 'The flow block is not closed.' });
    expect(problemOf(['step a: One'])).toBe('A flow needs at least two steps.');
    expect(problemOf(['step a: One', 'First, the request comes in.'])).toBe(
      'Line 2 of the flow is not a step, a question or an arrow: "First, the request comes in.".',
    );
    expect(problemOf(['step a: One', 'step a: Two'])).toBe('Line 2 of the flow uses the id "a" a second time.');
    expect(problemOf(['step a: One', 'step b: Two', 'a -> c'])).toBe(
      'An arrow names "c", which is not a step of the flow.',
    );
    expect(problemOf(['step a: One', 'step b: Two'])).toBe('No arrow leads to the step "b".');
    expect(problemOf(['step a: One', 'step b: Two', 'step c: Three', 'a -> b', 'a -> c'])).toBe(
      'The step "a" leads to 2 places. Only a question may do that.',
    );
    expect(problemOf(['ask a: Sure?', 'step b: Two', 'a -> b: yes'])).toBe(
      'The question "a" needs at least two answers.',
    );
    expect(problemOf(['ask a: Sure?', 'step b: Two', 'step c: Three', 'a -> b: yes', 'a -> c'])).toBe(
      'An answer to the question "a" has no label.',
    );
    expect(problemOf(['step a: One', 'step b: Two', 'a -> b', 'b -> b'])).toBe('The step "b" has an arrow to itself.');
    expect(problemOf([`step a: ${'x'.repeat(61)}`, 'step b: Two', 'a -> b'])).toBe(
      'Line 1 of the flow has more than 60 characters of text.',
    );
    const many = Array.from({ length: 31 }, (_, n) => `step s${n}: Step`);
    expect(problemOf(many)).toBe('The flow has more than 30 steps.');
  });
});

describe('asking for a flow', () => {
  it('shows the format, and the practice agent can tell the request by it', () => {
    const request = flowRequest({ path: 'src/middleware/rateLimit.ts', symbol: 'loginLimiter' });
    expect(request).toContain('Draw the flow of `loginLimiter` in `src/middleware/rateLimit.ts`');
    expect(request).toContain('```flow\n');
    expect(request).toContain('Do not change any file.');
    // The example in the request is itself a flow that can be drawn.
    expect(parseFlow(request).ok).toBe(true);
  });

  it('knows its own request again, for the same centre only', () => {
    const limiter = { path: 'src/middleware/rateLimit.ts', symbol: 'loginLimiter' };
    const file = { path: 'src/middleware/rateLimit.ts', symbol: null };
    expect(asksFlowOf(flowRequest(limiter), limiter)).toBe(true);
    expect(asksFlowOf(flowRequest(file), file)).toBe(true);
    expect(asksFlowOf(flowRequest(limiter), file)).toBe(false);
    expect(asksFlowOf(flowRequest(file), limiter)).toBe(false);
    expect(asksFlowOf('Draw me a picture of the limiter', limiter)).toBe(false);
  });
});

describe('laying a flow out', () => {
  it('puts the flow of the design where the design draws it', () => {
    const layout = layoutFlow(flowOf(ANSWER));
    expect([layout.width, layout.height]).toEqual([470, 288]);
    expect(layout.boxes.map((box) => [box.id, box.x, box.y, box.w, box.h])).toEqual([
      ['a', 40, 12, 180, 44],
      ['b', 20, 96, 220, 88],
      ['c', 300, 118, 150, 44],
      ['d', 40, 232, 180, 44],
    ]);
    // Each line stops five short of its box, where the arrowhead goes.
    expect(layout.lines.map((line) => line.d)).toEqual(['M 130 56 V 91', 'M 240 140 H 295', 'M 130 184 V 227']);
    expect(layout.lines.map((line) => [line.label, line.lx, line.ly, line.anchor])).toEqual([
      [null, 142, 60, 'start'],
      ['yes', 266, 112, 'middle'],
      ['no', 142, 188, 'start'],
    ]);
  });

  it('keeps the longer answer in the first column and joins paths that meet again', () => {
    const flow = flowOf(
      [
        '```flow',
        'step a: Read the form',
        'ask b: Signed in?',
        'step c: Show the sign-in page',
        'step d: Load the cart',
        'step f: Add up the prices',
        'step e: Show the page',
        'a -> b',
        'b -> d: yes',
        'b -> c: no',
        'd -> f',
        'f -> e',
        'c -> e',
        '```',
      ].join('\n'),
    );
    const layout = layoutFlow(flow);
    const at = (id: string) => layout.boxes.find((box) => box.id === id)!;
    // "yes" leads further, so it runs down; "no" goes off to the side.
    expect([at('a').x, at('d').x, at('f').x, at('e').x]).toEqual([40, 40, 40, 40]);
    expect(at('c').x).toBe(300);
    expect(at('c').y + at('c').h / 2).toBe(at('b').y + at('b').h / 2);
    // The side path comes back in above the step both lead to.
    const joining = layout.lines.find((line) => line.d.startsWith(`M ${at('c').x + at('c').w / 2} `))!;
    expect(joining.d).toBe(`M 375 ${at('c').y + at('c').h} V ${at('e').y - 20} H 130 V ${at('e').y - 5}`);
  });

  it('gives a long text a taller box, and an arrow back up a lane on the right', () => {
    const flow = flowOf(
      [
        '```flow',
        'step a: Take the next request from the queue of waiting ones',
        'ask b: Is there another request after this one in the line?',
        'step c: Stop',
        'a -> b',
        'b -> a: yes',
        'b -> c: no',
        '```',
      ].join('\n'),
    );
    const layout = layoutFlow(flow);
    const [a, b] = layout.boxes;
    expect(a.h).toBeGreaterThan(44);
    expect([b.w, b.h]).toEqual([300, 120]);
    const back = layout.lines.find((line) => line.label === 'yes')!;
    // Out of the question's right corner, up a lane beyond every box, into the first step's side.
    expect(back.d).toBe(`M ${b.x + b.w} ${b.y + b.h / 2} H ${layout.width - 20} V ${a.y + a.h / 2} H ${a.x + a.w + 5}`);
    expect(layout.width).toBeGreaterThan(b.x + b.w + 20);
  });
});
