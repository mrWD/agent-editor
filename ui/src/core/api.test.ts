import { describe, expect, it } from 'vitest';
import {
  answerForAgent,
  bodyProblem,
  contractWords,
  draftOf,
  matching,
  requestHeaders,
  requestPath,
  shownBody,
  sizeWords,
  statusTone,
  tookWords,
  type ApiAnswer,
  type ApiOperation,
} from './api';
import { describePointed } from './preview';

const operation = (over: Partial<ApiOperation> = {}): ApiOperation => ({
  id: 'GET /orders/{id}',
  method: 'GET',
  path: '/orders/{id}',
  summary: 'One order',
  tags: ['orders'],
  deprecated: false,
  parameters: [
    { name: 'id', in: 'path', required: true, kind: 'integer', example: '7', about: '' },
    { name: 'expand', in: 'query', required: false, kind: 'string', example: 'lines', about: '' },
    { name: 'X-Shop', in: 'header', required: false, kind: 'string', example: '', about: '' },
  ],
  body: null,
  responses: ['200'],
  change: null,
  ...over,
});

const answer = (over: Partial<ApiAnswer> = {}): ApiAnswer => ({
  status: 500,
  reason: 'Internal Server Error',
  headers: [['content-type', 'application/json']],
  body: '{"detail":"boom"}',
  kind: 'json',
  size: 17,
  cut: false,
  took: 12,
  sent: { method: 'POST', url: 'http://localhost:8000/api/orders?dry=1' },
  ...over,
});

describe('the request for an operation', () => {
  it('starts from what the description needs, and leaves an optional filter empty', () => {
    const draft = draftOf(operation());
    expect(draft.values).toEqual({ 'path:id': '7' });
    expect(draftOf(operation({ body: { kind: 'application/json', required: true, example: '{"a":1}' } })).body).toBe(
      '{"a":1}',
    );
  });

  it('fills the blanks of the path and adds the query, each piece written out', () => {
    const draft = { values: { 'path:id': 'a/b c', 'query:expand': 'lines&buyer' }, headers: '', body: '' };
    expect(requestPath('/api', operation(), draft)).toEqual({
      path: '/api/orders/a%2Fb%20c?expand=lines%26buyer',
      missing: [],
    });
  });

  it('names a blank of the path that has no value yet', () => {
    expect(requestPath('', operation(), { values: {}, headers: '', body: '' })).toEqual({
      path: '/orders/{id}',
      missing: ['id'],
    });
  });

  it('sends the header parameters, the lines typed, and says what the body is', () => {
    const one = operation({ body: { kind: 'application/json', required: true, example: '' } });
    const draft = { values: { 'header:X-Shop': 'berlin' }, headers: 'X-Trace: 1\n\n', body: '{}' };
    expect(requestHeaders(one, draft)).toEqual({
      headers: [
        ['X-Shop', 'berlin'],
        ['X-Trace', '1'],
        ['Content-Type', 'application/json'],
      ],
      problem: null,
    });
    // What the user says the body is stands; a body typed by hand for any request is guessed.
    expect(requestHeaders(one, { ...draft, headers: 'content-type: text/csv' }).headers).toContainEqual([
      'content-type',
      'text/csv',
    ]);
    expect(requestHeaders(null, { values: {}, headers: '', body: 'a,b' }).headers).toEqual([
      ['Content-Type', 'text/plain'],
    ]);
    expect(requestHeaders(null, { values: {}, headers: 'no colon here', body: '' }).problem).toContain(
      'is not a header',
    );
  });

  it('says why a body is not JSON before it is sent', () => {
    expect(bodyProblem('application/json', '{"a":1}')).toBeNull();
    expect(bodyProblem('application/json', '')).toBeNull();
    expect(bodyProblem('application/json', '{a:1}')).toMatch(/^The body is not JSON/);
    expect(bodyProblem('text/plain', '{a:1}')).toBeNull();
  });
});

describe('the answer', () => {
  it('is read in sizes, times and tones', () => {
    expect([sizeWords(17), sizeWords(1536), sizeWords(20 * 1024), sizeWords(3 * 1024 * 1024)]).toEqual([
      '17 B',
      '1.5 KB',
      '20 KB',
      '3.0 MB',
    ]);
    expect([tookWords(12), tookWords(1400)]).toEqual(['12 ms', '1.4 s']);
    expect([200, 302, 404, 503].map(statusTone)).toEqual(['good', 'moved', 'refused', 'failed']);
  });

  it('lays JSON out and leaves the rest as it came', () => {
    expect(shownBody(answer())).toBe('{\n  "detail": "boom"\n}');
    expect(shownBody(answer({ body: '{"cut', kind: 'json' }))).toBe('{"cut');
    expect(shownBody(answer({ body: 'plain', kind: 'text' }))).toBe('plain');
  });

  it('goes to the agent with its request, without the values of the headers kept for the project', () => {
    const made = answerForAgent(
      answer(),
      {
        headers: [
          ['Authorization', 'Bearer shop-key'],
          ['Content-Type', 'application/json'],
        ],
        body: '{"sku":"tea-1"}',
      },
      [['authorization', 'Bearer shop-key']],
    );
    expect(made.title).toBe('POST /api/orders answered 500');
    expect(made.text).toBe(
      [
        'POST http://localhost:8000/api/orders?dry=1',
        'authorization: (kept for the project, not shown)',
        'Content-Type: application/json',
        '',
        '{"sku":"tea-1"}',
        '',
        'Answered 500 Internal Server Error in 12 ms:',
        '{\n  "detail": "boom"\n}',
      ].join('\n'),
    );
    expect(made.text).not.toContain('shop-key');
  });

  it('is cut for the agent when it is long, and said to be cut', () => {
    const long = answerForAgent(
      answer({ body: 'x'.repeat(9000), kind: 'text', size: 9000 }),
      { headers: [], body: '' },
      [],
    );
    expect(long.text).toContain('… (cut: the answer is 8.8 KB)');
    expect(long.text.length).toBeLessThan(6400);
    const bytes = answerForAgent(answer({ kind: 'binary', body: '', size: 2048 }), { headers: [], body: '' }, []);
    expect(bytes.text).toContain('(2.0 KB that are not text)');
  });

  it('stands before the next message as what the user shows, with what they say about it', () => {
    const text = describePointed(null, [], '/work', [
      { n: 1, title: 'POST /orders answered 500', text: 'boom\n', note: ' it should be 201 ' },
    ]);
    expect(text).toBe(
      'The user shows you this from the preview: POST /orders answered 500. They say: "it should be 201"\n\n```\nboom\n```',
    );
  });
});

describe('the list and the contract', () => {
  it('finds operations by every word typed, in the method, the path, the summary or a tag', () => {
    const all = [
      operation(),
      operation({ id: 'POST /carts', method: 'POST', path: '/carts', summary: 'A new cart', tags: [] }),
    ];
    expect(matching(all, '')).toHaveLength(2);
    expect(matching(all, 'post cart').map((one) => one.id)).toEqual(['POST /carts']);
    expect(matching(all, 'ORDERS one').map((one) => one.id)).toEqual(['GET /orders/{id}']);
    expect(matching(all, 'nothing')).toEqual([]);
  });

  it('counts the changes of a contract by weight', () => {
    const change = (level: 'breaking' | 'careful' | 'safe') => ({ operation: 'GET /a', level, text: '' });
    expect(contractWords([])).toBe('No change in the contract');
    expect(contractWords([change('breaking'), change('safe'), change('breaking')])).toBe('2 breaking, 1 safe');
    expect(contractWords([change('careful')])).toBe('1 to check');
  });
});
