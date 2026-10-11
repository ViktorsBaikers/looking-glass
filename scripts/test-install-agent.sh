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
  # Where the installer staged this binary, and that directory's mode and owner.
  printf '%s %s\n' "$0" "$(stat -c '%a %u' "$(dirname "$0")")" >"$(dirname "$credential_path")/staged-from"
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
  # Build against the workspace's pins. The first resolve keeps every locked version it
  # still needs and drops the rest; any pin it adds is one the workspace never locked.
  cp "$ROOT/Cargo.lock" "$TMP/cmdgen/Cargo.lock"
  cargo metadata --quiet --format-version 1 --manifest-path "$manifest" >/dev/null
  local unpinned
  unpinned=$(comm -23 <(lock_pins "$TMP/cmdgen/Cargo.lock") <(lock_pins "$ROOT/Cargo.lock"))
  if [ -n "$unpinned" ]; then
    echo "install-command proof resolved packages the workspace Cargo.lock does not pin: $unpinned" >&2
    exit 1
  fi
  LG_TEST_AGENT_STATE_DIR="$1" LG_TEST_ENROLL_RESPONSE_FILE="$2" \
    CARGO_TARGET_DIR="$ROOT/target/install-command-proof" \
    cargo run --quiet --locked --manifest-path "$manifest"
}

lock_pins() { # "name version" of every registry package in a Cargo.lock, sorted
  awk '/^name = /{name=$3} /^version = /{version=$3} /^source = /{print name, version}' "$1" | sort
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
  if grep -q "LG_ENROLL_TOKEN=" <<<"$command"; then
    echo "generated paste command put the enrollment token in sudo/env argv" >&2
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
  # `env -i` drops TMPDIR, so the old mktemp staged under /tmp, which hardened hosts
  # mount noexec. The staged binary must sit in a root-owned 0700 directory beside
  # the install target.
  if ! grep -Eq '^/usr/local/bin/\.lg-agent-install\.[^/]+/lg-agent 700 0$' "$state_dir/staged-from"; then
    echo "generated install staged the agent outside a root-owned 0700 dir beside its target: $(cat "$state_dir/staged-from")" >&2
    exit 1
  fi
  if compgen -G '/usr/local/bin/.lg-agent-install.*' >/dev/null; then
    echo "installer left its staging directory behind" >&2
    exit 1
  fi
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
# `enable --now` is a no-op on a running unit, so a reinstall must restart it to load
# the new binary and credential.
grep -q "systemctl restart lookingglass-agent.service" "$good_log"
# The state dir is service-user writable: root must not follow a symlink planted there.
grep -Eq '^\+ chown -h lookingglass-agent:lookingglass-agent .*/agent-credential\.json$' "$good_log"
if grep '^+ chown ' "$good_log" | grep -vq '^+ chown -h '; then
  echo "installer ran a symlink-following chown as root" >&2
  exit 1
fi

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
# NoNewPrivileges must stay off so the ping file capability survives execve. The
# bounding set is exactly CAP_NET_RAW (for the exec'd tool) and CAP_NET_BIND_SERVICE,
# and the only ambient capability is CAP_NET_BIND_SERVICE (for the :443 data plane).
grep -q "NoNewPrivileges=false" "$good_log"
unit_caps=$(grep -E '^(CapabilityBoundingSet|AmbientCapabilities)=' "$good_log")
if [ "$unit_caps" != $'CapabilityBoundingSet=CAP_NET_RAW CAP_NET_BIND_SERVICE\nAmbientCapabilities=CAP_NET_BIND_SERVICE' ]; then
  echo "unexpected unit capability lines: $unit_caps" >&2
  exit 1
fi
# Raw-socket capability is granted on the exact ping the service PATH resolves, never
# blanket root. (mtr/traceroute may be absent on the build host — logged and skipped.)
if ! grep -q "granting cap_net_raw on the exact ping the service runs:" "$good_log"; then
  echo "no cap_net_raw grant on ping: this test needs an executable ping (iputils-ping) on the default service PATH" >&2
  exit 1
fi
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
line_restart=$(grep -n "systemctl restart lookingglass-agent.service" "$good_log" | cut -d: -f1)
if [ "$line_restart" -le "$line_enable" ]; then
  echo "service was restarted before it was enabled" >&2
  exit 1
fi
# setcap fails on a read-only /usr (Fedora CoreOS, Flatcar), so the grant must come
# before enrollment: then that failure stops the install while the token is unused.
line_setcap=$(grep -n '^+ setcap ' "$good_log" | head -n 1 | cut -d: -f1)
line_enroll=$(grep -n "enrolling agent" "$good_log" | cut -d: -f1)
if [ "$line_setcap" -ge "$line_enroll" ]; then
  echo "cap_net_raw was granted after enrollment spent the one-time token" >&2
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
grep -q "systemctl restart lookingglass-agent.service" "$second_log"
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
# The new binary is staged and enrolled first; a failed enrollment leaves the
# installed one as it was.
if grep -Eq '^\+ install .* /usr/local/bin/lg-agent$' "$missing_log"; then
  echo "agent binary was replaced although enrollment failed" >&2
  exit 1
fi

# --- A host without setcap fails before enrollment spends the one-time token ---
# A real (not dry-run) install with a ping to grant and no setcap on PATH. The check runs
# before the root check, so this works as any user; an empty PATH hides the host's setcap.
nocap_svc="$TMP/nocap-svc"
mkdir -p "$nocap_svc" "$TMP/empty-path"
printf '#!/bin/sh\n' >"$nocap_svc/ping"
chmod +x "$nocap_svc/ping"
nocap_log="$TMP/nocap.log"
nocap_fp=$(printf central | sha256sum | awk '{print $1}')
if PATH="$TMP/empty-path" \
  LG_AGENT_URL="https://agent.example/lg-agent" \
  LG_AGENT_SHA256="$SHA" \
  LG_CENTRAL_URL="https://central.example:8443" \
  LG_TUNNEL_URL="https://tunnel.central.example:8443" \
  LG_CENTRAL_FP="$nocap_fp" \
  LG_ENROLL_TOKEN="$TOKEN" \
  LG_AGENT_SERVICE_PATH="$nocap_svc" \
  "$BASH" "$SCRIPT" >"$nocap_log" 2>&1; then
  echo "installer without setcap unexpectedly succeeded" >&2
  exit 1
fi
if ! grep -q "setcap is required .*libcap2-bin.*enrollment token has not been used" "$nocap_log"; then
  echo "installer without setcap did not stop early naming libcap2-bin and the unused token: $(cat "$nocap_log")" >&2
  exit 1
fi
if grep -q "enrolling agent" "$nocap_log"; then
  echo "installer without setcap reached enrollment" >&2
  exit 1
fi

# --- A failing setcap (read-only /usr) stops the install before enrollment ---
# A dry run only prints setcap, so a copy runs it for real against a stub that fails.
failcap_copy="$TMP/failcap-install-agent.sh"
failcap_stubs="$TMP/failcap-stubs"
mkdir -p "$failcap_stubs"
printf '#!/bin/sh\nexit 1\n' >"$failcap_stubs/setcap"
chmod +x "$failcap_stubs/setcap"
sed 's/if ! run setcap /if ! setcap /' "$SCRIPT" >"$failcap_copy"
if ! grep -q 'if ! setcap ' "$failcap_copy"; then
  echo "installer no longer grants cap_net_raw with 'if ! run setcap'" >&2
  exit 1
fi
rm -f "$TMP/state/agent-credential.json"
failcap_log="$TMP/failcap.log"
if PATH="$failcap_stubs:$PATH" SCRIPT="$failcap_copy" LG_AGENT_SERVICE_PATH="$nocap_svc" \
  run_installer "$SHA" >"$failcap_log" 2>&1; then
  echo "installer continued after setcap failed" >&2
  exit 1
fi
if ! grep -q "could not grant cap_net_raw on $nocap_svc/ping.*enrollment token has not been used" "$failcap_log" ||
  grep -q "enrolling agent" "$failcap_log" || [ -e "$TMP/state/agent-credential.json" ]; then
  echo "failed setcap did not stop the install before enrollment with a clear error: $(cat "$failcap_log")" >&2
  exit 1
fi

# --- A host without a running systemd fails before enrollment spends the token ---
# Real (not dry-run) runs with no tool to grant, so setcap is not needed. The copy reads
# the systemd marker from $2, so each case runs the same on a systemd host or in a container.
precheck_copy="$TMP/precheck-install-agent.sh"
precheck_stubs="$TMP/precheck-stubs"
mkdir -p "$precheck_stubs"
printf '#!/bin/sh\n' >"$precheck_stubs/systemctl"
# A non-root id, so a run that passes every precheck stops at the root check.
printf '#!/bin/sh\necho 1000\n' >"$precheck_stubs/id"
chmod +x "$precheck_stubs/systemctl" "$precheck_stubs/id"
precheck_log="$TMP/precheck.log"
run_precheck() { # $1 PATH, $2 systemd marker dir
  sed "s#/run/systemd/system#$2#g" "$SCRIPT" >"$precheck_copy"
  if ! grep -qF "$2" "$precheck_copy"; then
    echo "installer no longer checks /run/systemd/system before enrollment" >&2
    exit 1
  fi
  if PATH="$1" \
    LG_AGENT_URL="https://agent.example/lg-agent" \
    LG_AGENT_SHA256="$SHA" \
    LG_CENTRAL_URL="https://central.example:8443" \
    LG_TUNNEL_URL="https://tunnel.central.example:8443" \
    LG_CENTRAL_FP="$nocap_fp" \
    LG_ENROLL_TOKEN="$TOKEN" \
    LG_AGENT_SERVICE_PATH="$TMP/empty-path" \
    "$BASH" "$precheck_copy" >"$precheck_log" 2>&1; then
    echo "installer precheck run unexpectedly succeeded: $(cat "$precheck_log")" >&2
    exit 1
  fi
  if grep -q -e "setcap is required" -e "enrolling agent" "$precheck_log"; then
    echo "installer with no tool to grant asked for setcap or reached enrollment: $(cat "$precheck_log")" >&2
    exit 1
  fi
}
run_precheck "$TMP/empty-path" "$TMP"
if ! grep -q "systemctl is required.*enrollment token has not been used" "$precheck_log"; then
  echo "installer without systemctl did not stop early naming the unused token: $(cat "$precheck_log")" >&2
  exit 1
fi
run_precheck "$precheck_stubs" "$TMP/no-systemd"
if ! grep -q "systemd is not running as init.*WSL.*LXC.*enrollment token has not been used" "$precheck_log"; then
  echo "installer on a host not booted with systemd did not stop early naming the unused token: $(cat "$precheck_log")" >&2
  exit 1
fi
run_precheck "$precheck_stubs" "$TMP"
if ! grep -q "install must run as root" "$precheck_log"; then
  echo "installer with systemctl and a running systemd did not pass its prechecks: $(cat "$precheck_log")" >&2
  exit 1
fi

# --- The staged binary lives beside its install target, never in $TMPDIR ---
# Enrollment executes the staged binary. /tmp (and so $TMPDIR, which the generated
# command's `env -i` drops anyway) may be mounted noexec on hardened hosts.
prefix="$TMP/prefix"
stage_tmpdir="$TMP/tmpdir"
mkdir -p "$prefix" "$stage_tmpdir"
stage_log="$TMP/stage.log"
TMPDIR="$stage_tmpdir" LG_AGENT_INSTALL_PATH="$prefix/lg-agent" run_installer "$SHA" >"$stage_log" 2>&1
if ! grep -Eq "^$prefix/\.lg-agent-install\.[^/]+/lg-agent 700 " "$TMP/state/staged-from"; then
  echo "agent binary was not staged in a 0700 directory beside its install target: $(cat "$TMP/state/staged-from")" >&2
  exit 1
fi
if [ -n "$(ls -A "$prefix")$(ls -A "$stage_tmpdir")" ]; then
  echo "installer left its staging directory behind after a successful enrollment" >&2
  exit 1
fi
if TMPDIR="$stage_tmpdir" LG_AGENT_INSTALL_PATH="$prefix/lg-agent" LG_FAKE_ENROLL_FAIL=1 \
  run_installer "$SHA" >"$stage_log" 2>&1; then
  echo "installer unexpectedly succeeded although enrollment failed" >&2
  exit 1
fi
if [ -n "$(ls -A "$prefix")$(ls -A "$stage_tmpdir")" ]; then
  echo "installer left its staging directory behind after a failed enrollment" >&2
  exit 1
fi

# --- Slice 12c: raw-socket grants target the exact first-on-PATH tool (cannot diverge) ---
# Two directories on the service PATH each carry ping/mtr/traceroute; the grant must land
# on the FIRST of each, the same file execve (and therefore the agent) runs.
svc_first="$TMP/svc/first"
svc_second="$TMP/svc/second"
mkdir -p "$svc_first" "$svc_second"
for tool in ping mtr mtr-packet traceroute; do
  printf '#!/bin/sh\n' >"$svc_first/$tool"
  printf '#!/bin/sh\n' >"$svc_second/$tool"
  chmod +x "$svc_first/$tool" "$svc_second/$tool"
done
# mtr opens no raw socket itself: it execs mtr-packet from PATH, and a file capability
# does not pass across that exec. The grant must land on mtr-packet — on the real file
# behind a symlink, as distros ship it — and never on the mtr front-end.
mkdir -p "$TMP/svc/real"
mv "$svc_first/mtr-packet" "$TMP/svc/real/mtr-packet"
ln -s "$TMP/svc/real/mtr-packet" "$svc_first/mtr-packet"
icmp_log="$TMP/icmp.log"
LG_AGENT_SERVICE_PATH="$svc_first:$svc_second" run_installer "$SHA" >"$icmp_log" 2>&1
for tool in ping traceroute; do
  grep -qF "granting cap_net_raw on the exact $tool the service runs: $svc_first/$tool" "$icmp_log"
done
grep -qxF "+ setcap cap_net_raw+ep $TMP/svc/real/mtr-packet" "$icmp_log"
if grep -E '^\+ setcap ' "$icmp_log" | grep -Eq '/mtr$|/svc/first/mtr-packet$'; then
  echo "raw-socket grant landed on the mtr front-end or on a symlink, not the real mtr-packet" >&2
  exit 1
fi
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
# agent probes/runs (birdc/vtysh) in the agent's scoped wrapper directory, which the
# agent's ScopedDaemonProbe resolves BGP through — exec runs that same file. The service
# user joins no daemon group: the wrapper alone is setgid to the group that owns the
# daemon's control socket (bird.ctl is bird:bird 0660, FRR's vty sockets frr:frrvty
# 0770), so root:root 0755 would fail with Permission denied.
# Linux ignores setgid on scripts, so the stand-in wrapper is a compiled program.
bgp_wrapper="$TMP/restricted-birdc"
cp /bin/true "$bgp_wrapper"
bgp_svc="$TMP/svcbgp"
mkdir -p "$bgp_svc"
printf '#!/bin/sh\n' >"$bgp_svc/ping"
chmod +x "$bgp_svc/ping"
# A getent that knows only the groups in LG_TEST_GROUPS, whatever this host installed.
bgp_stubs="$TMP/bgp-stubs"
mkdir -p "$bgp_stubs"
cat >"$bgp_stubs/getent" <<'SH'
#!/bin/sh
if [ "$1" = group ]; then
  case " ${LG_TEST_GROUPS:-} " in
    *" $2 "*) echo "$2:x:900:" ;;
    *) exit 2 ;;
  esac
  exit 0
fi
exec /usr/bin/getent "$@"
SH
chmod +x "$bgp_stubs/getent"
for bgp_case in "bird birdc bird vtysh" "frr vtysh frrvty birdc"; do
  read -r bgp_daemon bgp_probe bgp_group bgp_other <<<"$bgp_case"
  bgp_log="$TMP/bgp-$bgp_daemon.log"
  PATH="$bgp_stubs:$PATH" LG_TEST_GROUPS="$bgp_group" LG_AGENT_SERVICE_PATH="$bgp_svc" \
    LG_AGENT_BGP_WRAPPER="$bgp_wrapper" LG_AGENT_BGP_DAEMON="$bgp_daemon" \
    run_installer "$SHA" >"$bgp_log" 2>&1
  grep -qF "installing scoped read-only BGP wrapper on the service PATH: $bgp_svc/$bgp_probe" "$bgp_log"
  if ! grep -qxF "+ install -o root -g $bgp_group -m 2755 $bgp_wrapper $bgp_svc/$bgp_probe" "$bgp_log"; then
    echo "$bgp_daemon wrapper is not installed root:$bgp_group setgid (2755), so it cannot open the daemon socket: $(grep -F "$bgp_svc/$bgp_probe" "$bgp_log")" >&2
    exit 1
  fi
  if grep -Eq "usermod|gpasswd| -aG |adduser .*(bird|frr)|--groups[ =].*(bird|frr)" "$bgp_log"; then
    echo "scoped BGP setup fell back to a broad daemon group" >&2
    exit 1
  fi
  # The agent probes birdc before vtysh, so a stale wrapper of the other daemon (after a
  # BIRD -> FRR switch) would keep answering BGP queries.
  if ! grep -qxF "+ rm -f $bgp_svc/$bgp_other" "$bgp_log"; then
    echo "$bgp_daemon wrapper install left the other daemon's $bgp_other wrapper in place" >&2
    exit 1
  fi
done
# The daemon's group missing (daemon not installed) stops the install before enrollment
# spends the one-time token, naming the group.
bgp_log="$TMP/bgp-nogroup.log"
if PATH="$bgp_stubs:$PATH" LG_TEST_GROUPS="" LG_AGENT_SERVICE_PATH="$bgp_svc" \
  LG_AGENT_BGP_WRAPPER="$bgp_wrapper" LG_AGENT_BGP_DAEMON=frr \
  run_installer "$SHA" >"$bgp_log" 2>&1; then
  echo "installer accepted a BGP wrapper whose daemon group does not exist" >&2
  exit 1
fi
if ! grep -q "group frrvty does not exist.*enrollment token has not been used" "$bgp_log" || grep -q "enrolling agent" "$bgp_log"; then
  echo "missing daemon group did not stop the install before enrollment with a clear error: $(cat "$bgp_log")" >&2
  exit 1
fi
# A wrapper path that is missing, or a directory, stops the install before enrollment
# spends the one-time token, naming the path; nothing is installed.
mkdir -p "$TMP/wrapper-dir"
for bad_wrapper in "$TMP/no-such-wrapper" "$TMP/wrapper-dir"; do
  bgp_log="$TMP/bgp-badwrapper.log"
  if PATH="$bgp_stubs:$PATH" LG_TEST_GROUPS=bird LG_AGENT_SERVICE_PATH="$bgp_svc" \
    LG_AGENT_BGP_WRAPPER="$bad_wrapper" LG_AGENT_BGP_DAEMON=bird \
    run_installer "$SHA" >"$bgp_log" 2>&1; then
    echo "installer accepted a BGP wrapper that is not a regular file: $bad_wrapper" >&2
    exit 1
  fi
  if ! grep -qF "LG_AGENT_BGP_WRAPPER=$bad_wrapper is not" "$bgp_log" || ! grep -q "enrollment token has not been used" "$bgp_log" || grep -q "enrolling agent" "$bgp_log"; then
    echo "bad BGP wrapper $bad_wrapper did not stop the install before enrollment with a clear error: $(cat "$bgp_log")" >&2
    exit 1
  fi
done


# A script wrapper would install setgid, but Linux ignores setgid on scripts, so BGP
# would fail with Permission denied on the socket. Refused before enrollment.
script_wrapper="$TMP/script-birdc"
printf '#!/bin/sh\nexec /usr/sbin/birdc -r "$@"\n' >"$script_wrapper"
chmod +x "$script_wrapper"
bgp_log="$TMP/bgp-script.log"
if PATH="$bgp_stubs:$PATH" LG_TEST_GROUPS=bird LG_AGENT_SERVICE_PATH="$bgp_svc" \
  LG_AGENT_BGP_WRAPPER="$script_wrapper" LG_AGENT_BGP_DAEMON=bird \
  run_installer "$SHA" >"$bgp_log" 2>&1; then
  echo "installer accepted a script as the setgid BGP wrapper" >&2
  exit 1
fi
if ! grep -q "setgid needs a compiled wrapper.*enrollment token has not been used" "$bgp_log" || grep -q "enrolling agent" "$bgp_log"; then
  echo "script BGP wrapper did not stop the install before enrollment with a clear error: $(cat "$bgp_log")" >&2
  exit 1
fi

# The service PATH's first entry is the agent's scoped wrapper dir. A system dir there
# would replace the distro's birdc/vtysh with the wrapper and point the agent's probe at
# the unscoped client; a relative one fails after enrollment. Both refused before it.
for bad_path in /usr/sbin:/usr/bin /bin:/usr/bin relative/bin:/usr/bin; do
  bgp_log="$TMP/bgp-syspath.log"
  if PATH="$bgp_stubs:$PATH" LG_TEST_GROUPS=bird LG_AGENT_SERVICE_PATH="$bad_path" \
    LG_AGENT_BGP_WRAPPER="$bgp_wrapper" LG_AGENT_BGP_DAEMON=bird \
    run_installer "$SHA" >"$bgp_log" 2>&1; then
    echo "installer accepted $bad_path, whose first entry is not a dedicated wrapper dir" >&2
    exit 1
  fi
  if ! grep -q "LG_AGENT_SERVICE_PATH.*enrollment token has not been used" "$bgp_log" || grep -q -e "enrolling agent" -e "^+ install .*/birdc$" "$bgp_log"; then
    echo "service PATH $bad_path did not stop the install before enrollment with a clear error: $(cat "$bgp_log")" >&2
    exit 1
  fi
done
# A birdc/vtysh in the wrapper dir that is not a setgid wrapper is a real client the
# installer must neither replace nor remove.
client_svc="$TMP/client-svc"
mkdir -p "$client_svc"
cp /bin/true "$client_svc/vtysh"
bgp_log="$TMP/bgp-client.log"
if PATH="$bgp_stubs:$PATH" LG_TEST_GROUPS=bird LG_AGENT_SERVICE_PATH="$client_svc" \
  LG_AGENT_BGP_WRAPPER="$bgp_wrapper" LG_AGENT_BGP_DAEMON=bird \
  run_installer "$SHA" >"$bgp_log" 2>&1; then
  echo "installer accepted a wrapper dir holding a real vtysh it would remove" >&2
  exit 1
fi
if ! grep -qF "$client_svc/vtysh is not a setgid BGP wrapper" "$bgp_log" || grep -q -e "enrolling agent" -e "^+ rm " "$bgp_log"; then
  echo "a real vtysh in the wrapper dir did not stop the install before enrollment: $(cat "$bgp_log")" >&2
  exit 1
fi
# A previously installed (setgid) wrapper is replaced; a rerun without a wrapper keeps it
# and says so instead of reporting BGP unavailable.
chmod g+s "$client_svc/vtysh"
bgp_log="$TMP/bgp-rewrap.log"
PATH="$bgp_stubs:$PATH" LG_TEST_GROUPS=bird LG_AGENT_SERVICE_PATH="$client_svc" \
  LG_AGENT_BGP_WRAPPER="$bgp_wrapper" LG_AGENT_BGP_DAEMON=bird \
  run_installer "$SHA" >"$bgp_log" 2>&1
grep -qxF "+ rm -f $client_svc/vtysh" "$bgp_log"
bgp_log="$TMP/bgp-keep.log"
LG_AGENT_SERVICE_PATH="$client_svc" run_installer "$SHA" >"$bgp_log" 2>&1
if ! grep -qF "keeping the BGP wrapper already installed: $client_svc/vtysh" "$bgp_log" || grep -q "BGP access not configured" "$bgp_log"; then
  echo "a rerun without a wrapper misreported the installed wrapper: $(cat "$bgp_log")" >&2
  exit 1
fi
# LG_AGENT_BGP_WRAPPER naming the installed wrapper itself (directly or through a
# symlink) would make `install` copy the file onto itself and fail after enrollment
# spent the token; refused before it, pointing at the rerun that keeps the wrapper.
same_svc="$TMP/same-svc"
mkdir -p "$same_svc"
cp /bin/true "$same_svc/birdc"
chmod 2755 "$same_svc/birdc"
ln -s "$same_svc/birdc" "$TMP/birdc-alias"
for same_wrapper in "$same_svc/birdc" "$TMP/birdc-alias"; do
  bgp_log="$TMP/bgp-same.log"
  if PATH="$bgp_stubs:$PATH" LG_TEST_GROUPS=bird LG_AGENT_SERVICE_PATH="$same_svc" \
    LG_AGENT_BGP_WRAPPER="$same_wrapper" LG_AGENT_BGP_DAEMON=bird \
    run_installer "$SHA" >"$bgp_log" 2>&1; then
    echo "installer accepted the installed wrapper $same_wrapper as its own source" >&2
    exit 1
  fi
  if ! grep -qF "LG_AGENT_BGP_WRAPPER=$same_wrapper is the wrapper already installed" "$bgp_log" || ! grep -q "enrollment token has not been used" "$bgp_log" || grep -q "enrolling agent" "$bgp_log"; then
    echo "installed wrapper $same_wrapper as LG_AGENT_BGP_WRAPPER did not stop the install before enrollment: $(cat "$bgp_log")" >&2
    exit 1
  fi
done
