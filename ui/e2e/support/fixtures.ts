import { test as base, expect } from '@playwright/test';
import { App } from './app';
import { Core } from './core';
import { makeProject, type Project, type ProjectOptions } from './project';

export { expect };
export * from './app';
export * from './project';
export { pullTemplate, setEmulators, setSimulators, simulatorRuns, standInNote } from './standins';

type Options = {
  /** Give the test a core of its own: for tests that count everything on Home or restart the core. */
  freshCore: boolean;
  /** Pause between the pieces the practice agent sends. Anything but the default also means a core of its own. */
  mockDelayMs: number;
  /** A core whose practice agent ends at once. */
  brokenAgent: boolean;
  /** A core whose practice agent pretends each turn costs this many dollars. */
  practiceCost: number;
  /** A core with a second practice agent. */
  twoAgents: boolean;
  /** A core where the practice language server stands in for the TypeScript one. */
  practiceLspForTs: boolean;
  /** A core of the test's own that has not been through the first run. */
  firstRun: boolean;
};

type PerTest = {
  core: Core;
  app: App;
  /** Makes a project folder for the test. */
  project: (options?: ProjectOptions) => Promise<Project>;
};

type PerWorker = {
  /** The core the tests of one worker share. It is started with the first test that wants it. */
  workerCore: { get: () => Promise<Core> };
};

const DEFAULT_DELAY = 30;

export const test = base.extend<Options & PerTest, PerWorker>({
  freshCore: [false, { option: true }],
  mockDelayMs: [DEFAULT_DELAY, { option: true }],
  brokenAgent: [false, { option: true }],
  practiceCost: [0, { option: true }],
  twoAgents: [false, { option: true }],
  practiceLspForTs: [false, { option: true }],
  firstRun: [false, { option: true }],

  workerCore: [
    // eslint-disable-next-line no-empty-pattern -- Playwright reads the names of the fixtures from this pattern
    async ({}, use) => {
      let core: Core | null = null;
      await use({ get: async () => (core ??= await Core.start({ mockDelayMs: DEFAULT_DELAY })) });
      await (core as Core | null)?.dispose();
    },
    { scope: 'worker' },
  ],

  core: async (
    { workerCore, freshCore, mockDelayMs, brokenAgent, practiceCost, twoAgents, practiceLspForTs, firstRun },
    use,
    testInfo,
  ) => {
    const own =
      freshCore ||
      firstRun ||
      brokenAgent ||
      practiceLspForTs ||
      mockDelayMs !== DEFAULT_DELAY ||
      practiceCost > 0 ||
      twoAgents;
    const core = own
      ? await Core.start({ mockDelayMs, brokenAgent, practiceCost, twoAgents, practiceLspForTs, firstRun })
      : await workerCore.get();
    const logFrom = core.log.length;
    await use(core);
    if (testInfo.status !== testInfo.expectedStatus) {
      await testInfo.attach('core.log', { body: core.log.slice(logFrom), contentType: 'text/plain' });
    }
    if (own) await core.dispose();
  },

  project: async ({ core }, use) => {
    await use((options) => makeProject(core, options));
  },

  app: async ({ page, core }, use) => {
    // Without WebGL the terminal draws its text as plain elements, which a test can read.
    await page.addInitScript(() => {
      const original = HTMLCanvasElement.prototype.getContext;
      HTMLCanvasElement.prototype.getContext = function (this: HTMLCanvasElement, kind: string, ...rest: unknown[]) {
        if (kind === 'webgl' || kind === 'webgl2') return null;
        return (original as (...args: unknown[]) => unknown).call(this, kind, ...rest);
      } as typeof original;
    });
    // An error nobody caught in the page is a bug, whatever the test was looking at.
    const uncaught: string[] = [];
    page.on('pageerror', (error) => uncaught.push(error.stack || error.message));
    await use(new App(page, core));
    expect(uncaught, 'errors nobody caught in the page').toEqual([]);
  },
});
