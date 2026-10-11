import { globSync } from 'node:fs';
import { chromium } from '@playwright/test';
import { APP } from './ports';

// `vite dev` reports ready before it has compiled anything, so a worker's first
// test used to pay for the cold compile (and dependency pre-bundle) inside its
// expect timeout. Load every route once, up to network idle, before any test.
export default async function globalSetup() {
	const routes = globSync('**/+page.svelte', { cwd: new URL('../src/routes', import.meta.url) }).map(
		(file) => '/' + file.replace(/\/?\+page\.svelte$/, '').replace(/\[[^\]]+\]/g, 'warmup')
	);
	const browser = await chromium.launch();
	const page = await browser.newPage({ extraHTTPHeaders: { 'x-looking-glass-fixture': 'warmup' } });
	for (const route of routes) await page.goto(`${APP}${route}`, { waitUntil: 'networkidle', timeout: 120_000 });
	await browser.close();
}
