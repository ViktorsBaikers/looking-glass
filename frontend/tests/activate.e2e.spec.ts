import { APP, FIXTURE } from './ports';
import { expect, test } from '@playwright/test';

// Activation page (issue #3): a Pending administrator sets a password from a
// one-time link; expired or used links explain themselves.

const NO_LONGER_VALID = 'This activation link is no longer valid — ask a peer for a new one.';

test('activate.e2e sets a password once, routes to sign-in, and the link then dies', async ({
	page
}) => {
	const fixtureId = `activate-valid-${crypto.randomUUID()}`;
	await page.setExtraHTTPHeaders({ 'x-looking-glass-fixture': fixtureId });

	await page.goto(`${APP}/activate/fixture-activation-token`);
	await expect(page.getByText('dana', { exact: true })).toBeVisible();

	// Too short is refused client-side; the alert shows without submitting.
	await page.getByLabel('Password', { exact: true }).fill('short');
	await page.getByLabel('Confirm password').fill('short');
	await expect(page.getByText('At least 12 characters.')).toBeVisible();
	await expect(page.getByRole('button', { name: 'Set password' })).toBeDisabled();

	await page.getByLabel('Password', { exact: true }).fill('dana-activated-password');
	await page.getByLabel('Confirm password').fill('dana-activated-mismatch');
	await expect(page.getByText('Passwords do not match.')).toBeVisible();
	await page.getByLabel('Confirm password').fill('dana-activated-password');
	await page.getByRole('button', { name: 'Set password' }).click();
	await page.waitForURL(`${APP}/login`);

	// The activated peer can sign in with the chosen password.
	await page.getByLabel('Username').fill('dana');
	await page.getByLabel('Password', { exact: true }).fill('dana-activated-password');
	await page.getByRole('button', { name: 'Sign in' }).click();
	await page.waitForURL(`${APP}/admin`);

	// Single use: the same link is gone.
	await page.goto(`${APP}/activate/fixture-activation-token`);
	await expect(page.getByText(NO_LONGER_VALID)).toBeVisible();
});

test('activate.e2e shows the no-longer-valid message for an expired link', async ({
	page,
	request
}) => {
	const fixtureId = `activate-expired-${crypto.randomUUID()}`;
	await page.setExtraHTTPHeaders({ 'x-looking-glass-fixture': fixtureId });

	await page.goto(`${APP}/activate/fixture-expired-token`);
	await expect(page.getByText(NO_LONGER_VALID)).toBeVisible();
	await expect(page.getByLabel('Password')).toHaveCount(0);

	// Posting to a dead link is refused too.
	const post = await request.post(`${FIXTURE}/api/activate/fixture-expired-token`, {
		headers: { 'x-looking-glass-fixture': fixtureId },
		data: { password: 'long-enough-password' }
	});
	expect(post.status()).toBe(410);
});
