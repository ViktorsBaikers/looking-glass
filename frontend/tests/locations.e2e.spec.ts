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
	await expect(frankfurt).toContainText('6 methods offered');
	const vienna = card(page, 'Vienna');
	await expect(vienna).toContainText('Online');
	await expect(vienna).toContainText('Remote');
	await expect(vienna).toContainText('Just now');
	const sfo = card(page, 'San Francisco Hub');
	await expect(sfo).toContainText('Not enrolled');
	await expect(sfo).toContainText('Never');
	await expect(sfo).toContainText('0 methods offered');
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

	await expect(sort).toHaveText('Public order'); // default: the public tab order
	await expect(firstCard).toContainText('Frankfurt');

	await pick('recent', 'Recent');
	await expect(firstCard).toContainText('Vienna'); // only heartbeat is Vienna's
	await expect(page.getByRole('listitem').last()).toContainText('Singapore'); // never-seen tie-break by name

	// Public order is Frankfurt, Vienna, London, New York, Singapore, San Francisco Hub:
	// both expected orders differ from it, so a sort that does nothing fails here.
	const names = page.getByRole('listitem').getByRole('heading');
	await pick('status', 'Status'); // online by name, then not enrolled
	await expect(names).toHaveText(['Frankfurt', 'London', 'New York', 'Singapore', 'Vienna', 'San Francisco Hub']);

	await pick('name', 'Name');
	await expect(names).toHaveText(['Frankfurt', 'London', 'New York', 'San Francisco Hub', 'Singapore', 'Vienna']);
});

test('locations.e2e reorders by keyboard and the public tabs follow', async ({ page }) => {
	await signIn(page, `locations-reorder-keys-${crypto.randomUUID()}`);
	const rows = page.getByRole('listitem');
	await expect(rows.first()).toContainText('Frankfurt');

	const saved = page.waitForResponse(
		(response) => response.url().endsWith('/api/admin/locations/order') && response.status() === 204
	);
	await page.getByRole('button', { name: 'Reorder Frankfurt' }).focus();
	await page.keyboard.press('ArrowDown');
	await saved;
	await expect(rows.nth(0)).toContainText('Vienna');
	await expect(rows.nth(1)).toContainText('Frankfurt');
	await expect(page.getByRole('button', { name: 'Reorder Frankfurt' })).toBeFocused();
	await expect(page.getByText('Frankfurt moved to position 2 of 6.')).toBeAttached();

	// The order survives a reload and drives the visitor page's tab order.
	await page.reload();
	await expect(rows.first()).toContainText('Vienna');
	await page.goto(`${APP}/`);
	const tabs = page.getByRole('tab');
	await expect(tabs.nth(0)).toContainText('Vienna');
	await expect(tabs.nth(1)).toContainText('Frankfurt');
});

// A peer's change makes the list stale; central answers 409 and the
// optimistic move gives way to the server's list.
test('locations.e2e rolls a stale reorder back to the server list', async ({ page }) => {
	const bucket = `locations-reorder-stale-${crypto.randomUUID()}`;
	await signIn(page, bucket);
	const rows = page.getByRole('listitem');
	await expect(rows).toHaveCount(6);

	const added = await page.request.post(`${APP}/api/admin/locations`, {
		headers: { 'x-looking-glass-fixture': bucket },
		data: { name: 'Oslo', geo_label: 'Oslo, NO', kind: 'local', offered_methods: [] }
	});
	expect(added.status()).toBe(201);

	const refused = page.waitForResponse(
		(response) => response.url().endsWith('/api/admin/locations/order') && response.status() === 409
	);
	await page.getByRole('button', { name: 'Reorder Frankfurt' }).focus();
	await page.keyboard.press('ArrowDown');
	await refused;
	await expect(page.getByText('The location list changed. Reload it and try again.')).toBeVisible();
	await expect(rows).toHaveCount(7);
	await expect(rows.nth(0)).toContainText('Frankfurt');
	await expect(rows.nth(1)).toContainText('Vienna');
	await expect(rows.last()).toContainText('Oslo');
});

test('locations.e2e reorders by dragging a grip', async ({ page }) => {
	await signIn(page, `locations-reorder-drag-${crypto.randomUUID()}`);
	const rows = page.getByRole('listitem');
	const grip = await page.getByRole('button', { name: 'Reorder Frankfurt' }).boundingBox();
	const third = await rows.nth(2).boundingBox();
	if (!grip || !third) throw new Error('rows not laid out');

	const saved = page.waitForResponse(
		(response) => response.url().endsWith('/api/admin/locations/order') && response.status() === 204
	);
	await page.mouse.move(grip.x + grip.width / 2, grip.y + grip.height / 2);
	await page.mouse.down();
	await page.mouse.move(grip.x + grip.width / 2, third.y + third.height * 0.75, { steps: 12 });
	await page.mouse.up();
	await saved;
	await expect(rows.nth(2)).toContainText('Frankfurt');
	await page.reload();
	await expect(rows.nth(2)).toContainText('Frankfurt');
});

test('locations.e2e hides reorder grips outside the full public order', async ({ page }) => {
	await signIn(page, `locations-reorder-gate-${crypto.randomUUID()}`);
	await expect(page.getByRole('button', { name: /^Reorder / })).toHaveCount(6);
	await page.getByLabel('Search locations').fill('vienna');
	await expect(page.getByRole('button', { name: /^Reorder / })).toHaveCount(0);
	await expect(page.getByText('Clear the search to reorder locations.')).toBeVisible();
	await page.getByLabel('Search locations').fill('');
	await page.locator('select').first().selectOption('name');
	await expect(page.getByRole('button', { name: /^Reorder / })).toHaveCount(0);
});

test('locations.e2e adds a remote location and lands on its enrollment tab', async ({ page }) => {
	await signIn(page, `locations-add-remote-${crypto.randomUUID()}`);

	await page.getByRole('button', { name: 'Add location' }).first().click();
	// Initial focus (Close) lands a frame after the dialog opens; typing first can lose the text.
	const dialog = page.getByRole('dialog', { name: 'Add location' });
	await expect(dialog.getByRole('button', { name: 'Close dialog' })).toBeFocused();
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
	// Initial focus (Close) lands a frame after the dialog opens; typing first can lose the text.
	const dialog = page.getByRole('dialog', { name: 'Add location' });
	await expect(dialog.getByRole('button', { name: 'Close dialog' })).toBeFocused();
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
