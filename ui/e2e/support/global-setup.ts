import { spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import path from 'node:path';
import { AGENT_BINARY, CORE_BINARY, REPO_DIR, UI_DIR, UI_DIST } from './core';

/** Runs one build step and stops everything with a plain message when it fails. */
function build(what: string, command: string, args: string[], cwd: string) {
  const done = spawnSync(command, args, { cwd, stdio: 'inherit' });
  if (done.error) {
    throw new Error(`${what}: the command "${command}" could not be run. Is it installed? (${done.error.message})`);
  }
  if (done.status !== 0) {
    throw new Error(`${what} failed: "${command} ${args.join(' ')}" in ${cwd} ended with code ${done.status}.`);
  }
}

/**
 * Builds the core, the practice agent and the interface once, before any test.
 * Set E2E_SKIP_BUILD=1 to use what is built already.
 */
export default function globalSetup() {
  if (process.platform === 'win32') {
    throw new Error('These tests stand in for gh, glab and docker with shell scripts, so they do not run on Windows.');
  }
  if (!process.env.E2E_SKIP_BUILD) {
    build('Building the core', 'cargo', ['build', '--workspace'], REPO_DIR);
    build('Building the interface', 'npm', ['run', 'build'], UI_DIR);
  }
  const needed: [string, string][] = [
    [CORE_BINARY, 'cargo build --workspace'],
    [AGENT_BINARY, 'cargo build --workspace'],
    [path.join(UI_DIST, 'index.html'), 'npm run build (in ui/)'],
  ];
  for (const [file, how] of needed) {
    if (!existsSync(file)) throw new Error(`${file} is missing. Build it with: ${how}`);
  }
}
