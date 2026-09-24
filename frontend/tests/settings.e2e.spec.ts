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

	await expect(page.getByText('Default: system')).toHaveCount(2);
	await page.getByRole('combobox').click();
	await page.getByRole('option', { name: 'Dark' }).click();
	await expect(page.getByRole('combobox')).toHaveText('Dark');
	await expect(page.getByText('Default: dark')).toHaveCount(2);
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
