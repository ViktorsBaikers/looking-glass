#!/usr/bin/env bash
set -euo pipefail

INSTALL_PATH=${LG_AGENT_INSTALL_PATH:-/usr/local/bin/lg-agent}
DRY_RUN=${LG_INSTALL_DRY_RUN:-0}
if [ -n "${LG_INSTALL_DRY_RUN_STATE_DIR:-}" ]; then
  if [ "$DRY_RUN" != "1" ]; then
    echo "LG_INSTALL_DRY_RUN_STATE_DIR is allowed only with LG_INSTALL_DRY_RUN=1" >&2
    exit 1
  fi
  STATE_DIR=$LG_INSTALL_DRY_RUN_STATE_DIR
else
  STATE_DIR=${LG_AGENT_STATE_DIR:-/var/lib/lookingglass-agent}
fi
SERVICE_FILE=${LG_AGENT_SERVICE_FILE:-/etc/systemd/system/lookingglass-agent.service}
CREDENTIAL_PATH=${LG_AGENT_CREDENTIAL:-$STATE_DIR/agent-credential.json}
SERVICE_USER=${LG_AGENT_USER:-lookingglass-agent}
# The service unit's PATH. Its FIRST entry is the agent-scoped directory where the
# restricted BGP wrapper lands and which the agent's scoped probe resolves BGP through.
# The remaining entries are the systemd default binary dirs (incl. /usr/local/{s,}bin),
# all root-owned. One definition, shared by the unit, the raw-socket grant resolution,
# and the BGP wrapper placement — so a grant target can never diverge from the binary
# the service actually runs.
SERVICE_PATH=${LG_AGENT_SERVICE_PATH:-/usr/local/lib/lookingglass-agent/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin}
BGP_WRAPPER_DIR=${SERVICE_PATH%%:*}
BGP_WRAPPER=${LG_AGENT_BGP_WRAPPER:-}
BGP_DAEMON=${LG_AGENT_BGP_DAEMON:-}

need_env() {
  local key=$1
  if [ -z "${!key:-}" ]; then
    echo "missing required environment variable: $key" >&2
    exit 1
  fi
}

log() {
  printf '%s\n' "$*"
}

run() {
  if [ "$DRY_RUN" = "1" ]; then
    printf '+'
    printf ' %q' "$@"
    printf '\n'
  else
    "$@"
  fi
}

download_agent() {
  local url=$1
  local dest=$2
  case "$url" in
    https://*)
      if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$url" -o "$dest"
      elif command -v wget >/dev/null 2>&1; then
        wget -qO "$dest" "$url"
      else
        echo "curl or wget is required to fetch the agent binary" >&2
        exit 1
      fi
      ;;
    file://*)
      if [ "$DRY_RUN" != "1" ]; then
        echo "file:// agent binaries are allowed only with LG_INSTALL_DRY_RUN=1" >&2
        exit 1
      fi
      cp "${url#file://}" "$dest"
      ;;
    *)
      echo "LG_AGENT_URL must use https:// (file:// is dry-run only)" >&2
      exit 1
      ;;
  esac
}

verify_sha256() {
  local file=$1
  local expected
  local actual
  expected=$(printf '%s' "$LG_AGENT_SHA256" | tr 'A-F' 'a-f')
  if ! printf '%s\n' "$expected" | grep -Eq '^[0-9a-f]{64}$'; then
    echo "LG_AGENT_SHA256 must be a lowercase or uppercase SHA-256 hex digest" >&2
    exit 1
  fi
  actual=$(sha256sum "$file" | awk '{print $1}')
  if [ "$actual" != "$expected" ]; then
    echo "checksum mismatch for fetched agent binary" >&2
    echo "expected: $expected" >&2
    echo "actual:   $actual" >&2
    exit 1
  fi
  log "verified agent binary sha256: $actual"
}

systemd_env() {
  printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g'
}

service_user_can_exec() {
  # Executability from the SERVICE USER's view, not root's. execve runs the tool as
  # `lookingglass-agent` at run time, so a `0750 root:root` binary root can run but the
  # service user cannot must NOT be chosen — the grant would land on a binary the
  # service can never exec. A real install (root, user present) checks via the user;
  # a dry-run/preview falls back to the current user (production takes the runuser path).
  local target=$1
  if [ "$DRY_RUN" != "1" ] && [ "$(id -u)" = "0" ] && getent passwd "$SERVICE_USER" >/dev/null 2>&1; then
    runuser -u "$SERVICE_USER" -- test -x "$target"
  else
    test -x "$target"
  fi
}

resolve_on_service_path() {
  # The first binary of $1 in the service unit's PATH order that the SERVICE USER can
  # execute — the exact binary execve (as that user) will run. The capability grant
  # targets THIS, so the grant and the run cannot diverge (AC38). Mirrors the
  # executable, first-match semantics of shared::template::resolve_on_path.
  local name=$1 dir
  local -a dirs
  IFS=: read -r -a dirs <<<"$SERVICE_PATH"
  for dir in "${dirs[@]}"; do
    if [ -e "$dir/$name" ] && service_user_can_exec "$dir/$name"; then
      printf '%s\n' "$dir/$name"
      return 0
    fi
  done
  return 1
}

ensure_service_user() {
  # A non-root, no-login system account for the agent. --system, no home, nologin
  # shell — the least a long-running service needs. Never added to a daemon group.
  if getent passwd "$SERVICE_USER" >/dev/null 2>&1; then
    log "service user present: $SERVICE_USER"
    return
  fi
  log "creating non-root service user: $SERVICE_USER"
  run useradd --system --no-create-home --shell /usr/sbin/nologin "$SERVICE_USER"
}

grant_raw_socket_capabilities() {
  # Every diagnostic the agent offers needs a raw socket: ping (ICMP), mtr, and
  # traceroute. Grant cap_net_raw ONLY on the exact binary the service resolves and
  # runs for each — never the agent binary, never blanket root. The agent process
  # holds no ambient capability; only the tool it execs carries the file capability.
  # This is explicit so the tools do not silently depend on a distro setuid-root bit
  # (with NoNewPrivileges=false a setuid escalation surface — see README). A tool
  # absent on the service PATH is skipped; that method stays unavailable at run time
  # and fails closed with the existing clear "tool is not available" message (AC41).
  local tool tool_bin
  for tool in ping mtr traceroute; do
    if ! tool_bin=$(resolve_on_service_path "$tool"); then
      log "no executable $tool on the service PATH; the $tool method stays unavailable until it is installed"
      continue
    fi
    log "granting cap_net_raw on the exact $tool the service runs: $tool_bin"
    run setcap cap_net_raw+ep "$tool_bin"
  done
}

configure_bgp_access() {
  # BGP read-only access is scoped, never a broad daemon-group grant (broad group
  # membership is not read-only). Where the operator supplies a restricted read-only
  # wrapper, install it under the exact name the agent probes and runs (birdc/vtysh)
  # at the front of the service PATH, so the probed and executed binary are one file.
  # No wrapper -> BGP stays unavailable and the agent fails closed with its clear error.
  if [ -z "$BGP_WRAPPER" ]; then
    log "BGP access not configured; the agent reports BGP unavailable until a scoped read-only wrapper is installed"
    return
  fi
  local probe_name
  case "$BGP_DAEMON" in
    bird) probe_name=birdc ;;
    frr) probe_name=vtysh ;;
    *)
      echo "LG_AGENT_BGP_WRAPPER requires LG_AGENT_BGP_DAEMON=bird|frr" >&2
      exit 1
      ;;
  esac
  run install -d -m 0755 "$BGP_WRAPPER_DIR"
  log "installing scoped read-only BGP wrapper on the service PATH: $BGP_WRAPPER_DIR/$probe_name"
  run install -o root -g root -m 0755 "$BGP_WRAPPER" "$BGP_WRAPPER_DIR/$probe_name"
}

write_unit() {
  local tmp_unit=$1
  cat >"$tmp_unit" <<EOF
[Unit]
Description=Looking Glass remote agent
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=$SERVICE_USER
Group=$SERVICE_USER
WorkingDirectory=$STATE_DIR
Environment="LG_AGENT_CREDENTIAL=$(systemd_env "$CREDENTIAL_PATH")"
Environment="PATH=$SERVICE_PATH"
Environment="LG_AGENT_BGP_WRAPPER_DIR=$(systemd_env "$BGP_WRAPPER_DIR")"
ExecStart=$INSTALL_PATH
Restart=always
RestartSec=5s
# Least privilege: the agent runs unprivileged and carries NO ambient capability.
# Raw-socket diagnostics (ping/mtr/traceroute) get their capability from the file
# capability on each exact binary the agent execs, so NoNewPrivileges MUST stay off
# (it strips file capabilities on execve) and the bounding set keeps only CAP_NET_RAW
# reachable by the exec'd tool. NoNewPrivileges=false also re-enables setuid/setgid
# on child execs and the bounding set does NOT bound setuid transitions, so the PATH
# is pinned to root-owned dirs and the diagnostic method set is fixed (see README).
NoNewPrivileges=false
CapabilityBoundingSet=CAP_NET_RAW
AmbientCapabilities=
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
ReadWritePaths=$STATE_DIR

[Install]
WantedBy=multi-user.target
EOF
}

store_enrolled_credential() {
  local agent_binary=$1
  log "enrolling agent with one-time token (redacted)"
  if [ "$DRY_RUN" = "1" ]; then
    LG_CENTRAL_URL="$LG_CENTRAL_URL" LG_TUNNEL_URL="$LG_TUNNEL_URL" LG_CENTRAL_FP="$LG_CENTRAL_FP" LG_ENROLL_TOKEN="$LG_ENROLL_TOKEN" \
      LG_INSTALL_DRY_RUN="$DRY_RUN" LG_ENROLL_RESPONSE_FILE="${LG_ENROLL_RESPONSE_FILE:-}" \
      "$agent_binary" install-enroll "$CREDENTIAL_PATH"
  else
    LG_CENTRAL_URL="$LG_CENTRAL_URL" LG_TUNNEL_URL="$LG_TUNNEL_URL" LG_CENTRAL_FP="$LG_CENTRAL_FP" LG_ENROLL_TOKEN="$LG_ENROLL_TOKEN" \
      LG_INSTALL_DRY_RUN="$DRY_RUN" \
      "$agent_binary" install-enroll "$CREDENTIAL_PATH"
  fi
  if [ ! -s "$CREDENTIAL_PATH" ]; then
    echo "agent credential was not stored" >&2
    exit 1
  fi
  local mode
  mode=$(stat -c '%a' "$CREDENTIAL_PATH")
  if [ "$mode" != "600" ]; then
    echo "agent credential must be owner-only (0600), got $mode" >&2
    exit 1
  fi
  log "credential stored owner-only: $CREDENTIAL_PATH"
  log "credential ready before service enable"
}

need_env LG_AGENT_URL
need_env LG_AGENT_SHA256
need_env LG_CENTRAL_URL
need_env LG_TUNNEL_URL
need_env LG_CENTRAL_FP
need_env LG_ENROLL_TOKEN

workdir=$(mktemp -d)
trap 'rm -rf "$workdir"' EXIT
download="$workdir/lg-agent"
download_agent "$LG_AGENT_URL" "$download"
verify_sha256 "$download"
chmod +x "$download"

if [ "$DRY_RUN" != "1" ] && [ "$(id -u)" != "0" ]; then
  echo "install must run as root so it can install the binary and systemd unit" >&2
  exit 1
fi

ensure_service_user
run install -d -m 0750 -o "$SERVICE_USER" -g "$SERVICE_USER" "$STATE_DIR"
log "installing agent binary: $INSTALL_PATH"
run install -o root -g root -m 0755 "$download" "$INSTALL_PATH"
store_enrolled_credential "$download"
run chown "$SERVICE_USER:$SERVICE_USER" "$CREDENTIAL_PATH"
grant_raw_socket_capabilities
configure_bgp_access

unit="$workdir/lookingglass-agent.service"
write_unit "$unit"
log "systemd unit: lookingglass-agent.service"
if [ "$DRY_RUN" = "1" ]; then
  cat "$unit"
else
  install -o root -g root -m 0644 "$unit" "$SERVICE_FILE"
fi
run systemctl daemon-reload
run systemctl enable --now lookingglass-agent.service
