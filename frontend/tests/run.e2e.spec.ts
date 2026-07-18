import { expect, test } from '@playwright/test';

test('run.e2e streams a public diagnostic to completion', async ({ page }) => {
	await page.goto('http://127.0.0.1:4174/');
	await expect(page.getByLabel('Location')).toHaveValue('fra');
	await expect(page.getByLabel('Method')).toHaveValue('ping');
	await page.getByLabel('Target').fill('1.1.1.1');

	const startedAt = performance.now();
	const streamRequest = page.waitForRequest((request) =>
		new URL(request.url()).pathname === '/api/run/stream'
	);
	await page.getByRole('button', { name: 'Run' }).click();
	const request = await streamRequest;
	const stream = new URL(request.url());
	expect(request.resourceType()).toBe('eventsource');
	expect(stream.origin).toBe('http://127.0.0.1:4174');
	expect(stream.pathname).toBe('/api/run/stream');
	expect(stream.searchParams.get('location')).toBe('fra');
	expect(stream.searchParams.get('method')).toBe('ping');
	expect(stream.searchParams.get('target')).toBe('1.1.1.1');
	await expect(page.getByRole('status')).toHaveText('Connecting');
	await expect(page.getByRole('status').locator('svg.animate-spin')).toBeVisible();
	await expect(page.getByRole('button', { name: 'Cancel' })).toBeVisible();
	await expect(page.getByRole('log')).toContainText('64 bytes from 1.1.1.1: icmp_seq=1', {
		timeout: 2_000
	});
	expect(performance.now() - startedAt).toBeLessThan(2_000);
	await expect(page.getByRole('status')).toHaveText('Running');
	await expect(page.getByText('Done', { exact: true })).toHaveCount(0);
	await expect(page.getByText('Ping · 1.1.1.1 · 0.1s')).toHaveCount(0);
	await page.waitForTimeout(50);
	await expect(page.getByRole('status')).toHaveText('Running');
	await expect(page.getByText('Done', { exact: true })).toHaveCount(0);
	await expect(page.getByText('Ping · 1.1.1.1 · 0.1s')).toHaveCount(0);
	await expect(page.getByText('Ping · 1.1.1.1 · 0.1s')).toBeVisible();
	await expect(page.getByText('Done', { exact: true })).toBeVisible();
});
