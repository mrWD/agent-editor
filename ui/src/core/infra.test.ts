import { describe, expect, it } from 'vitest';
import {
  allForAgent,
  byTool,
  changeWords,
  checkForAgent,
  dangers,
  fileCount,
  folderWords,
  hasWords,
  infraForAgent,
  infraScope,
  stepLine,
  type InfraCheck,
  type InfraFile,
  type InfraStep,
  type InfraWhole,
} from './infra';
import type { JobRun } from './jobs';

const step = (text: string, over: Partial<InfraStep> = {}): InfraStep => ({ text, field: null, danger: null, ...over });

const files: InfraFile[] = [
  {
    path: 'infra/main.tf',
    tool: 'terraform',
    change: 'modified',
    steps: [
      step('Removes resource `aws_s3_bucket.exports`', { danger: 'Terraform destroys it on the next apply.' }),
      step('Adds resource `aws_sqs_queue.mail`'),
    ],
  },
  { path: 'infra/net.tf', tool: 'terraform', change: 'added', steps: [] },
  {
    path: 'k8s/web.yaml',
    tool: 'kubernetes',
    change: 'modified',
    steps: [
      step('Changes Deployment `web`'),
      step('of `web`: `shop/web:1.5`, was `shop/web:1.4`', { field: 'image' }),
      step('4, was 2', { field: 'replicas' }),
      step('`web` runs privileged', { danger: 'A privileged container can do on its host whatever root can.' }),
    ],
  },
  { path: 'Dockerfile.old', tool: 'docker', change: 'deleted', steps: [] },
];

const check = (over: Partial<InfraCheck> = {}): InfraCheck => ({
  id: 'terraform-plan:infra',
  tool: 'terraform',
  name: 'terraform plan',
  folder: 'infra',
  command: 'terraform plan -lock=false -refresh=false -input=false -no-color',
  ready: true,
  note: null,
  changed: true,
  ...over,
});

const run = (over: Partial<JobRun> = {}): JobRun => ({
  running: false,
  termId: null,
  exit: 0,
  output: 'Plan: 1 to add, 0 to change, 1 to destroy.\n',
  ...over,
});

describe('what a task changes in the infrastructure', () => {
  it('is counted, and what the project has is said tool by tool', () => {
    expect(changeWords(files)).toBe('2 changed, 1 new, 1 removed');
    expect(changeWords([])).toBe('This task has touched none of it');
    expect(dangers(files)).toBe(2);
    expect(dangers([])).toBe(0);
    expect(
      hasWords([
        { kind: 'terraform', files: 3 },
        { kind: 'docker', files: 1 },
        { kind: 'ci', files: 2 },
      ]),
    ).toBe('Terraform, 3 files · Dockerfiles, 1 file · CI, 2 files');
  });

  it('is grouped by tool in the order the core gave', () => {
    expect(byTool(files).map((group) => [group.tool, group.files.map((file) => file.path)])).toEqual([
      ['terraform', ['infra/main.tf', 'infra/net.tf']],
      ['kubernetes', ['k8s/web.yaml']],
      ['docker', ['Dockerfile.old']],
    ]);
    expect(byTool([])).toEqual([]);
  });

  it('reads on from the field a step is about', () => {
    expect(stepLine(step('4, was 2', { field: 'replicas' }))).toBe('replicas 4, was 2');
    expect(stepLine(step('Adds Service `web`'))).toBe('Adds Service `web`');
  });

  it('goes to the agent whole, with what deserves a second look said so', () => {
    const made = infraForAgent(files);
    expect(made.title).toBe('what this task changes in the infrastructure');
    expect(made.text).toBe(
      [
        'What this task changes in the infrastructure, read from the files. Nothing was run.',
        '',
        'Terraform',
        'infra/main.tf (changed)',
        '- Removes resource `aws_s3_bucket.exports`',
        '  Look twice: Terraform destroys it on the next apply.',
        '- Adds resource `aws_sqs_queue.mail`',
        'infra/net.tf (new)',
        '- Nothing in it could be read as a change.',
        '',
        'Kubernetes',
        'k8s/web.yaml (changed)',
        '- Changes Deployment `web`',
        '- image of `web`: `shop/web:1.5`, was `shop/web:1.4`',
        '- replicas 4, was 2',
        '- `web` runs privileged',
        '  Look twice: A privileged container can do on its host whatever root can.',
        '',
        'Dockerfiles',
        'Dockerfile.old (removed)',
        '',
        '2 of these deserve a second look.',
      ].join('\n'),
    );
    expect(infraForAgent(files.slice(0, 1)).text).toContain('1 of these deserves a second look.');
    expect(infraForAgent(files.slice(1, 2)).text).not.toContain('second look');
  });
});

describe('all the infrastructure of a project', () => {
  const whole: InfraWhole[] = [
    {
      path: 'infra/main.tf',
      tool: 'terraform',
      change: null,
      steps: [step('Resource `aws_db_instance.orders`')],
    },
    {
      path: 'Dockerfile',
      tool: 'docker',
      change: 'modified',
      steps: [
        step('base `node:20`', { field: 'image' }),
        step('Runs as `root`', { danger: 'Whatever gets hold of the process is root in the container.' }),
      ],
    },
    { path: 'compose.yaml', tool: 'compose', change: 'added', steps: [] },
  ];

  it('is on show when the task touched none of it, until the user says which', () => {
    expect(infraScope(null, 0)).toBe('all');
    expect(infraScope(null, 2)).toBe('touched');
    expect(infraScope('touched', 0)).toBe('touched');
    expect(infraScope('all', 2)).toBe('all');
  });

  it('is counted over its tools, grouped like the changes, and its marks are counted the same', () => {
    expect(
      fileCount([
        { kind: 'terraform', files: 3 },
        { kind: 'docker', files: 1 },
      ]),
    ).toBe(4);
    expect(fileCount([])).toBe(0);
    expect(byTool(whole).map((group) => group.tool)).toEqual(['terraform', 'docker', 'compose']);
    expect(dangers(whole)).toBe(1);
  });

  it('goes to the agent file by file, with what the task touched said so', () => {
    const made = allForAgent({ files: whole, more: 2 });
    expect(made.title).toBe('the infrastructure of this project');
    expect(made.text).toBe(
      [
        'The infrastructure of this project, read from the files as they stand. Nothing was run.',
        '',
        'Terraform',
        'infra/main.tf',
        '- Resource `aws_db_instance.orders`',
        '',
        'Dockerfiles',
        'Dockerfile (changed in this task)',
        '- image base `node:20`',
        '- Runs as `root`',
        '  Look twice: Whatever gets hold of the process is root in the container.',
        '',
        'Compose',
        'compose.yaml (new in this task)',
        '- Nothing in it could be read.',
        '',
        '2 more files were not read.',
        '',
        '1 of these deserves a second look.',
      ].join('\n'),
    );
    expect(allForAgent({ files: whole.slice(0, 1), more: 1 }).text).toContain('1 more file was not read.');
    expect(allForAgent({ files: whole.slice(0, 1), more: 0 }).text).not.toContain('second look');
  });
});

describe('a check of the infrastructure', () => {
  it('says where it runs', () => {
    expect(folderWords('infra/prod')).toBe('infra/prod');
    expect(folderWords('')).toBe('the folder of the task');
  });

  it('goes to the agent with what was started, how it ended and what it printed', () => {
    const made = checkForAgent(check(), '/usr/local/bin/terraform plan -lock=false', run());
    expect(made.title).toBe('terraform plan in infra ran');
    expect(made.text).toBe(
      [
        '$ /usr/local/bin/terraform plan -lock=false',
        '(in infra)',
        '',
        'Plan: 1 to add, 0 to change, 1 to destroy.',
        '',
        'It ended well.',
      ].join('\n'),
    );
    const failed = checkForAgent(check({ name: 'helm template', folder: '' }), 'helm template .', run({ exit: 1 }));
    expect(failed.title).toBe('helm template in the folder of the task failed');
    expect(failed.text).toContain('It ended with code 1.');
    expect(checkForAgent(check(), 'x', run({ running: true, exit: null })).title).toBe(
      'terraform plan in infra is running',
    );
    const stopped = checkForAgent(check(), 'x', run({ exit: null, stopped: true, output: null }));
    expect(stopped.title).toBe('terraform plan in infra was stopped');
    expect(stopped.text).toContain('(it printed nothing)');
    expect(stopped.text).toContain('It was stopped by hand.');
  });
});
