import { spawn, type ChildProcess } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, realpathSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { callsOf, STAND_INS, writeStandIns, type StandIn } from './standins';

const here = path.dirname(fileURLToPath(import.meta.url));
export const UI_DIR = path.resolve(here, '../..');
export const REPO_DIR = path.resolve(UI_DIR, '..');
export const CORE_BINARY = path.join(REPO_DIR, 'target', 'debug', 'ae-core');
export const AGENT_BINARY = path.join(REPO_DIR, 'target', 'debug', 'ae-mock-agent');
export const UI_DIST = path.join(UI_DIR, 'dist');

/** Nothing listens here, so a call to "Linear" fails at once and never leaves this machine. */
export const LINEAR_URL = 'http://127.0.0.1:9/graphql';
/** A preview of the app runs on this port. The tests must never take it. */
const FORBIDDEN_PORT = 4477;
/** Folders the tests must never write to. */
const FORBIDDEN_FOLDERS = [path.join(REPO_DIR, 'data'), path.resolve(REPO_DIR, '..', 'agent-editor-demos')];

const TEMP_PREFIX = 'ae-e2e-';

/** A port nothing listens on right now. */
export async function freePort(): Promise<number> {
  for (;;) {
    const port = await new Promise<number>((resolve, reject) => {
      const server = createServer();
      server.once('error', reject);
      server.listen(0, '127.0.0.1', () => {
        const address = server.address();
        server.close(() => resolve(typeof address === 'object' && address ? address.port : 0));
      });
    });
    if (port !== 0 && port !== FORBIDDEN_PORT) return port;
  }
}

export type CoreOptions = {
  /** Pause between the pieces the practice agent sends, in milliseconds. */
  mockDelayMs?: number;
  /** Start the core with a practice agent that ends at once, to see what a failed start does. */
  brokenAgent?: boolean;
  /** What the practice agent pretends each turn costs, in dollars. Nothing is charged. */
  practiceCost?: number;
  /** Add a second practice agent, so a task can be handed from one agent to another. */
  twoAgents?: boolean;
  /** Let the practice language server stand in for the TypeScript one, which the tests never have. */
  practiceLspForTs?: boolean;
};

export type TaskInfo = {
  id: string;
  projectId: string;
  title: string;
  cwd: string;
  branch: string | null;
  state: string;
};

/**
 * One core process with a temp folder of its own: its data, the stand-ins it
 * may call, a home folder and the test projects. It can only reach the
 * stand-ins, never GitHub, GitLab, Linear or Docker.
 */
export class Core {
  /** Everything of this core lives under this folder; it is removed at the end. */
  readonly root: string;
  readonly dataDir: string;
  /** The stand-ins, and the first entry of the core's PATH. */
  readonly toolsDir: string;
  readonly projectsDir: string;
  /** The environment the core runs with. Nothing is inherited from the test runner. */
  readonly env: Record<string, string>;
  private child: ChildProcess | null = null;
  private output = '';
  private portNumber = 0;

  private constructor(options: CoreOptions) {
    this.root = realpathSync(mkdtempSync(path.join(os.tmpdir(), TEMP_PREFIX)));
    this.dataDir = path.join(this.root, 'data');
    this.toolsDir = path.join(this.root, 'tools');
    this.projectsDir = path.join(this.root, 'projects');
    const home = path.join(this.root, 'home');
    const nodeBin = path.join(this.root, 'node');
    for (const dir of [this.dataDir, this.projectsDir, home, nodeBin]) mkdirSync(dir, { recursive: true });
    for (const forbidden of FORBIDDEN_FOLDERS) {
      if (this.dataDir.startsWith(forbidden)) throw new Error(`The tests must not use ${forbidden}`);
    }
    writeStandIns(this.toolsDir);

    // `npm run dev` of a test project needs node and npm. `npx` is left out on
    // purpose: without it the core finds no real agent, so none can be started.
    symlinkSync(process.execPath, path.join(nodeBin, 'node'));
    const npm = path.join(path.dirname(process.execPath), 'npm');
    if (existsSync(npm) && path.basename(realpathSync(npm)) === 'npm-cli.js') {
      symlinkSync(realpathSync(npm), path.join(nodeBin, 'npm'));
    }

    const gitConfig = path.join(this.root, 'gitconfig');
    writeFileSync(gitConfig, '[user]\n\tname = Test\n\temail = test@example.com\n[init]\n\tdefaultBranch = main\n');

    this.env = {
      PATH: [this.toolsDir, '/usr/bin', '/bin', '/usr/sbin', '/sbin', nodeBin].join(':'),
      HOME: home,
      // A plain shell that reads nothing from the user's own setup.
      SHELL: '/bin/sh',
      TMPDIR: os.tmpdir(),
      LANG: 'en_US.UTF-8',
      AE_LINEAR_URL: LINEAR_URL,
      AE_MOCK_DELAY_MS: String(options.mockDelayMs ?? 30),
      // The core looks at what is due (automations, requests that merge by themselves) often, so tests need not wait.
      AE_WATCH_EVERY_MS: '1500',
      GIT_CONFIG_GLOBAL: gitConfig,
      GIT_CONFIG_NOSYSTEM: '1',
      NPM_CONFIG_UPDATE_NOTIFIER: 'false',
    };
    if (options.brokenAgent) this.env.AE_MOCK_AGENT = '/usr/bin/false';
    if (options.practiceCost) this.env.AE_MOCK_COST = String(options.practiceCost);
    if (options.twoAgents) this.env.AE_MOCK_TWIN = '1';
    if (options.practiceLspForTs) this.env.AE_MOCK_LSP_TS = '1';
  }

  static async start(options: CoreOptions = {}): Promise<Core> {
    for (const binary of [CORE_BINARY, AGENT_BINARY]) {
      if (!existsSync(binary)) throw new Error(`${binary} is missing. Build it with: cargo build --workspace`);
    }
    if (!existsSync(path.join(UI_DIST, 'index.html'))) {
      throw new Error(`${UI_DIST} is missing. Build it with: npm run build (in ui/)`);
    }
    const core = new Core(options);
    core.portNumber = await freePort();
    await core.launch();
    return core;
  }

  get port(): number {
    return this.portNumber;
  }

  get url(): string {
    return `http://127.0.0.1:${this.portNumber}`;
  }

  /** The process id of the running core. */
  get pid(): number | undefined {
    return this.child?.pid;
  }

  /** What the core printed so far, for the report of a failed test. */
  get log(): string {
    return this.output;
  }

  private async launch() {
    this.checkSafety();
    // `PORT` set to the core's own port is the core's setting, not the project's (case A-07).
    const env = { ...this.env, PORT: String(this.portNumber) };
    const args = ['--port', String(this.portNumber), '--host', '127.0.0.1', '--ui-dir', UI_DIST];
    const child = spawn(CORE_BINARY, [...args, '--data-dir', this.dataDir], {
      cwd: this.root,
      env,
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    this.child = child;
    child.stdout?.on('data', (chunk: Buffer) => (this.output += chunk.toString()));
    child.stderr?.on('data', (chunk: Buffer) => (this.output += chunk.toString()));

    const deadline = Date.now() + 20_000;
    for (;;) {
      if (child.exitCode !== null) throw new Error(`The core ended while starting:\n${this.output}`);
      try {
        const answer = await fetch(`${this.url}/api/health`);
        if (answer.ok) return;
      } catch {
        // Not listening yet.
      }
      if (Date.now() > deadline) throw new Error(`The core did not start in time:\n${this.output}`);
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
  }

  /** Refuses to start a core that could reach a real service or the user's own data. */
  private checkSafety() {
    const first = this.env.PATH.split(':')[0];
    if (first !== this.toolsDir) throw new Error('The stand-ins must be first on the PATH of the core');
    for (const name of STAND_INS) {
      if (!existsSync(path.join(first, name))) throw new Error(`The stand-in for ${name} is missing`);
    }
    if ('LINEAR_API_KEY' in this.env) throw new Error('The core must not be given a Linear key');
    if (this.env.AE_LINEAR_URL !== LINEAR_URL) throw new Error('The core must not be able to reach Linear');
    if (this.portNumber === FORBIDDEN_PORT) throw new Error(`Port ${FORBIDDEN_PORT} belongs to the preview`);
  }

  /** Waits until the process is gone. */
  private async ended(child: ChildProcess, withinMs: number): Promise<boolean> {
    if (child.exitCode !== null || child.signalCode !== null) return true;
    return new Promise((resolve) => {
      const timer = setTimeout(() => resolve(false), withinMs);
      child.once('exit', () => {
        clearTimeout(timer);
        resolve(true);
      });
    });
  }

  /** Stops the core the polite way: it stops its agents and terminals first. */
  async stop() {
    const child = this.child;
    if (!child) return;
    this.child = null;
    child.kill('SIGINT');
    if (!(await this.ended(child, 5000))) {
      child.kill('SIGKILL');
      await this.ended(child, 5000);
    }
  }

  /** Ends the core without warning, as a crash would. */
  async kill() {
    const child = this.child;
    if (!child) return;
    this.child = null;
    child.kill('SIGKILL');
    await this.ended(child, 5000);
  }

  /** Starts the core again on the same port and the same data folder. */
  async startAgain() {
    if (this.child) throw new Error('The core is still running');
    await this.launch();
  }

  async restart() {
    await this.stop();
    await this.startAgain();
  }

  /** Stops the core and removes everything it had. */
  async dispose() {
    await this.stop();
    const temp = realpathSync(os.tmpdir());
    // Only a folder this class made itself is ever removed.
    if (this.root.startsWith(temp + path.sep) && path.basename(this.root).startsWith(TEMP_PREFIX)) {
      rmSync(this.root, { recursive: true, force: true, maxRetries: 3 });
    }
  }

  /** Every call a stand-in got so far. */
  calls(name: StandIn): string[] {
    return callsOf(this.toolsDir, name);
  }

  /** Sends one command to the core over its own protocol. For setting things up quickly, and for looking at state. */
  async request<T = unknown>(method: string, params: Record<string, unknown> = {}): Promise<T> {
    const socket = new WebSocket(`ws://127.0.0.1:${this.portNumber}/ws`);
    try {
      return await new Promise<T>((resolve, reject) => {
        socket.addEventListener('open', () => socket.send(JSON.stringify({ id: 1, method, params })));
        socket.addEventListener('error', () => reject(new Error(`The core cannot be reached for ${method}`)));
        socket.addEventListener('message', (message) => {
          if (typeof message.data !== 'string') return;
          const parsed = JSON.parse(message.data) as { id?: number; result?: T; error?: { message: string } };
          if (parsed.id !== 1) return;
          if (parsed.error) reject(new Error(parsed.error.message));
          else resolve(parsed.result as T);
        });
      });
    } finally {
      socket.close();
    }
  }

  /** What the core knows about a task: its folder and its branch above all. */
  async task(taskId: string): Promise<TaskInfo> {
    const tasks = await this.request<TaskInfo[]>('tasks.list');
    const found = tasks.find((task) => task.id === taskId);
    if (!found) throw new Error(`The core has no task ${taskId}`);
    return found;
  }
}
