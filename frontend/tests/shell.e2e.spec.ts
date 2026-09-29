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

// F-329: the menu drawer shares the dialog fixes (F-196, F-145/F-264). A raw
// click right after Escape does not wait for the exit animation, so a closing
// backdrop that still catches pointer events loses it; the closed drawer must
// then leave the page.
test('shell.e2e a click right after closing the menu drawer reaches the page, 10 times', { tag: '@webkit' }, async ({ page }) => {
	test.setTimeout(90_000);
	await page.setViewportSize({ width: 375, height: 812 });
	await signIn(page);
	const drawer = page.getByRole('dialog');
	const theme = page.getByRole('button', { name: /^Switch to (dark|light) theme$/ });
	const box = (await theme.boundingBox())!;
	for (let i = 0; i < 10; i++) {
		const before = await theme.getAttribute('aria-label');
		await page.getByRole('button', { name: 'Open administration menu' }).click({ timeout: 3000 });
		await expect(drawer).toHaveAttribute('data-state', 'open');
		await page.keyboard.press('Escape');
		await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2);
		await expect(theme, `click ${i}`).not.toHaveAttribute('aria-label', before!, { timeout: 3000 });
		await expect(drawer, `closed ${i}`).toHaveCount(0);
	}
});

// F-145/F-264 (F-336): when slow frames make the close animation's end go
// unheard (WebKit stops painting under a modal), the closed drawer still
// leaves the page. Swallowing animationend makes that race certain.
test('shell.e2e the menu drawer leaves the page when its close animation end is missed', { tag: '@webkit' }, async ({ page }) => {
	await page.setViewportSize({ width: 375, height: 812 });
	await signIn(page);
	await page.getByRole('button', { name: 'Open administration menu' }).click();
	const drawer = page.getByRole('dialog');
	await expect(drawer).toHaveAttribute('data-state', 'open');
	await page.evaluate(() => addEventListener('animationend', (event) => event.stopImmediatePropagation(), true));
	await page.keyboard.press('Escape');
	await expect(drawer).toHaveCount(0);
	// F-351: its backdrop leaves too; a stuck one keeps dimming the page.
	await expect(page.locator('[data-scope="dialog"][data-part="backdrop"]:visible')).toHaveCount(0);
});

// F-326: a failed logout from the sticky sidebar is shown where the admin is
// looking, and it does not follow them to the next page.
test('shell.e2e shows a failed logout in view and clears it on the next page', async ({ page }) => {
	await page.setViewportSize({ width: 1280, height: 420 });
	await signIn(page);
	await expect(page.getByRole('heading', { name: 'Locations' })).toBeVisible();
	await page.route('**/api/auth/logout', (route) => route.fulfill({ status: 503, body: '' }));
	await page.evaluate(() => scrollTo(0, document.documentElement.scrollHeight));
	await expect.poll(() => page.evaluate(() => scrollY)).toBeGreaterThan(0);

	const sidebar = page.getByRole('complementary', { name: 'Administration' });
	await sidebar.getByRole('button', { name: 'Log out' }).click();
	const alert = page.getByRole('alert').filter({ hasText: 'Could not log out' });
	await expect(alert).toBeInViewport();

	await sidebar.getByRole('link', { name: 'Settings' }).click();
	await page.waitForURL(`${APP}/admin/settings`);
	await expect(alert).toHaveCount(0);
});

test('shell.e2e keeps the current-page marks in forced colours', async ({ page, browserName }) => {
	test.skip(browserName !== 'chromium', 'forced-colors emulation is Chromium-only');
	await page.emulateMedia({ forcedColors: 'active' });
	await signIn(page);

	// Forced colours paint every author colour as a system colour; a mark that
	// is drawn in Canvas is invisible against the page.
	const system = (name: string) =>
		page.evaluate((name) => {
			const probe = document.createElement('i');
			probe.style.cssText = `forced-color-adjust: none; color: ${name}`;
			document.body.append(probe);
			const colour = getComputedStyle(probe).color;
			probe.remove();
			return colour;
		}, name);
	const canvas = await system('Canvas');
	const style = (link: ReturnType<Page['getByRole']>, property: string, pseudo?: string) =>
		link.evaluate(
			(element, [property, pseudo]) => getComputedStyle(element, pseudo).getPropertyValue(property),
			[property, pseudo ?? null] as const
		);

	// Header: only the current link carries its underline bar; no link shows a top bar.
	const current = header(page).getByRole('link', { name: 'Administration' });
	const other = header(page).getByRole('link', { name: 'Diagnostics' });
	await expect.poll(() => style(current, 'border-bottom-color')).not.toBe(canvas);
	await expect.poll(() => style(current, 'border-top-color')).toBe(canvas);
	await expect.poll(() => style(other, 'border-bottom-color')).toBe(canvas);
	await expect.poll(() => style(other, 'border-top-color')).toBe(canvas);

	// Admin sidebar: the stations' vertical line stays painted, and only the
	// current section's station is filled.
	const sidebar = page.getByRole('complementary', { name: 'Administration' });
	await expect
		.poll(() => style(sidebar.getByRole('navigation'), 'background-color', '::before'))
		.not.toBe(canvas);
	const station = sidebar.getByRole('link', { name: 'Locations' });
	const otherStation = sidebar.getByRole('link', { name: 'Settings' });
	await expect.poll(() => style(station, 'background-color', '::before')).not.toBe(canvas);
	await expect.poll(() => style(otherStation, 'background-color', '::before')).toBe(canvas);

	// F-325: hovering the current station keeps its Highlight ring.
	const highlight = await system('Highlight');
	await expect.poll(() => style(station, 'border-top-color', '::before')).toBe(highlight);
	await station.hover();
	// Let any transition the hover started (the 160 ms border one) finish, so a
	// changed ring has landed.
	await station.evaluate((element) =>
		Promise.all(element.getAnimations({ subtree: true }).map((animation) => animation.finished))
	);
	expect(await style(station, 'border-top-color', '::before')).toBe(highlight);
});
