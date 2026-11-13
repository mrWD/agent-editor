/**
 * Lays out the two pictures of the Map view that the core works out:
 * dependencies and calls. Both are small graphs in layers: what stands before
 * the centre, the centre, and one or two steps after it.
 *
 * A picture is laid out for the room it has. With enough width the layers are
 * columns, left to right, as the design draws them. In a narrow card they are
 * rows, top to bottom.
 */

/** A box to lay out: an id, its label and how many steps from the centre it stands. */
export type GraphNode = { id: string; label: string; layer: number };
export type GraphEdge = { from: string; to: string };

export type Placed = { id: string; x: number; y: number; w: number; h: number };
export type GraphLayout = {
  direction: 'right' | 'down';
  width: number;
  height: number;
  boxes: Placed[];
  /** One path per arrow, ready for an SVG `d`. */
  lines: string[];
};

/** The sizes the design draws. */
const BOX_HEIGHT = 40;
const MIN_WIDTH = 96;
const MAX_WIDTH = 220;
/** Between two layers: 24 to the turn, 24 from it, with the arrowhead at the end. */
const LAYER_GAP = 48;
/** Between boxes of a column, and of a row. */
const ROW_GAP = 20;
const SIDE_GAP = 12;
/** In a column the design starts 28 from the top and ends 12 from the bottom. */
const TOP = 28;
const BOTTOM = 12;
/** A line stops this far before the box it points at, to leave room for the arrowhead. */
const HEAD = 5;

/**
 * How wide a box must be for its label, set in 12-pixel JetBrains Mono, whose
 * letters are all 7.2 wide. With 16 on each side, three columns of ordinary
 * names fit the narrowest card the design allows.
 */
export function boxWidth(label: string): number {
  return Math.min(MAX_WIDTH, Math.max(MIN_WIDTH, Math.round(label.length * 7.2) + 32));
}

type Spot = { node: GraphNode; size: number; middle: number };

/**
 * Places the boxes across their layers. Each box after the centre hangs under
 * the first box that points at it, like a tree: leaves stand next to each
 * other, and a box that leads to several stands in the middle of them.
 * `sizeOf` is how much room a box takes across the layer; `gap` is the room between two.
 */
function across(nodes: GraphNode[], edges: GraphEdge[], sizeOf: (node: GraphNode) => number, gap: number): Spot[] {
  const spots = new Map<string, Spot>();
  const children = new Map<string, GraphNode[]>();
  const hasParent = new Set<string>();
  const byId = new Map(nodes.map((node) => [node.id, node]));
  for (const edge of edges) {
    const from = byId.get(edge.from);
    const to = byId.get(edge.to);
    // Only arrows that lead one layer on, away from the centre, shape the tree.
    if (!from || !to || from.layer < 0 || to.layer !== from.layer + 1 || hasParent.has(to.id)) continue;
    hasParent.add(to.id);
    children.set(from.id, [...(children.get(from.id) ?? []), to]);
  }

  // Lays out the tree under `node` starting at `start`; returns how much room it took.
  const place = (node: GraphNode, start: number): number => {
    const size = sizeOf(node);
    const below = children.get(node.id) ?? [];
    if (below.length === 0) {
      spots.set(node.id, { node, size, middle: start + size / 2 });
      return size;
    }
    const lay = (from: number): number => {
      let at = from;
      for (const child of below) at += place(child, at) + gap;
      return at - gap - from;
    };
    let span = lay(start);
    // A box wider than everything under it: move those to stand under its middle.
    if (size > span) span = lay(start + (size - span) / 2) + (size - span);
    const first = spots.get(below[0].id)!.middle;
    const last = spots.get(below[below.length - 1].id)!.middle;
    spots.set(node.id, { node, size, middle: Math.max((first + last) / 2, start + size / 2) });
    return Math.max(size, span);
  };

  let cursor = 0;
  for (const node of nodes) {
    if (node.layer >= 0 && !hasParent.has(node.id)) cursor += place(node, cursor) + gap;
  }

  // What stands before the centre is lined up around the middle of the centre.
  const centre = nodes.filter((node) => node.layer === 0).map((node) => spots.get(node.id)!.middle);
  const around = centre.length > 0 ? (Math.min(...centre) + Math.max(...centre)) / 2 : 0;
  for (const layer of [...new Set(nodes.filter((node) => node.layer < 0).map((node) => node.layer))]) {
    const row = nodes.filter((node) => node.layer === layer);
    const total = row.reduce((sum, node) => sum + sizeOf(node), 0) + gap * (row.length - 1);
    let at = around - total / 2;
    for (const node of row) {
      const size = sizeOf(node);
      spots.set(node.id, { node, size, middle: at + size / 2 });
      at += size + gap;
    }
  }

  // Nothing may stand before zero.
  const all = [...spots.values()];
  const least = Math.min(...all.map((spot) => spot.middle - spot.size / 2));
  for (const spot of all) spot.middle -= least;
  return all;
}

/** Lays a graph out with its layers as columns, left to right, as the design draws it. */
function toTheRight(nodes: GraphNode[], edges: GraphEdge[]): GraphLayout {
  const spots = across(nodes, edges, () => BOX_HEIGHT, ROW_GAP);
  const pitch = BOX_HEIGHT + ROW_GAP;
  // How many rows the tallest column needs.
  const rows = Math.round((Math.max(...spots.map((spot) => spot.middle)) - BOX_HEIGHT / 2) / pitch) + 1;
  // The design draws on three rows: one box stands on the middle row, two on the first and the last.
  const top = (spot: Spot): number => {
    const row = (spot.middle - BOX_HEIGHT / 2) / pitch;
    if (rows === 1) return TOP + pitch;
    return TOP + row * pitch * (rows === 2 ? 2 : 1);
  };

  const layers = [...new Set(nodes.map((node) => node.layer))].sort((a, b) => a - b);
  const lefts = new Map<number, number>();
  const widths = new Map<number, number>();
  let x = 0;
  for (const layer of layers) {
    const width = Math.max(...nodes.filter((node) => node.layer === layer).map((node) => boxWidth(node.label)));
    lefts.set(layer, x);
    widths.set(layer, width);
    x += width + LAYER_GAP;
  }
  const boxes: Placed[] = spots.map((spot) => ({
    id: spot.node.id,
    x: lefts.get(spot.node.layer)!,
    y: top(spot),
    w: widths.get(spot.node.layer)!,
    h: BOX_HEIGHT,
  }));
  const box = new Map(boxes.map((entry) => [entry.id, entry]));
  const lines: string[] = [];
  for (const edge of edges) {
    const from = box.get(edge.from);
    const to = box.get(edge.to);
    if (!from || !to) continue;
    const y1 = from.y + from.h / 2;
    const y2 = to.y + to.h / 2;
    const turn = from.x + from.w + LAYER_GAP / 2;
    // Out to the middle of the gap, along it, then into the target. Arrows of one box share their first part.
    lines.push(
      y1 === y2
        ? `M ${from.x + from.w} ${y1} H ${to.x - HEAD}`
        : `M ${from.x + from.w} ${y1} H ${turn} V ${y2} H ${to.x - HEAD}`,
    );
  }
  const lowest = Math.max(...boxes.map((entry) => entry.y + entry.h));
  return {
    direction: 'right',
    width: x - LAYER_GAP,
    height: Math.max(200, lowest + BOTTOM),
    boxes,
    lines,
  };
}

/** Lays a graph out with its layers as rows, top to bottom: for a card too narrow for columns. */
function downwards(nodes: GraphNode[], edges: GraphEdge[]): GraphLayout {
  const spots = across(nodes, edges, (node) => boxWidth(node.label), SIDE_GAP);
  const layers = [...new Set(nodes.map((node) => node.layer))].sort((a, b) => a - b);
  const pitch = BOX_HEIGHT + LAYER_GAP;
  const boxes: Placed[] = spots.map((spot) => ({
    id: spot.node.id,
    x: spot.middle - spot.size / 2,
    y: BOTTOM + layers.indexOf(spot.node.layer) * pitch,
    w: spot.size,
    h: BOX_HEIGHT,
  }));
  const box = new Map(boxes.map((entry) => [entry.id, entry]));
  const lines: string[] = [];
  for (const edge of edges) {
    const from = box.get(edge.from);
    const to = box.get(edge.to);
    if (!from || !to) continue;
    const x1 = from.x + from.w / 2;
    const x2 = to.x + to.w / 2;
    const turn = from.y + from.h + LAYER_GAP / 2;
    lines.push(
      x1 === x2
        ? `M ${x1} ${from.y + from.h} V ${to.y - HEAD}`
        : `M ${x1} ${from.y + from.h} V ${turn} H ${x2} V ${to.y - HEAD}`,
    );
  }
  return {
    direction: 'down',
    width: Math.max(...boxes.map((entry) => entry.x + entry.w)),
    height: Math.max(...boxes.map((entry) => entry.y + entry.h)) + BOTTOM,
    boxes,
    lines,
  };
}

/**
 * Lays a graph out for a card `room` pixels wide. Columns when they fit; rows
 * when those fit instead; else whichever needs less shrinking, which the
 * caller then scales down and lets the user drag.
 */
export function layoutGraph(nodes: GraphNode[], edges: GraphEdge[], room: number): GraphLayout {
  if (nodes.length === 0) return { direction: 'right', width: 0, height: 0, boxes: [], lines: [] };
  const wide = toTheRight(nodes, edges);
  if (wide.width <= room) return wide;
  const tall = downwards(nodes, edges);
  return tall.width <= room || tall.width < wide.width ? tall : wide;
}
