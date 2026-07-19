#!/usr/bin/env sh
set -eu

readme=${1:-README.md}

need() {
  grep -qi -- "$1" "$readme" || {
    echo "README missing: $2" >&2
    exit 1
  }
}

need_before() {
  line=$(grep -n -m 1 "$1" "$readme" | cut -d: -f1 || true)
  marker=$(grep -n -m 1 "$2" "$readme" | cut -d: -f1 || true)
  if [ -z "$line" ] || [ -z "$marker" ] || [ "$line" -ge "$marker" ]; then
    echo "README must name $3 before enrollment generation" >&2
    exit 1
  fi
}

check_shell_blocks() {
  shell_dir=$(mktemp -d)
  trap 'rm -rf "$shell_dir"' EXIT HUP INT TERM
  awk -v dir="$shell_dir" '
    /^```(sh|shell|bash)$/ { block += 1; in_block = 1; next }
    /^```$/ { in_block = 0; next }
    in_block { print > (dir "/" block ".sh") }
  ' "$readme"

  found=false
  for block in "$shell_dir"/*.sh; do
    [ -f "$block" ] || continue
    found=true
    sh -n "$block" || {
      echo "README has invalid shell syntax: $block" >&2
      exit 1
    }
  done
  [ "$found" = true ] || {
    echo "README has no shell command blocks" >&2
    exit 1
  }
}

lead_run=$(awk '
  /^docker run -d --name looking-glass \\$/ { in_run = 1 }
  in_run { print }
  in_run && /^[[:space:]]*"\$LG_IMAGE"$/ { exit }
' "$readme")
[ -n "$lead_run" ] || {
  echo "README missing the complete lead docker run block" >&2
  exit 1
}

need_lead_run() {
  printf '%s\n' "$lead_run" | grep -Fq -- "$1" || {
    echo "README lead docker run block missing: $2" >&2
    exit 1
  }
}

published_image_path=$(awk '
  /^LG_IMAGE=ghcr\.io\/viktorsbaikers\/looking-glass:v0\.1\.1$/ { in_block = 1 }
  in_block { print }
  in_block && /^```$/ { exit }
' "$readme")
local_image_path=$(awk '
  /^docker build -t looking-glass:local \.$/ { in_block = 1 }
  in_block { print }
  in_block && /^```$/ { exit }
' "$readme")

check_image_paths() {
  published_line=$(grep -n -m 1 '^LG_IMAGE=ghcr.io/viktorsbaikers/looking-glass:v0.1.1$' "$readme" | cut -d: -f1 || true)
  local_line=$(grep -n -m 1 '^LG_IMAGE=looking-glass:local$' "$readme" | cut -d: -f1 || true)
  start_line=$(grep -n -m 1 '^docker run -d --name looking-glass \\$' "$readme" | cut -d: -f1 || true)
  if [ -z "$published_line" ] || [ -z "$local_line" ] || [ -z "$start_line" ] || [ "$local_line" -ge "$start_line" ]; then
    echo "README must set each image path before the shared central start command" >&2
    exit 1
  fi
  printf '%s\n' "$published_image_path" | grep -Fq 'docker pull "$LG_IMAGE"' || {
    echo "README published-image path must pull the immutable image" >&2
    exit 1
  }
  if printf '%s\n' "$published_image_path" | grep -Fq 'docker run -d --name looking-glass'; then
    echo "README must keep the published pull separate from the central start command" >&2
    exit 1
  fi
  printf '%s\n' "$local_image_path" | grep -Fq 'LG_IMAGE=looking-glass:local' || {
    echo "README local-image path must set LG_IMAGE before the central start command" >&2
    exit 1
  }
}

screenshot=$(sed -n 's/.*](\([^)]*\.\(png\|jpg\|jpeg\|webp\)\)).*/\1/p' "$readme" | head -n 1)
if [ -z "$screenshot" ] || [ ! -f "$screenshot" ]; then
  echo "README must reference an existing screenshot image" >&2
  exit 1
fi

need "Install the central container" "central container install guide"
need "Remote agent install" "remote agent install guide"
need_before 'LG_AGENT_URL' 'Generate install command' 'LG_AGENT_URL'
need_before 'LG_AGENT_SHA256' 'Generate install command' 'LG_AGENT_SHA256'
need_before 'LG_INSTALLER_URL' 'Generate install command' 'LG_INSTALLER_URL'
need_before 'LG_INSTALLER_SHA256' 'Generate install command' 'LG_INSTALLER_SHA256'
need_before 'LG_AGENT_INSTALL_SCRIPT_URL' 'Generate install command' 'LG_AGENT_INSTALL_SCRIPT_URL'
need_before 'LG_AGENT_INSTALL_SCRIPT_SHA256' 'Generate install command' 'LG_AGENT_INSTALL_SCRIPT_SHA256'
need "Configuration reference" "configuration reference"
need "Upgrade and rollback" "upgrade/rollback notes"
need "Publication is ordered, not atomic" "ordered release publication"
need "LG_IMAGE=ghcr.io/viktorsbaikers/looking-glass:v0.1.1" "published immutable GHCR image/tag"
need 'The published immutable `v0.1.1` image is available' "published image status"
need 'The URL/SHA-256 values below identify the published `v0.1.1` release assets' "published release-asset status"
need 'published `v0.1.1` assets and the URL/SHA-256 values shown above' "published remote-install status"
need 'LG_IMAGE=looking-glass:local' "pre-publication local smoke image override"
need '`LG_TRUSTED_PROXIES` to the real proxy-to-container source IP' "real trusted proxy setup"
need_lead_run '-p 8080:8080' "central HTTP port publication"
need_lead_run '-p 8443:8443' "agent tunnel port publication"
need_lead_run '-v looking-glass-data:/data' "persistent data volume"
need 'docker volume create looking-glass-data' "persistent data volume creation"
need '--entrypoint chown "$LG_IMAGE" -R 999:999 /data' "persistent data volume ownership initialization"
need_lead_run 'looking-glass.crt:/run/looking-glass/tls.crt:ro' "read-only certificate mount"
need_lead_run 'looking-glass.key:/run/looking-glass/tls.key:ro' "read-only key mount"
need_lead_run '-e LG_DB_PATH=/data/lookingglass.redb' "database path"
need_lead_run '-e LG_FILES_DIR=/data/files' "files path"
need_lead_run '-e LG_TRUSTED_PROXIES="$LG_TRUSTED_PROXIES"' "trusted proxy source"
need_lead_run '-e LG_CENTRAL_URL="https://$LG_PUBLIC_NAME"' "central HTTPS origin"
need_lead_run '-e LG_TUNNEL_URL="https://$LG_PUBLIC_NAME:8443"' "tunnel HTTPS origin"
need_lead_run '-e LG_CENTRAL_CERT=/run/looking-glass/tls.crt' "central certificate path"
need 'The HTTPS enrollment proxy must present the same leaf certificate configured at `LG_CENTRAL_CERT`' "proxy enrollment certificate identity"
need_lead_run '-e LG_TUNNEL_CERT=/run/looking-glass/tls.crt' "tunnel certificate path"
need_lead_run '-e LG_TUNNEL_KEY=/run/looking-glass/tls.key' "tunnel key path"
need 'sudo chown 999:999 tls/looking-glass.key' "container-readable private-key ownership"
need 'chmod 600 tls/looking-glass.key' "private-key permissions"
need_lead_run '-e LG_AGENT_INSTALL_SCRIPT_URL="$LG_INSTALLER_URL"' "installer URL configuration"
need_lead_run '-e LG_AGENT_INSTALL_SCRIPT_SHA256="$LG_INSTALLER_SHA256"' "installer SHA configuration"
need_lead_run '-e LG_AGENT_URL="$LG_AGENT_URL"' "agent URL configuration"
need_lead_run '-e LG_AGENT_SHA256="$LG_AGENT_SHA256"' "agent SHA configuration"
need 'LG_INSTALLER_SHA256=d824313a58f19e937f5365b9f5db019e05ba5163e00ec6249513b118142a7880' "verified installer SHA-256"
need 'LG_AGENT_SHA256=9bb238a79847683432e9f20066b37ea1fbd027829ebb792d14fc983b6e9bb8c7' "verified agent SHA-256"
need 'Generate install command' "generated enrollment-command flow"
if grep -Eqi 'your-github-owner|OWNER/REPO|<64-lowercase-hex-digest>|docker run \.\.\.' "$readme"; then
  echo "README still contains an owner, digest, or abbreviated run placeholder" >&2
  exit 1
fi

check_shell_blocks
check_image_paths

echo "README check passed"
