import { APP } from './ports';
import { expect, test, type Page } from '@playwright/test';

async function signIn(page: Page, fixtureId: string) {
	// Sessions live in the fixture bucket keyed by the header, so an API login
	// (no hydration race) signs in every later page request carrying it.
	const response = await page.context().request.post(`${APP}/api/auth/login`, {
		data: { username: 'brooke', password: 'fixture-password' },
		headers: { 'x-looking-glass-fixture': fixtureId }
	});
	expect(response.status()).toBe(204);
	await page.setExtraHTTPHeaders({ 'x-looking-glass-fixture': fixtureId });
}

test('settings.e2e edits branding, previews it live, saves and resets the indicator', async ({
	page
}) => {
	const fixtureId = `settings-branding-${crypto.randomUUID()}`;
	await signIn(page, fixtureId);
	await page.goto(`${APP}/admin/settings`);

	await expect(page.getByRole('heading', { name: 'Settings', exact: true })).toBeVisible();
	await expect(page.getByText('No unsaved changes.')).toBeVisible();
	await expect(page.getByText('Saved settings preview.')).toBeVisible();

	await page.getByLabel('Site title').fill('Looking Glasssss');
	await expect(page.getByText('Unsaved changes.')).toBeVisible();
	await expect(page.getByText('Unsaved changes preview.')).toBeVisible();
	// Both theme preview cards reflect the draft title live.
	await expect(page.getByText('Looking Glasssss')).toHaveCount(2);

	await page.getByLabel('Custom content block (optional)').fill('Maintenance window tonight.');
	await expect(page.getByText('Maintenance window tonight.')).toHaveCount(2);

	await page.getByRole('button', { name: 'Save settings' }).click();
	await expect(page.getByText('Settings saved.')).toBeVisible();
	await expect(page.getByText('No unsaved changes.')).toBeVisible();
	await expect(page.getByText('Saved settings preview.')).toBeVisible();

	// The save persisted in the fixture bucket.
	await page.reload();
	await expect(page.getByLabel('Site title')).toHaveValue('Looking Glasssss');
	await expect(page.getByText('No unsaved changes.')).toBeVisible();
});

test('settings.e2e changes the default theme through the select and persists it', async ({
	page
}) => {
	const fixtureId = `settings-theme-${crypto.randomUUID()}`;
	await signIn(page, fixtureId);
	await page.goto(`${APP}/admin/settings`);

	await expect(page.getByText('Default: Follow system')).toHaveCount(2);
	await page.getByRole('combobox').click();
	await page.getByRole('option', { name: 'Dark' }).click();
	await expect(page.getByRole('combobox')).toHaveText('Dark');
	await expect(page.getByText('Default: Dark')).toHaveCount(2);
	await expect(page.getByText('Unsaved changes.')).toBeVisible();

	await page.getByRole('button', { name: 'Save settings' }).click();
	await expect(page.getByText('Settings saved.')).toBeVisible();

	await page.reload();
	await expect(page.getByRole('combobox')).toHaveText('Dark');
	await expect(page.getByText('No unsaved changes.')).toBeVisible();
});

test('settings.e2e toasts the server refusal and keeps the change unsaved', async ({ page }) => {
	await signIn(page, `settings-refused-${crypto.randomUUID()}`);
	await page.goto(`${APP}/admin/settings`);

	await page.getByLabel('Logo URL (optional)').fill('http://example.test/logo.svg');
	await page.getByRole('button', { name: 'Save settings' }).click();
	await expect(page.getByText('Logo and terms URLs must use https.')).toBeVisible();
	await expect(page.getByText('Unsaved changes.')).toBeVisible();
});

// F-192: the saved copy replaces the whole form, so a save in flight must
// refuse input everywhere (text, number, the theme select) while its Save
// button keeps focus; nothing typed meanwhile is silently dropped.
test('settings.e2e refuses edits while a save is in flight and keeps focus on Save', async ({ page }) => {
	await signIn(page, `settings-busy-${crypto.randomUUID()}`);
	await page.goto(`${APP}/admin/settings`);
	await expect(page.getByText('No unsaved changes.', { exact: true })).toBeVisible();
	let release!: () => void;
	const held = new Promise<void>((resolve) => (release = resolve));
	await page.route('**/api/admin/settings', async (route) => {
		if (route.request().method() === 'PUT') await held;
		await route.continue();
	});

	const title = page.getByLabel('Site title');
	await title.fill('Title submitted');
	const save = page.getByRole('button', { name: 'Save settings' });
	// A keyboard save from a field keeps focus on that field.
	await title.press('Enter');
	await expect(save).toHaveAttribute('aria-busy', 'true');
	await expect(title).toBeFocused();
	await expect(title).toHaveAttribute('readonly', '');

	const block = page.getByLabel('Custom content block (optional)');
	await block.click({ force: true });
	await page.keyboard.type('typed during save');
	await expect.soft(block).toHaveValue('');
	await expect.soft(block).toHaveAttribute('aria-disabled', 'true');
	const theme = page.getByRole('combobox');
	await theme.click({ force: true });
	await expect.soft(page.getByRole('option', { name: 'Dark' })).toHaveCount(0);
	await expect.soft(theme).toMatchAriaSnapshot('- combobox "Default theme" [disabled]');

	await save.focus();
	release();
	await expect(page.getByText('Settings saved.')).toBeVisible();
	await expect(page.getByText('No unsaved changes.', { exact: true })).toBeVisible();
	await expect(save).toBeFocused();
	await block.fill('typed after save');
	await expect(page.getByText('Unsaved changes.', { exact: true })).toBeVisible();
});
