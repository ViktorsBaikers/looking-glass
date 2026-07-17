#!/usr/bin/env sh
set -eu

readme=${1:-README.md}

need() {
  grep -qi "$1" "$readme" || {
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

screenshot=$(sed -n 's/.*](\([^)]*\.\(png\|jpg\|jpeg\|webp\)\)).*/\1/p' "$readme" | head -n 1)
if [ -z "$screenshot" ] || [ ! -f "$screenshot" ]; then
  echo "README must reference an existing screenshot image" >&2
  exit 1
fi

need "Install the central container" "central container install guide"
need "Remote agent install" "remote agent install guide"
need_before 'LG_AGENT_URL' 'generate an enrollment command' 'LG_AGENT_URL'
need_before 'LG_AGENT_SHA256' 'generate an enrollment command' 'LG_AGENT_SHA256'
need_before 'LG_INSTALLER_URL' 'generate an enrollment command' 'LG_INSTALLER_URL'
need_before 'LG_INSTALLER_SHA256' 'generate an enrollment command' 'LG_INSTALLER_SHA256'
need_before 'LG_AGENT_INSTALL_SCRIPT_URL' 'generate an enrollment command' 'LG_AGENT_INSTALL_SCRIPT_URL'
need_before 'LG_AGENT_INSTALL_SCRIPT_SHA256' 'generate an enrollment command' 'LG_AGENT_INSTALL_SCRIPT_SHA256'
need "Configuration reference" "configuration reference"
need "Upgrade and rollback" "upgrade/rollback notes"
need "LG_IMAGE=ghcr.io/" "operator-set GHCR image/tag"
need 'docker pull "$LG_IMAGE"' "GHCR image pull example"
need 'docker run' "central container run example"
if grep -q "ghcr.io/OWNER/REPO:VERSION" "$readme"; then
  echo "README still contains the placeholder GHCR image tag" >&2
  exit 1
fi

echo "README check passed"
