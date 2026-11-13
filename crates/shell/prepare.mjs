// Gets ready the two things the app carries inside: the built interface and
// the practice agent. Tauri runs this before `tauri dev` and `tauri build`;
// run it by hand (`node prepare.mjs`) before a plain `cargo build` or `cargo clippy`
// of this crate, which refuse to compile while these files are missing.

import { spawnSync } from 'node:child_process';
import { copyFileSync, cpSync, existsSync, mkdirSync, rmSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, '..', '..');
const ui = path.join(repo, 'ui');
const windows = process.platform === 'win32';

/** Runs one command and stops with a plain message when it fails. */
function run(what, command, args, cwd) {
  console.log(`\n${what}: ${command} ${args.join(' ')}`);
  // On Windows `npm` is a script, which only a shell can start.
  const done = spawnSync(command, args, { cwd, stdio: 'inherit', shell: windows && command === 'npm' });
  if (done.error) {
    console.error(`${what}: "${command}" could not be run. Is it installed? (${done.error.message})`);
    process.exit(1);
  }
  if (done.status !== 0) {
    console.error(`${what} failed: "${command} ${args.join(' ')}" in ${cwd} ended with code ${done.status}.`);
    process.exit(1);
  }
}

// ----- The interface -----

if (!existsSync(path.join(ui, 'node_modules'))) {
  console.error(`The interface has no packages yet. Run "npm ci" in ${ui} first.`);
  process.exit(1);
}
run('Building the interface', 'npm', ['run', 'build'], ui);

// The app gets a copy without the source maps: they are two thirds of the
// folder and nothing in the app reads them. A copy also means a later build of
// the interface cannot change what a finished app carries.
const staged = path.join(here, 'ui');
rmSync(staged, { recursive: true, force: true });
cpSync(path.join(ui, 'dist'), staged, { recursive: true, filter: (file) => !file.endsWith('.map') });
console.log(`The interface is in ${staged}`);

// ----- The practice agent -----

// Tauri looks for a program it bundles under "<name>-<system it is built for>".
function hostTriple() {
  const told = spawnSync('rustc', ['-vV'], { encoding: 'utf8' });
  const found = /^host: (.+)$/m.exec(told.stdout ?? '');
  if (!found) {
    console.error('Could not ask rustc which system this is. Is Rust installed?');
    process.exit(1);
  }
  return found[1].trim();
}

const host = hostTriple();
// Set by Tauri when the app is built for a system other than this one.
const triple = process.env.TAURI_ENV_TARGET_TRIPLE || host;
const cross = triple !== host;
const args = ['build', '--release', '-p', 'ae-mock-agent'];
if (cross) args.push('--target', triple);
run('Building the practice agent', 'cargo', args, repo);

const exe = triple.includes('windows') ? '.exe' : '';
const built = path.join(repo, 'target', ...(cross ? [triple] : []), 'release', `ae-mock-agent${exe}`);
const binaries = path.join(here, 'binaries');
mkdirSync(binaries, { recursive: true });
const sidecar = path.join(binaries, `ae-mock-agent-${triple}${exe}`);
copyFileSync(built, sidecar);
console.log(`The practice agent is in ${sidecar}`);
