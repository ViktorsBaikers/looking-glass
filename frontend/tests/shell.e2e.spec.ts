import { APP } from './ports';
import { expect, test, type Page } from '@playwright/test';

// Canonical shell (issue #2): Administration link by session, theme persistence,
// fail-closed admin gate, and the mobile sidebar drawer.

function header(page: Page) {
	return page.getByRole('banner');
}

async function signIn(page: Page) {
	await page.goto(`${APP}/login`);
	await page.getByLabel('Username').fill('brooke');
	await page.getByLabel('Password').fill('fixture-password');
	await page.getByRole('button', { name: 'Sign in' }).click();
	await page.waitForURL(`${APP}/admin`);
}

test.beforeEach(async ({ page }) => {
	await page.setExtraHTTPHeaders({ 'x-looking-glass-fixture': `shell-${crypto.randomUUID()}` });
});

test('shell.e2e shows the Administration link only while signed in', async ({ page }) => {
	await page.goto(`${APP}/`);
	await expect(header(page).getByRole('link', { name: 'Diagnostics' })).toBeVisible();
	await expect(header(page).getByRole('link', { name: 'Administration' })).toHaveCount(0);

	await signIn(page);
	await expect(header(page).getByRole('link', { name: 'Administration' })).toBeVisible();

	await page.getByRole('button', { name: 'Log out' }).click();
	await page.waitForURL(`${APP}/login`);
	await page.goto(`${APP}/`);
	await expect(header(page).getByRole('link', { name: 'Diagnostics' })).toBeVisible();
	await expect(header(page).getByRole('link', { name: 'Administration' })).toHaveCount(0);
});

test('shell.e2e redirects unauthenticated admin pages to sign-in', async ({ page }) => {
	for (const path of ['/admin', '/admin/locations/fra?tab=methods', '/admin/administrators', '/admin/settings']) {
		await page.goto(`${APP}${path}`);
		await page.waitForURL(`${APP}/login`);
	}
});

test('shell.e2e sends a revoked session to sign-in on the next admin navigation', async ({ page }) => {
	await signIn(page);
	await expect(page.getByRole('heading', { name: 'Locations' })).toBeVisible();
	// End the session behind the page's back (expiry, removal by a peer).
	await page.evaluate(() => fetch('/api/auth/logout', { method: 'POST' }));
	await page.getByRole('link', { name: 'Settings' }).click();
	await page.waitForURL(`${APP}/login`);
});

test('shell.e2e remembers the chosen theme across reloads', async ({ page }) => {
	await page.goto(`${APP}/`);
	const html = page.locator('html');
	const startsDark = /\bdark\b/.test((await html.getAttribute('class')) ?? '');
	await page
		.getByRole('button', { name: startsDark ? 'Switch to light theme' : 'Switch to dark theme' })
		.click();

	for (const reload of [false, true]) {
		if (reload) await page.reload();
		if (startsDark) await expect(html).not.toHaveClass(/\bdark\b/);
		else await expect(html).toHaveClass(/\bdark\b/);
	}
});

test('shell.e2e puts the admin sidebar in a drawer on phones', async ({ page }) => {
	await page.setViewportSize({ width: 375, height: 812 });
	await signIn(page);
	await expect(page.getByRole('link', { name: 'Administrators' })).toBeHidden();

	await page.getByRole('button', { name: 'Open administration menu' }).click();
	const drawer = page.getByRole('dialog');
	await drawer.getByRole('link', { name: 'Administrators' }).click();
	await page.waitForURL(`${APP}/admin/administrators`);
	await expect(page.getByRole('heading', { name: 'Administrators' })).toBeVisible();
});
