import { defineConfig } from '@playwright/test';
import { APP, FIXTURE } from './tests/ports';

const appPort = new URL(APP).port;

export default defineConfig({
	testDir: './tests',
	outputDir: `/tmp/looking-glass-playwright-${appPort}`,
	use: {
		baseURL: APP
	},
	webServer: [
		{
			command: 'node tests/fixture-server.mjs',
			url: FIXTURE,
			reuseExistingServer: false
		},
		{
			command: `npm run dev -- --config tests/vite-e2e.config.ts --host 127.0.0.1 --port ${appPort} --strictPort`,
			url: APP,
			reuseExistingServer: false
		}
	]
});
