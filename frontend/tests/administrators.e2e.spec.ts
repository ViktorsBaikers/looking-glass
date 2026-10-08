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
	await page.getByRole('button', { name: 'Create activation link' }).click();

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

// F-194: central counts password length in UTF-8 bytes (12 to 512); so does the
// change-password form, and it refuses an over-long one without a request.
test('administrators.e2e counts the new password in UTF-8 bytes and refuses an over-long one locally', async ({
	page
}) => {
	await signIn(page, `admins-password-bytes-${crypto.randomUUID()}`);
	const puts: string[] = [];
	page.on('request', (request) => {
		if (request.url().includes('/password')) puts.push(request.url());
	});
	await page.getByLabel('Current password', { exact: true }).fill('fixture-password');
	const next = page.getByLabel('New password', { exact: true });
	const confirm = page.getByLabel('Confirm new password');

	for (const long of ['x'.repeat(513), 'é'.repeat(300)]) {
		await next.fill(long);
		await confirm.fill(long);
		await page.getByRole('button', { name: 'Change password' }).click();
		await expect(page.getByText('At most 512 characters.')).toBeVisible();
	}
	await expect(page.getByText(/count as 2 to 4/)).toBeVisible();
	expect(puts).toEqual([]);

	await next.fill('中中中中');
	await confirm.fill('中中中中');
	await expect(page.getByText(/At (least|most)/)).toHaveCount(0);
});

// F-222: a missing current password or confirmation is reported on submit,
// not silently ignored.
test('administrators.e2e change password names the empty fields on submit', async ({ page }) => {
	await signIn(page, `admins-password-empty-${crypto.randomUUID()}`);
	let puts = 0;
	page.on('request', (request) => {
		if (request.url().includes('/api/admin/me/password')) puts++;
	});
	const current = page.getByLabel('Current password', { exact: true });
	const confirm = page.getByLabel('Confirm new password');
	await page.getByLabel('New password', { exact: true }).fill('a-long-new-password');
	await page.getByRole('button', { name: 'Change password' }).click();
	await expect(page.getByRole('alert').filter({ hasText: 'Enter your current password.' })).toBeVisible();
	await expect(page.getByRole('alert').filter({ hasText: 'Confirm the new password.' })).toBeVisible();
	await expect(current).toHaveAttribute('aria-invalid', 'true');
	await expect(confirm).toHaveAttribute('aria-invalid', 'true');
	await expect(current).toHaveAccessibleDescription('Enter your current password.');

	await current.fill('fixture-password');
	await expect(current).not.toHaveAttribute('aria-invalid', 'true');
	await page.getByRole('button', { name: 'Change password' }).click();
	await expect(page.getByText('Confirm the new password.')).toBeVisible();
	expect(puts).toBe(0);
});

// F-223: the length rule is a hint while typing; it turns into an error when
// the field is left.
test('administrators.e2e new password rules stay a hint until the field is left', async ({ page }) => {
	await signIn(page, `admins-password-hint-${crypto.randomUUID()}`);
	const next = page.getByLabel('New password', { exact: true });
	await next.pressSequentially('a');
	await expect(next).toHaveAccessibleDescription('At least 12 characters.');
	await expect(page.getByRole('alert')).toHaveCount(0);
	await expect(next).not.toHaveAttribute('aria-invalid', 'true');
	await next.press('Tab');
	await expect(page.getByRole('alert')).toHaveText('At least 12 characters.');
	await expect(next).toHaveAttribute('aria-invalid', 'true');
});

// F-170: the create button is disabled once its username is cleared, so after
// the link dialog closes focus goes back to the username field, not <main>.
test('administrators.e2e closing the new link dialog returns focus to the username field', async ({ page }) => {
	await signIn(page, `admins-create-focus-${crypto.randomUUID()}`);
	const username = page.getByLabel('Username');
	for (const [name, close] of [['casey', 'Escape'], ['frank', 'Close dialog']]) {
		await username.fill(name);
		await page.getByRole('button', { name: 'Create activation link' }).click();
		const dialog = page.getByRole('dialog');
		await expect(dialog).toBeVisible();
		await expect.poll(() => page.evaluate(() => !!document.activeElement?.closest('[role=dialog]'))).toBe(true);
		if (close === 'Escape') await page.keyboard.press('Escape');
		else await dialog.getByRole('button', { name: close }).click();
		await expect(dialog, `${close} closes`).toHaveCount(0);
		await expect(username, `after ${close}`).toBeFocused();
	}
});

// F-196/F-264/F-145: a closing or closed dialog never swallows a click on the
// page. The raw mouse click right after Escape does not wait for the exit
// animation, so a backdrop still catching pointer events loses it; the closed
// dialog must then leave the page.
test('administrators.e2e a click right after closing a dialog reaches the page, 20 times', { tag: '@webkit' }, async ({ page }) => {
	test.setTimeout(120_000);
	await signIn(page, `admins-backdrop-${crypto.randomUUID()}`);
	const confirm = page.getByRole('dialog').filter({ hasText: 'Regenerate activation link?' });
	const theme = page.getByRole('button', { name: /^Switch to (dark|light) theme$/ });
	const box = (await theme.boundingBox())!;
	for (let i = 0; i < 20; i++) {
		const before = await theme.getAttribute('aria-label');
		await row(page, 'dana').getByRole('button', { name: 'Regenerate' }).click({ timeout: 3000 });
		await expect(confirm).toHaveAttribute('data-state', 'open');
		await page.keyboard.press('Escape');
		await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2);
		await expect(theme, `click ${i}`).not.toHaveAttribute('aria-label', before!, { timeout: 3000 });
		await expect(confirm, `closed ${i}`).toHaveCount(0);
	}
});

// F-145/F-264 (F-336): when slow frames make the close animation's end go
// unheard (WebKit stops painting under a modal), the closed dialog still leaves
// the page. Swallowing animationend makes that race certain.
test('administrators.e2e a dialog leaves the page when its close animation end is missed', { tag: '@webkit' }, async ({
	page
}) => {
	await signIn(page, `admins-missed-end-${crypto.randomUUID()}`);
	await row(page, 'dana').getByRole('button', { name: 'Regenerate' }).click();
	const confirm = page.getByRole('dialog').filter({ hasText: 'Regenerate activation link?' });
	await expect(confirm).toHaveAttribute('data-state', 'open');
	await page.evaluate(() => addEventListener('animationend', (event) => event.stopImmediatePropagation(), true));
	await page.keyboard.press('Escape');
	await expect(confirm).toHaveCount(0);
});

// F-146: the activation-link dialog appears after every Regenerate confirm.
test('administrators.e2e regenerate shows the new link every time, 20 times', async ({ page }) => {
	test.setTimeout(180_000);
	await signIn(page, `admins-regen-repeat-${crypto.randomUUID()}`);
	const confirm = page.getByRole('dialog').filter({ hasText: 'Regenerate activation link?' });
	const linkDialog = page.getByRole('dialog').filter({ hasText: 'Activation link for dana' });
	for (let i = 0; i < 20; i++) {
		await row(page, 'dana').getByRole('button', { name: 'Regenerate' }).click({ timeout: 3000 });
		await confirm.getByRole('button', { name: 'Regenerate' }).click();
		await expect(linkDialog, `link ${i}`).toBeVisible({ timeout: 3000 });
		await linkDialog.getByRole('button', { name: 'Close dialog' }).click();
		await expect(linkDialog).toHaveCount(0);
	}
});
