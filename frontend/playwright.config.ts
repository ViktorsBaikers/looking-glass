import { defineConfig } from '@playwright/test';

export default defineConfig({
	testDir: './tests',
	outputDir: '/tmp/looking-glass-playwright',
	use: {
		baseURL: 'http://127.0.0.1:4173'
	},
	webServer: [
		{
			command: 'node tests/fixture-server.mjs',
			url: 'http://127.0.0.1:4173',
			reuseExistingServer: false
		},
		{
			command: 'npm run dev -- --config tests/vite-e2e.config.ts --host 127.0.0.1 --port 4174',
			url: 'http://127.0.0.1:4174',
			reuseExistingServer: false
		}
	]
});
