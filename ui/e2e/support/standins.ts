import { chmodSync, existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';

/**
 * Stand-ins for the command-line tools the core calls: `gh`, `glab` and `docker`.
 * They are shell scripts in a folder that is first on the core's PATH, so the
 * real tools are never reached. Each one writes every call to `<name>.log`
 * next to itself and keeps what it "knows" in files under `state/`.
 */

/** The pull request `gh pr create` makes when a project has no template of its own. */
export const PULL_READY = pullTemplate({});

/** What a pull request looks like to `gh pr view`. The stand-in fills in the number and the slug. */
export function pullTemplate(options: {
  checks?: 'passed' | 'failed' | 'pending' | 'none';
  mergeable?: 'MERGEABLE' | 'CONFLICTING' | 'UNKNOWN';
  review?: '' | 'APPROVED' | 'CHANGES_REQUESTED' | 'REVIEW_REQUIRED';
}): string {
  const run = (status: string, conclusion: string) => ({ __typename: 'CheckRun', status, conclusion });
  const rollup = {
    passed: [run('COMPLETED', 'SUCCESS')],
    failed: [run('COMPLETED', 'FAILURE')],
    pending: [run('COMPLETED', 'SUCCESS'), run('IN_PROGRESS', '')],
    none: [],
  }[options.checks ?? 'passed'];
  // No spaces: the stand-in finds a request by the text `"number":128,`.
  return JSON.stringify({
    number: '__NUMBER__',
    title: 'TITLE',
    url: 'https://github.com/__SLUG__/pull/__NUMBER__',
    state: 'OPEN',
    isDraft: false,
    mergeable: options.mergeable ?? 'MERGEABLE',
    reviewDecision: options.review ?? '',
    statusCheckRollup: rollup,
  }).replace('"__NUMBER__"', '__NUMBER__');
}

const GH = String.raw`#!/bin/sh
# A stand-in for the GitHub command line. It never leaves this machine.
here="$(cd "$(dirname "$0")" && pwd)"
echo "$@" | tr '\n' ' ' >> "$here/gh.log"
echo >> "$here/gh.log"

slug=""; head=""; prev=""
for word in "$@"; do
  [ "$prev" = "-R" ] && slug="$word"
  [ "$prev" = "--head" ] && head="$word"
  prev="$word"
done
flat() { printf '%s' "$1" | tr '/' '_'; }
state="$here/state/gh/$(flat "$slug")"

case "$1 $2" in
  "auth status") exit 0 ;;
  "issue list")
    if [ -f "$state/issues.json" ]; then cat "$state/issues.json"; else echo "[]"; fi ;;
  "pr view")
    file="$state/pull-$(flat "$3").json"
    if [ -f "$file" ]; then cat "$file"; else echo "no pull requests found for branch \"$3\"" >&2; exit 1; fi ;;
  "pr create")
    mkdir -p "$state"
    template="$state/template.json"
    [ -f "$template" ] || template="$here/state/gh/template.json"
    count=$(ls "$state" | grep -c '^pull-')
    number=$((128 + count))
    sed "s/__NUMBER__/$number/g; s|__SLUG__|$slug|g" "$template" > "$state/pull-$(flat "$head").json"
    echo "https://github.com/$slug/pull/$number" ;;
  "pr merge")
    for file in "$state"/pull-*.json; do
      [ -f "$file" ] || continue
      if grep -q "\"number\":$3," "$file"; then
        sed 's/"state":"OPEN"/"state":"MERGED"/' "$file" > "$file.tmp" && mv "$file.tmp" "$file"
      fi
    done ;;
  *) echo "unexpected call: $*" >&2; exit 2 ;;
esac
`;

const GLAB = String.raw`#!/bin/sh
# A stand-in for the GitLab command line. Nobody is signed in, and nothing is asked of GitLab.
here="$(cd "$(dirname "$0")" && pwd)"
echo "$@" | tr '\n' ' ' >> "$here/glab.log"
echo >> "$here/glab.log"
echo "glab stand-in: not signed in" >&2
exit 1
`;

const DOCKER = String.raw`#!/bin/sh
# A stand-in for Docker. No container is started: "running" containers are lines in a file.
here="$(cd "$(dirname "$0")" && pwd)"
echo "$@" | tr '\n' ' ' >> "$here/docker.log"
echo >> "$here/docker.log"
mkdir -p "$here/state/docker"

if [ "$1" = "info" ]; then
  if [ -f "$here/state/docker/engine-down" ]; then echo "Cannot connect to the Docker daemon" >&2; exit 1; fi
  echo "29.0.0"
  exit 0
fi

project=""; file=""; prev=""; last=""
for word in "$@"; do
  [ "$prev" = "-p" ] && project="$word"
  [ "$prev" = "-f" ] && file="$word"
  prev="$word"; last="$word"
done
running="$here/state/docker/$project.running"

case "$*" in
  # The test projects write their compose file as JSON, which is also YAML: reading it is printing it.
  *" config --format json") cat "$file" ;;
  *" ps --all --format json") [ -f "$running" ] && cat "$running"; exit 0 ;;
  *" up -d"*)
    # The task's own file is the one after -f; the port in it is the one "published".
    port=$(grep -o '"published": *"[0-9]*"' "$file" | head -1 | grep -o '[0-9][0-9]*')
    echo " Container $project-db-1  Started"
    printf '{"Service":"db","State":"running","ExitCode":0,"Publishers":[{"PublishedPort":%s,"TargetPort":5432}]}\n' "$port" > "$running"
    if grep -q '"worker"' "$file"; then
      printf '{"Service":"worker","State":"exited","ExitCode":3,"Publishers":[]}\n' >> "$running"
    fi ;;
  *" logs --follow"*)
    echo "$last-1  | database system is ready to accept connections"
    exec sleep 60 ;;
  *" down"*) rm -f "$running" ;;
  *" stop "*)
    if [ -f "$running" ]; then
      grep -v "\"Service\":\"$last\"" "$running" > "$running.tmp"
      mv "$running.tmp" "$running"
    fi ;;
  *) echo "unexpected call: $*" >&2; exit 2 ;;
esac
`;

export const STAND_INS = ['gh', 'glab', 'docker'] as const;
export type StandIn = (typeof STAND_INS)[number];

/** Writes the stand-ins into `dir` and makes them runnable. */
export function writeStandIns(dir: string) {
  mkdirSync(path.join(dir, 'state', 'gh'), { recursive: true });
  mkdirSync(path.join(dir, 'state', 'docker'), { recursive: true });
  const scripts: Record<StandIn, string> = { gh: GH, glab: GLAB, docker: DOCKER };
  for (const name of STAND_INS) {
    const file = path.join(dir, name);
    writeFileSync(file, scripts[name]);
    chmodSync(file, 0o755);
  }
  writeFileSync(path.join(dir, 'state', 'gh', 'template.json'), PULL_READY);
}

/** Every call a stand-in got so far, one per entry, as the words it was called with. */
export function callsOf(dir: string, name: StandIn): string[] {
  const file = path.join(dir, `${name}.log`);
  if (!existsSync(file)) return [];
  return readFileSync(file, 'utf8')
    .split('\n')
    .map((line) => line.trim())
    .filter(Boolean);
}

/** The folder where the `gh` stand-in keeps what it knows about one repository. */
export function ghState(dir: string, slug: string): string {
  const folder = path.join(dir, 'state', 'gh', slug.replaceAll('/', '_'));
  mkdirSync(folder, { recursive: true });
  return folder;
}
