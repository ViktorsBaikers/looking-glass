// Issue #7: the Location editor lives at /admin/locations/[id] with the active
// tab in ?tab= (deep-linkable, survives reload) and full CRUD over Test IPs,
// iperf endpoints and Test files, plus the Enrollment tab.
import { APP } from './ports';
import { expect, test, type Locator, type Page } from '@playwright/test';

const TAB_LABELS: Record<string, string> = {
	settings: 'Settings',
	methods: 'Methods',
	'test-ips': 'Test IPs',
	iperf: 'iperf endpoints',
	speedtest: 'Test files',
	enrollment: 'Enrollment'
};

async function signIn(page: Page, fixtureId: string) {
	await page.setExtraHTTPHeaders({ 'x-looking-glass-fixture': fixtureId });
	await page.goto(`${APP}/login`);
	await page.getByLabel('Username').fill('brooke');
	await page.getByLabel('Password').fill('fixture-password');
	await page.getByRole('button', { name: 'Sign in' }).click();
	await expect(page).toHaveURL(`${APP}/admin`);
}

function editorUrl(id: string, tab?: string) {
	return `${APP}/admin/locations/${id}${tab ? `?tab=${tab}` : ''}`;
}

/**
 * Waits for a freshly opened dialog's initial focus (Close, one frame after it
 * opens). Typing before that frame lands can lose the text or the focus.
 */
async function dialogReady(dialog: Locator) {
	await expect(dialog.getByRole('button', { name: 'Close dialog' })).toBeFocused();
}

/** Delays every GET of the location that follows a save, so the post-save refresh lands late. */
async function delayRefreshAfterSave(page: Page, id: string) {
	let saved = false;
	await page.route(`**/api/admin/locations/${id}`, async (route) => {
		if (route.request().method() === 'PUT') saved = true;
		else if (saved) await new Promise((resolve) => setTimeout(resolve, 1500));
		await route.continue();
	});
}

test('editor deep links select a tab and survive reload', async ({ page }) => {
	await signIn(page, `editor-tabs-${crypto.randomUUID()}`);

	// No query → the Settings tab is the default.
	await page.goto(editorUrl('fra'));
	await expect(page.getByRole('tab', { name: 'Settings', selected: true })).toBeVisible();

	for (const [id, label] of Object.entries(TAB_LABELS)) {
		await page.goto(editorUrl('fra', id));
		await expect(page.getByRole('tab', { name: label, selected: true })).toBeVisible();
		await page.reload();
		await expect(page.getByRole('tab', { name: label, selected: true })).toBeVisible();
	}

	// Clicking a tab rewrites the query in place (replaceState, no reload).
	await page.getByRole('tab', { name: 'Test IPs' }).click();
	await expect(page).toHaveURL(`${APP}/admin/locations/fra?tab=test-ips`);
	await expect(page.getByRole('tab', { name: 'Test IPs', selected: true })).toBeVisible();

	// 'Back to locations' returns to the list.
	await page.getByRole('link', { name: 'Back to locations' }).click();
	await expect(page).toHaveURL(`${APP}/admin`);
});

test('settings tab saves the location and rejects a bad ASN', async ({ page }) => {
	await signIn(page, `editor-settings-${crypto.randomUUID()}`);
	await page.goto(editorUrl('fra', 'settings'));
	await delayRefreshAfterSave(page, 'fra');

	await page.getByLabel('Display name').fill('Frankfurt Main');
	await page.getByRole('button', { name: 'Save location' }).click();
	await expect(page.getByText('Location saved.')).toBeVisible();
	await expect(page.getByLabel('Display name')).toHaveValue('Frankfurt Main');

	// Client-side ASN validation blocks the save with the coded message. The
	// edit is made after the toast, so the refresh must not undo it.
	await page.getByLabel('ASN').fill('0');
	await expect(page.getByRole('heading', { name: 'Frankfurt Main' })).toBeVisible();
	await page.getByRole('button', { name: 'Save location' }).click();
	await expect(
		page.getByText('ASN must be a whole number between 1 and 4294967295.')
	).toBeVisible();

	await page.getByLabel('ASN').fill('4294967296');
	await page.getByRole('button', { name: 'Save location' }).click();
	await expect(
		page.getByText('ASN must be a whole number between 1 and 4294967295.')
	).toBeVisible();

	// A valid ASN saves again. Wait out the first save's toast so the one
	// checked below is this save's, and wait on this save's own PUT.
	await expect(page.getByText('Location saved.')).toHaveCount(0);
	await page.getByLabel('ASN').fill('64502');
	const saved = page.waitForResponse(
		(response) =>
			response.request().method() === 'PUT' && response.url().endsWith('/api/admin/locations/fra')
	);
	await page.getByRole('button', { name: 'Save location' }).click();
	expect((await saved).ok()).toBe(true);
	await expect(page.getByText('Location saved.')).toBeVisible();
	await expect(page.getByLabel('ASN')).toHaveValue('64502');

	// The data-plane origin field is remote-only.
	await expect(page.getByLabel('Data-plane origin (optional)')).toHaveCount(0);
	await page.goto(editorUrl('vie', 'settings'));
	await expect(page.getByLabel('Data-plane origin (optional)')).toBeVisible();
});

test('an edit made after the save toast survives the post-save refresh', async ({ page }) => {
	await signIn(page, `editor-save-refresh-${crypto.randomUUID()}`);
	await page.goto(editorUrl('fra', 'settings'));
	await expect(page.getByLabel('ASN')).toHaveValue('64501');
	await delayRefreshAfterSave(page, 'fra');

	await page.getByLabel('Display name').fill('Frankfurt Main');
	await page.getByRole('button', { name: 'Save location' }).click();
	await expect(page.getByText('Location saved.')).toBeVisible();
	await page.getByLabel('ASN').fill('65001');
	// The header shows the saved name once the refresh has landed.
	await expect(page.getByRole('heading', { name: 'Frankfurt Main' })).toBeVisible();
	await expect(page.getByLabel('ASN')).toHaveValue('65001');
});

test('methods tab saves the offered methods', async ({ page }) => {
	const fixtureId = `editor-methods-${crypto.randomUUID()}`;
	await signIn(page, fixtureId);
	await page.goto(editorUrl('fra', 'methods'));

	await expect(page.getByRole('checkbox', { name: 'ping', exact: true })).toBeChecked();
	await expect(page.getByRole('checkbox', { name: 'bgp', exact: true })).not.toBeChecked();

	await page.getByText('ping', { exact: true }).click();
	await page.getByText('bgp', { exact: true }).click();
	await page.getByRole('button', { name: 'Save methods' }).click();
	await expect(page.getByText('Methods saved.')).toBeVisible();

	await page.reload();
	await expect(page.getByRole('checkbox', { name: 'ping', exact: true })).not.toBeChecked();
	await expect(page.getByRole('checkbox', { name: 'bgp', exact: true })).toBeChecked();
});

// While a save and its refresh run, the controls refuse changes but keep focus
// (R-TS-05): a natively disabled control would drop it to <body>. The in-flight
// checks read the DOM once, not with retries: the refresh would put a changed
// value back and hide the change.
test('a keyboard save keeps focus on the checkbox, refuses toggles and sends one PUT', async ({
	page
}) => {
	await signIn(page, `editor-methods-focus-${crypto.randomUUID()}`);
	await page.goto(editorUrl('fra', 'methods'));
	const bgp = page.getByRole('checkbox', { name: 'bgp', exact: true });
	await expect(bgp).not.toBeChecked();
	let puts = 0;
	page.on('request', (request) => {
		if (request.method() === 'PUT' && request.url().endsWith('/api/admin/locations/fra')) puts++;
	});
	await delayRefreshAfterSave(page, 'fra');

	await bgp.focus();
	await page.keyboard.press('Space');
	await expect(bgp).toBeChecked();
	await page.keyboard.press('Enter');
	const form = page.locator('form[aria-busy="true"]');
	await expect(form).toHaveCount(1);
	await expect(bgp).toBeFocused();
	await page.keyboard.press('Space');
	await page.keyboard.press('Enter');
	expect(await bgp.isChecked()).toBe(true);
	expect(await form.count()).toBe(1);

	await expect(page.getByText('Methods saved.')).toBeVisible();
	await expect(bgp).toBeFocused();
	await expect(bgp).toBeChecked();
	expect(puts).toBe(1);
});

test('a save started on the Node kind select keeps its focus and value', async ({ page }) => {
	await signIn(page, `editor-kind-focus-${crypto.randomUUID()}`);
	await page.goto(editorUrl('fra', 'settings'));
	const kind = page.getByLabel('Node kind');
	await expect(kind).toHaveText(/Local/);
	await delayRefreshAfterSave(page, 'fra');

	await kind.focus();
	await kind.evaluate((trigger) => trigger.closest('form')!.requestSubmit());
	const form = page.locator('form[aria-busy="true"]');
	await expect(form).toHaveCount(1);
	await expect(kind).toBeFocused();
	// Typeahead picks an option on the closed trigger; Enter opens the listbox.
	await page.keyboard.press('r');
	await page.keyboard.press('Enter');
	expect(await kind.textContent()).toMatch(/Local/);
	expect(await kind.getAttribute('aria-expanded')).toBe('false');
	expect(await form.count()).toBe(1);

	await expect(page.getByText('Location saved.')).toBeVisible();
	await expect(kind).toBeFocused();
	await expect(kind).toHaveText(/Local/);
});

// The locked Ark controls must also read and look unavailable while saving:
// exposed as disabled, dimmed and with a not-allowed cursor like the inputs.
test('while saving, the Node kind select and method cards read and look unavailable', async ({
	page
}) => {
	await signIn(page, `editor-busy-look-${crypto.randomUUID()}`);
	await page.goto(editorUrl('fra', 'settings'));
	const kind = page.getByLabel('Node kind');
	await expect(kind).toHaveText(/Local/);
	await delayRefreshAfterSave(page, 'fra');

	await page.getByRole('button', { name: 'Save location' }).click();
	await expect(page.locator('form[aria-busy="true"]')).toHaveCount(1);
	await expect(kind).toMatchAriaSnapshot('- combobox "Node kind" [disabled]');
	await expect(kind).toHaveCSS('cursor', 'not-allowed');
	await expect(kind).toHaveCSS('opacity', '0.55');
	await expect(page.getByText('Location saved.')).toBeVisible();
	await expect(kind).toBeEnabled();
	await expect(kind).toHaveCSS('cursor', 'pointer');
	await expect(kind).toHaveCSS('opacity', '1');

	await page.getByRole('tab', { name: 'Methods' }).click();
	const bgp = page.getByRole('checkbox', { name: 'bgp', exact: true });
	const card = page.locator('[data-scope="checkbox"][data-part="root"]').filter({ has: bgp });
	await expect(card).toHaveCSS('cursor', 'pointer');
	await page.getByRole('button', { name: 'Save methods' }).click();
	await expect(page.locator('form[aria-busy="true"]')).toHaveCount(1);
	await expect(card).toHaveCSS('cursor', 'not-allowed');
	await expect(card).toHaveCSS('opacity', '0.55');
	await expect(page.getByText('Methods saved.')).toBeVisible();
	await expect(card).toHaveCSS('cursor', 'pointer');
	await expect(card).toHaveCSS('opacity', '1');
});

test('test IPs table creates, edits and deletes rows', async ({ page }) => {
	await signIn(page, `editor-ips-${crypto.randomUUID()}`);
	await page.goto(editorUrl('fra', 'test-ips'));

	// Seeded rows are visible in the table.
	await expect(page.getByRole('cell', { name: '192.0.2.21', exact: true })).toBeVisible();

	await page.getByRole('button', { name: 'Add test IP' }).click();
	const dialog = page.getByRole('dialog', { name: 'Add test IP' });
	await dialogReady(dialog);
	await dialog.getByLabel('Name').fill('Edge anycast');
	await dialog.getByLabel('IP address').fill('2001:db8::77');
	await dialog.getByLabel('Type').click();
	await page.getByRole('option', { name: 'IPv6' }).click();
	await dialog.getByRole('button', { name: 'Save' }).click();
	await expect(dialog).toHaveCount(0);
	await expect(page.getByText('Test IP saved.')).toBeVisible();
	const newRow = page.getByRole('row', { name: /Edge anycast/ });
	await expect(newRow.getByRole('cell', { name: '2001:db8::77', exact: true })).toBeVisible();
	await expect(newRow.getByRole('cell', { name: 'IPv6', exact: true })).toBeVisible();

	await page.getByRole('button', { name: 'Edit Edge anycast' }).click();
	const editDialog = page.getByRole('dialog', { name: 'Edit test IP' });
	await dialogReady(editDialog);
	await editDialog.getByLabel('IP address').fill('2001:db8::7');
	await editDialog.getByRole('button', { name: 'Save' }).click();
	await expect(editDialog).toHaveCount(0);
	await expect(page.getByRole('cell', { name: '2001:db8::7', exact: true })).toBeVisible();

	await page.getByRole('button', { name: 'Delete Edge anycast' }).click();
	await page.getByRole('dialog', { name: 'Delete test IP?' }).getByRole('button', { name: 'Delete' }).click();
	await expect(page.getByText('Test IP deleted.')).toBeVisible();
	await expect(page.getByRole('cell', { name: '2001:db8::7', exact: true })).toHaveCount(0);
});

test('iperf table creates, edits and deletes endpoints', async ({ page }) => {
	await signIn(page, `editor-iperf-${crypto.randomUUID()}`);
	await page.goto(editorUrl('fra', 'iperf'));

	await expect(page.getByRole('cell', { name: '192.0.2.21', exact: true })).toBeVisible();

	await page.getByRole('button', { name: 'Add iperf endpoint' }).click();
	const dialog = page.getByRole('dialog', { name: 'Add iperf endpoint' });
	await dialogReady(dialog);
	await dialog.getByLabel('Name').fill('Backup server');
	await dialog.getByLabel('Host').fill('203.0.113.9');
	await dialog.getByLabel('Port').fill('5202');
	await dialog.getByLabel('Reverse command').fill('iperf3 -c 203.0.113.9 -R');
	await dialog.getByLabel('Standard command').fill('iperf3 -c 203.0.113.9');
	await dialog.getByRole('button', { name: 'Save' }).click();
	await expect(dialog).toHaveCount(0);
	await expect(page.getByText('iperf endpoint saved.')).toBeVisible();
	await expect(page.getByRole('cell', { name: 'Backup server', exact: true })).toBeVisible();
	await expect(page.getByRole('cell', { name: '5202', exact: true })).toBeVisible();

	await page.getByRole('button', { name: 'Edit Backup server' }).click();
	const editDialog = page.getByRole('dialog', { name: 'Edit iperf endpoint' });
	await dialogReady(editDialog);
	await editDialog.getByLabel('Port').fill('5203');
	await editDialog.getByRole('button', { name: 'Save' }).click();
	await expect(editDialog).toHaveCount(0);
	await expect(page.getByRole('cell', { name: '5203', exact: true })).toBeVisible();

	await page.getByRole('button', { name: 'Delete Backup server' }).click();
	await page
		.getByRole('dialog', { name: 'Delete iperf endpoint?' })
		.getByRole('button', { name: 'Delete' })
		.click();
	await expect(page.getByText('iperf endpoint deleted.')).toBeVisible();
	await expect(page.getByRole('cell', { name: 'Backup server', exact: true })).toHaveCount(0);
});

test('speedtest table creates, edits and deletes test files', async ({ page }) => {
	await signIn(page, `editor-files-${crypto.randomUUID()}`);
	await page.goto(editorUrl('fra', 'speedtest'));

	await expect(page.getByRole('cell', { name: '100 MB test file', exact: true })).toBeVisible();

	await page.getByRole('button', { name: 'Add test file' }).click();
	const dialog = page.getByRole('dialog', { name: 'Add test file' });
	await dialogReady(dialog);
	await dialog.getByLabel('Label').fill('1 GB test file');
	await dialog.getByLabel('Declared size').fill('1 GB');
	await dialog.getByLabel('Source on node').fill('/files/1gb.bin');
	await dialog.getByRole('button', { name: 'Save' }).click();
	await expect(dialog).toHaveCount(0);
	await expect(page.getByText('Test file saved.')).toBeVisible();
	await expect(page.getByRole('cell', { name: '1 GB test file', exact: true })).toBeVisible();
	await expect(page.getByRole('cell', { name: '/files/1gb.bin', exact: true })).toBeVisible();

	await page.getByRole('button', { name: 'Edit 1 GB test file' }).click();
	const editDialog = page.getByRole('dialog', { name: 'Edit test file' });
	await dialogReady(editDialog);
	await editDialog.getByLabel('Declared size').fill('2 GB');
	await editDialog.getByRole('button', { name: 'Save' }).click();
	await expect(editDialog).toHaveCount(0);
	await expect(page.getByRole('cell', { name: '2 GB', exact: true })).toBeVisible();

	await page.getByRole('button', { name: 'Delete 1 GB test file' }).click();
	await page
		.getByRole('dialog', { name: 'Delete test file?' })
		.getByRole('button', { name: 'Delete' })
		.click();
	await expect(page.getByText('Test file deleted.')).toBeVisible();
	await expect(page.getByRole('cell', { name: '1 GB test file', exact: true })).toHaveCount(0);
});

test('enrollment tab shows the install command, countdown and regenerate', async ({ page }) => {
	await signIn(page, `editor-enroll-${crypto.randomUUID()}`);
	await page.goto(editorUrl('sfo', 'enrollment'));

	const command = page.locator('code');
	await expect(command).toContainText('curl');
	const first = await command.textContent();

	await expect(page.getByText(/Expires in \d{2}:\d{2}/)).toBeVisible();
	await page.getByRole('button', { name: 'Copy install command' }).click();

	await page.getByRole('button', { name: 'Regenerate' }).click();
	await expect(page.locator('code')).not.toHaveText(first ?? '');

	// A local node has no agent to enroll.
	await page.goto(editorUrl('fra', 'enrollment'));
	await expect(page.getByText('Enrollment applies only to remote locations.')).toBeVisible();
});

test('enrollment tab reports a connected agent and revokes it', async ({ page }) => {
	await signIn(page, `editor-revoke-${crypto.randomUUID()}`);
	await page.goto(editorUrl('vie', 'enrollment'));

	// Vienna's agent is online in the fixture; the live poll flips within ~3 s.
	await expect(page.getByText('Connected — Vienna is online.')).toBeVisible({ timeout: 10000 });

	await page.getByRole('button', { name: 'Revoke agent' }).click();
	await page
		.getByRole('dialog', { name: 'Revoke this agent?' })
		.getByRole('button', { name: 'Revoke agent' })
		.click();
	await expect(page.getByText("Revoked Vienna's agent.")).toBeVisible();
	await expect(page.getByText(/Waiting for the agent to connect/)).toBeVisible();
	await expect(page.getByRole('button', { name: 'Revoke agent' })).toHaveCount(0);
});

// F-364: hiding the old command pulled the page up under the pointer, so the
// second click of a double-click on Regenerate hit Revoke agent.
test('Regenerate stays put while it mints, so a double-click cannot reach Revoke', async ({ page }) => {
	await signIn(page, `editor-regen-dblclick-${crypto.randomUUID()}`);
	await page.goto(editorUrl('vie', 'enrollment'));
	const command = page.locator('code');
	await expect(command).toContainText('curl');
	await expect(page.getByRole('button', { name: 'Revoke agent' })).toBeVisible();
	const first = await command.textContent();
	// Hold the next mint open, so both clicks land while it runs.
	await page.route('**/api/admin/locations/vie/enroll', async (route) => {
		await new Promise((resolve) => setTimeout(resolve, 2000));
		await route.continue();
	});

	const regenerate = page.getByRole('button', { name: 'Regenerate' });
	const before = (await regenerate.boundingBox())!;
	// The lower half of the button: where Revoke landed once the page moved up.
	await page.mouse.dblclick(before.x + before.width / 2, before.y + before.height * 0.75);
	await expect(regenerate).toHaveAttribute('aria-busy', 'true');
	expect(await regenerate.boundingBox()).toEqual(before);

	await expect(command).not.toHaveText(first ?? '');
	await expect(page.getByRole('dialog')).toHaveCount(0);
	expect(await regenerate.boundingBox()).toEqual(before);
});

// F-365: the refresh-failure retry follows the Enrollment retry's spacing, and
// its focus ring stays clear of the tab rail below it.
test('the refresh-failure Try again is spaced like the Enrollment retry', async ({ page }) => {
	await signIn(page, `editor-refresh-retry-${crypto.randomUUID()}`);
	const gapAboveRetry = () =>
		page.getByRole('button', { name: 'Try again' }).evaluate((button) => {
			const alert = document.querySelector('[role="alert"]')!;
			return button.getBoundingClientRect().top - alert.getBoundingClientRect().bottom;
		});

	await page.route('**/api/admin/locations/sfo/enroll', (route) =>
		route.fulfill({ status: 422, json: { error: 'validation', message: 'Refused.' } })
	);
	await page.goto(editorUrl('sfo', 'enrollment'));
	await expect(page.getByRole('alert')).toContainText('could not be generated');
	const enrollmentGap = await gapAboveRetry();

	await page.goto(editorUrl('fra', 'settings'));
	let saved = false;
	await page.route('**/api/admin/locations/fra', async (route) => {
		if (route.request().method() === 'PUT') saved = true;
		else if (saved) return route.fulfill({ status: 500, json: { error: 'internal' } });
		await route.continue();
	});
	await page.getByRole('button', { name: 'Save location' }).click();
	await expect(page.getByRole('alert')).toContainText('This location could not be refreshed.');
	expect(await gapAboveRetry()).toBe(enrollmentGap);

	// Keyboard focus, so the focus ring shows.
	await page.getByRole('tab', { name: 'Settings' }).focus();
	await page.keyboard.press('Shift+Tab');
	const retry = page.getByRole('button', { name: 'Try again' });
	await expect(retry).toBeFocused();
	const clearance = await retry.evaluate((button) => {
		const style = getComputedStyle(button);
		const ring = parseFloat(style.outlineWidth) + parseFloat(style.outlineOffset);
		const rail = document.querySelector('[role="tablist"]')!.getBoundingClientRect().top;
		return rail - (button.getBoundingClientRect().bottom + ring);
	});
	expect(clearance).toBeGreaterThan(0);
});

test('test IP save rejects an address that does not match the declared family', async ({ page }) => {
	await signIn(page, `editor-ip-family-${crypto.randomUUID()}`);
	await page.goto(editorUrl('fra', 'test-ips'));

	await page.getByRole('button', { name: 'Add test IP' }).click();
	const dialog = page.getByRole('dialog', { name: 'Add test IP' });
	await dialogReady(dialog);
	await dialog.getByLabel('Name').fill('Mismatched');
	await dialog.getByLabel('IP address').fill('203.0.113.7');
	await dialog.getByLabel('Type').click();
	await page.getByRole('option', { name: 'IPv6' }).click();
	await dialog.getByRole('button', { name: 'Save' }).click();

	// Central's check_address rejects the family mismatch with a 422; the dialog
	// stays open with the inline error, and the error toast shows the same text
	// (the modal hides the toaster from the a11y tree, so match visible text).
	await expect(dialog).toBeVisible();
	await expect(dialog.getByText('The address does not match the selected family.')).toBeVisible();
	await expect(page.getByText('The address does not match the selected family.')).toHaveCount(2);

	// A non-IP string gets central's other coded message.
	await dialog.getByLabel('IP address').fill('not-an-ip');
	await dialog.getByRole('button', { name: 'Save' }).click();
	await expect(dialog.getByText('Enter a valid IP address.')).toBeVisible();
});

test('location save failure toasts the server message', async ({ page }) => {
	await signIn(page, `editor-save-fail-${crypto.randomUUID()}`);
	await page.goto(editorUrl('fra', 'settings'));

	await page.getByLabel('Display name').fill('');
	await page.getByRole('button', { name: 'Save location' }).click();

	await expect(
		// central's garde message for LocationInput.name length(min = 1).
		page.getByRole('status').filter({ hasText: 'name: length is lower than 1' })
	).toBeVisible();
	// The inline form error is kept alongside the toast.
	await expect(page.getByRole('alert')).toContainText('name: length is lower than 1');
});

test('enrollment mints exactly one ticket on open and one per regenerate', async ({ page }) => {
	const enrollPosts: string[] = [];
	page.on('request', (request) => {
		if (request.method() === 'POST' && request.url().endsWith('/enroll')) enrollPosts.push(request.url());
	});
	await signIn(page, `editor-enroll-count-${crypto.randomUUID()}`);

	// Opening the editor on another tab must not mint anything.
	await page.goto(editorUrl('sfo', 'settings'));
	await expect(page.getByRole('heading', { name: 'San Francisco Hub' })).toBeVisible();
	expect(enrollPosts).toHaveLength(0);

	await page.goto(editorUrl('sfo', 'enrollment'));
	const command = page.locator('code');
	await expect(command).toContainText('curl');
	expect(enrollPosts).toHaveLength(1);

	const second = page.waitForRequest((request) => request.method() === 'POST' && request.url().endsWith('/enroll'));
	await page.getByRole('button', { name: 'Regenerate' }).click();
	await second;
	await expect(command).toContainText('curl');
	// Exactly one new POST for the click — a reactive double-mint would be 3 total.
	expect(enrollPosts).toHaveLength(2);
});

test('a stale location fetch does not overwrite the newer editor', async ({ page }) => {
	await signIn(page, `editor-stale-${crypto.randomUUID()}`);

	// Hold Frankfurt's GET in flight while the SPA navigates to Vienna.
	await page.route('**/api/admin/locations/fra', async (route) => {
		await new Promise((resolve) => setTimeout(resolve, 1200));
		await route.continue();
	});
	const fraLanded = page.waitForEvent('requestfinished', (request) =>
		request.url().endsWith('/api/admin/locations/fra')
	);
	await page.goto(editorUrl('fra'));
	await expect(page.getByText('Loading location…')).toBeVisible();

	// An injected same-origin link rides SvelteKit's client router, so the
	// editor component instance survives the param change (the stale-write race).
	await page.evaluate(() => {
		const link = document.createElement('a');
		link.href = '/admin/locations/vie';
		link.textContent = 'to Vienna';
		document.body.append(link);
	});
	await page.getByRole('link', { name: 'to Vienna' }).click();
	await expect(page.getByRole('heading', { name: 'Vienna' })).toBeVisible();

	// Frankfurt's slow response lands after Vienna rendered; it must be dropped.
	await fraLanded;
	await expect(page.getByRole('heading', { name: 'Vienna' })).toBeVisible();
	await expect(page.getByLabel('Display name')).toHaveValue('Vienna');
});

test('a local location switched to remote reads offline until an agent connects', async ({ page }) => {
	await signIn(page, `editor-kind-remote-${crypto.randomUUID()}`);

	// central's PUT echoes the stored status; reads derive a remote node's status
	// from its live agents. A node that was local has none, so a GET reads it
	// offline and it leaves the public list.
	const after = await page.evaluate(async () => {
		const location = await (await fetch('/api/admin/locations/fra')).json();
		const saved = await fetch('/api/admin/locations/fra', {
			method: 'PUT',
			headers: { 'content-type': 'application/json' },
			body: JSON.stringify({ ...location, kind: 'remote', data_plane_origin: 'https://fra.example.net' })
		});
		const read = await (await fetch('/api/admin/locations/fra')).json();
		const listed: { id: string }[] = await (await fetch('/api/locations')).json();
		return { saved: (await saved.json()).status, read: read.status, listed: listed.map((entry) => entry.id) };
	});
	expect(after.saved).toBe('online');
	expect(after.read).toBe('offline');
	expect(after.listed).not.toContain('fra');
});

test('the selected tab exposes its panel as a linked ARIA tabpanel', async ({ page }) => {
	await signIn(page, `editor-aria-${crypto.randomUUID()}`);
	await page.goto(editorUrl('fra', 'test-ips'));

	const trigger = page.getByRole('tab', { name: 'Test IPs', selected: true });
	const panel = page.getByRole('tabpanel', { name: 'Test IPs' });
	await expect(panel).toBeVisible();
	expect(await trigger.getAttribute('aria-controls')).toBe(await panel.getAttribute('id'));

	await page.getByRole('tab', { name: 'Methods' }).click();
	await expect(page.getByRole('tabpanel', { name: 'Methods' })).toBeVisible();
	await expect(page.getByRole('tabpanel', { name: 'Test IPs' })).toHaveCount(0);
});

// F-191: a save still in flight for row A answers after its dialog closed and
// row B's opened. The answer must not close B's form, drop its edits, show A's
// error in it or mark B's Save busy; the toast and the list still report A.
for (const outcome of ['saved', 'refused'] as const) {
	test(`a late ${outcome} save for one test IP leaves the form opened for another alone`, async ({ page }) => {
		await signIn(page, `editor-crud-race-${outcome}-${crypto.randomUUID()}`);
		await page.goto(editorUrl('fra', 'test-ips'));
		let release!: () => void;
		const held = new Promise<void>((resolve) => (release = resolve));
		await page.route('**/api/admin/test-ips/**', async (route) => {
			if (route.request().method() !== 'PUT') return route.continue();
			await held;
			if (outcome === 'saved') return route.continue();
			return route.fulfill({ status: 400, json: { error: 'validation', message: 'Row A rejected.' } });
		});
		const edits = page.getByRole('button', { name: 'Edit primary' });
		await expect(edits).toHaveCount(2);
		const dialog = page.getByRole('dialog', { name: 'Edit test IP' });

		await edits.nth(0).click();
		await dialogReady(dialog);
		await dialog.getByLabel('Name').fill('row-A-edit');
		const put = page.waitForRequest((request) => request.method() === 'PUT');
		await dialog.getByRole('button', { name: 'Save' }).click();
		await put;
		await page.keyboard.press('Escape');
		await expect(dialog).toHaveCount(0);

		await edits.nth(1).click();
		await dialogReady(dialog);
		await expect(dialog.getByLabel('IP address')).toHaveValue('2001:db8::21');
		await dialog.getByLabel('Name').fill('row-B-unsaved');
		await expect.soft(dialog.getByRole('button', { name: 'Save' })).not.toHaveAttribute('aria-busy', 'true');

		release();
		await expect(page.getByText(outcome === 'saved' ? 'Test IP saved.' : 'Row A rejected.').first()).toBeVisible();
		if (outcome === 'saved') await expect(page.getByText('row-A-edit', { exact: true })).toBeVisible();
		// Still open, not merely mid-way through a close animation.
		await expect(dialog).toHaveAttribute('data-state', 'open');
		await expect(dialog.getByLabel('Name')).toHaveValue('row-B-unsaved');
		await expect(dialog.getByLabel('IP address')).toHaveValue('2001:db8::21');
		await expect(dialog.getByRole('alert')).toHaveCount(0);
		await expect(dialog.getByLabel('Name')).toBeFocused();
	});
}

// CX-1: reopening the row whose save is still in flight shows the values being
// saved with Save busy, so a second Save cannot send the stale row back and
// revert the first. Once the save lands the form holds the saved values.
test('reopening a test IP while its save is in flight waits for that save', async ({ page }) => {
	await signIn(page, `editor-crud-reopen-${crypto.randomUUID()}`);
	await page.goto(editorUrl('fra', 'test-ips'));
	let release!: () => void;
	const held = new Promise<void>((resolve) => (release = resolve));
	const names: string[] = [];
	await page.route('**/api/admin/test-ips/**', async (route) => {
		if (route.request().method() !== 'PUT') return route.continue();
		names.push(route.request().postDataJSON().label);
		await held;
		return route.continue();
	});
	const rowA = page.getByRole('button', { name: 'Edit primary' }).nth(0);
	const dialog = page.getByRole('dialog', { name: 'Edit test IP' });
	const save = dialog.getByRole('button', { name: 'Save' });

	await rowA.click();
	await dialogReady(dialog);
	await dialog.getByLabel('Name').fill('first-save');
	await save.click();
	await expect.poll(() => names.length).toBe(1);
	await page.keyboard.press('Escape');
	await expect(dialog).toHaveCount(0);

	await rowA.click();
	await dialogReady(dialog);
	await expect.soft(dialog.getByLabel('Name')).toHaveValue('first-save');
	await expect.soft(save).toHaveAttribute('aria-busy', 'true');
	await save.click({ force: true });
	await dialog.getByLabel('Name').press('Enter');

	release();
	await expect(page.getByText('Test IP saved.').first()).toBeVisible();
	expect(names).toEqual(['first-save']);
	// The table sits behind the modal (aria-hidden), so match its text directly.
	await expect(page.getByText('first-save', { exact: true })).toBeVisible();
	await expect(dialog).toHaveAttribute('data-state', 'open');
	await expect(dialog.getByLabel('Name')).toHaveValue('first-save');
	await expect(save).not.toHaveAttribute('aria-busy', 'true');
	await expect(dialog.getByRole('alert')).toHaveCount(0);

	// The settled save no longer holds the row: a fresh open is editable.
	await page.keyboard.press('Escape');
	await expect(dialog).toHaveCount(0);
	await page.getByRole('button', { name: 'Edit first-save' }).click();
	await expect(dialog.getByLabel('Name')).toHaveValue('first-save');
	await expect(save).not.toHaveAttribute('aria-busy', 'true');
	expect(names).toEqual(['first-save']);
});

// F-202: the Methods save sends the whole form, so an invalid unsaved ASN on
// Settings blocks it. The Methods tab must say so instead of doing nothing.
test('Save methods explains the invalid ASN on Settings that blocks it', async ({ page }) => {
	await signIn(page, `editor-methods-asn-${crypto.randomUUID()}`);
	await page.goto(editorUrl('fra', 'settings'));
	let puts = 0;
	page.on('request', (request) => {
		if (request.method() === 'PUT') puts++;
	});
	await page.getByLabel('ASN').fill('abc');
	await page.getByRole('tab', { name: 'Methods' }).click();
	await page.getByText('bgp', { exact: true }).click();
	await page.getByRole('button', { name: 'Save methods' }).click();
	const alert = page.getByRole('tabpanel').getByRole('alert');
	await expect(alert).toContainText('Settings');
	await expect(alert).toContainText('ASN must be a whole number between 1 and 4294967295.');
	expect(puts).toBe(0);
});

// F-203: a sub-resource save refreshes the location; it must not throw away
// unsaved Settings and Methods edits.
test('deleting a test IP keeps unsaved Settings and Methods edits', async ({ page }) => {
	await signIn(page, `editor-refresh-merge-${crypto.randomUUID()}`);
	await page.goto(editorUrl('fra', 'settings'));
	await page.getByLabel('Display name').fill('Unsaved name');
	await page.getByRole('tab', { name: 'Methods' }).click();
	const bgp = page.getByRole('checkbox', { name: 'bgp', exact: true });
	await expect(bgp).not.toBeChecked();
	await page.getByText('bgp', { exact: true }).click();
	await expect(bgp).toBeChecked();

	await page.getByRole('tab', { name: 'Test IPs' }).click();
	const refreshed = page.waitForResponse(
		(response) => response.url().endsWith('/api/admin/locations/fra') && response.request().method() === 'GET'
	);
	await page.getByRole('button', { name: 'Delete primary' }).first().click();
	await page.getByRole('dialog', { name: 'Delete test IP?' }).getByRole('button', { name: 'Delete' }).click();
	await expect(page.getByText('Test IP deleted.')).toBeVisible();
	await refreshed;
	await expect(page.getByRole('cell', { name: '192.0.2.21', exact: true })).toHaveCount(0);

	await page.getByRole('tab', { name: 'Methods' }).click();
	await expect(bgp).toBeChecked();
	await page.getByRole('tab', { name: 'Settings' }).click();
	await expect(page.getByLabel('Display name')).toHaveValue('Unsaved name');
});

// F-228: while a CrudSection save runs, its fields are read-only and exposed
// as unavailable, and the focused field keeps focus (R-TS-05), so nothing
// typed after Save is silently dropped when the dialog closes.
test('a test IP dialog locks its fields while the save runs', async ({ page }) => {
	await signIn(page, `editor-crud-busy-${crypto.randomUUID()}`);
	await page.goto(editorUrl('fra', 'test-ips'));
	let release!: () => void;
	const held = new Promise<void>((resolve) => (release = resolve));
	const posted: unknown[] = [];
	await page.route('**/api/admin/locations/fra/test-ips', async (route) => {
		if (route.request().method() === 'POST') {
			posted.push(route.request().postDataJSON());
			await held;
		}
		return route.continue();
	});
	await page.getByRole('button', { name: 'Add test IP' }).click();
	const dialog = page.getByRole('dialog', { name: 'Add test IP' });
	const name = dialog.getByLabel('Name');
	const type = dialog.getByLabel('Type');
	await dialogReady(dialog);
	await dialog.getByLabel('IP address').fill('192.0.2.99');
	await name.fill('edge');
	await name.press('Enter');
	await expect.poll(() => posted.length).toBe(1);

	await expect(name).toHaveAttribute('readonly', '');
	await expect(name).toHaveAttribute('aria-disabled', 'true');
	await expect(dialog.getByLabel('IP address')).toHaveAttribute('aria-disabled', 'true');
	await expect(type).toMatchAriaSnapshot('- combobox "Type" [disabled]');
	await expect(name).toBeFocused();
	await name.pressSequentially('-typed');
	await expect(name).toHaveValue('edge');
	await type.focus();
	await page.keyboard.press('ArrowDown');
	await expect(type).toHaveAttribute('aria-expanded', 'false');
	await expect(type).toBeFocused();

	release();
	await expect(dialog).toHaveCount(0);
	await expect(page.getByText('Test IP saved.')).toBeVisible();
	expect(posted).toEqual([{ label: 'edge', address: '192.0.2.99', family: 'v4' }]);
});
