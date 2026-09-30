# Looking Glass

A self-hosted network diagnostics console. The central container serves the web UI,
stores state in one mounted directory, runs diagnostics on its built-in local node,
and coordinates enrolled remote agents over an outbound tunnel.

[![Looking Glass product tour](docs/media/looking-glass-tour.jpg)](docs/media/looking-glass-tour.mp4)

A 50-second tour: an MTR run from a remote location, the theme switch, the admin
console and agent enrollment. Light and dark screenshots of every screen are under
[Screenshots](#screenshots).

## Install the central container

The quickest setup is Docker Compose. `compose.yaml` runs central behind a bundled
[Caddy](https://caddyserver.com) that serves the web UI over HTTPS. You need a host
with Docker Compose and free ports 80, 443 and 8443.

Get `compose.yaml`:

```sh
mkdir looking-glass && cd looking-glass
curl -fsSLO https://raw.githubusercontent.com/ViktorsBaikers/looking-glass/main/compose.yaml
```

The Compose setup needs the first release after `v0.1.3`, and `latest` points at
`v0.1.3` until then. Meanwhile, build the image yourself, as in
[Run a build of your own tree](#run-a-build-of-your-own-tree).

Point a public DNS name at the host, open TCP 80, 443 and 8443 to the internet, and
put the name in `.env` beside `compose.yaml`:

```sh
echo 'LG_DOMAIN=lg.example.net' > .env
```

Without `.env`, `LG_DOMAIN` is `localhost`. Caddy then signs the UI's certificate
with its own internal CA and the browser warns about it: enough to try the UI on
your own machine, but remote agents need a name they can reach.

Start the stack and read the one-time setup token:

```sh
docker compose up -d
docker compose exec central cat /data/setup-token
```

Open `https://lg.example.net` (your `LG_DOMAIN`), enter the token on the installer
page, create the first administrator, then remove or restrict access to the token
file.

What the stack does:

- Caddy gets a Let's Encrypt certificate for `LG_DOMAIN` on ports 80 and 443 and
  forwards to central's plain-HTTP port 8080, which is not published. Central trusts
  `X-Forwarded-*` headers from Caddy's fixed address only.
- On first start, central generates its own agent tunnel key and certificate and
  keeps them in the `central-data` volume beside the database.
- Agents enroll and connect on port 8443, straight to central's TLS, and pin that
  key. Caddy's certificate can renew as often as it likes without touching them.

Back up the `central-data` volume (see [Upgrade and rollback](#upgrade-and-rollback)).
It holds the database and the tunnel key. Losing it, for example with
`docker compose down -v`, means starting over, and every agent has to be re-enrolled.
If the database survives but both `tunnel.crt` and `tunnel.key` are gone, central
generates a new pair and logs an error with the number of enrolled agents that must
re-enroll. If only one of the two files is left, central leaves it alone and keeps the
agent tunnel off until you restore the missing file or remove the one left.

The Compose network is IPv4-only. On a host with IPv6, Docker still publishes 80, 443
and 8443 on the host's IPv6 addresses, and relays each IPv6 visitor to the container
from the network gateway. Central then sees every IPv6 visitor as one client: they
share one run budget, and five failed logins from any of them lock every IPv6
administrator out for up to a minute. Agents that reach 8443 over IPv6 share the
tunnel port's per-address connection limit the same way. To keep a key per IPv6
visitor, run central without Compose behind a host-networked proxy (see
[Advanced](#advanced-central-without-compose)).

### Run a build of your own tree

To run a checkout of this repository instead of the published image, build it under
the image name `compose.yaml` uses, then start the stack as usual. A later
`docker compose pull` replaces it with the published image.

```sh
docker build -t ghcr.io/viktorsbaikers/looking-glass:latest .
docker compose up -d
```

A central built from source needs an agent and installer built from the same tree.
A release image carries the URLs and SHA-256 pins of its own release's installer and
agent; a source build carries none, so central refuses to generate install commands
until you set them. The published `v0.1.3` agent also checks a whole-certificate pin,
while central hands its agents a public-key (SPKI) pin, so every enrollment from a
source-built central with the `v0.1.3` assets fails. Build both from the tree, the
way `release.yml` does:

```sh
mkdir -p assets
cp scripts/install-agent.sh assets/install-agent.sh
docker run --rm -v "$PWD":/src:ro -v "$PWD/assets":/out rust:1.96-bookworm \
  sh -ceu 'cd /src && CARGO_TARGET_DIR=/tmp/target cargo build --locked --release --package agent && cp /tmp/target/release/agent /out/lg-agent'
sha256sum assets/install-agent.sh assets/lg-agent
```

The agent is built for the Docker host's CPU; add `--platform linux/amd64` (or
`linux/arm64`) to `docker run` to match the node. Serve both files over HTTPS from a
host the node can reach, and give central their URLs and hashes in a
`compose.override.yaml` beside `compose.yaml`, which Compose merges by itself:

```yaml
services:
  central:
    environment:
      LG_AGENT_INSTALL_SCRIPT_URL: https://files.example.net/install-agent.sh
      LG_AGENT_INSTALL_SCRIPT_SHA256: <sha256 of install-agent.sh>
      LG_AGENT_URL: https://files.example.net/lg-agent
      LG_AGENT_SHA256: <sha256 of lg-agent>
```

With `docker run` (below), pass the same four variables as `-e` options.

### Advanced: central without Compose

To run central alone behind a TLS proxy you already have, pull the published image
(or build it from a checkout, as above):

```sh
docker pull ghcr.io/viktorsbaikers/looking-glass:latest
```

Then set `LG_PUBLIC_NAME` to the public DNS name agents use to reach this host and
`LG_TRUSTED_PROXIES` to the real proxy-to-container source IP. The following is the
complete central start command:

```sh
LG_PUBLIC_NAME=lg.example.net
# Set this to the source IP the container sees for the TLS-terminating proxy.
LG_TRUSTED_PROXIES=SET_THE_REAL_PROXY_TO_CONTAINER_SOURCE_IP
docker run -d --name looking-glass \
  -p 127.0.0.1:8080:8080 \
  -p 8443:8443 \
  -v looking-glass-data:/data \
  -e LG_TRUSTED_PROXIES="$LG_TRUSTED_PROXIES" \
  -e LG_TUNNEL_URL="https://$LG_PUBLIC_NAME:8443" \
  ghcr.io/viktorsbaikers/looking-glass:latest
```

Docker creates the `looking-glass-data` volume on first use, owned by the container
user. As with Compose, central generates its tunnel key into that volume, install
commands point agents at `LG_TUNNEL_URL` and pin that key, and a release image
supplies the agent download settings (a source build needs them set, as above). The
one-time setup token is in the volume too:

```sh
docker exec looking-glass cat /data/setup-token
```

To provide the token yourself instead of generating a file, set `LG_SETUP_TOKEN`
before the first start.

Put the web surface behind a TLS-terminating reverse proxy that forwards to port
8080. `LG_TRUSTED_PROXIES` must be the real source IP that container receives from
that proxy, not a client address or a copied example. The proxy must attest
`X-Forwarded-Proto: https`; first-run setup and admin login are refused without that
trusted attestation. Publish port 8443 directly so agents can enroll and reach the
TLS/WebSocket tunnel at `LG_TUNNEL_URL`.

The proxy must pass the original `Host` header through unchanged. Central compares
it with the browser's `Origin`, so behind a proxy that rewrites `Host`, admin login,
admin changes and public diagnostic runs answer 403. nginx rewrites it by default;
add `proxy_set_header Host $host;`.

The proxy must also send the visitor's address in `X-Forwarded-For`. Central keys the
login lockout and the diagnostic rate limit on the client that a trusted proxy names
there. Without it, every visitor is counted as the proxy, so five failed logins from
anyone lock every administrator out for up to a minute, and all visitors share one
run budget. nginx sends no `X-Forwarded-For` by default; add
`proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;`. Central reads the
header from the right and takes the first address it does not trust, so a value the
visitor sent cannot replace the one the proxy appends.

nginx sends no `X-Forwarded-Proto` by default either; add
`proxy_set_header X-Forwarded-Proto $scheme;`. Without it, central cannot tell that
the visitor used HTTPS, so first-run setup, admin login, install-command generation
and proxy enrollment answer 403 `insecure_transport`.

The start command above publishes the plain-HTTP port 8080 on `127.0.0.1` only, for a
proxy on the same host. Published on every address, it would also take connections
that Docker's proxy relays from the bridge gateway, IPv6 clients among them. Central
trusts the gateway, so those clients could set their own `X-Forwarded-For` and
`X-Forwarded-Proto`. Point the proxy at `http://127.0.0.1:8080`. The container sees
that proxy as the Docker bridge gateway, so set `LG_TRUSTED_PROXIES` to the gateway
address, usually `172.17.0.1`:

```sh
docker network inspect bridge --format '{{(index .IPAM.Config 0).Gateway}}'
```

If the proxy runs on another machine, publish 8080 only on the address that proxy
connects to (`-p 10.0.0.5:8080:8080`, for example), never on every address, and set
`LG_TRUSTED_PROXIES` to the source address the container sees for that proxy.

A proxy in a container on the same host cannot use `http://127.0.0.1:8080`: inside
its container, `127.0.0.1` is the container itself. Started with `--network host`,
it shares the host's loopback and the host-proxy setup above applies unchanged.
Otherwise, put central and the proxy on one user-defined network with fixed
addresses, and publish 8080 nowhere:

```sh
docker network create --subnet 172.30.0.0/24 --ip-range 172.30.0.128/25 lg-proxy
```

Start central with the command above, but replace `-p 127.0.0.1:8080:8080` with
`--network lg-proxy --ip 172.30.0.3` and set `LG_TRUSTED_PROXIES=172.30.0.2`. Start the
proxy container with `--network lg-proxy --ip 172.30.0.2`, publish its HTTPS port,
and point it at central's fixed address (nginx: `proxy_pass http://172.30.0.3:8080;`).
`--ip-range` keeps the addresses Docker assigns by itself in the upper half of the
subnet, so Docker never gives the proxy's address to another container. Choose
another subnet if `172.30.0.0/24` is already in use on the host. `compose.yaml` uses
this same layout.

On a host with IPv6, Docker also publishes the proxy's HTTPS port on the host's IPv6
addresses. `lg-proxy` is IPv4-only, so Docker's userland proxy (`docker-proxy`)
relays each IPv6 visitor to the proxy container from the network gateway
(`172.30.0.128`: Docker takes it from the `--ip-range`). The proxy then names that
gateway in `X-Forwarded-For`, and central counts every IPv6 visitor as one client:
they share one run budget, and five failed logins from any of them lock every IPv6
administrator out. To keep a key per IPv6 visitor, either:

- run the proxy container with `--network host` and use the host-proxy setup above
  (central keeps `-p 127.0.0.1:8080:8080`, the proxy points at
  `http://127.0.0.1:8080`, and `LG_TRUSTED_PROXIES` is the bridge gateway); or
- create `lg-proxy` with IPv6 as well, so Docker forwards IPv6 visitors to the proxy
  container with their own addresses:

```sh
docker network create --ipv6 --subnet 172.30.0.0/24 --ip-range 172.30.0.128/25 \
  --subnet fd00:172:30::/64 lg-proxy
```

#### Your own tunnel certificate and proxy enrollment

Both are optional. To have the tunnel present a certificate you manage, mount it and
set `LG_TUNNEL_CERT` and `LG_TUNNEL_KEY` to the PEM files; central then uses them and
generates nothing. The private key must be readable by the container user (uid 999).

Earlier releases had agents enroll through the web proxy instead of port 8443. That
still works: set `LG_CENTRAL_URL` to the proxy's HTTPS origin and `LG_CENTRAL_CERT`
to the certificate the proxy presents; agents pin it when they enroll. The HTTPS enrollment proxy must present the same leaf certificate configured at `LG_CENTRAL_CERT`.
It must also be the tunnel's certificate, so set `LG_TUNNEL_CERT` and `LG_TUNNEL_KEY`
to that same pair; otherwise install-command generation answers 503
`identity_mismatch`. Deployments configured this way keep working unchanged after an
upgrade.

#### Renewing the TLS certificate

This applies only to a tunnel certificate you provide. The certificate central
generates is valid until the year 4096 and never needs renewing.

The published `v0.1.3` image and agent pin the SHA-256 of the whole certificate. With
them, every renewal disconnects every agent, even a renewal that keeps the private
key: after renewing, re-enroll each node with a newly generated install command.

A central and agents built from a later tree pin the SHA-256 of the certificate's
public key (its SubjectPublicKeyInfo) instead. There, a renewed certificate that keeps
the same private key keeps every enrolled agent connected. Renew with key reuse, for
example `certbot renew --reuse-key` (or `certbot certonly --reuse-key` when the
certificate is first issued), keep `LG_CENTRAL_CERT`, `LG_TUNNEL_CERT` and
`LG_TUNNEL_KEY` on the renewed files, and restart central.

A new key is a new identity. Every agent refuses central after a key change, and each
node has to be re-enrolled with a newly generated install command. Agents enrolled
before this release pinned the whole certificate. Each one moves its stored pin to the
public key on its first successful connection, so upgrade the agents and let them
connect once before the first renewal.

## Remote agent install

Enrollment-command generation needs four release-asset values: the installer and
agent URLs and their SHA-256 pins. A release image carries its own release's values,
so neither setup above sets them; a source build needs them set by hand (see
[Run a build of your own tree](#run-a-build-of-your-own-tree)). Central refuses to
generate an enrollment command if any URL or SHA-256 pin is missing or invalid.
The URL/SHA-256 values below identify the published `v0.1.3` release assets. They
pair with the published `v0.1.3` image only:

```sh
LG_INSTALLER_URL=https://github.com/ViktorsBaikers/looking-glass/releases/download/v0.1.3/install-agent.sh
LG_INSTALLER_SHA256=d824313a58f19e937f5365b9f5db019e05ba5163e00ec6249513b118142a7880
LG_AGENT_URL=https://github.com/ViktorsBaikers/looking-glass/releases/download/v0.1.3/lg-agent-x86_64-unknown-linux-gnu
LG_AGENT_SHA256=9bb238a79847683432e9f20066b37ea1fbd027829ebb792d14fc983b6e9bb8c7
```

In central's environment the installer pair is named `LG_AGENT_INSTALL_SCRIPT_URL`
and `LG_AGENT_INSTALL_SCRIPT_SHA256`. Remote installation from a release central uses
the published `v0.1.3` assets and the URL/SHA-256 values shown above.

After the central container is healthy, finish first-run setup with the token, sign
in as the administrator, create a remote location, and use that location's
**Generate install command** action. Copy the resulting command to the Linux node;
do not invent or hand-edit its token. It embeds:

- the HTTPS central API origin (`LG_CENTRAL_URL`), used for `/api/enroll`;
- the HTTPS tunnel origin (`LG_TUNNEL_URL`), used after enrollment for the outbound agent tunnel;
- central's pinned identity fingerprint (`LG_CENTRAL_FP`);
- a single-use enrollment token (`LG_ENROLL_TOKEN`);
- the published installer and agent release-asset URLs with SHA-256 pins.

Run the generated command on the Linux node exactly as shown. It self-escalates
through `sudo` when pasted by a sudo-capable user, scrubs the root environment with
an explicit allowlist, downloads and verifies the installer from a root-owned temp
directory, then verifies the agent binary before installing it. The installer
consumes the enrollment token immediately, writes the issued long-lived agent
credential owner-only under `/var/lib/lookingglass-agent`, and writes a systemd
unit that does not contain the enrollment token. The location becomes connected
after its agent reaches the tunnel; an enrollment token cannot be reused.

The command-control tunnel is outbound from the agent to central. Speedtest downloads
and iperf endpoints are separate data-plane services: expose them directly from the
node only when you want that node to offer speedtest data.

### Remote speed test over HTTPS

To offer a browser speed test from a remote node, set the location's
**Data-plane origin** to `https://` plus the node's public DNS name or IP address
(port 443 only). The agent then serves the speed test over HTTPS on TCP 443 and
obtains and renews its certificate from Let's Encrypt by itself, accepting the
Let's Encrypt Subscriber Agreement on your behalf and sending no contact address.
An IP address gets a six-day certificate, a DNS name a 90-day one; both renew
automatically. The one requirement: **the node must be reachable from the internet
on TCP 443 at the data-plane origin** (Let's Encrypt validates the certificate
there, from `acme-v02.api.letsencrypt.org`). The location editor shows when the
certificate was issued, when it expires, and the last issuance error, if any.

The node serves the location's test files from its own files directory. By default
that is `/var/lib/lookingglass-agent/data/files`: the agent's `LG_AGENT_FILES_DIR`
defaults to `data/files`, relative to the unit's working directory
`/var/lib/lookingglass-agent`. A test file whose **Source on node** is `100mb.bin` is
served at `https://<data-plane origin>/files/100mb.bin` from that directory:

```sh
sudo install -d -m 0755 /var/lib/lookingglass-agent/data/files
sudo install -m 0644 100mb.bin /var/lib/lookingglass-agent/data/files/100mb.bin
```

To serve another directory, set `LG_AGENT_FILES_DIR` in a unit drop-in
(`sudo systemctl edit lookingglass-agent`, then
`Environment=LG_AGENT_FILES_DIR=/srv/lg-files` under `[Service]`) and restart the
agent. The `lookingglass-agent` user must be able to read the files, and the unit's
`ProtectHome=true` hides `/home`, `/root` and `/run/user`.

## Runtime privileges

The installed agent runs as the non-root `lookingglass-agent` user. The agent's only
ambient capability is `CAP_NET_BIND_SERVICE`, for its HTTPS data plane on TCP 443.
Each diagnostic tool receives only the grant it needs:

- `ping`, `traceroute` and `mtr-packet` (the helper `mtr` runs to open its raw
  sockets) get `cap_net_raw+ep` on the exact executable the service user resolves
  from the service `PATH`, or on the real file behind it when that is a symlink.
- BGP is available only through a restricted wrapper in
  `LG_AGENT_BGP_WRAPPER_DIR`; the agent does not fall through to system `birdc` or
  `vtysh`.

If no scoped BGP wrapper is installed, BGP stays unavailable and fails closed with a
clear message.

To enable BGP on a node, install your read-only wrapper after the generated install
command has finished. Name it `birdc` for BIRD or `vtysh` for FRR. The agent runs it
with one of these argument lists and no others:

- BIRD: `birdc show route for <prefix>`
- FRR, IPv4: `vtysh -c 'show ip bgp <prefix>'`
- FRR, IPv6: `vtysh -c 'show bgp ipv6 <prefix>'`

`<prefix>` is an address or CIDR prefix the agent has already validated, so it holds
only hex digits, `.`, `:` and `/`. The wrapper must stay read-only: accept exactly these
forms, reject any other arguments, and pass the query to the real client (BIRD's
`birdc -r` allows only `show` commands).

The wrapper runs as the `lookingglass-agent` user, which cannot open the daemon's
control socket: BIRD's `bird.ctl` is `bird:bird` mode 0660 and FRR's sockets are
`frr:frrvty` mode 0770. Install the wrapper owned by root, group-owned by that socket
group, and setgid (mode 2755), so the wrapper alone gets the group while it runs.
Linux ignores the setgid bit on scripts, so the wrapper must be a compiled program that
execs the real client:

```sh
sudo install -d -m 0755 /usr/local/lib/lookingglass-agent/bin
sudo install -o root -g bird -m 2755 bgp-wrapper /usr/local/lib/lookingglass-agent/bin/birdc
```

For FRR, install it as `vtysh` with `-g frrvty`. A wrapper without the group and the
setgid bit fails with `Permission denied` on the socket. Do not add
`lookingglass-agent` to the `bird` or `frrvty` group instead: that gives every
program the agent runs full control of the daemon. If you run `install-agent.sh`
yourself, `LG_AGENT_BGP_WRAPPER=<file>` with `LG_AGENT_BGP_DAEMON=bird` or `frr` does
the same install, removes the other daemon's wrapper, and stops before enrollment if
the group does not exist or the file is a script.

When you switch between BIRD and FRR by hand, delete the other daemon's wrapper
(`birdc` when moving to FRR, `vtysh` when moving to BIRD): the agent tries `birdc`
first, so a leftover one keeps answering BGP queries.

The agent looks for the wrapper on each BGP request, so no restart is needed.

`NoNewPrivileges=false` is deliberate so file capabilities survive `execve`. The
fixed argv templates and root-owned service `PATH` bound the reachable commands; do
not add arbitrary tools to that path.

## Configuration reference

| Variable | Applies to | Default | Notes |
| --- | --- | --- | --- |
| `LG_DOMAIN` | compose | `localhost` | Public DNS name. Caddy serves the web UI for it, and central's `LG_TUNNEL_URL` becomes `https://LG_DOMAIN:8443`. |
| `PORT` | central | `8080` | HTTP listener inside the container. |
| `LG_DB_PATH` | central | `data/lookingglass.redb` | redb database path. In containers, mount this under a volume. |
| `LG_FILES_DIR` | central | `data/files` | Local-node downloadable test files root. |
| `LG_SETUP_TOKEN` | central | generated file | Optional first-run setup token. If unset, central writes `setup-token` beside `LG_DB_PATH`. |
| `LG_TRUSTED_PROXIES` | central | empty | Comma-separated proxy IPs trusted for client identity and TLS attestation. Empty fails closed for the admin and proxy-enrollment TLS checks. |
| `LG_CENTRAL_URL` | central | tunnel origin, when the tunnel is running | Plain HTTPS origin agents enroll at (`/api/enroll`); no path/query/fragment. With every `LG_CENTRAL_*` variable unset and the tunnel running, it is `LG_TUNNEL_URL`; otherwise it defaults to `https://localhost`. |
| `LG_TUNNEL_URL` | central | `https://localhost:8443` | Plain HTTPS tunnel origin embedded in agent install commands. It needs an explicit port (`https://host:8443`); no path/query/fragment and no bracketed IPv6 literal. Central refuses to start on an invalid value. |
| `LG_CENTRAL_CERT` | central | tunnel pin | PEM certificate the HTTPS enrollment proxy presents, for proxy enrollment only. With every `LG_CENTRAL_*` variable unset, install commands carry the tunnel certificate's pin instead. It must match the tunnel certificate, or install-command generation answers 503. |
| `LG_CENTRAL_IDENTITY` | central | unset | Legacy identity material. It cannot replace `LG_CENTRAL_CERT`: set alone, generating an install command fails with 422. |
| `LG_TUNNEL_CERT` / `LG_TUNNEL_KEY` | central | generated | PEM cert/key for the direct agent TLS tunnel listener. With both unset, central generates `tunnel.crt` and `tunnel.key` beside `LG_DB_PATH` on first start and loads them after that. With only one set, the tunnel stays off. |
| `LG_TUNNEL_BIND` | central | `0.0.0.0:8443` | Direct TLS/WebSocket tunnel bind address. |
| `LG_AGENT_INSTALL_SCRIPT_URL` | central | set in release images | HTTPS URL embedded in generated agent install commands. Empty in a source build. |
| `LG_AGENT_INSTALL_SCRIPT_SHA256` | central | set in release images | SHA-256 pin for the installer script. Empty in a source build. |
| `LG_AGENT_URL` | central | set in release images | HTTPS URL for the prebuilt agent binary release asset. Empty in a source build. |
| `LG_AGENT_SHA256` | central | set in release images | SHA-256 pin for the agent binary. Empty in a source build. |
| `LG_EXEC_MAX_CONCURRENT` | central | `8` | Global in-flight diagnostic cap per node. |
| `LG_EXEC_TIMEOUT_SECS` | central | `30` | Per-command timeout. |
| `LG_EXEC_MAX_OUTPUT_KIB` | central | `256` | Total output cap per command. |
| `LG_EXEC_RATE_MAX` | central | `20` | Per-client run attempts per window. |
| `LG_EXEC_RATE_WINDOW_SECS` | central | `60` | Per-client run rate window. |
| `LG_AGENT_CREDENTIAL` | agent | `data/agent-credential.json` | Stored credential path. The installer sets this in the unit. |
| `LG_AGENT_DATA_BIND` | agent | port 443 | Optional bind address override for the HTTPS speed-test data plane, which starts once central assigns a data-plane origin. |
| `LG_AGENT_FILES_DIR` | agent | `data/files` | Remote speedtest files root for the data plane. Relative to the unit's working directory, so an installed agent serves `/var/lib/lookingglass-agent/data/files`. |
| `LG_AGENT_BGP_WRAPPER_DIR` | agent | installer wrapper dir | Scoped directory the agent probes for `birdc`/`vtysh`. |

The `LG_EXEC_*` variables seed the diagnostic limits only on central's first start,
when it creates the database. After that the admin Settings page owns those values,
and changing the variables has no effect.

Installer-only variables include `LG_AGENT_INSTALL_PATH`, `LG_AGENT_STATE_DIR`,
`LG_AGENT_SERVICE_FILE`, `LG_AGENT_USER`, `LG_AGENT_SERVICE_PATH`,
`LG_AGENT_BGP_WRAPPER`, and `LG_AGENT_BGP_DAEMON`. The generated install command
runs the installer under `env -i` with a fixed list of variables, so it drops these;
they apply only when you run `install-agent.sh` yourself. The first entry of
`LG_AGENT_SERVICE_PATH` becomes the BGP wrapper directory, so it must be a dedicated
absolute directory, never a system one such as `/usr/sbin`; the installer refuses
otherwise. `LG_INSTALL_DRY_RUN=1` is for local installer tests only.

## Release image

`.github/workflows/release.yml` builds the Dockerfile and publishes to GHCR on:

- pushed tags matching `v*`;
- manual `workflow_dispatch` when `push=true`.

Publication is ordered, not atomic: the image job pushes to GHCR before the
release-assets job verifies and publishes the installer and agent assets. That job
creates the GitHub Release for the tag and attaches `install-agent.sh`, the agent
binary and `THIRD_PARTY_NOTICES.md`. Do not create the Release by hand first:
`gh release create` fails when one already exists for the tag. The image job waits
for that verification and bakes the tag's asset URLs and the README's SHA-256 pins
into the image as the `LG_AGENT_*` defaults.

Manual dispatch with `push=false` runs the same build path without logging in or
pushing, which is the no-credential dry-run equivalent. Manual dispatch with
`push=true` is a credentialed GHCR publish from the selected ref. Actual GHCR
package visibility, repository permissions, and branch/tag protection are
operator-owned setup.

### Cutting a release

The release job refuses to publish unless the installer and agent it builds match
the SHA-256 pins in this README, so update the README pins before tagging:

1. Build the agent exactly as `release.yml` does: the digest-pinned
   `rust:1.96-bookworm` container running `cargo build --locked --release --package agent`.
   Hash that binary and `scripts/install-agent.sh` with `sha256sum`.
2. Update both release-asset URLs, `LG_INSTALLER_SHA256` and `LG_AGENT_SHA256` in
   this README, and the matching lines in `scripts/check-readme.sh`.
   If dependencies changed, run `python3 scripts/third-party-notices.py`. Run
   `make verify` and commit.
3. Push the `vX.Y.Z` tag.

## Upgrade and rollback

1. Pull the new image: `docker compose pull` with Compose,
   `docker pull ghcr.io/viktorsbaikers/looking-glass:latest` without. To pin a
   version, replace `latest` with a tag such as `v0.1.3` in `compose.yaml` (or the
   `docker run` command).
2. Stop the old central container and back up the data volume (below).
3. With Compose, `docker compose up -d` starts the new image on the same volume.
   Without it, remove the old container (`docker rm looking-glass`), because the start
   command reuses its name, and start the new image with the same data volume,
   certificate/key mounts and environment. A release image brings its own agent
   download settings. If you set them yourself, then
   in the same step, switch `LG_AGENT_URL`, `LG_AGENT_SHA256`,
   `LG_AGENT_INSTALL_SCRIPT_URL` and `LG_AGENT_INSTALL_SCRIPT_SHA256` to the new
   release's agent and installer assets. A new central must never hand out a
   previous-release agent.
4. Verify `/health`, admin login, public location list, and one local diagnostic.

A deployment that already sets `LG_TUNNEL_CERT`, `LG_TUNNEL_KEY`, `LG_CENTRAL_URL`
and `LG_CENTRAL_CERT` behaves as before after the upgrade: central uses those files,
generates no key, and keeps enrollment on the proxy.

The redb file holds the session signing key and live sessions, so the backup goes
into a new root-owned 0700 directory; `cp -a` keeps the volume's own owner and modes
inside it. The copy lands in `looking-glass-data-backup/data/`. The command refuses
to run while `looking-glass-data-backup` exists, so move an earlier backup aside first.
With Compose, stop central with `docker compose stop central`, and use the volume name
`docker volume ls` shows for `central-data` (`looking-glass_central-data` in a
`looking-glass` directory) in place of `looking-glass-data`:

```sh
docker stop looking-glass
docker run --rm -v looking-glass-data:/data -v "$PWD":/backup alpine \
  sh -c 'umask 077 && mkdir /backup/looking-glass-data-backup && cp -a /data /backup/looking-glass-data-backup/'
```

Rollback needs the backup. A newer release migrates the database when it first opens
it, and an older release started on the migrated volume can lock every administrator
out. To roll back, stop and remove the new container, restore the pre-upgrade copy
into the volume, then start the previous release by its tag (for example
`ghcr.io/viktorsbaikers/looking-glass:v0.1.3`). Changes made after the
upgrade are lost. `cp -a` keeps the backup's modes, and `chown -R 999:999 /data`
gives the files back to the container user: on a macOS
Docker host (Docker Desktop, OrbStack) the copy in `$PWD` loses the uid 999
ownership, and the previous image then cannot open its database:

```sh
docker stop looking-glass && docker rm looking-glass
docker run --rm -v looking-glass-data:/data -v "$PWD":/backup alpine \
  sh -c 'find /data -mindepth 1 -delete && cp -a /backup/looking-glass-data-backup/data/. /data/ && chown -R 999:999 /data'
```

Upgrade central before its agents. A new agent serves its speed-test data plane only
once central assigns it an origin, which an older central never does. After the
upgrade, re-save every location whose data-plane origin is `http://` or names a port
other than 443 as `https://host`: the agent now serves it over HTTPS on port 443.

An agent whose stored `tunnel_url` has no explicit port, or uses a bracketed IPv6
literal, exits with status 1 at startup. Set `tunnel_url` in its credential file
(`/var/lib/lookingglass-agent/agent-credential.json` by default) to
`https://host:port`, or re-enroll the node.

For remote agents, keep the previous agent release asset available until the central
upgrade is accepted. To roll an agent back to a previous release, re-enroll it from a
previous-release central, or reinstall it with the previous release's install command
(its agent URL/checksum and `LG_CENTRAL_FP`) and a fresh `LG_ENROLL_TOKEN`, because an
enrollment token is single-use. Then run `sudo systemctl restart lookingglass-agent`:
an older installer does not restart a running agent, so the newer agent keeps running
until you do. Do not start a previous-release agent binary on a
credential a new agent has already used: the new agent repins it to central's public
key, which an older agent cannot check. A previous-release agent that meets a new
central, or a repinned credential, fails with
`central identity does not match the pinned fingerprint`.
That is version skew between central and agent, not an impostor.

## Screenshots

Captured from the SPA against the e2e fixture API (`frontend/tests/fixture-server.mjs`),
so every location, address and ASN is sample data from documentation ranges. The
visitor address, administrator usernames and the enrollment command are blurred.
The tour video is built from clips of the same fixture UI.

| Screen | Dark | Light |
| --- | --- | --- |
| Diagnostics | ![Diagnostics, dark](docs/screenshots/dark/diagnostics.png) | ![Diagnostics, light](docs/screenshots/light/diagnostics.png) |
| First-run installer | ![Installer, dark](docs/screenshots/dark/installer.png) | ![Installer, light](docs/screenshots/light/installer.png) |
| Locations | ![Locations, dark](docs/screenshots/dark/admin-locations.png) | ![Locations, light](docs/screenshots/light/admin-locations.png) |
| Location editor | ![Location editor, dark](docs/screenshots/dark/location-editor.png) | ![Location editor, light](docs/screenshots/light/location-editor.png) |
| Agent enrollment | ![Agent enrollment, dark](docs/screenshots/dark/enrollment.png) | ![Agent enrollment, light](docs/screenshots/light/enrollment.png) |
| Settings | ![Settings, dark](docs/screenshots/dark/settings.png) | ![Settings, light](docs/screenshots/light/settings.png) |
| Administrators | ![Administrators, dark](docs/screenshots/dark/administrators.png) | ![Administrators, light](docs/screenshots/light/administrators.png) |

## Develop and verify

The central binary embeds `frontend/build`, so build the SPA before Rust checks:

```sh
cd frontend && npm ci && npm run build && cd ..
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --all --locked
cargo build --locked --release
docker build -t looking-glass .
```

`make test` runs vitest, svelte-check and the Rust tests. `make verify` adds Rust
formatting, clippy, Playwright, the repository checks (`scripts/check-readme.sh`,
`scripts/check-release-workflow.sh`, `scripts/third-party-notices.py --check`) and a
release build. CI runs all of that on push and pull request with the release
toolchain (Rust 1.96), plus the Linux-only installer test
(`scripts/test-install-agent.sh`), a Docker image build and the Compose smoke test
(`scripts/test-compose.sh`). Enable branch protection to make a red result block
merges.

Playwright (`npm run test:e2e`, and so `make verify`) runs in Chromium and WebKit.
Install both browsers once, as CI does:

```sh
cd frontend && npx playwright install chromium webkit
```

On a fresh Linux host, add `--with-deps` to install their system libraries too.

## License

Looking Glass is released under the MIT License; see `LICENSE`. The third-party
software it ships with, and each package's license and copyright notices, are listed
in `THIRD_PARTY_NOTICES.md`. The container image carries both files in
`/usr/share/doc/looking-glass/`, and each GitHub Release attaches the notices.
