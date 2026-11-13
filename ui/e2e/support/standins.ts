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
  "api graphql")
    # The open review threads of a pull request: asked with owner=, name= and number=.
    owner=""; name=""; number=""
    for word in "$@"; do
      case "$word" in
        owner=*) owner=$(printf '%s' "$word" | sed 's/^owner=//') ;;
        name=*) name=$(printf '%s' "$word" | sed 's/^name=//') ;;
        number=*) number=$(printf '%s' "$word" | sed 's/^number=//') ;;
      esac
    done
    file="$here/state/gh/$owner"_"$name/threads-$number.json"
    if [ -f "$file" ]; then cat "$file"; else echo '{"data":{"repository":{"pullRequest":{"reviewThreads":{"nodes":[]}}}}}'; fi ;;
  "api --hostname") echo '{}' ;;
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
# A stand-in for Docker. No container is started: "running" containers are lines in a file,
# one file per compose project, and a named volume is a file that remembers its project.
here="$(cd "$(dirname "$0")" && pwd)"
# One call, one line, written in one go: several calls may be logged at the same moment.
printf '%s\n' "$(echo "$@" | tr '\n' ' ')" >> "$here/docker.log"
state="$here/state/docker"
mkdir -p "$state/volumes"

if [ "$1" = "info" ]; then
  if [ -f "$state/engine-down" ]; then echo "Cannot connect to the Docker daemon" >&2; exit 1; fi
  echo "29.0.0"
  exit 0
fi

project=""; file=""; prev=""; last=""; inside=""
for word in "$@"; do
  [ "$prev" = "-p" ] && project="$word"
  [ "$prev" = "-f" ] && file="$word"
  [ "$prev" = "exec" ] && [ -z "$inside" ] && inside="$word"
  prev="$word"; last="$word"
done
running="$state/$project.running"

# One service of the compose file as it is once "up". The worker stops with code 3 right after its start.
container() {
  if [ "$1" = "worker" ]; then
    printf '{"Service":"worker","State":"exited","ExitCode":3,"Publishers":[]}\n'
    return
  fi
  # The file is JSON written with two spaces a level: a service starts and ends four spaces in.
  port=$(sed -n "/^    \"$1\": {/,/^    }/p" "$file" | grep -o '"published": *"[0-9]*"' | head -1 | grep -o '[0-9][0-9]*')
  [ -z "$port" ] && port=0
  printf '{"Service":"%s","State":"running","ExitCode":0,"Publishers":[{"PublishedPort":%s,"TargetPort":5432}]}\n' "$1" "$port"
}

# Every running container of every compose project: its project, then its service.
all_running() {
  for one in "$state"/*.running; do
    [ -f "$one" ] || continue
    of=$(basename "$one" .running)
    grep '"State":"running"' "$one" | sed 's/.*"Service":"\([^"]*\)".*/\1/' | while read -r service; do
      echo "$of $service"
    done
  done
}

# What a container "uses": a database more than the rest.
memory_of() {
  case "$1" in db) echo "120MiB" ;; cache) echo "18MiB" ;; *) echo "64MiB" ;; esac
}

without() {
  if [ -f "$running" ]; then
    grep -v "\"Service\":\"$1\"" "$running" > "$running.tmp"
    mv "$running.tmp" "$running"
  fi
}

case "$*" in
  *" exec "*)
    # A shell "inside" the container: a plain shell of this machine with the service's name as its prompt.
    echo "A shell in $project-$inside-1"
    PS1="$inside# " exec /bin/sh ;;
  *" config --format json")
    # What of the task's environment reached Docker, for the tests to read.
    env | grep '^SHOP_' | sort > "$state/$project.env"
    # The test projects write their compose file as JSON, which is also YAML: reading it is printing it.
    # Docker names volumes after the project it is asked about; the test files say __PROJECT__ there.
    sed "s/__PROJECT__/$project/g" "$file" ;;
  *" ps --all --format json") [ -f "$running" ] && cat "$running"; exit 0 ;;
  "ps --format json")
    all_running | while read -r of service; do
      printf '{"ID":"%s-%s","Names":"%s-%s-1","Labels":"com.docker.compose.project=%s,com.docker.compose.service=%s","State":"running"}\n' "$of" "$service" "$of" "$service" "$of" "$service"
    done ;;
  "stats --no-stream --format json")
    all_running | while read -r of service; do
      printf '{"ID":"%s-%s","Name":"%s-%s-1","MemUsage":"%s / 7.651GiB"}\n' "$of" "$service" "$of" "$service" "$(memory_of "$service")"
    done ;;
  "system df --format json")
    printf '{"Type":"Images","Size":"2.2GB"}\n{"Type":"Local Volumes","Size":"0B"}\n' ;;
  "system df -v --format json")
    list=""
    for one in "$state"/volumes/*; do
      [ -f "$one" ] || continue
      [ -n "$list" ] && list="$list,"
      list="$list{\"Name\":\"$(basename "$one")\",\"Labels\":\"com.docker.compose.project=$(cat "$one")\",\"Size\":\"84.2MB\"}"
    done
    printf '{"Images":[],"Containers":[],"Volumes":[%s]}\n' "$list" ;;
  "volume rm "*)
    if [ ! -f "$state/volumes/$3" ]; then echo "Error response from daemon: get $3: no such volume" >&2; exit 1; fi
    rm -f "$state/volumes/$3" ;;
  *" up -d"*)
    # The services to start are named after the flags; none named means every service of the file.
    wanted=""; after=""
    for word in "$@"; do
      if [ -n "$after" ]; then
        case "$word" in -*) ;; *) wanted="$wanted $word" ;; esac
      fi
      [ "$word" = "up" ] && after=1
    done
    if [ -z "$wanted" ]; then
      for name in web db cache legacy worker; do
        grep -q "^    \"$name\": {" "$file" && wanted="$wanted $name"
      done
    fi
    touch "$running"
    for name in $wanted; do
      without "$name"
      container "$name" >> "$running"
      echo " Container $project-$name-1  Started"
    done
    # The named volumes of the file are made with the first start.
    sed -n '/^  "volumes": {/,/^  }/p' "$file" | grep -o '"name": *"[^"]*"' | sed 's/.*"\([^"]*\)"$/\1/' | while read -r volume; do
      [ -f "$state/volumes/$volume" ] || echo "$project" > "$state/volumes/$volume"
    done ;;
  *" logs --follow"*)
    echo "$last-1  | database system is ready to accept connections"
    exec sleep 60 ;;
  *" rm --stop --force --volumes "*) without "$last" ;;
  *" down"*)
    rm -f "$running"
    case "$*" in
      *"--volumes"*)
        for one in "$state"/volumes/*; do
          if [ -f "$one" ] && [ "$(cat "$one")" = "$project" ]; then rm -f "$one"; fi
        done ;;
    esac ;;
  *" stop "*) without "$last" ;;
  *) echo "unexpected call: $*" >&2; exit 2 ;;
esac
exit 0
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

/** A review comment as the tests write it. */
export type ReviewThread = { id: number; author: string; path: string; line: number; body: string; resolved?: boolean };

/** Review threads as GitHub answers them, for the `gh` stand-in to print. */
export function threadsAnswer(threads: ReviewThread[]): string {
  const nodes = threads.map((thread) => ({
    isResolved: thread.resolved ?? false,
    comments: {
      nodes: [
        {
          databaseId: thread.id,
          author: { login: thread.author },
          path: thread.path,
          line: thread.line,
          body: thread.body,
          url: `https://github.com/e2e/pull#discussion_r${thread.id}`,
        },
      ],
    },
  }));
  return JSON.stringify({ data: { repository: { pullRequest: { reviewThreads: { nodes } } } } });
}

/** The folder where the `gh` stand-in keeps what it knows about one repository. */
export function ghState(dir: string, slug: string): string {
  const folder = path.join(dir, 'state', 'gh', slug.replaceAll('/', '_'));
  mkdirSync(folder, { recursive: true });
  return folder;
}
