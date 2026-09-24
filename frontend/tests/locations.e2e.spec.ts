import { APP } from './ports';
import { expect, test, type Page } from '@playwright/test';

// Admin Locations list (issue #6): cards, search, sort, add, delete, revoke.
// Each mutating test gets its own fixture bucket so seeded data stays intact.

async function signIn(page: Page, bucket: string) {
	await page.setExtraHTTPHeaders({ 'x-looking-glass-fixture': bucket });
	await page.goto(`${APP}/login`);
	await page.getByLabel('Username').fill('brooke');
	await page.getByLabel('Password').fill('fixture-password');
	await page.getByRole('button', { name: 'Sign in' }).click();
	await expect(page.getByRole('heading', { name: 'Locations' })).toBeVisible();
}

const card = (page: Page, name: string) =>
	page.getByRole('listitem').filter({ has: page.getByRole('heading', { name }) });

test('locations.e2e lists cards with state, kind, last seen and method count', async ({ page }) => {
	await signIn(page, `locations-cards-${crypto.randomUUID()}`);

	await expect(page.getByRole('listitem')).toHaveCount(6);
	const frankfurt = card(page, 'Frankfurt');
	await expect(frankfurt).toContainText('Online');
	await expect(frankfurt).toContainText('Local');
	await expect(frankfurt).toContainText('6 methods configured');
	const vienna = card(page, 'Vienna');
	await expect(vienna).toContainText('Online');
	await expect(vienna).toContainText('Remote');
	await expect(vienna).toContainText('Just now');
	const sfo = card(page, 'San Francisco Hub');
	await expect(sfo).toContainText('Not enrolled');
	await expect(sfo).toContainText('Never');
	await expect(sfo).toContainText('0 methods configured');
	// Enroll is remote-only; Revoke only for an enrolled remote.
	await expect(sfo.getByRole('button', { name: 'Enroll' })).toBeVisible();
	await expect(sfo.getByRole('button', { name: 'Revoke' })).toHaveCount(0);
	await expect(frankfurt.getByRole('button', { name: 'Enroll' })).toHaveCount(0);
	await expect(vienna.getByRole('button', { name: 'Revoke' })).toBeVisible();
});

test('locations.e2e searches by name, geo label, state and ASN', async ({ page }) => {
	await signIn(page, `locations-search-${crypto.randomUUID()}`);
	const search = page.getByLabel('Search locations');

	await search.fill('vienna');
	await expect(page.getByRole('listitem')).toHaveCount(1);
	await expect(card(page, 'Vienna')).toBeVisible();

	await search.fill('SF-01');
	await expect(page.getByRole('listitem')).toHaveCount(1);
	await expect(card(page, 'San Francisco Hub')).toBeVisible();

	await search.fill('not enrolled');
	await expect(page.getByRole('listitem')).toHaveCount(1);
	await expect(card(page, 'San Francisco Hub')).toBeVisible();

	await search.fill('64502');
	await expect(page.getByRole('listitem')).toHaveCount(1);
	await expect(card(page, 'London')).toBeVisible();

	await search.fill('nope');
	await expect(page.getByRole('listitem')).toHaveCount(0);
	await expect(page.getByText('No locations match')).toBeVisible();
	await page.getByRole('button', { name: 'Clear search' }).click();
	await expect(page.getByRole('listitem')).toHaveCount(6);
});

test('locations.e2e sorts by name, status and recency', async ({ page }) => {
	await signIn(page, `locations-sort-${crypto.randomUUID()}`);
	const sort = page.getByLabel('Sort locations');
	const firstCard = page.getByRole('listitem').first();
	// The Ark Select exposes a visually-hidden native <select> (its form/ARIA
	// contract); driving it is deterministic, where clicking the portalled
	// listbox is not (it can land under the fixed public header).
	const hiddenSort = page.locator('select').first();
	const pick = async (value: string, label: string) => {
		await hiddenSort.selectOption(value);
		await expect(sort).toHaveText(label);
	};

	await expect(firstCard).toContainText('Frankfurt'); // default: name

	await pick('recent', 'Recent');
	await expect(firstCard).toContainText('Vienna'); // only heartbeat is Vienna's
	await expect(page.getByRole('listitem').last()).toContainText('Singapore'); // never-seen tie-break by name

	await pick('status', 'Status');
	await expect(firstCard).toContainText('Frankfurt');
	await expect(page.getByRole('listitem').last()).toContainText('San Francisco Hub');

	await pick('name', 'Name');
	await expect(firstCard).toContainText('Frankfurt');
});

test('locations.e2e adds a remote location and lands on its enrollment tab', async ({ page }) => {
	await signIn(page, `locations-add-remote-${crypto.randomUUID()}`);

	await page.getByRole('button', { name: 'Add location' }).first().click();
	await page.getByLabel('Display name').fill('Oslo');
	await page.getByLabel('Geographic label').fill('Oslo, NO');
	await page.getByLabel('Node kind').click();
	await page.getByRole('option', { name: 'Remote (enrolled agent)' }).click();
	await page.getByRole('button', { name: 'Create' }).click();

	await expect(page).toHaveURL(/\/admin\/locations\/[^/?]+\?tab=enrollment$/);
});

test('locations.e2e adds a local location and lands on its settings tab', async ({ page }) => {
	await signIn(page, `locations-add-local-${crypto.randomUUID()}`);

	await page.getByRole('button', { name: 'Add location' }).first().click();
	await page.getByLabel('Display name').fill('Oslo Local');
	await page.getByLabel('Node kind').click();
	await page.getByRole('option', { name: 'Local (built-in node)' }).click();
	await page.getByRole('button', { name: 'Create' }).click();

	await expect(page).toHaveURL(/\/admin\/locations\/[^/?]+\?tab=settings$/);
});

test('locations.e2e deletes only after confirming', async ({ page }) => {
	await signIn(page, `locations-delete-${crypto.randomUUID()}`);

	// Cancel keeps the location.
	await card(page, 'San Francisco Hub').getByRole('button', { name: 'Delete San Francisco Hub' }).click();
	await expect(page.getByRole('heading', { name: 'Delete this location?' })).toBeVisible();
	await page.getByRole('button', { name: 'Cancel' }).click();
	await expect(card(page, 'San Francisco Hub')).toBeVisible();

	// Confirm removes it and toasts.
	await card(page, 'San Francisco Hub').getByRole('button', { name: 'Delete San Francisco Hub' }).click();
	await page.getByRole('button', { name: 'Delete location' }).click();
	await expect(page.getByText('Deleted San Francisco Hub and everything under it.')).toBeVisible();
	await expect(card(page, 'San Francisco Hub')).toHaveCount(0);
	await expect(page.getByRole('listitem')).toHaveCount(5);
});

test('locations.e2e revokes an enrolled agent only after confirming', async ({ page }) => {
	await signIn(page, `locations-revoke-${crypto.randomUUID()}`);
	const vienna = card(page, 'Vienna');

	await vienna.getByRole('button', { name: 'Revoke' }).click();
	await expect(page.getByRole('heading', { name: 'Revoke this agent?' })).toBeVisible();
	await page.getByRole('button', { name: 'Cancel' }).click();
	await expect(vienna).toContainText('Online');

	await vienna.getByRole('button', { name: 'Revoke' }).click();
	await page.getByRole('button', { name: 'Revoke agent' }).click();
	await expect(page.getByText("Revoked Vienna's agent.")).toBeVisible();
	await expect(card(page, 'Vienna')).toContainText('Not enrolled');
});
