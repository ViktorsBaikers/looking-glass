// Ports are env-overridable so several Playwright runs can coexist.
export const FIXTURE = `http://127.0.0.1:${process.env.E2E_FIXTURE_PORT ?? 4173}`;
export const APP = `http://127.0.0.1:${process.env.E2E_APP_PORT ?? 4174}`;
// The remote node's HTTPS data plane (Vienna). The fixture serves it with a test
// CA it generates at start into TLS_DIR; the browser trusts that CA's key.
export const DATA_PLANE = `https://127.0.0.1:${process.env.E2E_DATA_PLANE_PORT ?? 4175}`;
export const TLS_DIR = `/tmp/looking-glass-e2e-tls-${new URL(DATA_PLANE).port}`;
