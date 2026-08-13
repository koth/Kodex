#!/usr/bin/env bash
# Build maju-relay-server via GitHub Actions, download the Linux x86_64
# artifact, and deploy it to the relay host over SSH.
#
# Flow:
#   1. trigger the "Build Relay Server" workflow via `gh` (or REST API)
#   2. wait for the run to finish
#   3. download the maju-relay-server-linux-x64 artifact
#   4. scp the tarball to the relay host and unpack under /opt/maju-relay
#
# Requirements:
#   - `gh` CLI authenticated to the repo, OR GITHUB_TOKEN env var with
#     actions:read + contents:read scope (used as fallback).
#   - ssh access to RELAY_HOST (key-based; no password prompts).
#
# Usage:
#   scripts/deploy-relay.sh                       # trigger build, wait, deploy
#   scripts/deploy-relay.sh --download-only       # skip build, deploy latest artifact
#   scripts/deploy-relay.sh user@host             # deploy to a different host
#   REF=master scripts/deploy-relay.sh            # build a specific git ref
#   SKIP_BUILD=1 scripts/deploy-relay.sh          # same as --download-only
#
# --download-only / SKIP_BUILD=1: do not trigger a new run; download the
# artifact from the most recent successful run and deploy that. Useful when
# a build already completed and you just want to (re)deploy it.

set -euo pipefail

# Parse flags before positional args so "user@host" still works after an
# optional --download-only.
SKIP_BUILD="${SKIP_BUILD:-0}"
argv=()
for arg in "$@"; do
  case "$arg" in
    --download-only|--no-build) SKIP_BUILD=1 ;;
    -h|--help)
      sed -n '2,45p' "${BASH_SOURCE[0]}"
      exit 0 ;;
    *) argv+=("$arg") ;;
  esac
done
set -- ${argv[@]+"${argv[@]}"}

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_SLUG="koth/Maju"            # override with REPO_SLUG env if forked
WORKFLOW="Build Relay Server"     # workflow name in relay-server.yml
ARTIFACT_NAME="maju-relay-server-linux-x64"
ARTIFACT_GLOB="maju-relay-server-linux-x64.tar.gz*"

RELAY_HOST="${1:-root@120.48.49.190}"
REF="${REF:-}"
REMOTE_DIR="${REMOTE_DIR:-/opt/maju-relay}"

STAGING_DIR="$(mktemp -d)"
trap 'rm -rf "$STAGING_DIR"' EXIT

err()  { printf '\033[31m%s\033[0m\n' "$*" >&2; }
info() { printf '\033[36m%s\033[0m\n' "$*"; }
ok()   { printf '\033[32m%s\033[0m\n' "$*"; }

# ---------------------------------------------------------------------------
# Step 0: pick a GitHub API client (gh CLI preferred, REST fallback).
# ---------------------------------------------------------------------------

have_gh() { command -v gh >/dev/null 2>&1 && gh auth status >/dev/null 2>&1; }

if have_gh; then
  GH=gh
else
  GH=""
fi

api() {  # REST API helper used by the fallback path
  if [[ -z "${GITHUB_TOKEN:-}" ]]; then
    err "gh CLI not available/authenticated and GITHUB_TOKEN is not set."
    err "Either run 'brew install gh && gh auth login' or export a PAT with"
    err "actions:read + contents:read scope as GITHUB_TOKEN."
    exit 1
  fi
  curl -fsSL \
    -H "Accept: application/vnd.github+json" \
    -H "Authorization: Bearer ${GITHUB_TOKEN}" \
    -H "X-GitHub-Api-Version: 2022-11-28" \
    "$@"
}

require_auth() {
  if [[ -z "$GH" && -z "${GITHUB_TOKEN:-}" ]]; then
    err "gh CLI not available/authenticated and GITHUB_TOKEN is not set."
    err "Either run 'brew install gh && gh auth login' or export a PAT with"
    err "actions:read + contents:read scope as GITHUB_TOKEN."
    exit 1
  fi
}

# Most recent successful run id for the relay workflow. --download-only uses
# this directly; the build path also reuses it after dispatching a new run.
latest_successful_run_id() {
  if [[ -n "$GH" ]]; then
    gh run list --repo "$REPO_SLUG" --workflow "$WORKFLOW" \
      --status success --limit 1 \
      --json databaseId --jq '.[0].databaseId'
  else
    api "https://api.github.com/repos/${REPO_SLUG}/actions/workflows/relay-server.yml/runs?status=success&per_page=1" \
      | python3 -c 'import sys,json
r=json.load(sys.stdin)["workflow_runs"]
print(r[0]["id"] if r else "")'
  fi
}

require_auth

# ---------------------------------------------------------------------------
# Step 1: trigger the workflow and watch it (skipped in download-only mode).
# ---------------------------------------------------------------------------

if [[ "$SKIP_BUILD" == "1" ]]; then
  info "Skip-build mode: looking up the latest successful run of '${WORKFLOW}'."
  RUN_ID="$(latest_successful_run_id)"
  if [[ -z "$RUN_ID" ]]; then
    err "No successful run found for workflow '${WORKFLOW}'."
    err "Trigger a build first without --download-only."
    exit 1
  fi
  ok "Using existing successful run ${RUN_ID}."
else
  info "Triggering workflow '${WORKFLOW}' on ${REPO_SLUG} (ref=${REF:-default branch})"

  if [[ -n "$GH" ]]; then
    # gh workflow run accepts an optional ref flag.
    if [[ -n "$REF" ]]; then
      gh workflow run "$WORKFLOW" --repo "$REPO_SLUG" --ref "$REF" \
        -f ref="$REF"
    else
      gh workflow run "$WORKFLOW" --repo "$REPO_SLUG"
    fi
  else
    ref_for_api="$REF"
    if [[ -z "$ref_for_api" ]]; then
      # default branch
      ref_for_api="$(api "https://api.github.com/repos/${REPO_SLUG}" \
        | python3 -c 'import sys,json;print(json.load(sys.stdin)["default_branch"])')"
    fi
    api -X POST "https://api.github.com/repos/${REPO_SLUG}/actions/workflows/relay-server.yml/dispatches" \
      -d "{\"ref\":\"${ref_for_api}\",\"inputs\":{\"ref\":\"${ref_for_api}\"}}"
  fi

  ok "Workflow dispatched. Waiting for the run to appear..."
  sleep 5

  # Resolve the most recent run id for this workflow (the one we just queued).
  if [[ -n "$GH" ]]; then
    RUN_ID="$(gh run list --repo "$REPO_SLUG" --workflow "$WORKFLOW" --limit 1 \
      --json databaseId,status --jq '.[0].databaseId')"
  else
    RUN_ID="$(api "https://api.github.com/repos/${REPO_SLUG}/actions/workflows/relay-server.yml/runs?per_page=1" \
      | python3 -c 'import sys,json;print(json.load(sys.stdin)["workflow_runs"][0]["id"])')"
  fi

  if [[ -z "$RUN_ID" ]]; then
    err "Could not resolve a workflow run id. Trigger may have failed."
    exit 1
  fi

  info "Watching run ${RUN_ID}..."

  if [[ -n "$GH" ]]; then
    gh run watch "$RUN_ID" --repo "$REPO_SLUG" --exit-status
  else
    # Poll the run status until completion.
    while :; do
      state_json="$(api "https://api.github.com/repos/${REPO_SLUG}/actions/runs/${RUN_ID}")"
      status="$(printf '%s' "$state_json" | python3 -c 'import sys,json;print(json.load(sys.stdin)["status"])')"
      conclusion="$(printf '%s' "$state_json" | python3 -c 'import sys,json;print(json.load(sys.stdin).get("conclusion") or "")')"
      if [[ "$status" == "completed" ]]; then
        if [[ "$conclusion" != "success" ]]; then
          err "Workflow run ${RUN_ID} ended with conclusion=${conclusion}"
          err "Inspect logs: https://github.com/${REPO_SLUG}/actions/runs/${RUN_ID}"
          exit 1
        fi
        break
      fi
      printf '.' ; sleep 10
    done
    printf '\n'
  fi

  ok "Run ${RUN_ID} succeeded."
fi

# ---------------------------------------------------------------------------
# Step 2: download the artifact.
# ---------------------------------------------------------------------------

info "Downloading artifact ${ARTIFACT_NAME}..."

if [[ -n "$GH" ]]; then
  gh run download "$RUN_ID" --repo "$REPO_SLUG" \
    --name "$ARTIFACT_NAME" --dir "$STAGING_DIR"
else
  # REST: list artifacts for the run, find ours, download the zip.
  art_url="$(api "https://api.github.com/repos/${REPO_SLUG}/actions/runs/${RUN_ID}/artifacts" \
    | python3 -c 'import sys,json
a=[x for x in json.load(sys.stdin)["artifacts"] if x["name"]=="'"$ARTIFACT_NAME"'"]
print(a[0]["archive_download_url"] if a else "")')"
  if [[ -z "$art_url" ]]; then
    err "Artifact ${ARTIFACT_NAME} not found in run ${RUN_ID}."
    exit 1
  fi
  api -L -o "$STAGING_DIR/artifact.zip" "$art_url"
  (cd "$STAGING_DIR" && unzip -q artifact.zip && rm artifact.zip)
fi

tarball="$STAGING_DIR/maju-relay-server-linux-x64.tar.gz"
if [[ ! -f "$tarball" ]]; then
  err "Expected tarball not found after download: $tarball"
  ls -l "$STAGING_DIR" >&2
  exit 1
fi

# Verify sha256 if the checksum file was shipped alongside.
if [[ -f "$STAGING_DIR/maju-relay-server-linux-x64.tar.gz.sha256" ]]; then
  (cd "$STAGING_DIR" && shasum -a 256 -c maju-relay-server-linux-x64.tar.gz.sha256)
  ok "Checksum verified."
fi

ok "Artifact downloaded: $(du -h "$tarball" | awk '{print $1}')"

# ---------------------------------------------------------------------------
# Step 3: scp + unpack on the relay host.
# ---------------------------------------------------------------------------

info "Deploying to ${RELAY_HOST}:${REMOTE_DIR}"

ssh "${RELAY_HOST}" "mkdir -p '${REMOTE_DIR}'"
scp "$tarball" "${RELAY_HOST}:/tmp/maju-relay-server-linux-x64.tar.gz"

# Unpack the tarball and (re)start the systemd service if it is installed.
# The binary has no --help/clap surface, so we never exec it directly here —
# running it would bind sockets and block. Lifecycle goes through systemd,
# which is the only thing that injects /etc/maju-relay/env into the process.
ssh "${RELAY_HOST}" "set -euo pipefail
  mkdir -p '${REMOTE_DIR}'
  # Extract to a staging dir then atomically swap into place.
  rm -rf '${REMOTE_DIR}.new'
  mkdir -p '${REMOTE_DIR}.new'
  tar -xzf /tmp/maju-relay-server-linux-x64.tar.gz -C '${REMOTE_DIR}.new'
  mv '${REMOTE_DIR}.new/maju-relay-server' '${REMOTE_DIR}.tmp'
  rm -rf '${REMOTE_DIR}.new'
  if [[ -d '${REMOTE_DIR}' ]]; then
    mv '${REMOTE_DIR}' '${REMOTE_DIR}.old'
  fi
  mv '${REMOTE_DIR}.tmp' '${REMOTE_DIR}'
  rm -rf '${REMOTE_DIR}.old'
  chmod +x '${REMOTE_DIR}/bin/maju-relay-server'
  rm -f /tmp/maju-relay-server-linux-x64.tar.gz
  ls -l '${REMOTE_DIR}/bin/maju-relay-server'
"

ok "Binary deployed to ${RELAY_HOST}:${REMOTE_DIR}/bin/maju-relay-server"

# ---------------------------------------------------------------------------
# Step 4: run DB migrations, restart the systemd service, and smoke-test /health.
# ---------------------------------------------------------------------------

info "Restarting maju-relay.service (if installed)..."

# `systemctl cat` exits non-zero when the unit does not exist, so gate on it
# rather than calling restart unconditionally. Without the service installed
# we leave the binary in place and tell the user how to wire it up.
if ssh "${RELAY_HOST}" "systemctl cat maju-relay.service >/dev/null 2>&1"; then
  ssh "${RELAY_HOST}" "set -euo pipefail
    systemctl daemon-reload
    # SQLite schema migrations are embedded in the binary and applied by
    # Db::open, which normal startup also runs. Execute them explicitly
    # here (with the same env file systemd reads) so a migration failure
    # aborts the deploy before we stop the currently-running service.
    env_file=/etc/maju-relay/env
    db_path='/var/lib/maju-relay/kodex-relay.sqlite'
    if [[ -r \"\$env_file\" ]]; then
      configured=\$(sed -n 's/^RELAY_DB_PATH=//p' \"\$env_file\" | tail -n1)
      if [[ -n \"\$configured\" ]]; then db_path=\"\$configured\"; fi
    fi
    runuser -u maju-relay -- env RELAY_DB_PATH=\"\$db_path\" \
      '${REMOTE_DIR}/bin/maju-relay-server' migrate
    systemctl restart maju-relay.service
    # Wait up to ~10s for the service to report active.
    for _ in \$(seq 1 20); do
      if [ \"\$(systemctl is-active maju-relay.service)\" = active ]; then break; fi
      sleep 0.5
    done
    state=\$(systemctl is-active maju-relay.service)
    if [ \"\$state\" != active ]; then
      echo 'maju-relay failed to become active (\$state); recent logs:' >&2
      journalctl -u maju-relay.service --no-pager -n 30 >&2 || true
      exit 1
    fi
    # Health probe: relay exposes GET /health on RELAY_HEALTH_ADDR (default
    # 127.0.0.1:8788). 200 confirms the binary matches the env/config.
    code=\$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:8788/health || true)
    echo \"health: \$code\"
    if [ \"\$code\" != 200 ]; then
      echo 'health endpoint did not return 200; recent logs:' >&2
      journalctl -u maju-relay.service --no-pager -n 30 >&2 || true
      exit 1
    fi
    systemctl --no-pager status maju-relay.service | head -8
  "
  ok "maju-relay.service restarted and /health returned 200."
else
  err "maju-relay.service not installed on ${RELAY_HOST}."
  err "Install it with: scripts/relay/install-relay-service.sh ${RELAY_HOST}"
  err "Then start with: ssh ${RELAY_HOST} 'systemctl start maju-relay'"
fi

ok "Done. wss://${RELAY_HOST#*@}/ -> nginx -> 127.0.0.1:8787"
