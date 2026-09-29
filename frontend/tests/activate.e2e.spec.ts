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

// F-194: central counts password length in UTF-8 bytes (12 to 512); so does the page.
test('activate.e2e counts the password length in UTF-8 bytes like central', async ({ page }) => {
	await page.setExtraHTTPHeaders({ 'x-looking-glass-fixture': `activate-bytes-${crypto.randomUUID()}` });
	await page.goto(`${APP}/activate/fixture-activation-token`);
	const password = page.getByLabel('Password', { exact: true });
	const confirm = page.getByLabel('Confirm password');
	const submit = page.getByRole('button', { name: 'Set password' });

	// 300 x é: 300 UTF-16 units, 600 bytes, which central refuses.
	await password.fill('é'.repeat(300));
	await confirm.fill('é'.repeat(300));
	await expect(page.getByText('At most 512 characters.')).toBeVisible();
	await expect(page.getByText(/count as 2 to 4/)).toBeVisible();
	await expect(submit).toBeDisabled();

	// 4 CJK characters: 12 bytes, which central accepts.
	await password.fill('中中中中');
	await confirm.fill('中中中中');
	await expect(page.getByText(/At (least|most)/)).toHaveCount(0);
	await expect(submit).toBeEnabled();
});

// F-223: the rules describe the field while typing without an alert or an
// invalid state; leaving the field turns them into errors.
test('activate.e2e password rules stay a hint until the field is left', async ({ page }) => {
	await page.setExtraHTTPHeaders({ 'x-looking-glass-fixture': `activate-hint-${crypto.randomUUID()}` });
	await page.goto(`${APP}/activate/fixture-activation-token`);
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

// F-260: editing Password after Confirm was left must not turn the mismatch
// into an alert on each keystroke; it is a hint while typing and an error,
// announced once, when Password is left.
test('activate.e2e a password edit after Confirm keeps the mismatch a hint while typing', async ({ page }) => {
	await page.setExtraHTTPHeaders({ 'x-looking-glass-fixture': `activate-cross-${crypto.randomUUID()}` });
	await page.goto(`${APP}/activate/fixture-activation-token`);
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
