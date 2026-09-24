import { APP } from './ports';
import { expect, test, type Page } from '@playwright/test';

const statusPanel = (page: Page) => page.getByRole('group', { name: 'Location status' });

test.describe('diagnostics page', () => {
	test.beforeEach(async ({ page }) => {
		await page.goto(`${APP}/`);
		await expect(page.getByRole('tab', { name: 'Frankfurt (AS64501)' })).toBeVisible();
	});

	test('labels location tabs with name and ASN, defaulting to the first', async ({ page }) => {
		const tablist = page.getByRole('tablist', { name: 'Location' });
		await expect(tablist.getByRole('tab')).toHaveText([
			'Frankfurt (AS64501)',
			'Vienna (AS64500)',
			'London (AS64502)',
			'New York (AS64503)',
			'Singapore (AS64504)'
		]);
		// The offline, ASN-less San Francisco Hub is not in the public catalogue.
		await expect(page.getByRole('tab', { name: /San Francisco/ })).toHaveCount(0);
		await expect(page.getByRole('tab', { name: 'Frankfurt (AS64501)' })).toHaveAttribute(
			'aria-selected',
			'true'
		);
	});

	test('switches location by click and by arrow key', async ({ page }) => {
		await page.getByRole('tab', { name: 'Vienna (AS64500)' }).click();
		await expect(page.getByRole('tab', { name: 'Vienna (AS64500)' })).toHaveAttribute(
			'aria-selected',
			'true'
		);
		await expect(statusPanel(page).getByText('AS64500')).toBeVisible();

		// Arrow keys move the selection (Ark Tabs automatic activation).
		await page.getByRole('tab', { name: 'Vienna (AS64500)' }).focus();
		await page.keyboard.press('ArrowRight');
		await expect(page.getByRole('tab', { name: 'London (AS64502)' })).toHaveAttribute(
			'aria-selected',
			'true'
		);
		await expect(statusPanel(page).getByText('AS64502')).toBeVisible();
	});

	test("exposes the selected location's controls as its linked tab panel", async ({ page }) => {
		await page.getByRole('tab', { name: 'Vienna (AS64500)' }).click();
		const panel = page.getByRole('tabpanel', { name: 'Vienna (AS64500)' });
		await expect(panel.getByRole('combobox', { name: 'Method' })).toBeVisible();
		await expect(panel.getByRole('button', { name: 'Run Diagnostic' })).toBeVisible();
		await expect(page.getByRole('tabpanel')).toHaveCount(1);
	});

	test('method select lists only offered methods and resets on location change', async ({
		page
	}) => {
		const method = page.getByRole('combobox', { name: 'Method' });
		await method.click();
		await expect(page.getByRole('option', { name: 'Ping', exact: true })).toBeVisible();
		await expect(page.getByRole('option', { name: 'Ping (IPv6)' })).toBeVisible();
		await expect(page.getByRole('option', { name: 'MTR', exact: true })).toBeVisible();
		await expect(page.getByRole('option', { name: 'MTR (IPv6)' })).toBeVisible();
		await expect(page.getByRole('option', { name: 'Traceroute', exact: true })).toBeVisible();
		await expect(page.getByRole('option', { name: 'Traceroute (IPv6)' })).toBeVisible();
		// Frankfurt does not offer BGP.
		await expect(page.getByRole('option', { name: /BGP/ })).toHaveCount(0);

		await page.getByRole('option', { name: 'MTR (IPv6)' }).click();
		await expect(method).toHaveText('MTR (IPv6)');

		// Vienna has no mtr6 — the selection must reset to a valid method.
		await page.getByRole('tab', { name: 'Vienna (AS64500)' }).click();
		await expect(method).toHaveText('Ping');
		await method.click();
		await expect(page.getByRole('option', { name: 'BGP', exact: true })).toBeVisible();
		await expect(page.getByRole('option', { name: 'Traceroute (IPv6)' })).toHaveCount(0);
	});

	test('adapts the target placeholder and refuses non-public IPv4 (BGP exempt)', async ({
		page
	}) => {
		const target = page.getByLabel('Target');
		await expect(target).toHaveAttribute('placeholder', 'e.g. 1.1.1.1 or example.com');

		await target.fill('10.0.0.1');
		await expect(page.getByRole('alert')).toHaveText(
			'Enter a publicly routable IPv4 address or hostname.'
		);
		await expect(page.getByRole('button', { name: 'Run Diagnostic' })).toBeDisabled();

		// BGP takes route prefixes and is exempt from the client-side refusal.
		await page.getByRole('tab', { name: 'Vienna (AS64500)' }).click();
		await page.getByRole('combobox', { name: 'Method' }).click();
		await page.getByRole('option', { name: 'BGP', exact: true }).click();
		await expect(target).toHaveAttribute('placeholder', 'e.g. 8.8.8.0/24 or 2001:db8::/32');
		await expect(page.getByRole('alert')).toHaveCount(0);
		await expect(page.getByRole('button', { name: 'Run Diagnostic' })).toBeEnabled();
	});

	test('status panel shows location and visitor facts', async ({ page }) => {
		await page.getByRole('tab', { name: 'Vienna (AS64500)' }).click();

		const status = statusPanel(page);
		await expect(status.getByText('Status')).toBeVisible();
		await expect(status.getByText('Online')).toBeVisible();
		await expect(status.getByText('Network')).toBeVisible();
		await expect(status.getByText('AS64500')).toBeVisible();

		// Geo label with map link; facility with its link.
		await expect(status.getByText('Vienna, AT')).toBeVisible();
		const mapLink = page.getByRole('link', { name: 'Open map for Vienna, AT' });
		await expect(mapLink).toHaveAttribute(
			'href',
			'https://www.openstreetmap.org/search?query=Vienna'
		);
		await expect(
			page.getByRole('link', { name: 'Open facility page for Interxion VIE1' })
		).toHaveAttribute('href', 'https://www.digitalrealty.com/data-centers/vienna/vie1');

		// First Test IPs per family with copy buttons; extras behind the collapsible.
		await expect(status.getByText('192.0.2.1', { exact: true })).toBeVisible();
		await expect(status.getByText('2001:db8::1', { exact: true })).toBeVisible();
		await expect(status.getByRole('button', { name: 'Copy IPv4 test IP' })).toBeVisible();
		await expect(status.getByRole('button', { name: 'Copy IPv6 test IP' })).toBeVisible();
		await expect(page.getByText('192.0.2.2')).toBeHidden();
		await status.getByText('More test IPs').click();
		await expect(status.getByText('192.0.2.2')).toBeVisible();
		await expect(status.getByText('2001:db8::2')).toBeVisible();

		// Visitor facts.
		await expect(status.getByText('Your IP')).toBeVisible();
		await expect(status.getByText('198.51.100.7')).toBeVisible();
		await expect(status.getByText('Latency ≈')).toBeVisible();
		const latency = status.locator(
			'xpath=//span[text()="Latency ≈"]/following-sibling::span'
		);
		await expect(latency).toHaveText(/^(\d+ ms|—)$/, { timeout: 5_000 });
	});
});
