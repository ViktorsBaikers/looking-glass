import { APP, FIXTURE } from './ports';
import { expect, test } from '@playwright/test';

test.describe('speed tests section', () => {
	test.beforeEach(async ({ page }) => {
		await page.goto(`${APP}/`);
		await expect(page.getByRole('tab', { name: 'Frankfurt (AS64501)' })).toBeVisible();
	});

	test('iperf3 Client card lists standard and reverse commands with copy', async ({ page }) => {
		await expect(page.getByText('iperf3 Client')).toBeVisible();
		await expect(page.getByText('Standard Test')).toBeVisible();
		await expect(page.getByText('iperf3 -c 192.0.2.21', { exact: true })).toBeVisible();
		await expect(page.getByText('Reverse Test')).toBeVisible();
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

	test('Start Speed Test measures download then upload', async ({ page }) => {
		test.setTimeout(60_000);

		// Hold the first upload response so the transient busy state is observable
		// even though the local fixture answers almost instantly.
		const { promise: released, resolve: release } = Promise.withResolvers<void>();
		await page.route('**/api/locations/fra/speedtest/upload', async (route) => {
			await released;
			await route.continue();
		});
		await page.getByRole('button', { name: 'Start Speed Test' }).click();
		await expect(page.getByRole('button', { name: 'Testing…' })).toBeDisabled();

		const progress = page.getByRole('progressbar', { name: 'Speed test progress' });
		await expect(progress).toBeVisible();

		// Reaching the upload sink proves the download phase completed.
		release();

		// Runs to completion: live readouts settle and the button returns.
		await expect(page.getByRole('button', { name: 'Start Speed Test' })).toBeEnabled({
			timeout: 25_000
		});
		await expect(progress).toHaveAttribute('aria-valuenow', '100');
		const results = page.getByRole('group', { name: 'Speed test results' });
		await expect(results).toContainText(/Download\s*\d+\s*Mbps/);
		await expect(results).toContainText(/Upload\s*\d+\s*Mbps/);
	});

	test('Start Speed Test measures a remote node through its cross-origin data plane', async ({
		page
	}) => {
		test.setTimeout(60_000);
		await page.getByRole('tab', { name: 'Vienna (AS64500)' }).click();

		const uploadResponse = page.waitForResponse(
			(response) =>
				response.url() === `${FIXTURE}/speedtest/upload` && response.request().method() === 'POST'
		);
		await page.getByRole('button', { name: 'Start Speed Test' }).click();
		expect((await uploadResponse).ok()).toBe(true);

		await expect(page.getByRole('button', { name: 'Start Speed Test' })).toBeEnabled({
			timeout: 25_000
		});
		await expect(page.getByRole('alert')).toHaveCount(0);
		const results = page.getByRole('group', { name: 'Speed test results' });
		await expect(results).toContainText(/Upload\s*[1-9]\d*\s*Mbps/);
	});

	test('Speed test is disabled with no test files configured', async ({ page }) => {
		const fixtureId = `no-files-${crypto.randomUUID()}`;
		await page.setExtraHTTPHeaders({ 'x-looking-glass-fixture': fixtureId });
		await page.goto(`${APP}/`);
		await expect(page.getByRole('tab', { name: 'Frankfurt (AS64501)' })).toBeVisible();

		await expect(page.getByText('No test files configured')).toBeVisible();
		await expect(page.getByRole('button', { name: 'Start Speed Test' })).toBeDisabled();
		await expect(page.getByRole('link', { name: /test file/ })).toHaveCount(0);
	});
});
