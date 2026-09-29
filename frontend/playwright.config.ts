import { createHash, X509Certificate } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { defineConfig } from '@playwright/test';
import { APP, FIXTURE, TLS_DIR } from './tests/ports';

const appPort = new URL(APP).port;

// Chromium trusts the fixture's test CA by its key for the HTTPS data plane.
// The fixture writes the CA when it starts; the worker processes that launch
// the browsers load this config after that.
function trustTestCa(): string[] {
	try {
		const ca = new X509Certificate(readFileSync(`${TLS_DIR}/ca.pem`));
		const spki = ca.publicKey.export({ type: 'spki', format: 'der' });
		const pin = createHash('sha256').update(spki).digest('base64');
		return [`--ignore-certificate-errors-spki-list=${pin}`];
	} catch {
		return [];
	}
}

export default defineConfig({
	testDir: './tests',
	globalSetup: './tests/global-setup.ts',
	outputDir: `/tmp/looking-glass-playwright-${appPort}`,
	use: { baseURL: APP },
	projects: [
		{ name: 'chromium', use: { browserName: 'chromium', launchOptions: { args: trustTestCa() } } },
		// WebKit runs the accessibility spec (Safari scrolls focused fields its own
		// way: the sticky Save bar check) and the dialog and drawer tests tagged
		// @webkit (WebKit stops painting under a modal, so a close can get stuck:
		// F-264, F-336). The rest of those specs stays Chromium-only.
		{
			name: 'webkit',
			testMatch: ['a11y.e2e.spec.ts', 'administrators.e2e.spec.ts', 'shell.e2e.spec.ts'],
			grep: /a11y\.e2e\.spec\.ts|@webkit/,
			use: { browserName: 'webkit' }
		}
	],
	webServer: [
		{
			command: 'node tests/fixture-server.mjs',
			url: FIXTURE,
			reuseExistingServer: false,
			env: { E2E_TLS_DIR: TLS_DIR }
		},
		{
			command: `npm run dev -- --config tests/vite-e2e.config.ts --host 127.0.0.1 --port ${appPort} --strictPort`,
			url: APP,
			reuseExistingServer: false
		}
	]
});
