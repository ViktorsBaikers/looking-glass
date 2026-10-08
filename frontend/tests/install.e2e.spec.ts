import { APP, FIXTURE } from './ports';
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

	await page.goto(`${APP}/`);
	const statusResponse = await initialStatus;
	expect(statusResponse.status()).toBe(200);
	expect(await statusResponse.json()).toEqual({ installed: false });
	await expect(page).toHaveURL(`${APP}/install`);
	await expect(page.getByRole('heading', { name: 'Create the first administrator' })).toBeVisible();

	const protectedRoute = await request.get(`${FIXTURE}/api/locations`, {
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
	await page.goto(`${APP}/install`);
	const statusResponse = await initialStatus;
	expect(statusResponse.status()).toBe(200);
	expect(await statusResponse.json()).toEqual({ installed: false });
	await expect(page.getByRole('heading', { name: 'Create the first administrator' })).toBeVisible();
	await expect(page.getByRole('button', { name: 'Create account' })).toBeDisabled();

	// A wrong token gets central's 403 and its message, shown inline.
	await page.getByLabel('Setup token').fill('wrong-setup-token');
	await page.getByLabel('Username').fill('admin');
	await page.getByLabel('Password', { exact: true }).fill('fixture-password');
	await page.getByLabel('Confirm password').fill('fixture-password');
	const refusedSetup = page.waitForResponse(
		(response) => response.url().endsWith('/api/setup') && response.request().method() === 'POST'
	);
	await page.getByRole('button', { name: 'Create account' }).click();
	expect((await refusedSetup).status()).toBe(403);
	await expect(page.getByRole('alert')).toHaveText('A valid first-run setup token is required.');

	await page.getByLabel('Setup token').fill('fixture-setup-token');
	const setupResponse = page.waitForResponse(
		(response) => response.url().endsWith('/api/setup') && response.request().method() === 'POST'
	);
	await page.getByRole('button', { name: 'Create account' }).click();
	expect((await setupResponse).status()).toBe(201);
	await expect(page).toHaveURL(`${APP}/login`);
	await expect(page.getByRole('heading', { name: 'Sign in' })).toBeVisible();
	const unauthenticatedMe = await request.get(`${FIXTURE}/api/admin/me`, {
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
	await expect(page).toHaveURL(`${APP}/login`);
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
	expect(await authenticatedMe.json()).toMatchObject({ username: 'admin' });
	await expect(page).toHaveURL(`${APP}/admin`);
	await expect(page.getByRole('heading', { name: 'Locations' })).toBeVisible();
	await expect(page.getByRole('link', { name: 'Locations' })).toBeVisible();
	await page.getByRole('link', { name: 'Settings' }).click();
	await expect(page).toHaveURL(`${APP}/admin/settings`);
	await page.getByRole('link', { name: 'Locations' }).click();
	await expect(page).toHaveURL(`${APP}/admin`);

	const secondSetup = await request.post(`${FIXTURE}/api/setup`, {
		headers: { 'x-looking-glass-fixture': fixtureId },
		data: {
			setup_token: 'fixture-setup-token',
			username: 'second-admin',
			password: 'another-password'
		}
	});
	// Pins the fixture to central's ApiError::AlreadyInstalled (auth.rs).
	expect(secondSetup.status()).toBe(409);
	expect(await secondSetup.json()).toEqual({
		error: 'already_installed',
		message: 'Setup has already been completed.'
	});

	const closingStatusTraffic: Response[] = [];
	const collectClosingStatus = (response: Response) => {
		if (response.url().endsWith('/api/setup/status') && response.request().method() === 'GET') {
			closingStatusTraffic.push(response);
		}
	};
	page.on('response', collectClosingStatus);
	await page.goto(`${APP}/install`);
	await page.waitForURL(`${APP}/login`);
	await page.waitForLoadState('networkidle');
	page.off('response', collectClosingStatus);

	const closingInstallStatusResponses: Response[] = [];
	for (const response of closingStatusTraffic) {
		if ((await response.request().headerValue('referer')) === `${APP}/install`) {
			closingInstallStatusResponses.push(response);
		}
	}
	expect(closingInstallStatusResponses.length).toBeGreaterThanOrEqual(2);
	for (const response of closingInstallStatusResponses) {
		expect(await response.request().headerValue('x-looking-glass-fixture')).toBe(fixtureId);
		expect(response.status()).toBe(200);
		expect(await response.json()).toEqual({ installed: true });
	}
	await expect(page).toHaveURL(`${APP}/login`);
	await expect(page.getByRole('heading', { name: 'Create the first administrator' })).toHaveCount(0);
});

// F-234: like activate (F-223), the rules describe the field while typing without
// an alert or an invalid state; leaving the field turns them into errors.
test('install.e2e password rules stay a hint until the field is left', async ({ page }) => {
	await page.setExtraHTTPHeaders({ 'x-looking-glass-fixture': `fresh-install-hint-${crypto.randomUUID()}` });
	await page.goto(`${APP}/install`);
	const password = page.getByLabel('Password', { exact: true });
	const confirm = page.getByLabel('Confirm password');
	await password.pressSequentially('a');
	await expect(password).toHaveAccessibleDescription('At least 12 characters.');
	await expect(page.getByRole('alert')).toHaveCount(0);
	await expect(password).not.toHaveAttribute('aria-invalid', 'true');

	await password.press('Tab');
	await expect(confirm).toBeFocused();
	await expect(page.getByRole('alert')).toHaveText('At least 12 characters.');
	await expect(password).toHaveAttribute('aria-invalid', 'true');

	await password.fill('a-long-enough-password');
	await confirm.pressSequentially('a-long');
	await expect(confirm).toHaveAccessibleDescription('Passwords do not match.');
	await expect(page.getByRole('alert')).toHaveCount(0);
	await expect(confirm).not.toHaveAttribute('aria-invalid', 'true');
	await confirm.blur();
	await expect(page.getByRole('alert')).toHaveText('Passwords do not match.');
	await expect(confirm).toHaveAttribute('aria-invalid', 'true');
});

// The install form blocks these payloads client-side, so they go straight to
// the fixture. Pins central's installer.rs check order and bodies: the Json
// extractor, then the token, then garde's first error with no field path
// (installer.rs first_message), then the username rule. The administrators
// create shares the username rule but uses admin_api's `field: ` prefix.
test('install.e2e answers central setup validation bodies in central order', async ({ request }) => {
	const headers = { 'x-looking-glass-fixture': `fresh-install-${crypto.randomUUID()}` };
	const setup = (data: Record<string, unknown>) => request.post(`${FIXTURE}/api/setup`, { headers, data });
	const token = 'fixture-setup-token';
	const invalid = (message: string) => ({ error: 'invalid_input', message });

	const missingField = await setup({ setup_token: 'wrong-setup-token', password: 'short' });
	expect(missingField.status()).toBe(422);
	const wrongToken = await setup({ setup_token: 'wrong-setup-token', username: '', password: 'short' });
	expect(wrongToken.status()).toBe(403);

	const cases: [Record<string, unknown>, string][] = [
		// garde checks the fields in name order, so password's error comes first.
		[{ setup_token: token, username: '', password: 'short' }, 'length is lower than 12'],
		[{ setup_token: token, username: '', password: 'fixture-password' }, 'length is lower than 1'],
		[{ setup_token: token, username: 'a'.repeat(65), password: 'fixture-password' }, 'length is greater than 64'],
		[{ setup_token: token, username: 'admin', password: 'short' }, 'length is lower than 12'],
		[{ setup_token: token, username: 'admin', password: 'p'.repeat(513) }, 'length is greater than 512'],
		[
			{ setup_token: token, username: 'bad name', password: 'fixture-password' },
			'Username may contain only letters, digits, and . _ -'
		]
	];
	for (const [data, message] of cases) {
		const response = await setup(data);
		expect(response.status()).toBe(422);
		expect(await response.json()).toEqual(invalid(message));
	}

	expect((await setup({ setup_token: token, username: 'admin', password: 'fixture-password' })).status()).toBe(201);
	const login = await request.post(`${FIXTURE}/api/auth/login`, {
		headers,
		data: { username: 'admin', password: 'fixture-password' }
	});
	expect(login.status()).toBe(204);
	const peer = await request.post(`${FIXTURE}/api/admin/administrators`, { headers, data: { username: '' } });
	expect(peer.status()).toBe(422);
	expect(await peer.json()).toEqual(invalid('username: length is lower than 1'));
});

// F-260: as on activate, editing Password after Confirm was left keeps the
// mismatch a hint while typing and shows it as one error when Password is left.
test('install.e2e a password edit after Confirm keeps the mismatch a hint while typing', async ({ page }) => {
	await page.setExtraHTTPHeaders({ 'x-looking-glass-fixture': `fresh-install-cross-${crypto.randomUUID()}` });
	await page.goto(`${APP}/install`);
	const password = page.getByLabel('Password', { exact: true });
	const confirm = page.getByLabel('Confirm password');
	await password.fill('correct-horse-battery');
	await confirm.fill('correct-horse-battery');
	await password.focus();
	await page.evaluate(() => {
		const w = window as unknown as { alerts: string[] };
		w.alerts = [];
		new MutationObserver((records) => {
			for (const record of records)
				for (const node of record.addedNodes)
					if (node instanceof Element)
						for (const el of [node, ...node.querySelectorAll('*')])
							if (el.getAttribute('role') === 'alert') w.alerts.push(el.textContent ?? '');
		}).observe(document.body, { childList: true, subtree: true });
	});
	const alerts = () => page.evaluate(() => (window as unknown as { alerts: string[] }).alerts);

	for (const key of ['End', 'Backspace', 'y', 'Backspace']) await password.press(key);
	await expect(confirm).toHaveAccessibleDescription('Passwords do not match.');
	await expect(confirm).not.toHaveAttribute('aria-invalid', 'true');
	await expect(page.getByRole('alert')).toHaveCount(0);
	expect(await alerts()).toEqual([]);

	await password.press('Tab');
	await expect(page.getByRole('alert')).toHaveText('Passwords do not match.');
	await expect(confirm).toHaveAttribute('aria-invalid', 'true');
	expect(await alerts()).toEqual(['Passwords do not match.']);

	// Confirm's own edit after it was left on a match is a hint while typing too.
	await confirm.evaluate((el: HTMLInputElement) => el.setSelectionRange(el.value.length, el.value.length));
	await confirm.press('Backspace');
	await expect(page.getByRole('alert')).toHaveCount(0);
	await confirm.blur();
	await confirm.focus();
	await page.evaluate(() => ((window as unknown as { alerts: string[] }).alerts = []));
	await confirm.evaluate((el: HTMLInputElement) => el.setSelectionRange(el.value.length, el.value.length));
	await confirm.press('Backspace');
	await expect(confirm).toHaveAccessibleDescription('Passwords do not match.');
	await expect(page.getByRole('alert')).toHaveCount(0);
	expect(await alerts()).toEqual([]);
});
