#!/usr/bin/env bash
#
# Run a local SonarQube analysis of this repository.
#
#   scripts/sonar-local.sh
#
# It starts SonarQube in Docker, scans the code and prints a summary.
# Nothing leaves this machine. It is safe to run again at any time.
# The exit code is 0 when the quality gate passes and 1 when it fails.
#
# Secrets are kept in .sonar/ and are never printed:
#   .sonar/admin-password   password of the "admin" user, for the web page
#   .sonar/token            token the scanner logs in with
#
# To start from scratch:
#   docker rm -f -v ae-sonarqube && rm -rf .sonar .scannerwork

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STATE="$ROOT/.sonar"

CONTAINER="ae-sonarqube"
PORT=9000
SONAR_URL="http://localhost:$PORT"
PROJECT_KEY="agent-editor"

# 26.10 is the last Community Build that ships the Rust analyzer.
# From 26.11 on, Rust needs a separate community plugin.
# The server keeps its data inside the container. A different image
# needs a new container, so remove the old one first.
SERVER_IMAGE="${SONAR_SERVER_IMAGE:-sonarqube:26.10.0.133078-community}"
SCANNER_IMAGE="${SONAR_SCANNER_IMAGE:-sonarsource/sonar-scanner-cli:12.4}"

PASSWORD_FILE="$STATE/admin-password"
TOKEN_FILE="$STATE/token"
TOKEN_NAME="ae-local-scan"

say() { printf '\n==> %s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

# Call the Web API with a login. curl reads the login from stdin,
# so it never shows up in the process list.
#   api LOGIN PATH [curl options]
api() {
  local login=$1 path=$2
  shift 2
  curl -fsS -K - "$@" "$SONAR_URL/$path" <<<"user = \"$login\""
}

# Call the Web API as admin.
admin() { api "admin:$(<"$PASSWORD_FILE")" "$@"; }

# True if the server accepts this login.
login_ok() {
  [ "$(api "$1" api/authentication/validate 2>/dev/null | jq -r '.valid' 2>/dev/null)" = "true" ]
}

# Start the container. Reuse it if it already exists.
start_server() {
  local state
  state="$(docker container inspect -f '{{.State.Status}}' "$CONTAINER" 2>/dev/null || true)"
  case "$state" in
    running)
      echo "Container $CONTAINER is already running."
      ;;
    "")
      echo "Creating container $CONTAINER from $SERVER_IMAGE."
      # The port is open to this machine only.
      docker run -d --name "$CONTAINER" \
        -p "127.0.0.1:$PORT:9000" \
        -e SONAR_ES_BOOTSTRAP_CHECKS_DISABLE=true \
        "$SERVER_IMAGE" >/dev/null
      ;;
    *)
      echo "Starting container $CONTAINER (it was $state)."
      docker start "$CONTAINER" >/dev/null
      ;;
  esac
}

# Wait until the server says UP. Give up after ten minutes.
wait_for_server() {
  local status=""
  for _ in $(seq 1 120); do
    status="$(curl -s --max-time 5 "$SONAR_URL/api/system/status" | jq -r '.status // empty' 2>/dev/null || true)"
    if [ "$status" = "UP" ]; then
      echo "SonarQube is up at $SONAR_URL."
      return 0
    fi
    if [ "$(docker container inspect -f '{{.State.Running}}' "$CONTAINER" 2>/dev/null)" != "true" ]; then
      die "container $CONTAINER stopped. See: docker logs $CONTAINER"
    fi
    sleep 5
  done
  die "SonarQube is not up after ten minutes (last status: ${status:-none}). See: docker logs $CONTAINER"
}

# First run: replace the default admin password and make a scanner token.
# Later runs: check that the saved ones still work.
setup_credentials() {
  mkdir -p "$STATE"
  chmod 700 "$STATE"

  if [ -s "$PASSWORD_FILE" ] && login_ok "admin:$(<"$PASSWORD_FILE")"; then
    echo "The saved admin password works."
  elif login_ok "admin:admin"; then
    # A new server still has the default password.
    # The suffix makes sure all the required kinds of characters are there.
    local new_password
    new_password="$(openssl rand -hex 20)-Aq7"
    (umask 077 && printf '%s\n' "$new_password" >"$PASSWORD_FILE.new")
    curl -fsS -o /dev/null -K - -X POST "$SONAR_URL/api/users/change_password" <<EOF
user = "admin:admin"
data-urlencode = "login=admin"
data-urlencode = "previousPassword=admin"
data-urlencode = "password=$new_password"
EOF
    mv "$PASSWORD_FILE.new" "$PASSWORD_FILE"
    echo "Set a random admin password. It is in .sonar/admin-password."
  else
    die "cannot log in as admin. If the server is new or broken, start from scratch (see the top of this script)."
  fi

  if [ -s "$TOKEN_FILE" ] && login_ok "$(<"$TOKEN_FILE"):"; then
    echo "The saved scanner token works."
  else
    # Drop an old token with the same name, then make a new one.
    admin api/user_tokens/revoke -X POST --data-urlencode "name=$TOKEN_NAME" >/dev/null
    (umask 077 && admin api/user_tokens/generate -X POST \
      --data-urlencode "name=$TOKEN_NAME" \
      --data-urlencode "type=GLOBAL_ANALYSIS_TOKEN" | jq -r '.token // empty' >"$TOKEN_FILE.new")
    [ -s "$TOKEN_FILE.new" ] || die "the server did not return a token"
    mv "$TOKEN_FILE.new" "$TOKEN_FILE"
    echo "Made a scanner token. It is in .sonar/token."
  fi
  chmod 600 "$PASSWORD_FILE" "$TOKEN_FILE"
}

# Make the reports the scanner reads: test coverage and Clippy findings.
make_reports() {
  say "Interface tests with coverage"
  [ -d "$ROOT/ui/node_modules" ] || (cd "$ROOT/ui" && npm ci)
  (cd "$ROOT/ui" && npm run test:coverage)
  [ -s "$ROOT/ui/coverage/lcov.info" ] || die "ui/coverage/lcov.info was not written"

  say "Clippy report for the Rust code"
  # The scanner container has no cargo, so Clippy runs here.
  # It builds in its own folder and never waits for a build in target/.
  export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$STATE/cargo-target}"
  (cd "$ROOT" && cargo clippy --workspace --message-format=json >"$STATE/clippy-report.json")

  # Rust coverage needs cargo-llvm-cov. Without it this step is skipped.
  rm -f "$STATE/rust-lcov.info"
  if cargo llvm-cov --version >/dev/null 2>&1; then
    say "Rust tests with coverage"
    (cd "$ROOT" && cargo llvm-cov --workspace --lcov --output-path "$STATE/rust-lcov.info") ||
      echo "warning: Rust coverage failed. The scan goes on without it."
  else
    echo "No Rust coverage: cargo-llvm-cov is not installed (cargo install cargo-llvm-cov)."
  fi
}

# Run the scanner in Docker and wait for the quality gate.
run_scanner() {
  say "Scanner"
  local report="$ROOT/.scannerwork/report-task.txt"
  mkdir -p "$STATE/scanner-cache" "$ROOT/.scannerwork"
  rm -f "$report"

  local -a extra=()
  [ -s "$STATE/rust-lcov.info" ] && extra+=("-Dsonar.rust.lcov.reportPaths=.sonar/rust-lcov.info")

  # The repository is mounted at the same path as on this machine.
  # Reports with full paths in them stay valid inside the container.
  # The token travels as an environment variable, not as an argument.
  # The scanner keeps its work folder inside the container: the Rust analyzer
  # does not start from a mounted folder. Only its short report comes out.
  local rc=0
  SONAR_TOKEN="$(<"$TOKEN_FILE")" docker run --rm --platform linux/amd64 \
    -e SONAR_HOST_URL="http://host.docker.internal:$PORT" \
    -e SONAR_TOKEN \
    -v "$ROOT:$ROOT" -w "$ROOT" \
    -v "$STATE/scanner-cache:/opt/sonar-scanner/.sonar/cache" \
    "$SCANNER_IMAGE" \
    -Dsonar.scanner.metadataFilePath="$report" \
    -Dsonar.qualitygate.wait=true -Dsonar.qualitygate.timeout=900 \
    ${extra[@]+"${extra[@]}"} || rc=$?

  # The scanner image is built for Intel processors. On an Apple-silicon Mac it
  # needs Docker's emulation, which is not always there. When the container
  # could not even start, run the same scanner on this machine through npm.
  if [ ! -f "$report" ]; then
    echo "The scanner container did not run here (exit code $rc). Trying the scanner from npm instead."
    rc=0
    (cd "$ROOT" && SONAR_TOKEN="$(<"$TOKEN_FILE")" npx --yes @sonar/scan \
      -Dsonar.host.url="http://localhost:$PORT" \
      -Dsonar.scanner.metadataFilePath="$report" \
      -Dsonar.qualitygate.wait=true -Dsonar.qualitygate.timeout=900 \
      ${extra[@]+"${extra[@]}"}) || rc=$?
  fi

  # A failed quality gate also ends the scanner with an error.
  # The report file tells whether the analysis itself reached the server.
  [ -f "$report" ] || die "the scanner failed (exit code $rc) before it sent anything"

  # Make sure the server has finished its part.
  local task_id task_status=""
  task_id="$(sed -n 's/^ceTaskId=//p' "$report")"
  for _ in $(seq 1 120); do
    task_status="$(admin "api/ce/task?id=$task_id" | jq -r '.task.status')"
    case "$task_status" in
      SUCCESS) return 0 ;;
      PENDING | IN_PROGRESS) sleep 5 ;;
      *) break ;;
    esac
  done
  die "the server did not finish the analysis (task $task_id is $task_status)"
}

# Save every open issue to .sonar/issues.json.
# Security hotspots are a separate list. They go to .sonar/hotspots.json.
# This server reports most of them as plain issues, so that list is often empty.
save_findings() {
  local pages="$STATE/pages" page=1 total=0 got=0
  rm -rf "$pages"
  mkdir -p "$pages"

  # The API gives at most 500 issues a page and 10 000 in all.
  while :; do
    admin "api/issues/search?components=$PROJECT_KEY&issueStatuses=OPEN,CONFIRMED&ps=500&p=$page" >"$pages/issues-$page.json"
    total="$(jq '.paging.total' "$pages/issues-$page.json")"
    got=$((got + $(jq '.issues | length' "$pages/issues-$page.json")))
    if [ "$got" -ge "$total" ] || [ "$page" -ge 20 ]; then break; fi
    page=$((page + 1))
  done
  (umask 077 && jq -s '{total: .[0].paging.total, issues: (map(.issues) | add)}' "$pages"/issues-*.json >"$STATE/issues.json")

  rm -rf "$pages"

  (umask 077 && admin "api/hotspots/search?project=$PROJECT_KEY&status=TO_REVIEW&ps=500" |
    jq '{total: .paging.total, hotspots: .hotspots}' >"$STATE/hotspots.json") ||
    echo "warning: could not read the list of security hotspots."
}

# Print the numbers from the server.
print_summary() {
  local gate measures
  gate="$(admin "api/qualitygates/project_status?projectKey=$PROJECT_KEY" | jq -r '.projectStatus.status')"
  measures="$(admin "api/measures/component?component=$PROJECT_KEY&metricKeys=bugs,vulnerabilities,code_smells,security_hotspots,coverage,duplicated_lines_density,duplicated_lines,ncloc")"
  metric() {
    jq -r --arg key "$1" '[.component.measures[] | select(.metric == $key) | .value][0] // "n/a"' <<<"$measures"
  }

  # Rust files count in the coverage only when there is a Rust report.
  local covered="Rust and interface"
  [ -s "$STATE/rust-lcov.info" ] || covered="interface only, no Rust report"

  say "Summary"
  printf '  %-20s %s\n' \
    "Quality gate" "$gate" \
    "Lines of code" "$(metric ncloc)" \
    "Bugs" "$(metric bugs)" \
    "Vulnerabilities" "$(metric vulnerabilities)" \
    "Security hotspots" "$(metric security_hotspots)" \
    "Code smells" "$(metric code_smells)" \
    "Coverage" "$(metric coverage)% ($covered)" \
    "Duplication" "$(metric duplicated_lines_density)% ($(metric duplicated_lines) lines)" \
    "Open issues" "$(jq '.total' "$STATE/issues.json"), saved to .sonar/issues.json" \
    "Dashboard" "$SONAR_URL/dashboard?id=$PROJECT_KEY" \
    "Login" "admin, password in .sonar/admin-password"

  [ "$gate" = "OK" ]
}

for tool in docker curl jq openssl npm cargo; do
  command -v "$tool" >/dev/null 2>&1 || die "$tool is not installed"
done

say "SonarQube server"
start_server
wait_for_server
setup_credentials
make_reports
run_scanner
save_findings
print_summary
