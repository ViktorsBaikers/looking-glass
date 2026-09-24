// Issue #7: the Location editor lives at /admin/locations/[id] with the active
// tab in ?tab= (deep-linkable, survives reload) and full CRUD over Test IPs,
// iperf endpoints and Test files, plus the Enrollment tab.
import { APP } from './ports';
import { expect, test, type Page } from '@playwright/test';

const TAB_LABELS: Record<string, string> = {
	settings: 'Settings',
	methods: 'Methods',
	'test-ips': 'Test IPs',
	iperf: 'iperf',
	speedtest: 'Speedtest',
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

	await page.getByLabel('Display name').fill('Frankfurt Main');
	await page.getByRole('button', { name: 'Save location' }).click();
	await expect(page.getByText('Location saved.')).toBeVisible();
	await expect(page.getByLabel('Display name')).toHaveValue('Frankfurt Main');

	// Client-side ASN validation blocks the save with the coded message.
	await page.getByLabel('ASN').fill('0');
	await page.getByRole('button', { name: 'Save location' }).click();
	await expect(
		page.getByText('ASN must be a whole number between 1 and 4294967295.')
	).toBeVisible();

	await page.getByLabel('ASN').fill('4294967296');
	await page.getByRole('button', { name: 'Save location' }).click();
	await expect(
		page.getByText('ASN must be a whole number between 1 and 4294967295.')
	).toBeVisible();

	await page.getByLabel('ASN').fill('64501');
	await page.getByRole('button', { name: 'Save location' }).click();
	await expect(page.getByText('Location saved.')).toBeVisible();
	await expect(page.getByLabel('ASN')).toHaveValue('64501');

	// The data-plane origin field is remote-only.
	await expect(page.getByLabel('Data-plane origin (optional)')).toHaveCount(0);
	await page.goto(editorUrl('vie', 'settings'));
	await expect(page.getByLabel('Data-plane origin (optional)')).toBeVisible();
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

test('test IPs table creates, edits and deletes rows', async ({ page }) => {
	await signIn(page, `editor-ips-${crypto.randomUUID()}`);
	await page.goto(editorUrl('fra', 'test-ips'));

	// Seeded rows are visible in the table.
	await expect(page.getByRole('cell', { name: '192.0.2.21', exact: true })).toBeVisible();

	await page.getByRole('button', { name: 'Add IP' }).click();
	const dialog = page.getByRole('dialog', { name: 'Add IP' });
	await dialog.getByLabel('Name').fill('Edge anycast');
	await dialog.getByLabel('IP address').fill('203.0.113.7');
	await dialog.getByLabel('Type').click();
	await page.getByRole('option', { name: 'IPv6' }).click();
	await dialog.getByRole('button', { name: 'Save' }).click();
	await expect(dialog).toHaveCount(0);
	await expect(page.getByText('Test IP saved.')).toBeVisible();
	const newRow = page.getByRole('row', { name: /Edge anycast/ });
	await expect(newRow.getByRole('cell', { name: '203.0.113.7', exact: true })).toBeVisible();
	await expect(newRow.getByRole('cell', { name: 'IPv6', exact: true })).toBeVisible();

	await page.getByRole('button', { name: 'Edit Edge anycast' }).click();
	const editDialog = page.getByRole('dialog', { name: 'Edit IP' });
	await editDialog.getByLabel('IP address').fill('2001:db8::7');
	await editDialog.getByRole('button', { name: 'Save' }).click();
	await expect(editDialog).toHaveCount(0);
	await expect(page.getByRole('cell', { name: '2001:db8::7', exact: true })).toBeVisible();

	await page.getByRole('button', { name: 'Delete Edge anycast' }).click();
	await page.getByRole('dialog', { name: 'Delete IP?' }).getByRole('button', { name: 'Delete' }).click();
	await expect(page.getByText('Test IP deleted.')).toBeVisible();
	await expect(page.getByRole('cell', { name: '2001:db8::7', exact: true })).toHaveCount(0);
});

test('iperf table creates, edits and deletes endpoints', async ({ page }) => {
	await signIn(page, `editor-iperf-${crypto.randomUUID()}`);
	await page.goto(editorUrl('fra', 'iperf'));

	await expect(page.getByRole('cell', { name: '192.0.2.21', exact: true })).toBeVisible();

	await page.getByRole('button', { name: 'Add endpoint' }).click();
	const dialog = page.getByRole('dialog', { name: 'Add endpoint' });
	await dialog.getByLabel('Name').fill('Backup server');
	await dialog.getByLabel('Host').fill('203.0.113.9');
	await dialog.getByLabel('Port').fill('5202');
	await dialog.getByLabel('Incoming command').fill('iperf3 -c 203.0.113.9 -R');
	await dialog.getByLabel('Outgoing command').fill('iperf3 -c 203.0.113.9');
	await dialog.getByRole('button', { name: 'Save' }).click();
	await expect(dialog).toHaveCount(0);
	await expect(page.getByText('iperf endpoint saved.')).toBeVisible();
	await expect(page.getByRole('cell', { name: 'Backup server', exact: true })).toBeVisible();
	await expect(page.getByRole('cell', { name: '5202', exact: true })).toBeVisible();

	await page.getByRole('button', { name: 'Edit Backup server' }).click();
	const editDialog = page.getByRole('dialog', { name: 'Edit endpoint' });
	await editDialog.getByLabel('Port').fill('5203');
	await editDialog.getByRole('button', { name: 'Save' }).click();
	await expect(editDialog).toHaveCount(0);
	await expect(page.getByRole('cell', { name: '5203', exact: true })).toBeVisible();

	await page.getByRole('button', { name: 'Delete Backup server' }).click();
	await page
		.getByRole('dialog', { name: 'Delete endpoint?' })
		.getByRole('button', { name: 'Delete' })
		.click();
	await expect(page.getByText('iperf endpoint deleted.')).toBeVisible();
	await expect(page.getByRole('cell', { name: 'Backup server', exact: true })).toHaveCount(0);
});

test('speedtest table creates, edits and deletes test files', async ({ page }) => {
	await signIn(page, `editor-files-${crypto.randomUUID()}`);
	await page.goto(editorUrl('fra', 'speedtest'));

	await expect(page.getByRole('cell', { name: '100 MB test file', exact: true })).toBeVisible();

	await page.getByRole('button', { name: 'Add file' }).click();
	const dialog = page.getByRole('dialog', { name: 'Add file' });
	await dialog.getByLabel('Label').fill('1 GB test file');
	await dialog.getByLabel('Declared size').fill('1 GB');
	await dialog.getByLabel('Source on node').fill('/files/1gb.bin');
	await dialog.getByRole('button', { name: 'Save' }).click();
	await expect(dialog).toHaveCount(0);
	await expect(page.getByText('Test file saved.')).toBeVisible();
	await expect(page.getByRole('cell', { name: '1 GB test file', exact: true })).toBeVisible();
	await expect(page.getByRole('cell', { name: '/files/1gb.bin', exact: true })).toBeVisible();

	await page.getByRole('button', { name: 'Edit 1 GB test file' }).click();
	const editDialog = page.getByRole('dialog', { name: 'Edit file' });
	await editDialog.getByLabel('Declared size').fill('2 GB');
	await editDialog.getByRole('button', { name: 'Save' }).click();
	await expect(editDialog).toHaveCount(0);
	await expect(page.getByRole('cell', { name: '2 GB', exact: true })).toBeVisible();

	await page.getByRole('button', { name: 'Delete 1 GB test file' }).click();
	await page
		.getByRole('dialog', { name: 'Delete file?' })
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
		.getByRole('dialog', { name: 'Revoke agent?' })
		.getByRole('button', { name: 'Revoke' })
		.click();
	await expect(page.getByText('Agent revoked.')).toBeVisible();
	await expect(page.getByText(/Waiting for the agent to connect/)).toBeVisible();
	await expect(page.getByRole('button', { name: 'Revoke agent' })).toHaveCount(0);
});
