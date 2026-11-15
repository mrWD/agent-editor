/**
 * The Infra view of the Preview: what the core says a task changes in the
 * infrastructure of a project, and the words made of it. Nothing here asks the core.
 */

import type { JobRun } from './jobs';

export type InfraTool = 'terraform' | 'kubernetes' | 'helm' | 'docker' | 'compose' | 'ci';

/** One thing a change to a file does. */
export type InfraStep = {
  text: string;
  /** The field that matters it is about; the text reads on from that word. */
  field: 'image' | 'replicas' | 'resources' | 'env' | 'ports' | null;
  /** Why it deserves a second look. */
  danger: string | null;
};

export type InfraFile = {
  path: string;
  tool: InfraTool;
  change: 'added' | 'modified' | 'deleted';
  steps: InfraStep[];
};

/** A check of a tool that is started by hand: `terraform validate`, `terraform plan`, `helm template`. */
export type InfraCheck = {
  /** What it is and where, such as `terraform-plan:infra/prod`. */
  id: string;
  tool: InfraTool;
  name: string;
  /** The folder it runs in, within the task's folder. Empty for the task's folder itself. */
  folder: string;
  command: string;
  /** Whether its tool is on this computer. */
  ready: boolean;
  note: string | null;
  /** Whether the task touched a file of its folder. */
  changed: boolean;
};

export type InfraState = {
  /** The tools the project has files of, with how many. */
  tools: { kind: InfraTool; files: number }[];
  /** The infrastructure files the task touched, grouped by tool. */
  files: InfraFile[];
  note: string | null;
  checks: InfraCheck[];
};

export const toolWord: Record<InfraTool, string> = {
  terraform: 'Terraform',
  kubernetes: 'Kubernetes',
  helm: 'Helm',
  docker: 'Dockerfiles',
  compose: 'Compose',
  ci: 'CI',
};

export const changeWord: Record<InfraFile['change'], string> = {
  added: 'new',
  modified: 'changed',
  deleted: 'removed',
};

/** "Terraform, 3 files · Kubernetes, 1 file": what the project has. */
export function hasWords(tools: InfraState['tools']): string {
  return tools.map((one) => `${toolWord[one.kind]}, ${one.files} ${one.files === 1 ? 'file' : 'files'}`).join(' · ');
}

/** The files of each tool together, in the order the core gave them. */
export function byTool(files: InfraFile[]): { tool: InfraTool; files: InfraFile[] }[] {
  const groups: { tool: InfraTool; files: InfraFile[] }[] = [];
  for (const file of files) {
    const last = groups[groups.length - 1];
    if (last && last.tool === file.tool) last.files.push(file);
    else groups.push({ tool: file.tool, files: [file] });
  }
  return groups;
}

/** "2 changed, 1 new": the infrastructure files of a task, counted. */
export function changeWords(files: InfraFile[]): string {
  const count = (change: InfraFile['change']) => files.filter((one) => one.change === change).length;
  const said = (['modified', 'added', 'deleted'] as const)
    .filter((change) => count(change) > 0)
    .map((change) => `${count(change)} ${changeWord[change]}`)
    .join(', ');
  return said || 'This task has touched none of it';
}

/** How many steps of the task's changes deserve a second look. */
export function dangers(files: InfraFile[]): number {
  return files.reduce((sum, one) => sum + one.steps.filter((step) => step.danger !== null).length, 0);
}

/** A step in one line, with the field it is about in front. */
export function stepLine(step: InfraStep): string {
  return step.field ? `${step.field} ${step.text}` : step.text;
}

/** Where a check runs, in words. */
export function folderWords(folder: string): string {
  return folder === '' ? 'the folder of the task' : folder;
}

/** What the task changes in the infrastructure as text for the agent, with what deserves a second look said so. */
export function infraForAgent(files: InfraFile[]): { title: string; text: string } {
  const lines = ['What this task changes in the infrastructure, read from the files. Nothing was run.'];
  for (const group of byTool(files)) {
    lines.push('', toolWord[group.tool]);
    for (const file of group.files) {
      lines.push(`${file.path} (${changeWord[file.change]})`);
      for (const step of file.steps) {
        lines.push(`- ${stepLine(step)}`);
        if (step.danger) lines.push(`  Look twice: ${step.danger}`);
      }
      if (file.steps.length === 0 && file.change !== 'deleted')
        lines.push('- Nothing in it could be read as a change.');
    }
  }
  const marked = dangers(files);
  if (marked > 0) lines.push('', `${marked} of these ${marked === 1 ? 'deserves' : 'deserve'} a second look.`);
  return { title: 'what this task changes in the infrastructure', text: lines.join('\n') };
}

/** A run of a check as text for the agent: what was started and where, how it ended, what it printed. */
export function checkForAgent(check: InfraCheck, command: string, run: JobRun): { title: string; text: string } {
  const ended = run.running
    ? 'It is still running.'
    : run.stopped
      ? 'It was stopped by hand.'
      : run.exit === null
        ? ''
        : run.exit === 0
          ? 'It ended well.'
          : `It ended with code ${run.exit}.`;
  const how = run.running ? 'is running' : run.stopped ? 'was stopped' : run.exit === 0 ? 'ran' : 'failed';
  return {
    title: `${check.name} in ${folderWords(check.folder)} ${how}`,
    text: [
      `$ ${command}`,
      `(in ${folderWords(check.folder)})`,
      '',
      run.output?.trim() || '(it printed nothing)',
      '',
      ended,
    ]
      .join('\n')
      .trimEnd(),
  };
}
