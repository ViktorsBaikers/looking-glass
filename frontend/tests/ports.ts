// Ports are env-overridable so several Playwright runs can coexist.
export const FIXTURE = `http://127.0.0.1:${process.env.E2E_FIXTURE_PORT ?? 4173}`;
export const APP = `http://127.0.0.1:${process.env.E2E_APP_PORT ?? 4174}`;
