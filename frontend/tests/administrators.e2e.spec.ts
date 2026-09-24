import { APP, FIXTURE } from './ports';
import { expect, test, type Page } from '@playwright/test';

// Administrators page (issue #3): peer list with badges, one-time activation
// links, regenerate, remove guard rails, and password change — all through
// roles and visible text against the hermetic fixture server.

async function signIn(page: Page, fixtureId: string) {
	await page.setExtraHTTPHeaders({ 'x-looking-glass-fixture': fixtureId });
	await page.goto(`${APP}/login`);
	await page.getByLabel('Username').fill('brooke');
	await page.getByLabel('Password').fill('fixture-password');
	await page.getByRole('button', { name: 'Sign in' }).click();
	await page.waitForURL(`${APP}/admin`);
	await page.goto(`${APP}/admin/administrators`);
	await expect(page.getByRole('heading', { name: 'Administrators' })).toBeVisible();
}

function row(page: Page, username: string) {
	return page.getByRole('listitem').filter({ hasText: username });
}

test('administrators.e2e lists peers with YOU / ACTIVE / PENDING badges, date added and account count', async ({
	page
}) => {
	await signIn(page, `admins-list-${crypto.randomUUID()}`);

	await expect(page.getByText('3 accounts')).toBeVisible();

	const brooke = row(page, 'brooke');
	await expect(brooke.getByText('You', { exact: true })).toBeVisible();
	await expect(brooke.getByText('Active', { exact: true })).toBeVisible();
	await expect(brooke.getByText(/Added \d{1,2}\/\d{1,2}\/\d{4}/)).toBeVisible();
	await expect(brooke.getByRole('button', { name: 'Regenerate' })).toHaveCount(0);

	const dana = row(page, 'dana');
	await expect(dana.getByText('Pending', { exact: true })).toBeVisible();
	await expect(dana.getByText(/Added \d{1,2}\/\d{1,2}\/\d{4}/)).toBeVisible();
	await expect(dana.getByRole('button', { name: 'Regenerate' })).toBeVisible();

	await expect(row(page, 'erin').getByText('Pending', { exact: true })).toBeVisible();
});

test('administrators.e2e create pending shows the activation link once and closing discards it', async ({
	page
}) => {
	await signIn(page, `admins-create-${crypto.randomUUID()}`);

	await page.getByLabel('Username').fill('casey');
	await page.getByRole('button', { name: 'Create pending' }).click();

	const dialog = page.getByRole('dialog');
	await expect(dialog).toBeVisible();
	const link = dialog.getByText(/\/activate\//);
	await expect(link).toBeVisible();
	const linkText = (await link.textContent()) ?? '';
	expect(linkText).toMatch(/\/activate\/act-/);
	await expect(dialog.getByRole('button', { name: /Copy activation link/ })).toBeVisible();

	await dialog.getByRole('button', { name: 'Close dialog' }).click();
	await expect(dialog).toHaveCount(0);
	// Closing discards the link: it is never shown again.
	await expect(page.getByText(linkText)).toHaveCount(0);

	await expect(page.getByText('4 accounts')).toBeVisible();
	const casey = row(page, 'casey');
	await expect(casey.getByText('Pending', { exact: true })).toBeVisible();
	await expect(casey.getByRole('button', { name: 'Regenerate' })).toBeVisible();
});

test('administrators.e2e regenerate replaces a pending link and kills the old one', async ({
	page,
	request
}) => {
	const fixtureId = `admins-regen-${crypto.randomUUID()}`;
	await signIn(page, fixtureId);

	const oldLink = await request.get(`${FIXTURE}/api/activate/fixture-activation-token`, {
		headers: { 'x-looking-glass-fixture': fixtureId }
	});
	expect(oldLink.status()).toBe(200);

	await row(page, 'dana').getByRole('button', { name: 'Regenerate' }).click();
	const confirm = page.getByRole('dialog').filter({ hasText: 'Regenerate activation link?' });
	await confirm.getByRole('button', { name: 'Regenerate' }).click();
	await expect(confirm).toHaveCount(0);

	const dialog = page.getByRole('dialog');
	await expect(dialog).toBeVisible();
	const link = dialog.getByText(/\/activate\//);
	await expect(link).toBeVisible();
	const linkText = (await link.textContent()) ?? '';
	expect(linkText).toMatch(/\/activate\/act-/);
	expect(linkText).not.toContain('fixture-activation-token');
	await dialog.getByRole('button', { name: 'Close dialog' }).click();

	// The previous link is invalidated immediately.
	const dead = await request.get(`${FIXTURE}/api/activate/fixture-activation-token`, {
		headers: { 'x-looking-glass-fixture': fixtureId }
	});
	expect(dead.status()).toBe(410);
});

test('administrators.e2e remove deletes a peer and refuses self-removal with the server message', async ({
	page
}) => {
	await signIn(page, `admins-remove-${crypto.randomUUID()}`);

	await row(page, 'erin').getByRole('button', { name: 'Remove' }).click();
	let confirm = page.getByRole('dialog');
	await confirm.getByRole('button', { name: 'Remove' }).click();
	await expect(row(page, 'erin')).toHaveCount(0);
	await expect(page.getByText('2 accounts')).toBeVisible();

	// Guard rail: removing yourself is refused and the server message surfaces.
	await row(page, 'brooke').getByRole('button', { name: 'Remove' }).click();
	confirm = page.getByRole('dialog');
	await confirm.getByRole('button', { name: 'Remove' }).click();
	await expect(page.getByText('You cannot remove your own account.')).toBeVisible();
	await expect(row(page, 'brooke')).toBeVisible();
});

test('administrators.e2e change password validates, reports a wrong current password, and signs in with the new one', async ({
	page
}) => {
	await signIn(page, `admins-password-${crypto.randomUUID()}`);

	// Client-side validation before any request.
	await page.getByLabel('Current password', { exact: true }).fill('fixture-password');
	await page.getByLabel('New password', { exact: true }).fill('short');
	await page.getByLabel('Confirm new password').fill('short');
	await page.getByRole('button', { name: 'Change password' }).click();
	await expect(page.getByText('At least 12 characters.')).toBeVisible();

	await page.getByLabel('New password', { exact: true }).fill('replacement-password');
	await page.getByLabel('Confirm new password').fill('different-password');
	await page.getByRole('button', { name: 'Change password' }).click();
	await expect(page.getByText('Passwords do not match.')).toBeVisible();

	// Wrong current password: server refusal surfaced as a toast.
	await page.getByLabel('Current password', { exact: true }).fill('not-my-password');
	await page.getByLabel('Confirm new password').fill('replacement-password');
	await page.getByRole('button', { name: 'Change password' }).click();
	await expect(page.getByText('The current password is incorrect.')).toBeVisible();

	// Happy path: current session survives, the new password works afterwards.
	await page.getByLabel('Current password', { exact: true }).fill('fixture-password');
	await page.getByRole('button', { name: 'Change password' }).click();
	await expect(page.getByText('Password changed.')).toBeVisible();

	await page.getByRole('button', { name: 'Log out' }).click();
	await page.waitForURL(`${APP}/login`);
	await page.getByLabel('Username').fill('brooke');
	await page.getByLabel('Password', { exact: true }).fill('replacement-password');
	await page.getByRole('button', { name: 'Sign in' }).click();
	await page.waitForURL(`${APP}/admin`);
});
