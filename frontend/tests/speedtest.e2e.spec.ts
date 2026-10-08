import { request as httpRequest } from 'node:http';
import { APP, DATA_PLANE, FIXTURE } from './ports';
import { expect, test } from '@playwright/test';

test.describe('speed tests section', () => {
	test.beforeEach(async ({ page }) => {
		await page.goto(`${APP}/`);
		await expect(page.getByRole('tab', { name: 'Frankfurt (AS64501)' })).toBeVisible();
	});

	test('iperf endpoints card lists standard and reverse commands with copy', async ({ page }) => {
		await expect(page.getByText('iperf endpoints')).toBeVisible();
		await expect(page.getByText('Standard test')).toBeVisible();
		await expect(page.getByText('iperf3 -c 192.0.2.21', { exact: true })).toBeVisible();
		await expect(page.getByText('Reverse test')).toBeVisible();
		await expect(page.getByText('iperf3 -c 192.0.2.21 -R', { exact: true })).toBeVisible();
		await expect(page.getByRole('button', { name: 'Copy 192.0.2.21 standard command' })).toBeVisible();
		await expect(page.getByRole('button', { name: 'Copy 192.0.2.21 reverse command' })).toBeVisible();
	});

	test('test files are listed as direct download links', async ({ page }) => {
		const big = page.getByRole('link', { name: /100 MB test file/ });
		await expect(big).toHaveAttribute('href', '/api/locations/fra/files/fra-file-100m/download');
		const small = page.getByRole('link', { name: /10 MB test file/ });
		await expect(small).toHaveAttribute('href', '/api/locations/fra/files/fra-file-10m/download');
	});

	test('Start speed test measures download then upload', async ({ page }) => {
		test.setTimeout(60_000);

		// Hold the first upload response so the transient busy state is observable
		// even though the local fixture answers almost instantly.
		const { promise: released, resolve: release } = Promise.withResolvers<void>();
		await page.route('**/api/locations/fra/speedtest/upload', async (route) => {
			await released;
			await route.continue();
		});
		await page.getByRole('button', { name: 'Start speed test' }).click();
		await expect(page.getByRole('button', { name: 'Testing…' })).toBeDisabled();

		const progress = page.getByRole('progressbar', { name: 'Speed test progress' });
		await expect(progress).toBeVisible();

		// Reaching the upload sink proves the download phase completed.
		release();

		// Runs to completion: live readouts settle and the button returns.
		await expect(page.getByRole('button', { name: 'Start speed test' })).toBeEnabled({
			timeout: 25_000
		});
		await expect(progress).toHaveAttribute('aria-valuenow', '100');
		const results = page.getByRole('group', { name: 'Speed test results' });
		await expect(results).toContainText(/Download\s*\d+\s*Mbps/);
		await expect(results).toContainText(/Upload\s*\d+\s*Mbps/);
	});

	test('Start speed test measures a remote node through its cross-origin data plane', async ({
		page
	}) => {
		test.setTimeout(60_000);
		await page.getByRole('tab', { name: 'Vienna (AS64500)' }).click();

		const uploadResponse = page.waitForResponse(
			(response) =>
				response.url() === `${DATA_PLANE}/speedtest/upload` && response.request().method() === 'POST'
		);
		await page.getByRole('button', { name: 'Start speed test' }).click();
		expect((await uploadResponse).ok()).toBe(true);

		await expect(page.getByRole('button', { name: 'Start speed test' })).toBeEnabled({
			timeout: 25_000
		});
		await expect(page.getByRole('alert')).toHaveCount(0);
		const results = page.getByRole('group', { name: 'Speed test results' });
		await expect(results).toContainText(/Upload\s*[1-9]\d*\s*Mbps/);
	});

	// An unreachable data plane measured nothing: a failure, not "Download 0 Mbps".
	test('a download lost at the network level is reported as a failure', async ({ page }) => {
		await page.getByRole('tab', { name: 'Vienna (AS64500)' }).click();
		await page.route(`${DATA_PLANE}/**`, (route) => route.abort('failed'));
		await page.getByRole('button', { name: 'Start speed test' }).click();
		await expect(page.getByRole('alert')).toHaveText('Speed test failed — the test file could not be downloaded.');
		await expect(page.getByRole('button', { name: 'Start speed test' })).toBeEnabled();
		const results = page.getByRole('group', { name: 'Speed test results' });
		await expect(results).toContainText(/Download\s*—\s*Mbps/);
	});

	test('an upload lost at the network level does not blame the server', async ({ page }) => {
		test.setTimeout(60_000);
		await page.route('**/api/locations/fra/speedtest/upload', (route) => route.abort('failed'));
		await page.getByRole('button', { name: 'Start speed test' }).click();
		await expect(page.getByRole('alert')).toHaveText('Upload test failed — the upload could not be completed.', {
			timeout: 25_000
		});
		const results = page.getByRole('group', { name: 'Speed test results' });
		await expect(results).toContainText(/Upload\s*—\s*Mbps/);
	});

	test('Speed test is disabled with no test files configured', async ({ page }) => {
		const fixtureId = `no-files-${crypto.randomUUID()}`;
		await page.setExtraHTTPHeaders({ 'x-looking-glass-fixture': fixtureId });
		await page.goto(`${APP}/`);
		await expect(page.getByRole('tab', { name: 'Frankfurt (AS64501)' })).toBeVisible();

		await expect(page.getByText('This location has no test files.')).toBeVisible();
		await expect(page.getByRole('button', { name: 'Start speed test' })).toBeDisabled();
		await expect(page.getByRole('link', { name: /test file/ })).toHaveCount(0);
	});
});

// The remote node's data-plane certificate status (issued, expires, last
// error), as its agent reported it, reaches the admin location editor.
test('the admin location editor shows the remote data-plane certificate', async ({ page }) => {
	await page.setExtraHTTPHeaders({ 'x-looking-glass-fixture': `certificate-${crypto.randomUUID()}` });
	await page.goto(`${APP}/login`);
	await page.getByLabel('Username').fill('brooke');
	await page.getByLabel('Password').fill('fixture-password');
	await page.getByRole('button', { name: 'Sign in' }).click();
	await expect(page).toHaveURL(`${APP}/admin`);

	await page.goto(`${APP}/admin/locations/vie?tab=settings`);
	await expect(page.getByLabel('Data-plane origin (optional)')).toHaveAccessibleDescription(
		'HTTPS certificate issued 2026-09-20 00:00 UTC, expires 2026-09-26 00:00 UTC.'
	);
});

// A speed-test upload cancelled mid-body must not take the fixture server, and so
// every later test in the run, down with it.
test('the fixture keeps serving after an upload is cancelled mid-body', async ({ request }) => {
	const upload = httpRequest(`${FIXTURE}/api/locations/fra/speedtest/upload`, {
		method: 'POST',
		// The fixture's 100 Continue proves its handler is reading the body.
		headers: { 'content-length': 1 << 20, expect: '100-continue' }
	});
	upload.on('error', () => {});
	await new Promise((resolve, reject) => {
		upload.once('continue', resolve);
		upload.once('error', reject);
	});
	await new Promise((resolve) => upload.write(Buffer.alloc(1024), resolve));
	const closed = new Promise((resolve) => upload.once('close', resolve));
	upload.destroy();
	await closed;

	const after = await request.get(`${FIXTURE}/api/visitor`).then(
		(response) => response.status(),
		(error: Error) => error.message
	);
	expect(after).toBe(200);
});
