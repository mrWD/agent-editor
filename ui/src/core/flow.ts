/**
 * The flow chart of the Map view.
 *
 * What happens step by step when a piece of code runs cannot be read off the
 * code without understanding it, so the task's agent is asked. The request
 * shows the agent a small strict format; this file writes that request, reads
 * the answer and lays the chart out.
 *
 * ## The flow format
 *
 * The answer must hold one block that opens with a line "```flow" and closes
 * with a line "```". Anything outside the block is ignored. Inside it every
 * line that is not empty is a node or an arrow:
 *
 * ```
 * step a: Request to POST /login
 * ask b: Over the limit? @ src/middleware/rateLimit.ts:3
 * step* c: Respond 429 @ src/middleware/rateLimit.ts:8
 * step d: Check the password
 * a -> b
 * b -> c: yes
 * b -> d: no
 * ```
 *
 * - A node is `step <id>: <text>` (something that happens) or `ask <id>: <text>`
 *   (a question). A `*` right after the word marks what the task changed.
 *   ` @ <path>:<line>` at the end is optional and says where in the code it happens.
 * - An arrow is `<from> -> <to>`, or `<from> -> <to>: <label>` with a short label
 *   such as "yes".
 * - An id is 1 to 12 letters, digits or underscores, and is used once.
 * - A text is 1 to 60 characters; a label is at most 12.
 * - There are 2 to 30 nodes. The first one is where the flow starts.
 * - Every arrow joins two nodes of the block. Every node but the first is
 *   reached by an arrow. A `step` leads to at most one node. An `ask` leads to
 *   at least two, and each of its arrows has a label.
 *
 * An answer that breaks a rule is not drawn; the reader is told which rule.
 */

export type FlowNode = {
  id: string;
  kind: 'step' | 'ask';
  text: string;
  /** The agent says the task changed this. */
  changed: boolean;
  /** Where in the code it happens, when the agent said so. */
  path: string | null;
  line: number | null;
};

export type FlowArrow = { from: string; to: string; label: string | null };
export type Flow = { nodes: FlowNode[]; arrows: FlowArrow[] };

export type FlowAnswer = { ok: true; flow: Flow } | { ok: false; problem: string };

/** What a flow is asked about: a definition in a file, or a whole file. */
export type FlowSubject = { path: string; symbol: string | null };

const MAX_NODES = 30;
const MAX_TEXT = 60;
const MAX_LABEL = 12;

const EXAMPLE = [
  '```flow',
  'step a: The first thing that happens',
  'ask b: A question with two answers? @ src/file.ts:12',
  'step* c: A step this task changed @ src/file.ts:14',
  'step d: Another step',
  'a -> b',
  'b -> c: yes',
  'b -> d: no',
  '```',
].join('\n');

/** The first line of a request. The interface finds its own requests by it. */
function opening(subject: FlowSubject): string {
  return subject.symbol
    ? `Draw the flow of \`${subject.symbol}\` in \`${subject.path}\`: what happens, step by step, when it is used.`
    : `Draw the flow of \`${subject.path}\`: what happens, step by step, when its code runs.`;
}

/** The message that asks an agent for the flow of `subject`. */
export function flowRequest(subject: FlowSubject): string {
  return [
    `${opening(subject)} Do not change any file.`,
    `Answer with one block in exactly this format, so the editor can draw it:\n\n${EXAMPLE}`,
    [
      'The rules:',
      '- `step` is something that happens. `ask` is a question; give each of its arrows a short label.',
      '- Put `*` right after the word for what this task changed.',
      '- ` @ path:line` is optional. It says where in the code the step happens.',
      `- Ids are short and used once. The first line is where the flow starts. A step leads to one place at most.`,
      `- Use at most ${MAX_NODES} nodes and ${MAX_TEXT} characters of text in each.`,
    ].join('\n'),
  ].join('\n\n');
}

/** Whether a message the user sent is the request for the flow of `subject`. */
export function asksFlowOf(message: string, subject: FlowSubject): boolean {
  return message.startsWith(opening(subject));
}

const NODE = /^(step|ask)(\*?)\s+([A-Za-z0-9_]{1,12})\s*:\s*(.+)$/;
const ARROW = /^([A-Za-z0-9_]{1,12})\s*->\s*([A-Za-z0-9_]{1,12})(?:\s*:\s*(.+))?$/;
const PLACE = /^(.*\S)\s+@\s+(\S+?):(\d+)$/;

const bad = (problem: string): FlowAnswer => ({ ok: false, problem });

/** Reads the flow out of an agent's answer, or says in plain words what is wrong with it. */
export function parseFlow(answer: string): FlowAnswer {
  const lines = answer.split(/\r?\n/);
  const open = lines.findIndex((line) => line.trim() === '```flow');
  if (open < 0) return bad('The answer has no flow block.');
  const length = lines.slice(open + 1).findIndex((line) => line.trim() === '```');
  if (length < 0) return bad('The flow block is not closed.');

  const nodes: FlowNode[] = [];
  const arrows: FlowArrow[] = [];
  for (let index = open + 1; index < open + 1 + length; index++) {
    const line = lines[index].trim();
    // Counted inside the block, as the agent wrote it.
    const where = `Line ${index - open} of the flow`;
    if (!line) continue;
    const node = NODE.exec(line);
    if (node) {
      const [, kind, star, id, rest] = node;
      const place = PLACE.exec(rest);
      const text = (place ? place[1] : rest).trim();
      if (text.length > MAX_TEXT) return bad(`${where} has more than ${MAX_TEXT} characters of text.`);
      if (nodes.some((known) => known.id === id)) return bad(`${where} uses the id "${id}" a second time.`);
      nodes.push({
        id,
        kind: kind as FlowNode['kind'],
        text,
        changed: star === '*',
        path: place ? place[2] : null,
        line: place ? Number(place[3]) : null,
      });
      continue;
    }
    const arrow = ARROW.exec(line);
    if (arrow) {
      const label = arrow[3]?.trim() || null;
      if (label && label.length > MAX_LABEL) return bad(`${where} has a label of more than ${MAX_LABEL} characters.`);
      arrows.push({ from: arrow[1], to: arrow[2], label });
      continue;
    }
    return bad(`${where} is not a step, a question or an arrow: "${line.slice(0, 60)}".`);
  }

  if (nodes.length < 2) return bad('A flow needs at least two steps.');
  if (nodes.length > MAX_NODES) return bad(`The flow has more than ${MAX_NODES} steps.`);
  const known = new Set(nodes.map((node) => node.id));
  for (const arrow of arrows) {
    const missing = [arrow.from, arrow.to].find((id) => !known.has(id));
    if (missing) return bad(`An arrow names "${missing}", which is not a step of the flow.`);
    if (arrow.from === arrow.to) return bad(`The step "${arrow.from}" has an arrow to itself.`);
  }
  for (const node of nodes) {
    const out = arrows.filter((arrow) => arrow.from === node.id);
    if (node.kind === 'step' && out.length > 1) {
      return bad(`The step "${node.id}" leads to ${out.length} places. Only a question may do that.`);
    }
    if (node.kind === 'ask' && out.length < 2) return bad(`The question "${node.id}" needs at least two answers.`);
    if (node.kind === 'ask' && out.some((arrow) => !arrow.label)) {
      return bad(`An answer to the question "${node.id}" has no label.`);
    }
    if (node !== nodes[0] && !arrows.some((arrow) => arrow.to === node.id)) {
      return bad(`No arrow leads to the step "${node.id}".`);
    }
  }
  return { ok: true, flow: { nodes, arrows } };
}

// ----- laying the chart out -----

export type FlowBox = FlowNode & { x: number; y: number; w: number; h: number };
/** A line of the chart: the path to draw and, for a labelled arrow, where its label goes. */
export type FlowLine = { d: string; label: string | null; lx: number; ly: number; anchor: 'start' | 'middle' };
export type FlowLayout = { width: number; height: number; boxes: FlowBox[]; lines: FlowLine[] };

/** The sizes the design draws: a step is 180 by 44, a question is a diamond of 220 by 88. */
const PAD_X = 20;
const PAD_Y = 12;
const COLUMN_GAP = 60;
const ROW_GAP = 40;
/** The room an arrowhead takes: a line stops this far before the box it points at. */
const HEAD = 5;

function sizeOf(node: FlowNode, column: number): { w: number; h: number } {
  const long = node.text.length;
  if (node.kind === 'ask') {
    // A diamond holds little text for its size; a long question gets a larger one.
    if (long <= 28) return { w: 220, h: 88 };
    return long <= 44 ? { w: 260, h: 104 } : { w: 300, h: 120 };
  }
  // Beside the main column the design draws narrower steps, so two columns fit a card.
  const w = column === 0 ? 180 : 150;
  const perLine = column === 0 ? 23 : 19;
  const rows = Math.ceil(long / perLine);
  return { w, h: rows <= 2 ? 44 : 26 + rows * 19 };
}

/**
 * Lays a flow out as the design draws it: the main path runs down the first
 * column, and each other answer of a question goes off to the right of it.
 */
export function layoutFlow(flow: Flow): FlowLayout {
  const out = (id: string) => flow.arrows.filter((arrow) => arrow.from === id);
  const byId = new Map(flow.nodes.map((node) => [node.id, node]));

  // How many steps follow a node on its longest way out. The longer answer of a question is the main path.
  const depths = new Map<string, number>();
  const depth = (id: string, seen: Set<string>): number => {
    const known = depths.get(id);
    if (known !== undefined) return known;
    if (seen.has(id)) return 0;
    seen.add(id);
    const value = Math.max(0, ...out(id).map((arrow) => 1 + depth(arrow.to, seen)));
    seen.delete(id);
    depths.set(id, value);
    return value;
  };

  const cells = new Map<string, { column: number; row: number }>();
  const taken = new Set<string>();
  const place = (id: string, column: number, row: number) => {
    if (cells.has(id)) return;
    let at = column;
    while (taken.has(`${at},${row}`)) at++;
    cells.set(id, { column: at, row });
    taken.add(`${at},${row}`);
    const arrows = out(id);
    if (arrows.length === 0) return;
    // Of equally long answers the one written last goes down: "no" is usually written after "yes".
    let main = arrows[0];
    for (const arrow of arrows) {
      if (depth(arrow.to, new Set()) >= depth(main.to, new Set())) main = arrow;
    }
    place(main.to, at, row + 1);
    for (const arrow of arrows) {
      if (arrow !== main) place(arrow.to, at + 1, row);
    }
  };
  place(flow.nodes[0].id, 0, 0);
  // A node no arrow reaches cannot come out of `parseFlow`; give it a place all the same.
  let spare = Math.max(0, ...[...cells.values()].map((cell) => cell.row)) + 1;
  for (const node of flow.nodes) {
    if (!cells.has(node.id)) place(node.id, 0, spare++);
  }

  const sizes = new Map(flow.nodes.map((node) => [node.id, sizeOf(node, cells.get(node.id)!.column)]));
  const columns: number[] = [];
  const rows: number[] = [];
  for (const node of flow.nodes) {
    const cell = cells.get(node.id)!;
    const size = sizes.get(node.id)!;
    columns[cell.column] = Math.max(columns[cell.column] ?? 0, size.w);
    rows[cell.row] = Math.max(rows[cell.row] ?? 0, size.h);
  }
  const lefts: number[] = [];
  let x = PAD_X;
  for (let column = 0; column < columns.length; column++) {
    lefts[column] = x;
    x += (columns[column] ?? 0) + COLUMN_GAP;
  }
  const tops: number[] = [];
  let y = PAD_Y;
  for (let row = 0; row < rows.length; row++) {
    tops[row] = y;
    // Under a question there is a label next to the arrow, so the gap is a little larger.
    const holdsQuestion = flow.nodes.some((node) => node.kind === 'ask' && cells.get(node.id)!.row === row);
    y += (rows[row] ?? 0) + (holdsQuestion ? ROW_GAP + 8 : ROW_GAP);
  }

  const boxes: FlowBox[] = flow.nodes.map((node) => {
    const cell = cells.get(node.id)!;
    const size = sizes.get(node.id)!;
    return {
      ...node,
      x: lefts[cell.column] + (columns[cell.column] - size.w) / 2,
      y: tops[cell.row] + (rows[cell.row] - size.h) / 2,
      ...size,
    };
  });
  const box = (id: string) => boxes.find((entry) => entry.id === id)!;
  let width = Math.max(...boxes.map((entry) => entry.x + entry.w)) + PAD_X;
  const height = Math.max(...boxes.map((entry) => entry.y + entry.h)) + PAD_Y;

  const lines: FlowLine[] = [];
  let goesBack = false;
  for (const arrow of flow.arrows) {
    if (!byId.has(arrow.from) || !byId.has(arrow.to)) continue;
    const from = box(arrow.from);
    const to = box(arrow.to);
    const a = cells.get(arrow.from)!;
    const b = cells.get(arrow.to)!;
    const fromX = from.x + from.w / 2;
    const fromY = from.y + from.h / 2;
    const toX = to.x + to.w / 2;
    const toY = to.y + to.h / 2;
    if (b.row > a.row && a.column === b.column) {
      // Straight down. The label stands to the right of the line.
      const d = `M ${fromX} ${from.y + from.h} V ${to.y - HEAD}`;
      lines.push({ d, label: arrow.label, lx: fromX + 12, ly: from.y + from.h + 4, anchor: 'start' });
    } else if (b.row === a.row && b.column > a.column) {
      // Straight to the right. The label stands over the line.
      const d = `M ${from.x + from.w} ${fromY} H ${to.x - HEAD}`;
      lines.push({ d, label: arrow.label, lx: from.x + from.w + 26, ly: fromY - 28, anchor: 'middle' });
    } else if (b.row > a.row) {
      // Down and across, into the top of the target: two paths joining above a step.
      const turn = to.y - ROW_GAP / 2;
      const d = `M ${fromX} ${from.y + from.h} V ${turn} H ${toX} V ${to.y - HEAD}`;
      lines.push({ d, label: arrow.label, lx: fromX + 12, ly: from.y + from.h + 4, anchor: 'start' });
    } else {
      // Back up, or sideways to an earlier column: around the right edge, into the target's side.
      goesBack = true;
      const lane = width - PAD_X + 16;
      const d = `M ${from.x + from.w} ${fromY} H ${lane} V ${toY} H ${to.x + to.w + HEAD}`;
      lines.push({ d, label: arrow.label, lx: from.x + from.w + 26, ly: fromY - 28, anchor: 'middle' });
    }
  }
  if (goesBack) width += 16;
  return { width, height, boxes, lines };
}
