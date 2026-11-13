import { describe, expect, it } from 'vitest';
import { boxWidth, layoutGraph, type GraphEdge, type GraphNode } from './graph';

const node = (id: string, layer: number, label = id): GraphNode => ({ id, label, layer });
const edge = (from: string, to: string): GraphEdge => ({ from, to });

/** The dependencies of the design: a file, the two it imports, and what each of those needs. */
const DEPS = {
  nodes: [
    node('auth.ts', 0),
    node('rateLimit.ts', 1),
    node('users.ts', 1),
    node('express-rate-limit', 2),
    node('db/client.ts', 2),
  ],
  edges: [
    edge('auth.ts', 'rateLimit.ts'),
    edge('auth.ts', 'users.ts'),
    edge('rateLimit.ts', 'express-rate-limit'),
    edge('users.ts', 'db/client.ts'),
  ],
};

/** The calls of the design: a route, the limiter it uses, and the three calls the limiter makes. */
const CALLS = {
  nodes: [
    node('POST /login', 0),
    node('loginLimiter', 1),
    node('rateLimit()', 2),
    node('res.status()', 2),
    node('res.json()', 2),
  ],
  edges: [
    edge('POST /login', 'loginLimiter'),
    edge('loginLimiter', 'rateLimit()'),
    edge('loginLimiter', 'res.status()'),
    edge('loginLimiter', 'res.json()'),
  ],
};

const tops = (layout: ReturnType<typeof layoutGraph>) => Object.fromEntries(layout.boxes.map((box) => [box.id, box.y]));
const lefts = (layout: ReturnType<typeof layoutGraph>) =>
  Object.fromEntries(layout.boxes.map((box) => [box.id, box.x]));

describe('how wide a box is', () => {
  it('fits its label and stays within sensible bounds', () => {
    expect(boxWidth('a.ts')).toBe(96);
    expect(boxWidth('express-rate-limit')).toBe(162);
    expect(boxWidth('a'.repeat(80))).toBe(220);
  });
});

describe('a picture with room: columns, left to right', () => {
  it('draws the dependencies of the design on its three rows', () => {
    const layout = layoutGraph(DEPS.nodes, DEPS.edges, 474);
    expect(layout.direction).toBe('right');
    expect(layout.height).toBe(200);
    // Two leaves stand on the first and the last row; what leads to both stands between them.
    expect(tops(layout)).toEqual({
      'auth.ts': 88,
      'rateLimit.ts': 28,
      'users.ts': 148,
      'express-rate-limit': 28,
      'db/client.ts': 148,
    });
    // Columns are as wide as their widest label and 48 apart.
    expect(lefts(layout)).toMatchObject({ 'auth.ts': 0, 'rateLimit.ts': 144, 'express-rate-limit': 310 });
    expect(layout.width).toBe(472);
    expect(layout.boxes.every((box) => box.h === 40)).toBe(true);
    // Arrows of one box share the way to the middle of the gap; each stops five short of its box.
    expect(layout.lines).toEqual([
      'M 96 108 H 120 V 48 H 139',
      'M 96 108 H 120 V 168 H 139',
      'M 262 48 H 305',
      'M 262 168 H 305',
    ]);
  });

  it('draws the calls of the design: one box in the middle row, three leaves on all rows', () => {
    const layout = layoutGraph(CALLS.nodes, CALLS.edges, 474);
    expect(layout.direction).toBe('right');
    expect(tops(layout)).toEqual({
      'POST /login': 88,
      loginLimiter: 88,
      'rateLimit()': 28,
      'res.status()': 88,
      'res.json()': 148,
    });
    expect(layout.height).toBe(200);
    expect(layout.lines[0]).toBe('M 111 108 H 154');
  });

  it('lines up what stands before the centre around it, and grows past three rows', () => {
    const nodes = [node('a', -1), node('b', -1), node('centre', 0), ...['p', 'q', 'r', 's'].map((id) => node(id, 1))];
    const edges = [edge('a', 'centre'), edge('b', 'centre'), ...['p', 'q', 'r', 's'].map((id) => edge('centre', id))];
    const layout = layoutGraph(nodes, edges, 1000);
    const y = tops(layout);
    expect([y.p, y.q, y.r, y.s]).toEqual([28, 88, 148, 208]);
    expect(y.centre).toBe(118);
    expect([y.a, y.b]).toEqual([88, 148]);
    expect(layout.height).toBe(260);
    // Both arrows into the centre end at its side.
    expect(layout.lines.slice(0, 2)).toEqual(['M 96 108 H 120 V 138 H 139', 'M 96 168 H 120 V 138 H 139']);
  });

  it('puts a single box on the middle row', () => {
    const layout = layoutGraph([node('alone.ts', 0)], [], 474);
    expect(layout.boxes).toEqual([{ id: 'alone.ts', x: 0, y: 88, w: 96, h: 40 }]);
    expect([layout.width, layout.height, layout.lines]).toEqual([96, 200, []]);
  });
});

describe('a picture in a narrow card: rows, top to bottom', () => {
  it('turns the layers into rows when columns do not fit', () => {
    const layout = layoutGraph(DEPS.nodes, DEPS.edges, 420);
    expect(layout.direction).toBe('down');
    expect(layout.width).toBeLessThanOrEqual(420);
    expect(tops(layout)).toEqual({
      'auth.ts': 12,
      'rateLimit.ts': 100,
      'users.ts': 100,
      'express-rate-limit': 188,
      'db/client.ts': 188,
    });
    expect(layout.height).toBe(240);
    // No two boxes of a row overlap.
    for (const y of [12, 100, 188]) {
      const row = layout.boxes.filter((box) => box.y === y).sort((a, b) => a.x - b.x);
      for (let i = 1; i < row.length; i++) expect(row[i].x).toBeGreaterThanOrEqual(row[i - 1].x + row[i - 1].w + 12);
    }
    // A box stands over the middle of what it leads to.
    const at = (id: string) => layout.boxes.find((box) => box.id === id)!;
    const middle = (id: string) => at(id).x + at(id).w / 2;
    expect(middle('auth.ts')).toBe((middle('rateLimit.ts') + middle('users.ts')) / 2);
    expect(layout.lines[2]).toBe(`M ${middle('rateLimit.ts')} 140 V 183`);
  });

  it('keeps the layout that needs less shrinking when neither fits', () => {
    const many = Array.from({ length: 12 }, (_, n) => node(`callee${n}()`, 1));
    const nodes = [node('centre', 0), ...many];
    const edges = many.map((callee) => edge('centre', callee.id));
    // Twelve boxes side by side are far wider than two columns.
    expect(layoutGraph(nodes, edges, 200).direction).toBe('right');
    expect(layoutGraph([], [], 200)).toEqual({ direction: 'right', width: 0, height: 0, boxes: [], lines: [] });
  });
});
