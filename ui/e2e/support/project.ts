import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { freePort, type Core } from './core';
import { ghState, threadsAnswer, type ReviewThread } from './standins';

/** The file the practice agent edits when the message names no other. */
export const DEMO_FILE = 'src/middleware/rateLimit.ts';
export const DEMO_TEXT = 'export const loginLimiter = rateLimit({\n  windowMs: 60 * 1000,\n  limit: 5,\n});\n';
/** The line the practice agent writes in place of `  limit: 5,`. */
export const AGENT_LINE = '  limit: limits.login,';

/** A file long enough for two changes that git shows apart. */
export const APP_FILE = 'src/app.ts';
export const APP_TEXT = [
  "import { loginLimiter } from './middleware/rateLimit';",
  "import { limits } from './limits/config';",
  '',
  'export type Order = { id: number; total: number };',
  '',
  'const orders: Order[] = [];',
  '',
  'export function addOrder(total: number): Order {',
  '  const order = { id: orders.length + 1, total };',
  '  orders.push(order);',
  '  return order;',
  '}',
  '',
  'export function listOrders(): Order[] {',
  '  return [...orders];',
  '}',
  '',
  'export function totalOf(list: Order[]): number {',
  '  let sum = 0;',
  '  for (const order of list) sum += order.total;',
  '  return sum;',
  '}',
  '',
  'export function findOrder(id: number): Order | undefined {',
  '  return orders.find((order) => order.id === id);',
  '}',
  '',
  'export function describe(order: Order): string {',
  '  return `order ${order.id}: ${order.total}`;',
  '}',
  '',
  'export const app = { loginLimiter, limits, addOrder, listOrders };',
  '',
].join('\n');

/** A dev script that says where it "listens", as real ones do, and then stays up. Nothing is served. */
export const DEV_SCRIPT = "printf 'ready\\n  Local: http://localhost:4567/\\n'; exec sleep 120";

export type GithubIssue = { number: number; title: string; body?: string; labels?: string[] };

export type ProjectOptions = {
  /** The start of the folder's name. A short tag is added, so every project has a name of its own. */
  name?: string;
  /** Without git the task works in the folder itself. */
  git?: boolean;
  /**
   * Where pushes go: nowhere, a bare repository on disk, or a bare repository
   * on disk behind an address that reads as GitHub.
   */
  remote?: 'none' | 'bare' | 'github';
  /** Leave out the file the practice agent edits by default, so it writes NOTES.md. */
  withoutDemoFile?: boolean;
  /** The `dev` script of package.json. */
  devScript?: string;
  /** More scripts of package.json, such as `lint` and `test`: the project's own checks. */
  scripts?: Record<string, string>;
  /** The port the compose file publishes for its `db` service. */
  dbPort?: number;
  /** Add a `worker` service; the Docker stand-in lets it stop with code 3 right after its start. */
  worker?: boolean;
  /** The open issues the `gh` stand-in lists for this project. */
  issues?: GithubIssue[];
};

let counter = 0;

/** A test project on disk: a folder with a few source files, by default a git repository with one commit. */
export class Project {
  /** The address of the remote as it is written in the configuration, such as `e2e/shop-ab12`. */
  slug: string | null = null;
  /** The bare repository pushes land in. */
  bare: string | null = null;

  constructor(
    private readonly core: Core,
    readonly name: string,
    readonly dir: string,
  ) {}

  /** Runs git in the project folder, or in `cwd`, with nothing of the user's own git setup. */
  git(args: string[], cwd = this.dir): string {
    return execFileSync('git', ['-C', cwd, ...args], { env: this.core.env, encoding: 'utf8' }).trim();
  }

  write(file: string, text: string) {
    const target = path.join(this.dir, file);
    mkdirSync(path.dirname(target), { recursive: true });
    writeFileSync(target, text);
  }

  read(file: string): string {
    return readFileSync(path.join(this.dir, file), 'utf8');
  }

  /** The branches the bare repository has, when the project has one. */
  remoteBranches(): string[] {
    if (!this.bare) return [];
    return this.git(['branch', '--format=%(refname:short)'], this.bare).split('\n').filter(Boolean);
  }

  /** Says which issues the `gh` stand-in lists for this project. */
  setIssues(issues: GithubIssue[]) {
    if (!this.slug) throw new Error('Only a project with a GitHub address has issues');
    const listed = issues.map((issue) => ({
      number: issue.number,
      title: issue.title,
      url: `https://github.com/${this.slug}/issues/${issue.number}`,
      state: 'OPEN',
      body: issue.body ?? '',
      labels: (issue.labels ?? []).map((name) => ({ name })),
      assignees: [],
      updatedAt: '2026-11-12T19:05:30Z',
    }));
    writeFileSync(path.join(ghState(this.core.toolsDir, this.slug), 'issues.json'), JSON.stringify(listed));
  }

  /** Says what the next pull request of this project looks like to `gh pr view`. */
  setPullTemplate(template: string) {
    if (!this.slug) throw new Error('Only a project with a GitHub address has pull requests');
    writeFileSync(path.join(ghState(this.core.toolsDir, this.slug), 'template.json'), template);
  }

  /** Says which review comments pull request `number` of this project has. */
  setReviewThreads(number: number, threads: ReviewThread[]) {
    if (!this.slug) throw new Error('Only a project with a GitHub address has pull requests');
    writeFileSync(path.join(ghState(this.core.toolsDir, this.slug), `threads-${number}.json`), threadsAnswer(threads));
  }

  /** Changes the pull request of a branch behind the core's back, as GitHub does when checks or reviews move. */
  changePull(branch: string, change: (pull: string) => string) {
    if (!this.slug) throw new Error('Only a project with a GitHub address has pull requests');
    const file = path.join(ghState(this.core.toolsDir, this.slug), `pull-${branch.replaceAll('/', '_')}.json`);
    writeFileSync(file, change(readFileSync(file, 'utf8')));
  }
}

/** Makes a project folder for a test inside the core's own temp folder. */
export async function makeProject(core: Core, options: ProjectOptions = {}): Promise<Project> {
  // Only digits: a name ends up in addresses the practice agent reads, and it takes words such as "fix" as a request.
  const tag = `${Date.now() % 100000}${counter++}`;
  const name = `${options.name ?? 'shop'}-${tag}`;
  const dir = path.join(core.projectsDir, name);
  if (existsSync(dir)) throw new Error(`${dir} exists already`);
  mkdirSync(dir, { recursive: true });
  const project = new Project(core, name, dir);

  project.write('README.md', '# Shop\n\nA small shop API for the end-to-end tests.\n');
  project.write('.gitignore', 'node_modules/\nignored.log\n');
  project.write(
    'package.json',
    JSON.stringify(
      { name: 'shop', private: true, scripts: { dev: options.devScript ?? DEV_SCRIPT, ...options.scripts } },
      null,
      2,
    ) + '\n',
  );
  // JSON is YAML too. Written this way, the Docker stand-in can "read" the file by printing it.
  const services: Record<string, unknown> = {
    db: {
      image: 'postgres:16',
      ports: [{ target: 5432, published: String(options.dbPort ?? (await freePort())), protocol: 'tcp' }],
    },
  };
  if (options.worker) services.worker = { build: { context: '.' } };
  project.write('compose.yaml', JSON.stringify({ name: 'shop', services }, null, 2) + '\n');
  if (!options.withoutDemoFile) project.write(DEMO_FILE, DEMO_TEXT);
  project.write(APP_FILE, APP_TEXT);
  project.write('src/limits/config.ts', 'export const limits = { login: 5, search: 30 };\n');
  // Not text: the editor must refuse to show it.
  writeFileSync(path.join(dir, 'logo.bin'), Buffer.from([0x89, 0x50, 0x4e, 0x47, 0, 1, 2, 3, 0, 255, 254, 0]));

  if (options.git !== false) {
    project.git(['init', '-q', '-b', 'main']);
    project.git(['config', 'user.name', 'Test']);
    project.git(['config', 'user.email', 'test@example.com']);
    project.git(['add', '.']);
    project.git(['commit', '-q', '-m', 'Start']);

    const remote = options.remote ?? 'none';
    if (remote !== 'none') {
      const bare = path.join(core.root, 'remotes', `${name}.git`);
      mkdirSync(bare, { recursive: true });
      project.git(['init', '-q', '--bare', bare]);
      project.bare = bare;
      if (remote === 'bare') {
        project.git(['remote', 'add', 'origin', bare]);
      } else {
        // The address reads as GitHub; git itself is told to use the folder instead.
        const address = `git@github.com:e2e/${name}.git`;
        project.slug = `e2e/${name}`;
        project.git(['remote', 'add', 'origin', address]);
        project.git(['config', `url.${bare}.insteadOf`, address]);
      }
    }
  }
  if (options.issues) project.setIssues(options.issues);
  return project;
}
