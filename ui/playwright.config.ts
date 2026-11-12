import { defineConfig, devices } from '@playwright/test';

// The same window size in every engine: the workbench is laid out for a desktop screen.
const viewport = { width: 1440, height: 900 };

/**
 * End-to-end tests of the interface against a real core. `npm run e2e` builds
 * the core and the interface first; every worker then starts a core of its own
 * that can reach only stand-ins for GitHub, GitLab and Docker (see e2e/support).
 */
export default defineConfig({
  testDir: './e2e',
  outputDir: './test-results',
  globalSetup: './e2e/support/global-setup.ts',
  forbidOnly: Boolean(process.env.CI),
  // Locally a failure is a failure. In CI one retry tells a flaky test from a broken one; the report shows both.
  retries: process.env.CI ? 1 : 0,
  timeout: 60_000,
  expect: { timeout: 10_000 },
  reporter: [['list'], ['html', { outputFolder: 'playwright-report', open: 'never' }]],
  use: {
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
  projects: [
    { name: 'chromium', use: { ...devices['Desktop Chrome'], viewport } },
    { name: 'webkit', use: { ...devices['Desktop Safari'], viewport } },
    { name: 'firefox', use: { ...devices['Desktop Firefox'], viewport } },
  ],
});
