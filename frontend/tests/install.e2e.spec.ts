import { expect, test, type Response } from '@playwright/test';

test('install.e2e creates the only admin and closes the installer', async ({ page, request }) => {
	const fixtureId = `fresh-install-${crypto.randomUUID()}`;
	await page.setExtraHTTPHeaders({ 'x-looking-glass-fixture': fixtureId });
	const initialStatus = page.waitForResponse(
		(response) => response.url().endsWith('/api/setup/status') && response.request().method() === 'GET'
	);
	await page.goto('http://127.0.0.1:4174/install');
	const statusResponse = await initialStatus;
	expect(statusResponse.status()).toBe(200);
	expect(await statusResponse.json()).toEqual({ installed: false });
	await expect(page.getByRole('heading', { name: 'Create the admin account' })).toBeVisible();
	await expect(page.getByRole('button', { name: 'Create account' })).toBeDisabled();

	await page.getByLabel('Setup token').fill('fixture-setup-token');
	await page.getByLabel('Username').fill('admin');
	await page.getByLabel('Password', { exact: true }).fill('fixture-password');
	await page.getByLabel('Confirm password').fill('fixture-password');
	const setupResponse = page.waitForResponse(
		(response) => response.url().endsWith('/api/setup') && response.request().method() === 'POST'
	);
	await page.getByRole('button', { name: 'Create account' }).click();
	expect((await setupResponse).status()).toBe(204);
	await expect(page).toHaveURL('http://127.0.0.1:4174/login');
	await expect(page.getByRole('heading', { name: 'Sign in' })).toBeVisible();

	await page.getByLabel('Username').fill('admin');
	await page.getByLabel('Password').fill('fixture-password');
	const loginResponse = page.waitForResponse(
		(response) => response.url().endsWith('/api/auth/login') && response.request().method() === 'POST'
	);
	await page.getByRole('button', { name: 'Sign in' }).click();
	expect((await loginResponse).status()).toBe(204);
	await expect(page).toHaveURL('http://127.0.0.1:4174/');

	const secondSetup = await request.post('http://127.0.0.1:4173/api/setup', {
		headers: { 'x-looking-glass-fixture': fixtureId },
		data: {
			setup_token: 'fixture-setup-token',
			username: 'second-admin',
			password: 'another-password'
		}
	});
	expect(secondSetup.status()).toBe(409);
	expect(await secondSetup.json()).toEqual({
		error: 'already_installed',
		message: 'Setup already completed.'
	});

	const closingStatusTraffic: Response[] = [];
	const collectClosingStatus = (response: Response) => {
		if (response.url().endsWith('/api/setup/status') && response.request().method() === 'GET') {
			closingStatusTraffic.push(response);
		}
	};
	page.on('response', collectClosingStatus);
	await page.goto('http://127.0.0.1:4174/install');
	await page.waitForURL('http://127.0.0.1:4174/login');
	await page.waitForLoadState('networkidle');
	page.off('response', collectClosingStatus);

	const closingInstallStatusResponses: Response[] = [];
	for (const response of closingStatusTraffic) {
		if ((await response.request().headerValue('referer')) === 'http://127.0.0.1:4174/install') {
			closingInstallStatusResponses.push(response);
		}
	}
	expect(closingInstallStatusResponses.length).toBeGreaterThanOrEqual(2);
	for (const response of closingInstallStatusResponses) {
		expect(await response.request().headerValue('x-looking-glass-fixture')).toBe(fixtureId);
		expect(response.status()).toBe(200);
		expect(await response.json()).toEqual({ installed: true });
	}
	await expect(page).toHaveURL('http://127.0.0.1:4174/login');
	await expect(page.getByRole('heading', { name: 'Create the admin account' })).toHaveCount(0);
});
