#!/usr/bin/env sh
set -eu

readme=${1:-README.md}
agents=${2:-AGENTS.md}
compose=${3:-compose.yaml}

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

# Line of the first match of $1 (fixed string), empty when absent.
line_of() {
  grep -nF -m 1 -- "$1" "$readme" | cut -d: -f1 || true
}

need_order() {
  first=$(line_of "$1")
  second=$(line_of "$2")
  if [ -z "$first" ] || [ -z "$second" ] || [ "$first" -ge "$second" ]; then
    echo "README must show $3" >&2
    exit 1
  fi
}

advanced_run=$(awk '
  /^docker run -d --name looking-glass \\$/ { in_run = 1 }
  in_run { print }
  in_run && /^[[:space:]]*ghcr\.io\/viktorsbaikers\/looking-glass:latest$/ { exit }
' "$readme")
[ -n "$advanced_run" ] || {
  echo "README missing the complete Advanced docker run block" >&2
  exit 1
}

need_advanced_run() {
  printf '%s\n' "$advanced_run" | grep -Fq -- "$1" || {
    echo "README Advanced docker run block missing: $2" >&2
    exit 1
  }
}

# compose.yaml runs the published latest image, never a local build.
grep -Fqx -- '    image: ghcr.io/viktorsbaikers/looking-glass:latest' "$compose" || {
  echo "$compose central image must be ghcr.io/viktorsbaikers/looking-glass:latest" >&2
  exit 1
}
if grep -Eq '^[[:space:]]*build:' "$compose"; then
  echo "$compose must not build the image" >&2
  exit 1
fi
if grep -q 'LG_[I]MAGE' "$readme" "$compose"; then
  echo "README and $compose must name the image directly, not through an image variable" >&2
  exit 1
fi

# ERE (-E): BSD sed has no BRE `\|` alternation.
screenshot=$(sed -En 's/.*]\(([^)]*\.(png|jpg|jpeg|webp))\).*/\1/p' "$readme" | head -n 1)
if [ -z "$screenshot" ] || [ ! -f "$screenshot" ]; then
  echo "README must reference an existing screenshot image" >&2
  exit 1
fi

if grep -Fq 'releases/download/v0.1.1/' "$readme"; then
  echo "README must not reference stale v0.1.1 release URLs" >&2
  exit 1
fi

need "Install the central container" "central container install guide"
grep -Fqx 'echo '"'"'LG_DOMAIN=lg.example.net'"'"' > .env' "$readme" || {
  echo "README missing: LG_DOMAIN in .env for the Compose quick start" >&2
  exit 1
}
need_order "echo 'LG_DOMAIN=lg.example.net' > .env" 'docker compose up -d' "LG_DOMAIN in .env before docker compose up -d"
need_order 'docker compose up -d' 'docker compose exec central cat /data/setup-token' "docker compose up -d before reading the setup token"
need_order 'docker compose exec central cat /data/setup-token' '### Advanced: central without Compose' "the Compose quick start before the Advanced section"
need_order '### Advanced: central without Compose' 'docker run -d --name looking-glass' "the docker run start command inside the Advanced section"
need 'Open `https://lg.example.net` (your `LG_DOMAIN`)' "open the wizard at the LG_DOMAIN origin"
need 'It holds the database and the tunnel key' "central-data volume backup"
need 'The Compose network is IPv4-only' "IPv6 visitors share one key on the IPv4-only Compose network"
grep -Fqx 'curl -fsSLO https://raw.githubusercontent.com/ViktorsBaikers/looking-glass/main/compose.yaml' "$readme" || {
  echo "README missing: one-command compose.yaml download" >&2
  exit 1
}
need_order 'curl -fsSLO https://raw.githubusercontent.com/ViktorsBaikers/looking-glass/main/compose.yaml' 'docker compose up -d' "the compose.yaml download before docker compose up -d"
need 'docker build -t ghcr.io/viktorsbaikers/looking-glass:latest .' "Compose source build"
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
grep -Fqx 'docker pull ghcr.io/viktorsbaikers/looking-glass:latest' "$readme" || {
  echo "README missing: docker pull of the published image" >&2
  exit 1
}
need 'The URL/SHA-256 values below identify the published `v0.1.3` release assets' "published release-asset status"
need 'LG_INSTALLER_URL=https://github.com/ViktorsBaikers/looking-glass/releases/download/v0.1.3/install-agent.sh' "published installer URL"
need 'LG_AGENT_URL=https://github.com/ViktorsBaikers/looking-glass/releases/download/v0.1.3/lg-agent-x86_64-unknown-linux-gnu' "published agent URL"
need 'published `v0.1.3` assets and the URL/SHA-256 values shown above' "published remote-install status"
need 'A central built from source needs an agent and installer built from the same tree' "same-tree agent and installer for a source-built central"
need 'The published `v0.1.3` image and agent pin the SHA-256 of the whole certificate' "v0.1.3 agents disconnect on every certificate renewal"
need '/var/lib/lookingglass-agent/data/files' "remote node speed-test files directory"
need '`LG_TRUSTED_PROXIES` to the real proxy-to-container source IP' "real trusted proxy setup"
need_advanced_run '-p 127.0.0.1:8080:8080' "central HTTP port published on loopback only"
if printf '%s\n' "$advanced_run" | grep -Eq -- '-p (0\.0\.0\.0:|\[::\]:)?8080:'; then
  echo "README Advanced docker run block publishes plain-HTTP 8080 on every address" >&2
  exit 1
fi
need 'docker network inspect bridge' "trusted proxy is the bridge gateway behind a host-local proxy"
need_advanced_run '-p 8443:8443' "agent tunnel port publication"
need_advanced_run '-v looking-glass-data:/data' "persistent data volume"
need_advanced_run '-e LG_TRUSTED_PROXIES="$LG_TRUSTED_PROXIES"' "trusted proxy source"
need_advanced_run '-e LG_TUNNEL_URL="https://$LG_PUBLIC_NAME:8443"' "tunnel HTTPS origin"
need 'The HTTPS enrollment proxy must present the same leaf certificate configured at `LG_CENTRAL_CERT`' "proxy enrollment certificate identity"
need 'LG_INSTALLER_SHA256=d824313a58f19e937f5365b9f5db019e05ba5163e00ec6249513b118142a7880' "verified installer SHA-256"
need 'LG_AGENT_SHA256=9bb238a79847683432e9f20066b37ea1fbd027829ebb792d14fc983b6e9bb8c7' "verified agent SHA-256"
need 'Generate install command' "generated enrollment-command flow"
need 'must pass the original `Host` header through' "reverse-proxy Host preservation"
grep -Fq 'proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;' "$readme" || {
  echo "README missing: nginx X-Forwarded-For directive" >&2
  exit 1
}
need 'Without it, every visitor is counted as the proxy' "why the proxy must send X-Forwarded-For"
grep -Fq 'proxy_set_header X-Forwarded-Proto $scheme;' "$readme" || {
  echo "README missing: nginx X-Forwarded-Proto directive" >&2
  exit 1
}
need '403 `insecure_transport`' "why the proxy must send X-Forwarded-Proto"
need 'docker network create --subnet 172.30.0.0/24 --ip-range 172.30.0.128/25 lg-proxy' "user-defined network for a same-host proxy container"
need 'replace `-p 127.0.0.1:8080:8080` with' "central unpublished on the proxy network"
need '`--network lg-proxy --ip 172.30.0.3`' "central pinned address on the proxy network"
need '`LG_TRUSTED_PROXIES=172.30.0.2`' "trusted proxy is the proxy container's pinned address"
need '`--network lg-proxy --ip 172.30.0.2`' "proxy container pinned address"
need 'proxy_pass http://172.30.0.3:8080;' "proxy container upstream"
need 'every IPv6 visitor as one client' "IPv6 visitors share one key behind a containerised proxy"
need 'docker network create --ipv6 --subnet 172.30.0.0/24 --ip-range 172.30.0.128/25' "IPv6-enabled proxy network keeps per-visitor keys"
need 'run the proxy container with `--network host`' "host-networked proxy keeps per-visitor keys"
need 'sudo install -o root -g bird -m 2755 bgp-wrapper /usr/local/lib/lookingglass-agent/bin/birdc' "BGP wrapper installed setgid to the daemon socket group"
need 'For FRR, install it as `vtysh` with `-g frrvty`' "FRR wrapper group"
need 'When you switch between BIRD and FRR by hand, delete the other daemon' "remove the other daemon's BGP wrapper on a manual switch (the agent probes birdc first)"
need '`birdc show route for <prefix>`' "argv the agent passes to the BIRD wrapper"
need "\`vtysh -c 'show bgp ipv6 <prefix>'\`" "argv the agent passes to the FRR wrapper"
need 'Linux ignores the setgid bit on scripts' "BGP wrapper must be a compiled program"
if grep -Fq 'install -o root -g root -m 0755 bgp-wrapper' "$readme"; then
  echo "README installs the BGP wrapper root:root 0755, which cannot open the daemon socket" >&2
  exit 1
fi
need 'needs an explicit port' "LG_TUNNEL_URL port requirement"
need 'Upgrade central before its agents' "central-before-agents upgrade order"
need 'Rollback needs the backup' "rollback restores the pre-upgrade backup"
need 'cp -a /backup/looking-glass-data-backup/data/. /data/ && chown -R 999:999 /data' "rollback restore re-owns the data to 999:999"
need 'In the same step, switch `LG_AGENT_URL`' "agent download settings switched to the new release with the central upgrade"
need 'central identity does not match the pinned fingerprint' "agent/central version-skew error"
need 'That is version skew between central and agent, not an impostor' "skew error is not an impostor"
need 'update the README pins' "release procedure updates README pins before tagging"
if grep -Eqi 'stable fallback|GitHub Releases published|\.devrites/' "$readme"; then
  echo "README still documents a removed identity fallback, release trigger, or untracked evidence file" >&2
  exit 1
fi
if grep -Eqi 'your-github-owner|OWNER/REPO|<64-lowercase-hex-digest>|docker run \.\.\.' "$readme"; then
  echo "README still contains an owner, digest, or abbreviated run placeholder" >&2
  exit 1
fi
if grep -Fqi 'same mounted volume' "$readme"; then
  echo "README rollback must restore the pre-upgrade backup, not reuse the same mounted volume" >&2
  exit 1
fi
if [ -f "$agents" ] && grep -qi 'tailwind' "$agents"; then
  echo "AGENTS.md names Tailwind; the frontend stack is Panda CSS + Ark UI" >&2
  exit 1
fi

check_shell_blocks

echo "README check passed"
