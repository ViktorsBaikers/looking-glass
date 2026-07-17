# Looking Glass

A self-hosted network diagnostics console. The central container serves the web UI,
stores state in one mounted directory, runs diagnostics on its built-in local node,
and coordinates enrolled remote agents over an outbound tunnel.

![First-run installer](docs/screenshots/installer.png)

The screenshot above is captured from the running application. Browser proof for the
populated public diagnostics page is recorded in `.devrites/work/looking-glass/browser-evidence.md`.

## Install the central container

The central app needs one writable volume for the redb database, local test files,
and the generated first-run setup token. Set the image variable to your published
GHCR repository and tag before pasting the run command.

```sh
LG_IMAGE=ghcr.io/your-github-owner/looking-glass:v0.1.0
docker pull "$LG_IMAGE"
docker run -d --name looking-glass \
  -p 8080:8080 \
  -v looking-glass-data:/data \
  -e LG_DB_PATH=/data/lookingglass.redb \
  -e LG_FILES_DIR=/data/files \
  -e LG_TRUSTED_PROXIES=127.0.0.1 \
  "$LG_IMAGE"
```

Put the web surface behind a TLS-terminating reverse proxy and configure
`LG_TRUSTED_PROXIES` with the proxy IPs that are allowed to attest
`X-Forwarded-Proto: https`. Admin login and agent enrollment are refused unless
that trusted proxy attests TLS.

On first start, the app writes the one-time setup token beside the database:

```sh
docker exec looking-glass cat /data/setup-token
```

Enter that token on the installer page, create the single admin account, then remove
or restrict access to the token file. To provide the token yourself instead of
generating a file, set `LG_SETUP_TOKEN` before the first start.

## Remote agent install

Before generating an enrollment command, publish the installer script and agent
binary, then set their HTTPS URLs and SHA-256 pins on the central container. This
example uses `LG_INSTALLER_URL` and `LG_INSTALLER_SHA256` as shell variables and
passes them through the central configuration names:

```sh
LG_INSTALLER_URL=https://downloads.example/looking-glass/install-agent.sh
LG_INSTALLER_SHA256=<64-lowercase-hex-digest>
LG_AGENT_URL=https://downloads.example/looking-glass/lg-agent
LG_AGENT_SHA256=<64-lowercase-hex-digest>

docker run ... \
  -e LG_AGENT_INSTALL_SCRIPT_URL="$LG_INSTALLER_URL" \
  -e LG_AGENT_INSTALL_SCRIPT_SHA256="$LG_INSTALLER_SHA256" \
  -e LG_AGENT_URL="$LG_AGENT_URL" \
  -e LG_AGENT_SHA256="$LG_AGENT_SHA256" \
  "$LG_IMAGE"
```

Central refuses enrollment-command generation unless all four configured values
are present and valid. Add these `-e` options to the central install command above;
the abbreviated `docker run ...` line only highlights the required asset settings.

Create a remote location in the admin panel, then generate an enrollment command.
The command embeds:

- the HTTPS central API origin (`LG_CENTRAL_URL`), used for `/api/enroll`;
- the HTTPS tunnel origin (`LG_TUNNEL_URL`), used after enrollment for the outbound agent tunnel;
- central's pinned identity fingerprint (`LG_CENTRAL_FP`);
- a single-use enrollment token (`LG_ENROLL_TOKEN`);
- verified installer and agent release asset URLs with SHA-256 pins.

Run the generated command on the Linux node exactly as shown. It self-escalates
through `sudo` when pasted by a sudo-capable user, scrubs the root environment with
an explicit allowlist, downloads and verifies the installer from a root-owned temp
directory, then verifies the agent binary before installing it. The installer
consumes the enrollment token immediately, writes the issued long-lived agent
credential owner-only under `/var/lib/lookingglass-agent`, and writes a systemd
unit that does not contain the enrollment token.

The command-control tunnel is outbound from the agent to central. Speedtest downloads
and iperf endpoints are separate data-plane services: expose them directly from the
node only when you want that node to offer speedtest data.

## Runtime privileges

The installed agent runs as the non-root `lookingglass-agent` user. The agent binary
gets no ambient capability. Each diagnostic tool receives only the grant it needs:

- `ping`, `mtr`, and `traceroute` get `cap_net_raw+ep` on the exact executable the
  service user resolves from the service `PATH`.
- BGP is available only through a restricted wrapper in
  `LG_AGENT_BGP_WRAPPER_DIR`; the agent does not fall through to system `birdc` or
  `vtysh`.

If no scoped BGP wrapper is installed, BGP stays unavailable and fails closed with a
clear message.

`NoNewPrivileges=false` is deliberate so file capabilities survive `execve`. The
fixed argv templates and root-owned service `PATH` bound the reachable commands; do
not add arbitrary tools to that path.

## Configuration reference

| Variable | Applies to | Default | Notes |
| --- | --- | --- | --- |
| `PORT` | central | `8080` | HTTP listener inside the container. |
| `LG_DB_PATH` | central | `data/lookingglass.redb` | redb database path. In containers, mount this under a volume. |
| `LG_FILES_DIR` | central | `data/files` | Local-node downloadable test files root. |
| `LG_SETUP_TOKEN` | central | generated file | Optional first-run setup token. If unset, central writes `setup-token` beside `LG_DB_PATH`. |
| `LG_TRUSTED_PROXIES` | central | empty | Comma-separated proxy IPs trusted for client identity and TLS attestation. Empty fails closed for admin/enroll TLS checks. |
| `LG_CENTRAL_URL` | central | `https://localhost` | Plain HTTPS API origin that serves `/api/enroll`; no path/query/fragment. |
| `LG_TUNNEL_URL` | central | `https://localhost:8443` | Plain HTTPS tunnel origin embedded in agent install commands; no path/query/fragment. |
| `LG_CENTRAL_CERT` | central | unset | PEM certificate used as central API identity material for enrollment fingerprinting. |
| `LG_CENTRAL_IDENTITY` | central | ephemeral | Stable fallback identity material when `LG_CENTRAL_CERT` is not set. |
| `LG_TUNNEL_CERT` / `LG_TUNNEL_KEY` | central | unset | PEM cert/key for the direct agent TLS tunnel listener. If unset, remote agents cannot connect. |
| `LG_TUNNEL_BIND` | central | `0.0.0.0:8443` | Direct TLS/WebSocket tunnel bind address. |
| `LG_AGENT_INSTALL_SCRIPT_URL` | central | unset | HTTPS URL embedded in generated agent install commands. |
| `LG_AGENT_INSTALL_SCRIPT_SHA256` | central | unset | SHA-256 pin for the installer script. |
| `LG_AGENT_URL` | central | unset | HTTPS URL for the prebuilt agent binary release asset. |
| `LG_AGENT_SHA256` | central | unset | SHA-256 pin for the agent binary. |
| `LG_EXEC_MAX_CONCURRENT` | central | `8` | Global in-flight diagnostic cap per node. |
| `LG_EXEC_TIMEOUT_SECS` | central | `30` | Per-command timeout. |
| `LG_EXEC_MAX_OUTPUT_KIB` | central | `256` | Total output cap per command. |
| `LG_EXEC_RATE_MAX` | central | `20` | Per-client run attempts per window. |
| `LG_EXEC_RATE_WINDOW_SECS` | central | `60` | Per-client run rate window. |
| `LG_AGENT_CREDENTIAL` | agent | `data/agent-credential.json` | Stored credential path. The installer sets this in the unit. |
| `LG_AGENT_DATA_BIND` | agent | unset | Optional remote speedtest file server bind address. |
| `LG_AGENT_FILES_DIR` | agent | `data/files` | Remote speedtest files root when data plane is enabled. |
| `LG_AGENT_BGP_WRAPPER_DIR` | agent | installer wrapper dir | Scoped directory the agent probes for `birdc`/`vtysh`. |

Installer-only variables include `LG_AGENT_INSTALL_PATH`, `LG_AGENT_STATE_DIR`,
`LG_AGENT_SERVICE_FILE`, `LG_AGENT_USER`, `LG_AGENT_SERVICE_PATH`,
`LG_AGENT_BGP_WRAPPER`, and `LG_AGENT_BGP_DAEMON`. `LG_INSTALL_DRY_RUN=1` is for
local installer tests only.

## Release image

`.github/workflows/release.yml` builds the Dockerfile and publishes to GHCR on:

- tags matching `v*`;
- GitHub Releases published from repository tags matching `v*`;
- manual `workflow_dispatch` when `push=true`.

Manual dispatch with `push=false` runs the same build path without logging in or
pushing, which is the no-credential dry-run equivalent. Manual dispatch with
`push=true` is a credentialed GHCR publish from the selected ref. Actual GHCR
package visibility, repository permissions, and branch/tag protection are
operator-owned setup.

## Upgrade and rollback

1. Pull the new image tag.
2. Stop the old central container.
3. Start the new image with the same mounted data volume and environment.
4. Verify `/health`, admin login, public location list, and one local diagnostic.

Rollback is the same process with the previous image tag and the same volume. Before
upgrading, keep a copy of the mounted data directory or redb file:

```sh
docker stop looking-glass
docker run --rm -v looking-glass-data:/data -v "$PWD":/backup alpine \
  sh -c 'cp -a /data /backup/looking-glass-data-backup'
```

For remote agents, keep the previous agent release asset available until the central
upgrade is accepted. If an agent upgrade fails, reinstall with the previous generated
asset URL/checksum and the stored credential path; do not reuse an enrollment token.

## Develop and verify

The central binary embeds `frontend/build`, so build the SPA before Rust checks:

```sh
cd frontend && npm ci && npm run build && cd ..
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all
cargo build --release
docker build -t looking-glass .
```

`make verify` runs the frontend build, Rust formatting, clippy, tests, and release build.
CI runs the same checks on push and pull request; enable branch protection to make a red
result block merges.
