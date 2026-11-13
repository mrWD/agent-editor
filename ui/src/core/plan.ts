/**
 * The plan of a task: a flow chart of what the task is meant to become.
 *
 * The core keeps the plan and makes every change to it; this file holds what
 * the interface knows of it, the commands, and how the chart is laid out. The
 * layout is the one of the Map view's flow chart (`layoutFlow`), used once for
 * each piece of the plan that hangs together.
 */

import { createStore, reconcile } from 'solid-js/store';
import { core } from './client';
import { layoutFlow, type FlowArrow, type FlowNode } from './flow';

export type BlockKind = 'step' | 'ask' | 'note';

/**
 * Where a block stands: in the plan and not built, being built, built,
 * proposed by an agent and waiting for a yes, taken by the user for
 * themselves, or in the code before the task started.
 */
export type BlockState = 'planned' | 'building' | 'built' | 'proposed' | 'yours' | 'before';

/** A file of the task, and a line in it when that is known. */
export type Place = { path: string; line: number | null };

export type Block = {
  id: string;
  kind: BlockKind;
  /** The name on the block. */
  text: string;
  /** "What this block should do", when the name does not say enough. */
  details: string;
  state: BlockState;
  /** The agent that built or proposed it, and its name at that time. */
  by: string | null;
  byName: string | null;
  /** When it was built or proposed. */
  at: number | null;
  /** For a built block: the files that turn changed. For one drawn from the code: where it is. */
  files: Place[];
  /** Where a block an agent wrote came from. `null` for the user's own. */
  from: 'draft' | 'build' | 'code' | null;
};

export type PlanArrow = { from: string; to: string; label: string | null };

export type PlanCounts = { built: number; total: number; building: number; planned: number; proposed: number };

export type Plan = {
  /** Grows with every change, so an older plan is never taken for the newest. */
  rev: number;
  blocks: Block[];
  arrows: PlanArrow[];
  /** "Agents build only what is in the plan." */
  only: boolean;
  /** Why the agent's last draft could not be used. */
  problem: string | null;
  /** What the agent is doing for the plan right now. */
  busy: { kind: 'build'; blockId: string } | { kind: 'draft' } | { kind: 'draw' } | null;
  /** The block "Build the next block" would build, or why there is none. */
  next: string | null;
  nextWhy: string | null;
  counts: PlanCounts;
};

const [all, setAll] = createStore<Record<string, Plan>>({});

/** What is known of a task's plan. `undefined` until it has been asked for. */
export function planOf(taskId: string): Plan | undefined {
  return all[taskId];
}

function take(taskId: string, plan: Plan) {
  // An answer may arrive after the news of a later change. The later plan stays.
  if ((all[taskId]?.rev ?? -1) > plan.rev) return;
  setAll(taskId, reconcile(plan, { key: 'id' }));
}

export async function loadPlan(taskId: string): Promise<void> {
  take(taskId, await core.request<Plan>('plan.get', { taskId }));
}

/** Keeps a task's plan current while its screen is shown. Returns what stops it. */
export function watchPlan(taskId: string): () => void {
  const load = () => void loadPlan(taskId).catch(() => {});
  load();
  // eslint-disable-next-line solid/reactivity -- called later, as an event handler: it reads what is current then
  return core.onEvent((event) => {
    if (event.event === 'plan.changed' && event.data.taskId === taskId) take(taskId, event.data.plan as Plan);
    // After the connection was lost, changes may have been missed.
    if (event.event === 'link.open' || event.event === 'resync') load();
  });
}

/**
 * Sends one command of the plan. Every command answers with the plan as it
 * stands after; the same plan goes to every window as news.
 */
async function command(method: string, taskId: string, more: Record<string, unknown> = {}): Promise<string | null> {
  const { blockId, ...plan } = await core.request<Plan & { blockId?: string }>(`plan.${method}`, { taskId, ...more });
  take(taskId, plan);
  return blockId ?? null;
}

/** Adds a block: after the block `after`, joined to it by an arrow, or loose. Returns its id. */
export const addBlock = (taskId: string, kind: BlockKind, after?: string | null) =>
  command('add', taskId, { kind, after: after ?? undefined });
export const editBlock = (taskId: string, blockId: string, change: { text?: string; details?: string }) =>
  command('edit', taskId, { blockId, ...change });
/** Draws an arrow, or changes the label of the one that is there. */
export const connectBlocks = (taskId: string, from: string, to: string, label: string) =>
  command('connect', taskId, { from, to, label });
export const disconnectBlocks = (taskId: string, from: string, to: string) =>
  command('disconnect', taskId, { from, to });
export const removeBlock = (taskId: string, blockId: string) => command('remove', taskId, { blockId });
/** "I will do it myself", or giving the block back. */
export const takeBlock = (taskId: string, blockId: string, mine: boolean) => command('take', taskId, { blockId, mine });
/** Says yes to one proposed block, or to all of them when none is named. */
export const acceptBlock = (taskId: string, blockId?: string) => command('accept', taskId, { blockId });
export const declineBlock = (taskId: string, blockId?: string) => command('decline', taskId, { blockId });
export const setOnlyPlan = (taskId: string, on: boolean) => command('only', taskId, { on });
export const dismissPlanProblem = (taskId: string) => command('dismiss', taskId);
/** Builds one block, or the next one the plan allows when none is named. It sends the agent a message. */
export const buildBlock = (taskId: string, blockId?: string) => command('build', taskId, { blockId });
/** Asks the agent to draw a plan: of the task as its first message describes it, or of the code as it is. */
export const draftPlan = (taskId: string, from: 'issue' | 'code') => command('draft', taskId, { from });

// ----- words -----

const kindWord: Record<BlockKind, string> = { step: 'Step', ask: 'Question', note: 'Note' };

export function stateWord(state: BlockState): string {
  switch (state) {
    case 'planned':
      return 'Planned';
    case 'building':
      return 'Being built';
    case 'built':
      return 'Built';
    case 'proposed':
      return 'Proposed';
    case 'yours':
      return 'Planned, yours';
    case 'before':
      return 'Was in the code before';
  }
}

/** "2 of 5 built · 1 being built · 1 planned · 1 proposed", with what there is none of left out. */
export function countsLine(counts: PlanCounts): string {
  if (counts.total === 0) return 'Nothing to build yet';
  const parts = [`${counts.built} of ${counts.total} built`];
  if (counts.building > 0) parts.push(`${counts.building} being built`);
  if (counts.planned > 0) parts.push(`${counts.planned} planned`);
  if (counts.proposed > 0) parts.push(`${counts.proposed} proposed`);
  return parts.join(' · ');
}

/** What a block is called for someone who cannot see the chart: what it is, how it stands, where it leads. */
export function describeBlock(block: Block, plan: Pick<Plan, 'blocks' | 'arrows'>): string {
  const next = plan.arrows
    .filter((arrow) => arrow.from === block.id)
    .map((arrow) => {
      const text = plan.blocks.find((other) => other.id === arrow.to)?.text ?? '';
      return arrow.label ? `${arrow.label}: ${text}` : text;
    });
  // A note is neither built nor planned.
  const stands = block.kind === 'note' ? '' : ` ${stateWord(block.state)}.`;
  const stop = /[.?!…]$/.test(block.text) ? '' : '.';
  return `${kindWord[block.kind]}: ${block.text}${stop}${stands}${next.length > 0 ? ` Then ${next.join('; ')}.` : ''}`;
}

// ----- laying the chart out -----

/** A block of the chart with its place. */
export type PlanBox = { id: string; x: number; y: number; w: number; h: number };
/** An arrow of the chart: the path to draw and where its label goes. */
export type PlanLine = PlanArrow & { d: string; lx: number; ly: number; anchor: 'start' | 'middle' };
/** Where the small "add a block after this one" button of a block stands. */
export type PlanPlus = { after: string; x: number; y: number };
export type PlanLayout = { width: number; height: number; boxes: PlanBox[]; lines: PlanLine[]; plus: PlanPlus[] };

/** The button under the last block of a line: 24 across, 8 under the block. */
const PLUS = 24;
const PLUS_GAP = 8;

/**
 * Moves a path of the flow layout to the right and down. Those paths are made
 * of three commands only: `M x y`, `V y` and `H x`.
 */
export function movedBy(d: string, right: number, down: number): string {
  const parts = d.split(' ');
  const out: string[] = [];
  for (let index = 0; index < parts.length; index++) {
    const part = parts[index];
    if (part === 'M') {
      out.push(part, String(Number(parts[index + 1]) + right), String(Number(parts[index + 2]) + down));
      index += 2;
    } else if (part === 'V' || part === 'H') {
      out.push(part, String(Number(parts[index + 1]) + (part === 'V' ? down : right)));
      index += 1;
    } else {
      out.push(part);
    }
  }
  return out.join(' ');
}

/** The blocks that hang together through arrows, group by group, in the order of the plan. */
function groupsOf(blocks: Block[], arrows: PlanArrow[]): Block[][] {
  const group = new Map<string, number>();
  const groups: Block[][] = [];
  for (const block of blocks) {
    if (group.has(block.id)) continue;
    const mine = groups.length;
    const open = [block.id];
    group.set(block.id, mine);
    while (open.length > 0) {
      const at = open.pop()!;
      for (const arrow of arrows) {
        const other = arrow.from === at ? arrow.to : arrow.to === at ? arrow.from : null;
        if (other !== null && !group.has(other) && blocks.some((known) => known.id === other)) {
          group.set(other, mine);
          open.push(other);
        }
      }
    }
    groups.push(blocks.filter((known) => group.get(known.id) === mine));
  }
  return groups;
}

/** Whether a block shows a line under its name: who is building it, that it is the user's, that it waits. */
export function hasStateLine(block: Block): boolean {
  return block.kind !== 'note' && (block.state === 'building' || block.state === 'yours' || block.state === 'proposed');
}

/**
 * Lays a plan out. Each group of blocks that hang together is laid out as the
 * Map view lays out a flow, starting from the block nothing leads to, and the
 * groups stand under each other: loose blocks and notes come out as rows of their own.
 */
export function layoutPlan(plan: Pick<Plan, 'blocks' | 'arrows'>): PlanLayout {
  const known = new Set(plan.blocks.map((block) => block.id));
  const arrows = plan.arrows.filter((arrow) => known.has(arrow.from) && known.has(arrow.to));
  const byId = new Map(plan.blocks.map((block) => [block.id, block]));
  const layout: PlanLayout = { width: 0, height: 0, boxes: [], lines: [], plus: [] };
  /** Where the middle of the first group's first block is. */
  let line: number | null = null;

  for (const group of groupsOf(plan.blocks, arrows)) {
    const inGroup = new Set(group.map((block) => block.id));
    const own = arrows.filter((arrow) => inGroup.has(arrow.from));
    // The flow starts at its first node: the block nothing leads to, when there is one.
    const start = group.find((block) => !own.some((arrow) => arrow.to === block.id)) ?? group[0];
    const nodes: FlowNode[] = [start, ...group.filter((block) => block !== start)].map((block) => ({
      id: block.id,
      kind: block.kind === 'ask' ? 'ask' : 'step',
      text: block.text,
      changed: false,
      path: null,
      line: null,
    }));
    const flowArrows: FlowArrow[] = own.map((arrow) => ({ from: arrow.from, to: arrow.to, label: arrow.label }));
    const flow = layoutFlow({ nodes, arrows: flowArrows }, (node) => (hasStateLine(byId.get(node.id)!) ? 1 : 0));

    // Groups stand under each other on one line: the start of each is centred under the start of the first.
    const first = flow.boxes.find((box) => box.id === start.id)!;
    const middle = first.x + first.w / 2;
    line ??= middle;
    const right = Math.max(0, line - middle);
    const top = layout.height;
    for (const box of flow.boxes) {
      layout.boxes.push({ id: box.id, x: box.x + right, y: box.y + top, w: box.w, h: box.h });
    }
    // The layout answers with one line per arrow, in the order of the arrows.
    flow.lines.forEach((drawn, index) => {
      layout.lines.push({
        ...own[index],
        d: movedBy(drawn.d, right, top),
        lx: drawn.lx + right,
        ly: drawn.ly + top,
        anchor: drawn.anchor,
      });
    });
    let bottom = flow.height;
    for (const block of group) {
      // What can be built on gets a button under it, when it is the end of its line.
      const leads = own.some((arrow) => arrow.from === block.id);
      if (leads || block.kind === 'note' || block.state === 'proposed') continue;
      const box = flow.boxes.find((entry) => entry.id === block.id)!;
      const y = box.y + box.h + PLUS_GAP;
      layout.plus.push({ after: block.id, x: box.x + right + box.w / 2 - PLUS / 2, y: y + top });
      bottom = Math.max(bottom, y + PLUS + PLUS_GAP);
    }
    layout.width = Math.max(layout.width, flow.width + right);
    layout.height = top + bottom;
  }
  return layout;
}
