#!/usr/bin/env bash
# Smoke test for compose.yaml: checks the resolved config, brings the stack up,
# and proves central trusts HTTPS through Caddy but not a forged
# X-Forwarded-Proto from any other container.
# Usage: bash scripts/test-compose.sh [image]
# Runs central from the given image, or builds looking-glass:smoke from the
# repository first, so it never pulls or retags the published image.
# Publishes host ports 80, 443 and 8443 while it runs.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
PROJECT="lg-compose-smoke-$$"
IMAGE=${1:-}
if [ -z "$IMAGE" ]; then
  IMAGE=looking-glass:smoke
  docker build -t "$IMAGE" "$ROOT"
fi
TMP=$(mktemp -d)
: >"$TMP/empty.env"
printf 'services:\n  central:\n    image: %s\n' "$IMAGE" >"$TMP/override.yaml"

# An empty env file keeps a developer's .env from changing what is tested.
compose() {
  docker compose --project-directory "$ROOT" -f "$ROOT/compose.yaml" \
    -f "$TMP/override.yaml" --env-file "$TMP/empty.env" -p "$PROJECT" "$@"
}
fail() {
  echo "FAIL: $*" >&2
  exit 1
}
cleanup() {
  local status=$?
  if [ "$status" -ne 0 ]; then
    compose logs --no-color >&2 || true
  fi
  compose down -v --remove-orphans >/dev/null 2>&1 || true
  rm -rf "$TMP"
  exit "$status"
}
trap cleanup EXIT

# Resolved config for the default domain and for a public one.
unset LG_DOMAIN
compose config -q
compose config --format json >"$TMP/default.json"
LG_DOMAIN=lg.example.net compose config --format json >"$TMP/public.json"
python3 - "$TMP/default.json" "$TMP/public.json" <<'PY'
import ipaddress, json, re, sys

def check(path, domain):
    cfg = json.load(open(path))
    services = cfg["services"]
    assert set(services) == {"central", "caddy"}, sorted(services)
    central, caddy = services["central"], services["caddy"]

    assert re.fullmatch(r"caddy:\d+\.\d+(\.\d+)?", caddy["image"]), caddy["image"]

    ipam = cfg["networks"]["lg"]["ipam"]["config"][0]
    subnet = ipaddress.ip_network(ipam["subnet"])
    ip_range = ipaddress.ip_network(ipam["ip_range"])
    caddy_ip = ipaddress.ip_address(caddy["networks"]["lg"]["ipv4_address"])
    central_ip = ipaddress.ip_address(central["networks"]["lg"]["ipv4_address"])
    for ip in (caddy_ip, central_ip):
        assert ip in subnet and ip not in ip_range, (ip, subnet, ip_range)

    env = central["environment"]
    assert env["LG_TRUSTED_PROXIES"] == str(caddy_ip), env
    assert env["LG_TUNNEL_URL"] == f"https://{domain}:8443", env
    assert not [k for k in env if k.startswith("LG_CENTRAL_")], env
    assert caddy["command"] == [
        "caddy", "reverse-proxy", "--from", domain, "--to", "central:8080"
    ], caddy["command"]

    def published(service):
        return {(p["target"], p.get("published")) for p in service.get("ports", [])}
    assert published(caddy) == {(80, "80"), (443, "443")}, published(caddy)
    assert published(central) == {(8443, "8443")}, published(central)

    def data_volume(service):
        return [v["source"] for v in service["volumes"]
                if v["type"] == "volume" and v["target"] == "/data"]
    assert data_volume(central) == ["central-data"], central["volumes"]
    assert data_volume(caddy) == ["caddy-data"], caddy["volumes"]
    assert {"central-data", "caddy-data"} <= set(cfg["volumes"]), cfg["volumes"]

check(sys.argv[1], "localhost")
check(sys.argv[2], "lg.example.net")
PY
echo "ok: compose config (default localhost, LG_DOMAIN=lg.example.net)"

LG_DOMAIN=localhost compose up -d --wait --wait-timeout 300
echo "ok: central and caddy healthy"

# Via a file: under pipefail, grep -q quitting early would fail the pipe.
compose logs --no-color central >"$TMP/central.log"
grep -q 'tunnel identity generated' "$TMP/central.log" \
  || fail "central did not log a generated tunnel identity"
echo "ok: central generated its tunnel identity"

# Agents enroll on the tunnel port; an unknown token is refused, not unrouted.
status=$(curl -ksS -o "$TMP/enroll.json" -w '%{http_code}' -X POST \
  -H 'Content-Type: application/json' --data '{"protocol_version":1,"token":"bogus"}' \
  https://localhost:8443/api/enroll)
[ "$status" = 401 ] || fail "POST :8443/api/enroll answered $status: $(cat "$TMP/enroll.json")"
grep -q '"error":"unauthorized"' "$TMP/enroll.json" || fail "unexpected enroll body: $(cat "$TMP/enroll.json")"
echo "ok: POST https://localhost:8443/api/enroll (bogus token) -> 401 $(cat "$TMP/enroll.json")"

status=$(curl -ksS --resolve localhost:443:127.0.0.1 -o "$TMP/status.json" -w '%{http_code}' \
  https://localhost/api/setup/status)
[ "$status" = 200 ] || fail "GET /api/setup/status via Caddy answered $status"
grep -q '"installed":false' "$TMP/status.json" || fail "unexpected status body: $(cat "$TMP/status.json")"
echo "ok: GET https://localhost/api/setup/status -> 200 $(cat "$TMP/status.json")"

token=$(compose exec -T central cat /data/setup-token | tr -d '\r\n')
[ -n "$token" ] || fail "empty setup token"
setup_body=$(printf '{"setup_token":"%s","username":"smoke","password":"compose-smoke-password"}' "$token")

# Forged first, with the real token: if central trusted this header, it would
# create the admin and the Caddy request below could no longer answer 201.
caddy_image=$(compose config --images | grep '^caddy:')
forged=$(docker run --rm --network "${PROJECT}_lg" "$caddy_image" \
  curl -sS -w '\n%{http_code}' -X POST \
    -H 'X-Forwarded-Proto: https' -H 'Content-Type: application/json' \
    --data "$setup_body" http://central:8080/api/setup)
forged_status=${forged##*$'\n'}
[ "$forged_status" = 403 ] || fail "forged X-Forwarded-Proto answered $forged_status: $forged"
grep -q '"error":"insecure_transport"' <<<"$forged" || fail "forged request not refused as insecure_transport: $forged"
echo "ok: forged X-Forwarded-Proto to central:8080 -> 403 insecure_transport"

# Origin makes central run its same-origin check, which needs Caddy to keep the
# Host header and say https.
status=$(curl -ksS --resolve localhost:443:127.0.0.1 -o "$TMP/setup.json" -w '%{http_code}' \
  -X POST -H 'Origin: https://localhost' -H 'Content-Type: application/json' \
  --data "$setup_body" https://localhost/api/setup)
[ "$status" = 201 ] || fail "POST /api/setup via Caddy answered $status: $(cat "$TMP/setup.json")"
echo "ok: POST https://localhost/api/setup -> 201"

echo "compose smoke test passed"
