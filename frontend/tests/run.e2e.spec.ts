import { APP } from './ports';
import { expect, test, type Page } from '@playwright/test';

const metricsGroup = (page: Page) => page.getByRole('group', { name: 'Run metrics' });

async function selectMethod(page: Page, label: string) {
	await page.getByRole('combobox', { name: 'Method' }).click();
	await page.getByRole('option', { name: label, exact: true }).click();
}

async function run(page: Page, target: string) {
	await page.getByLabel('Target').fill(target);
	await page.getByRole('button', { name: 'Run Diagnostic' }).click();
}

test.describe('diagnostic runs', () => {
	test.beforeEach(async ({ page }) => {
		await page.goto(`${APP}/`);
		await expect(page.getByRole('tab', { name: 'Frankfurt (AS64501)' })).toBeVisible();
	});

	test('runs a ping: streams, completes, and derives metric cards', async ({ page }) => {
		const streamRequest = page.waitForRequest(
			(request) => new URL(request.url()).pathname === '/api/run/stream'
		);
		await run(page, '1.1.1.1');

		const request = await streamRequest;
		expect(request.resourceType()).toBe('eventsource');
		const stream = new URL(request.url());
		expect(stream.origin).toBe(APP);
		expect(stream.searchParams.get('location')).toBe('fra');
		expect(stream.searchParams.get('method')).toBe('ping');
		expect(stream.searchParams.get('target')).toBe('1.1.1.1');

		await expect(page.getByRole('log')).toContainText('64 bytes from 1.1.1.1: icmp_seq=1');
		await expect(page.getByRole('status')).toHaveText('Completed');
		await expect(page.getByText('Frankfurt ~ ping')).toBeVisible();
		await expect(page.getByRole('button', { name: 'Run Diagnostic' })).toBeEnabled();

		// Metric cards derived from the finished output.
		const metrics = metricsGroup(page);
		await expect(metrics).toBeVisible();
		await expect(metrics.getByText('Latency', { exact: true })).toBeVisible();
		await expect(metrics.getByText('12.1', { exact: true })).toBeVisible();
		await expect(metrics.getByText('Packet loss', { exact: true })).toBeVisible();
		await expect(metrics.getByText('0/4', { exact: true })).toBeVisible();
		await expect(metrics.getByText('Jitter', { exact: true })).toBeVisible();
		await expect(metrics.getByText('0.2', { exact: true })).toBeVisible();
		await expect(metrics.getByText('TTL', { exact: true })).toBeVisible();
		await expect(metrics.getByText('56', { exact: true })).toBeVisible();

		// Each card explains itself through its info tooltip. Zag's pointer
		// tracking needs a real two-step mouse move; a teleport hover won't open it.
		const about = page.getByRole('button', { name: 'About Latency' });
		await about.scrollIntoViewIfNeeded();
		const box = await about.boundingBox();
		if (!box) throw new Error('About Latency trigger has no bounding box');
		await page.mouse.move(box.x + box.width / 2 - 40, box.y + box.height / 2 - 40);
		await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
		await expect(about).toHaveAttribute('data-state', 'open');
		await expect(page.getByText('Average round-trip time across all replies.')).toBeVisible();
	});

	test('Run Diagnostic morphs to Cancel and cancelling ends the run', async ({ page }) => {
		await run(page, 'slow.test');
		const cancel = page.getByRole('button', { name: 'Cancel' });
		await expect(cancel).toBeVisible();
		await expect(page.getByRole('status')).toHaveText('Streaming');

		await cancel.click();
		await expect(page.getByRole('status')).toHaveText('Canceled');
		await expect(page.getByRole('log')).toContainText('Run canceled.');
		await expect(page.getByRole('button', { name: 'Run Diagnostic' })).toBeEnabled();
		// No metric cards for a run that never finished.
		await expect(metricsGroup(page)).toHaveCount(0);
	});

	test('a refused run shows a friendly failure', async ({ page }) => {
		await run(page, 'fail.test');
		await expect(page.getByRole('status')).toHaveText('Failed');
		await expect(page.getByRole('log')).toContainText('The node refused the run.');
	});

	test('an mtr run renders the hop table and final-hop metrics', async ({ page }) => {
		await selectMethod(page, 'MTR');
		await run(page, '1.1.1.1');

		await expect(page.getByRole('status')).toHaveText('Completed');
		await expect(page.getByText('Frankfurt ~ mtr')).toBeVisible();
		const table = page.getByRole('table');
		await expect(table).toBeVisible();
		await expect(table.getByRole('cell', { name: '192.0.2.1' })).toBeVisible();
		await expect(table.getByRole('cell', { name: '1.1.1.1' })).toBeVisible();

		const metrics = metricsGroup(page);
		await expect(metrics.getByText('12.2', { exact: true })).toBeVisible();
		await expect(metrics.getByText('final hop avg', { exact: true })).toBeVisible();
		await expect(metrics.getByText('Hops', { exact: true })).toBeVisible();
		await expect(metrics.getByText('3', { exact: true })).toBeVisible();
	});

	test('a traceroute run shows only the hop-count card', async ({ page }) => {
		await selectMethod(page, 'Traceroute');
		await run(page, '1.1.1.1');

		await expect(page.getByRole('status')).toHaveText('Completed');
		const metrics = metricsGroup(page);
		await expect(metrics).toBeVisible();
		await expect(metrics.getByText('Hops', { exact: true })).toBeVisible();
		await expect(metrics.getByText('3', { exact: true })).toBeVisible();
		await expect(metrics.locator(':scope > div')).toHaveCount(1);
	});

	test('a BGP run shows no metric cards', async ({ page }) => {
		await page.getByRole('tab', { name: 'Vienna (AS64500)' }).click();
		await selectMethod(page, 'BGP');
		await run(page, '8.8.8.0/24');

		await expect(page.getByRole('status')).toHaveText('Completed');
		await expect(page.getByRole('log')).toContainText('BGP routing table entry for 8.8.8.0/24');
		await expect(metricsGroup(page)).toHaveCount(0);
	});
});
