import { chmodSync, existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { deflateSync } from 'node:zlib';

/**
 * Stand-ins for the command-line tools the core calls: `gh`, `glab` and `docker`,
 * and for the programs the Preview runs: a browser that takes screenshots, Xcode's
 * `xcrun`, Android's `adb` and `emulator`, `idb`, which taps on a simulator, and
 * `terraform` and `helm`, which check infrastructure files.
 * They are shell scripts. The first three are in a folder that is first on the
 * core's PATH; the core is told where the others are (`AE_BROWSER`, `AE_XCRUN`,
 * `AE_ADB`, `AE_EMULATOR`, `AE_IDB`, `AE_TERRAFORM`, `AE_HELM`), so the real tools
 * are never reached.
 * Each one writes every call to `<name>.log` next to itself and keeps what it
 * "knows" in files under `state/`.
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
  "auth token") echo "gho_stand_in_token" ;;
  "issue view")
    # One issue, whole: what a test wrote for its number.
    if [ -f "$state/issue-$3.json" ]; then cat "$state/issue-$3.json"; else echo "GraphQL: Could not resolve to an Issue with the number of $3." >&2; exit 1; fi ;;
  "pr view")
    case "$3" in
      *[!0-9]*)
        file="$state/pull-$(flat "$3").json"
        if [ -f "$file" ]; then cat "$file"; else echo "no pull requests found for branch \"$3\"" >&2; exit 1; fi ;;
      *)
        # A request asked for by its number is asked for whole: what a test wrote for it, else the request as it is.
        if [ -f "$state/whole-$3.json" ]; then cat "$state/whole-$3.json"; exit 0; fi
        for file in "$state"/pull-*.json; do
          [ -f "$file" ] || continue
          if grep -q "\"number\":$3," "$file"; then cat "$file"; exit 0; fi
        done
        echo "GraphQL: Could not resolve to a PullRequest with the number of $3." >&2; exit 1 ;;
    esac ;;
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
  "api repos/"*"/deployments/"*)
    # How one deployment stands: what a test wrote for its number, or nothing yet.
    slug=$(printf '%s' "$2" | sed 's|^repos/||; s|/deployments.*||')
    number=$(printf '%s' "$2" | sed 's|.*/deployments/||; s|/.*||')
    file="$here/state/gh/$(flat "$slug")/deploy-$number.json"
    if [ -f "$file" ]; then cat "$file"; else echo "[]"; fi ;;
  "api repos/"*"/deployments?"*)
    # The deployments of a repository: what a test wrote, whatever branch or commit is asked for.
    slug=$(printf '%s' "$2" | sed 's|^repos/||; s|/deployments.*||')
    file="$here/state/gh/$(flat "$slug")/deploys.json"
    if [ -f "$file" ]; then cat "$file"; else echo "[]"; fi ;;
  "run list")
    # The runs of a branch, newest first: what a test wrote, or none.
    if [ -f "$state/runs.json" ]; then cat "$state/runs.json"; else echo "[]"; fi ;;
  "run view")
    if [ -f "$state/jobs-$3.json" ]; then cat "$state/jobs-$3.json"; else echo '{"jobs":[]}'; fi ;;
  "run rerun")
    # A run started again is a run in progress, and so are its jobs that had failed.
    run="$3"; [ "$3" = "--job" ] && run=""
    for file in "$state/runs.json" "$state"/jobs-*.json; do
      [ -f "$file" ] || continue
      sed 's/"status":"completed","conclusion":"failure"/"status":"in_progress","conclusion":""/g' "$file" > "$file.tmp" && mv "$file.tmp" "$file"
    done ;;
  "run cancel")
    for file in "$state/runs.json" "$state"/jobs-*.json; do
      [ -f "$file" ] || continue
      sed 's/"status":"in_progress","conclusion":""/"status":"completed","conclusion":"cancelled"/g' "$file" > "$file.tmp" && mv "$file.tmp" "$file"
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

const BROWSER = String.raw`#!/bin/sh
# A stand-in for Chrome. No page is drawn: the "screenshot" is a picture that lies ready next to it.
here="$(cd "$(dirname "$0")" && pwd)"
# One write for each call: the core runs several of these at once, and two writes of one call could be split by another's.
printf '%s\n' "$(echo "$*" | tr '\n' ' ')" >> "$here/browser.log"
# What the core is configured with is not passed on to the programs it starts.
if env | grep -q '^AE_'; then echo "the settings of the core were passed on" >> "$here/browser.log"; fi
file=""; height=""; url=""
for word in "$@"; do
  case "$word" in
    --screenshot=*) file=$(printf '%s' "$word" | sed 's/^--screenshot=//') ;;
    --window-size=*) height=$(printf '%s' "$word" | sed 's/^.*,//') ;;
    http://*) url="$word" ;;
  esac
done
[ -n "$file" ] || { echo "no --screenshot= was given" >&2; exit 2; }
# What a browser does: it asks for the page it was sent to and, once the app's page in it has
# loaded, for the wait that page asks for, saying how high its window is.
curl -s -o /dev/null "$url"
[ -f "$here/state/browser/never-loads" ] || curl -s -o /dev/null "$url/settle?h=$height"
if [ -f "$here/state/browser/fails" ]; then echo "The browser stand-in was told to fail" >&2; exit 1; fi
cp "$here/state/browser/page.png" "$file"
`;

const XCRUN = String.raw`#!/bin/sh
# A stand-in for Xcode's xcrun. No simulator is started: a "simulator" is a line in a file,
# and its "screen" is one of two pictures that lie ready, in turn.
here="$(cd "$(dirname "$0")" && pwd)"
# One write for each call: the core runs several of these at once, and two writes of one call could be split by another's.
printf '%s\n' "$(echo "$*" | tr '\n' ' ')" >> "$here/xcrun.log"
state="$here/state/xcrun"
state_of() { cat "$state/$1.state" 2>/dev/null || echo Shutdown; }

case "$1 $2" in
  "simctl list")
    if [ -f "$state/no-simctl" ]; then
      echo 'xcrun: error: unable to find utility "simctl", not a developer tool or in PATH' >&2; exit 72
    fi
    # devices.txt: one simulator on each line, as "udid|name".
    printf '{"devices":{"com.apple.CoreSimulator.SimRuntime.iOS-27-0":['
    sep=""
    while IFS='|' read -r udid name; do
      [ -n "$udid" ] || continue
      printf '%s{"udid":"%s","name":"%s","state":"%s","isAvailable":true}' "$sep" "$udid" "$name" "$(state_of "$udid")"
      sep=","
    done < "$state/devices.txt"
    printf ']}}\n' ;;
  "simctl bootstatus")
    [ -f "$state/slow" ] && sleep 1
    echo Booted > "$state/$3.state" ;;
  "simctl shutdown") echo Shutdown > "$state/$3.state" ;;
  "simctl openurl")
    [ "$(state_of "$3")" = Booted ] || { echo "Unable to lookup in current state: Shutdown" >&2; exit 149; }
    echo "$4" > "$state/$3.url" ;;
  "simctl io")
    [ "$(state_of "$3")" = Booted ] || { echo "Unable to lookup in current state: Shutdown" >&2; exit 149; }
    count=$(cat "$state/$3.shots" 2>/dev/null || echo 0)
    count=$((count + 1))
    echo "$count" > "$state/$3.shots"
    # Like the real one, it writes the file it is told, even one called "-", and prints no picture.
    if [ $((count % 2)) = 1 ]; then cat "$state/screen-a.png" > "$6"; else cat "$state/screen-b.png" > "$6"; fi
    echo "Wrote screenshot to: $6" >&2 ;;
  "simctl ui") exit 0 ;;
  "open -a") exit 0 ;;
  *) echo "unexpected call: $*" >&2; exit 2 ;;
esac
`;

const IDB = String.raw`#!/bin/sh
# A stand-in for idb, which taps and types on a simulator. It takes every touch and writes it down.
here="$(cd "$(dirname "$0")" && pwd)"
printf '%s
' "$(echo "$*" | tr '
' ' ')" >> "$here/idb.log"
case "$1" in
  describe)
    # The "screens" of the xcrun stand-in are 60 by 130 pixels: three pixels to a point.
    echo '{"udid":"x","screen_dimensions":{"width":60,"height":130,"density":3.0,"width_points":20,"height_points":43.3333}}' ;;
  ui) exit 0 ;;
  *) echo "unexpected call: $*" >&2; exit 2 ;;
esac
`;

const EMULATOR = String.raw`#!/bin/sh
# A stand-in for Android's emulator. It lists the names a test wrote, and "starts" one by adding a
# line to what the adb stand-in lists: no window, no Android.
here="$(cd "$(dirname "$0")" && pwd)"
printf '%s
' "$(echo "$*" | tr '
' ' ')" >> "$here/emulator.log"
adb="$here/state/adb"
case "$1" in
  -list-avds)
    # The real one prints a line of its own news first.
    echo "INFO    | Storing crashdata in: /tmp/android/emu-crash.db, detection is enabled for process: 4242"
    cat "$here/state/emulator/avds.txt" 2>/dev/null; exit 0 ;;
  -avd)
    echo "emulator-5556          device product:sdk_gphone64 model:sdk_gphone64 transport_id:7" >> "$adb/devices.txt"
    echo "$2" > "$adb/emulator-5556.name" ;;
  *) echo "unexpected call: $*" >&2; exit 2 ;;
esac
`;

const ADB = String.raw`#!/bin/sh
# A stand-in for Android's adb. No emulator is there: an "emulator" is a line in a file,
# and its "screen" is a picture that lies ready.
here="$(cd "$(dirname "$0")" && pwd)"
# One write for each call: the core runs several of these at once, and two writes of one call could be split by another's.
printf '%s\n' "$(echo "$*" | tr '\n' ' ')" >> "$here/adb.log"
state="$here/state/adb"
serial=""
if [ "$1" = "-s" ]; then serial="$2"; shift 2; fi

case "$1 $2" in
  "devices -l")
    echo "List of devices attached"
    # devices.txt: the lines as adb prints them.
    [ -f "$state/devices.txt" ] && cat "$state/devices.txt"
    echo ;;
  "emu avd") cat "$state/$serial.name" 2>/dev/null || exit 1; echo OK ;;
  "shell getprop") if [ "$3" = sys.boot_completed ]; then echo 1; else echo 15; fi ;;
  "shell input") exit 0 ;;
  "shell cmd") exit 0 ;;
  "emu kill")
    grep -v "^$serial " "$state/devices.txt" > "$state/devices.tmp"
    mv "$state/devices.tmp" "$state/devices.txt"
    echo "OK: killing emulator, bye bye" ;;
  "reverse --list") cat "$state/$serial.reverse" 2>/dev/null; exit 0 ;;
  "reverse --remove")
    grep -v " $3 " "$state/$serial.reverse" > "$state/$serial.reverse.tmp"
    mv "$state/$serial.reverse.tmp" "$state/$serial.reverse" ;;
  "reverse tcp:"*) echo "UsbFfs $2 $3" >> "$state/$serial.reverse" ;;
  "shell am")
    # The address comes last, still in the quotes the shell of the device would take off.
    for word in "$@"; do last="$word"; done
    echo "$last" > "$state/$serial.url"
    echo "Starting: Intent { act=android.intent.action.VIEW }" ;;
  "exec-out screencap") cat "$state/screen.png" ;;
  *) echo "unexpected call: $*" >&2; exit 2 ;;
esac
`;

const TERRAFORM = String.raw`#!/bin/sh
# A stand-in for Terraform. It reaches no cloud and changes nothing: it says what a real one might.
# A folder that holds broken.tf does not validate.
here="$(cd "$(dirname "$0")" && pwd)"
printf '%s\n' "$*" >> "$here/terraform.log"
case "$1" in
  validate)
    if [ -f broken.tf ]; then
      echo "Error: Unsupported block type"
      echo "  on broken.tf line 1: Blocks of type \"resorce\" are not expected here."
      exit 1
    fi
    echo "Success! The configuration is valid." ;;
  plan)
    echo "  # aws_s3_bucket.exports will be destroyed"
    echo "Plan: 1 to add, 1 to change, 1 to destroy."
    [ -f slow.tf ] && sleep 30
    exit 0 ;;
  *) echo "unexpected call: $*" >&2; exit 2 ;;
esac
`;

const HELM = String.raw`#!/bin/sh
# A stand-in for Helm. It installs nothing: it prints what the chart of its folder "renders" to.
here="$(cd "$(dirname "$0")" && pwd)"
printf '%s\n' "$*" >> "$here/helm.log"
case "$1" in
  template)
    echo "---"
    echo "# Source: $(basename "$PWD")/templates/web.yaml"
    echo "kind: Deployment" ;;
  *) echo "unexpected call: $*" >&2; exit 2 ;;
esac
`;

export const STAND_INS = ['gh', 'glab', 'docker'] as const;
/** The stand-ins the core is pointed at one by one, instead of finding them on its PATH. */
export const NAMED_STAND_INS = ['browser', 'xcrun', 'adb', 'emulator', 'idb', 'terraform', 'helm'] as const;
export type StandIn = (typeof STAND_INS)[number] | (typeof NAMED_STAND_INS)[number];

/** A PNG of one colour, made here so that no picture file has to be kept with the tests. */
export function png(width: number, height: number, [red, green, blue]: [number, number, number]): Buffer {
  const table = Array.from({ length: 256 }, (_, n) => {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    return c >>> 0;
  });
  const check = (bytes: Buffer) => {
    let c = 0xffffffff;
    for (const byte of bytes) c = table[(c ^ byte) & 0xff]! ^ (c >>> 8);
    return (c ^ 0xffffffff) >>> 0;
  };
  const part = (kind: string, body: Buffer) => {
    const named = Buffer.concat([Buffer.from(kind, 'latin1'), body]);
    const [length, sum] = [Buffer.alloc(4), Buffer.alloc(4)];
    length.writeUInt32BE(body.length);
    sum.writeUInt32BE(check(named));
    return Buffer.concat([length, named, sum]);
  };
  const head = Buffer.alloc(13);
  head.writeUInt32BE(width, 0);
  head.writeUInt32BE(height, 4);
  // Eight bits for each of red, green and blue.
  head.set([8, 2, 0, 0, 0], 8);
  // Each row: a zero that says "as written", then its pixels.
  const row = Buffer.concat([Buffer.from([0]), Buffer.alloc(width * 3, Buffer.from([red, green, blue]))]);
  const rows = Buffer.concat(Array.from({ length: height }, () => row));
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    part('IHDR', head),
    part('IDAT', deflateSync(rows)),
    part('IEND', Buffer.alloc(0)),
  ]);
}

/** A simulator as the tests set it up. */
export type Simulator = { udid: string; name: string; running?: boolean };

/** Says which simulators the `xcrun` stand-in lists, and which of them run. */
export function setSimulators(dir: string, simulators: Simulator[]) {
  const state = path.join(dir, 'state', 'xcrun');
  writeFileSync(path.join(state, 'devices.txt'), simulators.map((one) => `${one.udid}|${one.name}\n`).join(''));
  for (const one of simulators) {
    writeFileSync(path.join(state, `${one.udid}.state`), one.running ? 'Booted\n' : 'Shutdown\n');
  }
}

/** Whether a simulator of the `xcrun` stand-in "runs". */
export function simulatorRuns(dir: string, udid: string): boolean {
  return readFileSync(path.join(dir, 'state', 'xcrun', `${udid}.state`), 'utf8').trim() === 'Booted';
}

/** An emulator as the tests set it up. Its serial is what adb calls it: `emulator-5554`. */
export type Emulator = { serial: string; name: string };

/** Says which devices the `adb` stand-in lists: the emulators, and a phone on a cable, which is not one. */
export function setEmulators(dir: string, emulators: Emulator[]) {
  const state = path.join(dir, 'state', 'adb');
  const lines = emulators.map(
    (one, index) => `${one.serial}          device product:sdk_gphone64 model:sdk_gphone64 transport_id:${index + 1}\n`,
  );
  lines.push('1eaed69e               device usb:1-1 product:phone model:A_phone_on_a_cable transport_id:9\n');
  writeFileSync(path.join(state, 'devices.txt'), lines.join(''));
  for (const one of emulators) writeFileSync(path.join(state, `${one.serial}.name`), `${one.name}\n`);
}

/** Says which emulators the Device Manager "knows": the `emulator` stand-in lists them, running or not. */
export function setAvds(dir: string, names: string[]) {
  writeFileSync(path.join(dir, 'state', 'emulator', 'avds.txt'), names.map((name) => `${name}\n`).join(''));
}

/** What the stand-in of a device tool keeps in one of its files; empty when the file is not there. */
export function standInNote(dir: string, tool: 'xcrun' | 'adb', file: string): string {
  const target = path.join(dir, 'state', tool, file);
  return existsSync(target) ? readFileSync(target, 'utf8').trim() : '';
}

/** Writes the stand-ins into `dir` and makes them runnable. */
export function writeStandIns(dir: string) {
  for (const name of ['gh', 'docker', 'browser', 'xcrun', 'adb', 'emulator']) {
    mkdirSync(path.join(dir, 'state', name), { recursive: true });
  }
  const scripts: Record<StandIn, string> = {
    gh: GH,
    glab: GLAB,
    docker: DOCKER,
    browser: BROWSER,
    xcrun: XCRUN,
    adb: ADB,
    emulator: EMULATOR,
    idb: IDB,
    terraform: TERRAFORM,
    helm: HELM,
  };
  for (const name of [...STAND_INS, ...NAMED_STAND_INS]) {
    const file = path.join(dir, name);
    writeFileSync(file, scripts[name]);
    chmodSync(file, 0o755);
  }
  writeFileSync(path.join(dir, 'state', 'gh', 'template.json'), PULL_READY);
  // The pictures the stand-ins hand out: a "screenshot" of a page, and the "screens" of the devices.
  // No simulator and no emulator to begin with: a test says which there are.
  writeFileSync(path.join(dir, 'state', 'browser', 'page.png'), png(16, 12, [245, 181, 68]));
  writeFileSync(path.join(dir, 'state', 'xcrun', 'screen-a.png'), png(60, 130, [220, 60, 60]));
  writeFileSync(path.join(dir, 'state', 'xcrun', 'screen-b.png'), png(60, 130, [60, 60, 220]));
  writeFileSync(path.join(dir, 'state', 'adb', 'screen.png'), png(54, 120, [60, 160, 90]));
  setSimulators(dir, []);
  setAvds(dir, []);
  setEmulators(dir, []);
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
