#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
SCRIPT="$ROOT/scripts/install-agent.sh"

TMP=$(mktemp -d)
PROOF_USER_TO_CLEAN=""
PROOF_SUDOERS_TO_CLEAN=""
PROOF_SUDOERS_CREATED=0
cleanup() {
  local failed=0
  if [ -n "$PROOF_SUDOERS_TO_CLEAN" ] && [ "$PROOF_SUDOERS_CREATED" = "1" ]; then
    rm -f "$PROOF_SUDOERS_TO_CLEAN" || failed=1
  fi
  if [ -n "$PROOF_USER_TO_CLEAN" ] && getent passwd "$PROOF_USER_TO_CLEAN" >/dev/null 2>&1; then
    userdel -r "$PROOF_USER_TO_CLEAN" >/dev/null 2>&1 || userdel "$PROOF_USER_TO_CLEAN" >/dev/null 2>&1 || failed=1
  fi
  rm -rf "$TMP" || failed=1
  return "$failed"
}
on_exit() {
  local rc=$?
  local cleanup_rc=0
  cleanup || cleanup_rc=$?
  if [ "$rc" -ne 0 ]; then
    exit "$rc"
  fi
  exit "$cleanup_rc"
}
trap on_exit EXIT

BIN="$TMP/lg-agent"
cat >"$BIN" <<'SH'
#!/bin/sh
set -eu
if [ "${1:-}" = "store-enrollment" ]; then
  echo "old response-file install path must not be used" >&2
  exit 2
fi
if [ "${1:-}" = "install-enroll" ]; then
  if [ "${LG_FAKE_ENROLL_FAIL:-0}" = "1" ]; then
    exit 42
  fi
  if [ -n "${LG_ENROLL_RESPONSE_FILE:-}" ] && [ "${LG_INSTALL_DRY_RUN:-0}" != "1" ]; then
    exit 43
  fi
  credential_path=$2
  if [ -n "${LG_ENROLL_RESPONSE_FILE:-}" ]; then
    grep -q '"agent_id":"agent-xyz"' "$LG_ENROLL_RESPONSE_FILE"
  fi
  mkdir -p "$(dirname "$credential_path")"
  printf '{"agent_id":"agent-xyz","credential":"cred-abc123","central_url":"%s","tunnel_url":"%s","fingerprint":"%s"}\n' "$LG_CENTRAL_URL" "$LG_TUNNEL_URL" "$LG_CENTRAL_FP" >"$credential_path"
  chmod 600 "$credential_path"
fi
SH
chmod +x "$BIN"
SHA=$(sha256sum "$BIN" | awk '{print $1}')
TOKEN="token-123"
RESPONSE="$TMP/enroll-response.json"
cat >"$RESPONSE" <<'JSON'
{"protocol_version":1,"agent_id":"agent-xyz","credential":"cred-abc123"}
JSON

generated_install_command() {
  local manifest="$TMP/cmdgen/Cargo.toml"
  mkdir -p "$TMP/cmdgen/src"
  cat >"$manifest" <<EOF
[package]
name = "install-command-proof"
version = "0.0.0"
edition = "2021"

[dependencies]
shared = { path = "$ROOT/crates/shared" }
EOF
  cat >"$TMP/cmdgen/src/main.rs" <<EOF
use shared::protocol::{fingerprint, EnrollmentParams};

fn main() {
    let state_dir = std::env::var("LG_TEST_AGENT_STATE_DIR").expect("LG_TEST_AGENT_STATE_DIR");
    let response = std::env::var("LG_TEST_ENROLL_RESPONSE_FILE").expect("LG_TEST_ENROLL_RESPONSE_FILE");
    let params = EnrollmentParams {
        central_url: "https://central.example:8443".to_string(),
        tunnel_url: "https://tunnel.central.example:8443".to_string(),
        fingerprint: fingerprint(b"central"),
        token: "$TOKEN".to_string(),
        agent_url: "file://$BIN".to_string(),
        agent_sha256: "$SHA".to_string(),
        install_script_url: "file://$SCRIPT".to_string(),
        install_script_sha256: "$(sha256sum "$SCRIPT" | awk '{print $1}')".to_string(),
    };
    let production = params.install_command();
    assert!(production.contains("PATH='/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin'"));
    assert!(!production.contains("LG_INSTALL_DRY_RUN"));
    assert!(!production.contains("LG_AGENT_STATE_DIR"));
    assert!(!production.contains("LG_INSTALL_DRY_RUN_STATE_DIR"));
    assert!(!production.contains("LG_ENROLL_RESPONSE_FILE"));
    println!("{}", params.install_command_for_test(&[
        ("LG_INSTALL_DRY_RUN", "1"),
        ("LG_INSTALL_DRY_RUN_STATE_DIR", &state_dir),
        ("LG_ENROLL_RESPONSE_FILE", &response),
    ]));
}
EOF
  LG_TEST_AGENT_STATE_DIR="$1" LG_TEST_ENROLL_RESPONSE_FILE="$2" \
    CARGO_TARGET_DIR="$ROOT/target/install-command-proof" \
    cargo run --quiet --manifest-path "$manifest"
}

run_installer() {
  LG_INSTALL_DRY_RUN=1 \
    LG_AGENT_URL="file://$BIN" \
    LG_AGENT_SHA256="$1" \
    LG_CENTRAL_URL="https://central.example:8443" \
    LG_TUNNEL_URL="https://tunnel.central.example:8443" \
    LG_CENTRAL_FP="$(printf central | sha256sum | awk '{print $1}')" \
    LG_ENROLL_TOKEN="$TOKEN" \
    LG_ENROLL_RESPONSE_FILE="$RESPONSE" \
    LG_INSTALL_DRY_RUN_STATE_DIR="$TMP/state" \
    LG_FAKE_ENROLL_FAIL="${LG_FAKE_ENROLL_FAIL:-0}" \
    LG_AGENT_SERVICE_PATH="${LG_AGENT_SERVICE_PATH:-}" \
    LG_AGENT_BGP_WRAPPER="${LG_AGENT_BGP_WRAPPER:-}" \
    LG_AGENT_BGP_DAEMON="${LG_AGENT_BGP_DAEMON:-}" \
    bash "$SCRIPT"
}

run_generated_paste_proof() {
  local host="$TMP/fresh-host"
  local state_dir="$host/state"
  local command
  local started_at
  local finished_at
  local proof_user="lgproof$$"
  local sudoers_file
  mkdir -p "$state_dir"

  command=$(generated_install_command "$state_dir" "$RESPONSE")
  if ! printf '%s\n' "$command" | grep -q "PATH='/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin'"; then
    echo "generated proof command did not preserve the production root PATH" >&2
    exit 1
  fi
  if printf '%s\n' "$command" | grep -q "$host/bin"; then
    echo "generated proof command used a test-only PATH override" >&2
    exit 1
  fi

  started_at=$(date +%s)
  if [ "$(id -u)" = "0" ]; then
    if ! command -v useradd >/dev/null 2>&1 || ! command -v userdel >/dev/null 2>&1 || ! command -v runuser >/dev/null 2>&1 || ! command -v sudo >/dev/null 2>&1 || [ ! -d /etc/sudoers.d ]; then
      echo "real non-root sudo-boundary proof requires useradd, userdel, runuser, sudo, and /etc/sudoers.d" >&2
      exit 1
    fi
    useradd --system --create-home --shell /bin/bash "$proof_user"
    PROOF_USER_TO_CLEAN="$proof_user"
    sudoers_file=$(mktemp "/etc/sudoers.d/${proof_user}_XXXXXX")
    PROOF_SUDOERS_TO_CLEAN="$sudoers_file"
    PROOF_SUDOERS_CREATED=1
    printf '%s ALL=(root) NOPASSWD: ALL\n' "$proof_user" >"$sudoers_file"
    chmod 0440 "$sudoers_file"
    runuser -u "$proof_user" -- bash -c "$command" >"$host/install.log" 2>&1
    rm -f "$sudoers_file"
    PROOF_SUDOERS_TO_CLEAN=""
    PROOF_SUDOERS_CREATED=0
    if userdel -r "$proof_user" >/dev/null 2>&1 || userdel "$proof_user" >/dev/null 2>&1; then
      PROOF_USER_TO_CLEAN=""
    else
      echo "failed to remove temporary proof user: $proof_user" >&2
      exit 1
    fi
  else
    if ! sudo -n true >/dev/null 2>&1; then
      echo "real non-root sudo-boundary proof requires passwordless sudo in non-root runs" >&2
      exit 1
    fi
    bash -c "$command" >"$host/install.log" 2>&1
  fi
  finished_at=$(date +%s)

  grep -q "sudo env -i" <<<"$command"
  grep -q "LG_INSTALL_DRY_RUN='1'" <<<"$command"
  grep -q "LG_INSTALL_DRY_RUN_STATE_DIR='$state_dir'" <<<"$command"
  grep -q "LG_ENROLL_RESPONSE_FILE='$RESPONSE'" <<<"$command"
  if grep -q "sudo -E" <<<"$command"; then
    echo "generated paste command preserved ambient sudo environment" >&2
    exit 1
  fi
  if grep -Eq "LG_FAKE_ENROLL_FAIL|LG_AGENT_BGP_WRAPPER|LG_AGENT_BGP_DAEMON" <<<"$command"; then
    echo "generated paste command carried ambient test-only installer variables" >&2
    exit 1
  fi
  grep -q "credential ready before service enable" "$host/install.log"
  grep -q "systemctl enable --now lookingglass-agent.service" "$host/install.log"
  test -s "$state_dir/agent-credential.json"
  if grep -q "LG_ENROLL_TOKEN" "$host/install.log" || grep -q "$TOKEN" "$host/install.log"; then
    echo "enrollment token leaked into paste-proof output" >&2
    exit 1
  fi
  if [ "${LG_INSTALL_PROOF_TRANSCRIPT:-0}" = "1" ]; then
    echo "fresh install proof: clean_state=$host"
    echo "fresh install proof: command_shape=EnrollmentParams::install_command_for_test(no test PATH override)"
    echo "fresh install proof: production_command=EnrollmentParams::install_command(no dry-run env, fixed production PATH)"
    echo "fresh install proof: sudo_boundary=real non-root sudo escalation observed"
    echo "fresh install proof: credential=$state_dir/agent-credential.json"
    echo "fresh install proof: service_start=systemctl enable --now lookingglass-agent.service"
    echo "fresh install proof: online_state=not_observed; AC7 online-state carried to Slice 17/seal"
    echo "fresh install proof: elapsed_seconds=$((finished_at - started_at))"
  fi
}

bad_log="$TMP/bad.log"
if run_installer 0000000000000000000000000000000000000000000000000000000000000000 >"$bad_log" 2>&1; then
  echo "checksum mismatch unexpectedly succeeded" >&2
  exit 1
fi
grep -q "checksum mismatch" "$bad_log"
if grep -q "installing agent binary" "$bad_log"; then
  echo "installer continued after checksum mismatch" >&2
  exit 1
fi

good_log="$TMP/good.log"
run_installer "$SHA" >"$good_log" 2>&1
grep -q "verified agent binary sha256" "$good_log"
grep -q 'Environment="LG_AGENT_CREDENTIAL=/tmp/' "$good_log"
grep -q 'Environment="PATH=/usr/local/lib/lookingglass-agent/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"' "$good_log"
grep -q "ExecStart=/usr/local/bin/lg-agent" "$good_log"
grep -q "systemd unit: lookingglass-agent.service" "$good_log"
grep -q "credential stored owner-only" "$good_log"
grep -q "credential ready before service enable" "$good_log"
grep -q "systemctl enable --now lookingglass-agent.service" "$good_log"

# --- Slice 12c: least-privilege runtime + scoped BGP contract (AC38) ---
# The service runs as a non-root user the installer creates, never as root.
grep -q "User=lookingglass-agent" "$good_log"
grep -q "Group=lookingglass-agent" "$good_log"
grep -Eq "creating non-root service user: lookingglass-agent|service user present: lookingglass-agent" "$good_log"
# The agent process carries NO ambient raw-socket capability, and no capability is
# ever granted to the agent binary itself — only the ping it execs is capable.
if grep -qi "AmbientCapabilities=CAP_NET_RAW" "$good_log"; then
  echo "agent service was granted an ambient raw-socket capability" >&2
  exit 1
fi
if grep "setcap" "$good_log" | grep -qF "/usr/local/bin/lg-agent"; then
  echo "a capability was granted to the agent binary itself" >&2
  exit 1
fi
# NoNewPrivileges must stay off so the ping file capability survives execve, and the
# bounding set keeps only CAP_NET_RAW available to the exec'd tool.
grep -q "NoNewPrivileges=false" "$good_log"
grep -q "CapabilityBoundingSet=CAP_NET_RAW" "$good_log"
# Raw-socket capability is granted on the exact ping the service PATH resolves, never
# blanket root. (mtr/traceroute may be absent on the build host — logged and skipped.)
grep -q "granting cap_net_raw on the exact ping the service runs:" "$good_log"
# The agent probes BGP ONLY through its scoped wrapper directory (fails closed off it).
grep -q 'Environment="LG_AGENT_BGP_WRAPPER_DIR=/usr/local/lib/lookingglass-agent/bin"' "$good_log"
# The installer never joins the agent to a broad bird/frr daemon group.
if grep -Eq "usermod|gpasswd| -aG |adduser .*(bird|frr)|--groups[ =].*(bird|frr)" "$good_log"; then
  echo "installer added the agent to a broad daemon group" >&2
  exit 1
fi
# With no wrapper configured, BGP stays unavailable (the agent fails closed).
grep -q "BGP access not configured" "$good_log"
if grep -q "LG_ENROLL_TOKEN" "$good_log" || grep -q "$TOKEN" "$good_log"; then
  echo "enrollment token leaked into dry-run output" >&2
  exit 1
fi
line_credential=$(grep -n "credential ready before service enable" "$good_log" | cut -d: -f1)
line_enable=$(grep -n "systemctl enable --now lookingglass-agent.service" "$good_log" | cut -d: -f1)
if [ "$line_credential" -ge "$line_enable" ]; then
  echo "service was enabled before the credential was ready" >&2
  exit 1
fi
if [ ! -f "$TMP/state/agent-credential.json" ]; then
  echo "installer did not store the issued credential" >&2
  exit 1
fi
mode=$(stat -c '%a' "$TMP/state/agent-credential.json")
if [ "$mode" != "600" ]; then
  echo "credential mode must be owner-only 600, got $mode" >&2
  exit 1
fi
if grep -q "LG_ENROLL_TOKEN" "$TMP/state/agent-credential.json" || grep -q "$TOKEN" "$TMP/state/agent-credential.json"; then
  echo "enrollment token leaked into the stored credential" >&2
  exit 1
fi

run_generated_paste_proof

second_log="$TMP/second.log"
run_installer "$SHA" >"$second_log" 2>&1
grep -q "verified agent binary sha256" "$second_log"
if grep -q "LG_ENROLL_TOKEN" "$second_log" || grep -q "$TOKEN" "$second_log"; then
  echo "enrollment token leaked into second dry-run output" >&2
  exit 1
fi

missing_log="$TMP/missing.log"
if LG_FAKE_ENROLL_FAIL=1 run_installer "$SHA" >"$missing_log" 2>&1; then
  echo "installer unexpectedly enabled service without a credential" >&2
  exit 1
fi
if grep -q "systemctl enable --now lookingglass-agent.service" "$missing_log"; then
  echo "service enable ran after enrollment failed" >&2
  exit 1
fi

# --- Slice 12c: raw-socket grants target the exact first-on-PATH tool (cannot diverge) ---
# Two directories on the service PATH each carry ping/mtr/traceroute; the grant must land
# on the FIRST of each, the same file execve (and therefore the agent) runs.
svc_first="$TMP/svc/first"
svc_second="$TMP/svc/second"
mkdir -p "$svc_first" "$svc_second"
for tool in ping mtr traceroute; do
  printf '#!/bin/sh\n' >"$svc_first/$tool"
  printf '#!/bin/sh\n' >"$svc_second/$tool"
  chmod +x "$svc_first/$tool" "$svc_second/$tool"
done
icmp_log="$TMP/icmp.log"
LG_AGENT_SERVICE_PATH="$svc_first:$svc_second" run_installer "$SHA" >"$icmp_log" 2>&1
for tool in ping mtr traceroute; do
  grep -qF "granting cap_net_raw on the exact $tool the service runs: $svc_first/$tool" "$icmp_log"
done
if grep -qF "$svc_second/ping" "$icmp_log"; then
  echo "raw-socket grant targeted a shadowed tool, not the first on PATH" >&2
  exit 1
fi
# The granted PATH and the unit PATH are one string: the grant cannot diverge from run.
grep -qF "Environment=\"PATH=$svc_first:$svc_second\"" "$icmp_log"

# --- Slice 12c: grant follows executability, not PATH order alone (root vs service user) ---
# A NON-executable `ping` earlier on PATH is one execve skips; the grant must skip it too
# and land on the later executable ping — otherwise grant (binary A) and run (binary B)
# diverge. Models the "root-can-exec / service-user-cannot" shadow with a plain non-exec
# file, resolved as the current (non-root) user in the dry-run harness.
xd_first="$TMP/xdiv/first"
xd_second="$TMP/xdiv/second"
mkdir -p "$xd_first" "$xd_second"
for tool in ping mtr traceroute; do
  printf '#!/bin/sh\n' >"$xd_first/$tool"
  chmod 0644 "$xd_first/$tool" # present but NOT executable — must be skipped
  printf '#!/bin/sh\n' >"$xd_second/$tool"
  chmod +x "$xd_second/$tool"
done
xdiv_log="$TMP/xdiv.log"
LG_AGENT_SERVICE_PATH="$xd_first:$xd_second" run_installer "$SHA" >"$xdiv_log" 2>&1
grep -qF "granting cap_net_raw on the exact ping the service runs: $xd_second/ping" "$xdiv_log"
if grep -qF "cap_net_raw+ep $xd_first/ping" "$xdiv_log"; then
  echo "grant landed on a non-executable ping the service user cannot run" >&2
  exit 1
fi

# --- Slice 12c: BGP access is a scoped wrapper under the probe name, never a group ---
# An operator-supplied restricted read-only wrapper installs under the exact name the
# agent probes/runs (birdc) in the agent's scoped wrapper directory, which the agent's
# ScopedDaemonProbe resolves BGP through — exec runs that same file. No broad group grant.
bgp_wrapper="$TMP/restricted-birdc"
printf '#!/bin/sh\necho scoped-read-only\n' >"$bgp_wrapper"
chmod +x "$bgp_wrapper"
bgp_svc="$TMP/svcbgp"
mkdir -p "$bgp_svc"
printf '#!/bin/sh\n' >"$bgp_svc/ping"
chmod +x "$bgp_svc/ping"
bgp_log="$TMP/bgp.log"
LG_AGENT_SERVICE_PATH="$bgp_svc" LG_AGENT_BGP_WRAPPER="$bgp_wrapper" LG_AGENT_BGP_DAEMON=bird \
  run_installer "$SHA" >"$bgp_log" 2>&1
grep -qF "installing scoped read-only BGP wrapper on the service PATH: $bgp_svc/birdc" "$bgp_log"
grep -qF "install -o root -g root -m 0755 $bgp_wrapper $bgp_svc/birdc" "$bgp_log"
if grep -Eq "usermod|gpasswd| -aG |adduser .*(bird|frr)|--groups[ =].*(bird|frr)" "$bgp_log"; then
  echo "scoped BGP setup fell back to a broad daemon group" >&2
  exit 1
fi
