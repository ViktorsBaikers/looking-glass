import { expect, test, type Response } from '@playwright/test';

test('install.e2e redirects a fresh public entry to the installer after its bound setup probe', async ({
	page,
	request
}) => {
	const fixtureId = `fresh-install-${crypto.randomUUID()}`;
	await page.setExtraHTTPHeaders({ 'x-looking-glass-fixture': fixtureId });
	const initialStatus = page.waitForResponse(
		(response) =>
			response.url().endsWith('/api/setup/status') &&
			response.request().method() === 'GET' &&
			response.request().headers()['x-looking-glass-fixture'] === fixtureId
	);

	await page.goto('http://127.0.0.1:4174/');
	const statusResponse = await initialStatus;
	expect(statusResponse.status()).toBe(200);
	expect(await statusResponse.json()).toEqual({ installed: false });
	await expect(page).toHaveURL('http://127.0.0.1:4174/install');
	await expect(page.getByRole('heading', { name: 'Create the admin account' })).toBeVisible();

	const protectedRoute = await request.get('http://127.0.0.1:4173/api/locations', {
		headers: { 'x-looking-glass-fixture': fixtureId }
	});
	expect(protectedRoute.status()).toBe(403);
	expect(await protectedRoute.json()).toMatchObject({ error: 'setup_required' });
});

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
	const unauthenticatedMe = await request.get('http://127.0.0.1:4173/api/admin/me', {
		headers: { 'x-looking-glass-fixture': fixtureId }
	});
	expect(unauthenticatedMe.status()).toBe(401);
	expect(await unauthenticatedMe.json()).toMatchObject({ error: 'unauthorized' });

	await page.getByLabel('Username').fill('admin');
	await page.getByLabel('Password').fill('wrong-password');
	const failedLoginResponse = page.waitForResponse(
		(response) => response.url().endsWith('/api/auth/login') && response.request().method() === 'POST'
	);
	await page.getByRole('button', { name: 'Sign in' }).click();
	expect((await failedLoginResponse).status()).toBe(401);
	await expect(page).toHaveURL('http://127.0.0.1:4174/login');
	await expect(page.getByRole('alert')).toHaveText('Invalid username or password.');

	await page.getByLabel('Password').fill('fixture-password');
	const loginResponse = page.waitForResponse(
		(response) => response.url().endsWith('/api/auth/login') && response.request().method() === 'POST'
	);
	const meResponse = page.waitForResponse(
		(response) => response.url().endsWith('/api/admin/me') && response.request().method() === 'GET'
	);
	await page.getByRole('button', { name: 'Sign in' }).click();
	expect((await loginResponse).status()).toBe(204);
	const authenticatedMe = await meResponse;
	expect(authenticatedMe.status()).toBe(200);
	expect(await authenticatedMe.json()).toEqual({ username: 'admin' });
	await expect(page).toHaveURL('http://127.0.0.1:4174/admin');
	await expect(page.getByRole('heading', { name: 'Locations' })).toBeVisible();
	await expect(page.getByRole('link', { name: 'Locations' })).toBeVisible();
	await page.getByRole('link', { name: 'Settings' }).click();
	await expect(page).toHaveURL('http://127.0.0.1:4174/admin/settings');
	await page.getByRole('link', { name: 'Locations' }).click();
	await expect(page).toHaveURL('http://127.0.0.1:4174/admin');

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
