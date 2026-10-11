// Accessibility and layout checks in real Chromium and WebKit: contrast from the resolved
// tokens, focus kept through busy states, described-by wiring, reflow at 320 px,
// sticky-bar focus obscuring, forced colours, touch tooltips and confirm copy.
import { APP } from './ports';
import { expect, test, type Locator, type Page } from '@playwright/test';

const SCHEMES = ['light', 'dark'] as const;

async function signIn(page: Page, fixtureId: string) {
	const response = await page.context().request.post(`${APP}/api/auth/login`, {
		data: { username: 'brooke', password: 'fixture-password' },
		headers: { 'x-looking-glass-fixture': fixtureId }
	});
	expect(response.status()).toBe(204);
	await page.setExtraHTTPHeaders({ 'x-looking-glass-fixture': fixtureId });
}

/** WCAG contrast of `prop` against the nearest painted background at or above `bgFrom`. */
async function contrast(target: Locator, prop: 'color' | 'border-top-color', bgFrom?: Locator) {
	return target.evaluate(
		(el, [prop, bgEl]) => {
			const rgb = (value: string) => {
				const m = value.match(/[\d.]+/g)?.map(Number) ?? [0, 0, 0, 0];
				return { c: m.slice(0, 3), a: m.length > 3 ? m[3] : 1 };
			};
			let node: Element | null = (bgEl as Element | null) ?? el;
			let bg = rgb('rgba(0,0,0,0)');
			while (node && (bg = rgb(getComputedStyle(node).backgroundColor)).a === 0) {
				node = node.parentElement;
			}
			const lum = ([r, g, b]: number[]) => {
				const f = (v: number) => {
					const s = v / 255;
					return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
				};
				return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b);
			};
			const a = lum(rgb(getComputedStyle(el).getPropertyValue(prop as string)).c);
			const b = lum(bg.c);
			return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
		},
		[prop, bgFrom ? await bgFrom.elementHandle() : null] as const
	);
}

const horizontalOverflow = (page: Page) =>
	page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);

const focusedId = (page: Page) => page.evaluate(() => document.activeElement?.id || document.activeElement?.tagName);

/**
 * The colours painted in `clip`: the luminance of the one most of it shows (the
 * ground) and the best contrast any other painted colour there has against it.
 */
async function painted(page: Page, clip: { x: number; y: number; width: number; height: number }) {
	const png = await page.screenshot({ clip, animations: 'disabled' });
	return page.evaluate(async (b64) => {
		const img = new Image();
		img.src = `data:image/png;base64,${b64}`;
		await img.decode();
		const canvas = document.createElement('canvas');
		[canvas.width, canvas.height] = [img.width, img.height];
		const ctx = canvas.getContext('2d')!;
		ctx.drawImage(img, 0, 0);
		const data = ctx.getImageData(0, 0, img.width, img.height).data;
		const counts = new Map<string, number>();
		for (let i = 0; i < data.length; i += 4) {
			const key = `${data[i]},${data[i + 1]},${data[i + 2]}`;
			counts.set(key, (counts.get(key) ?? 0) + 1);
		}
		const lum = (key: string) => {
			const [r, g, b] = key.split(',').map((v) => {
				const s = Number(v) / 255;
				return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
			});
			return 0.2126 * r + 0.7152 * g + 0.0722 * b;
		};
		const ground = lum([...counts].sort((a, b) => b[1] - a[1])[0][0]);
		let best = 1;
		for (const key of counts.keys()) {
			const l = lum(key);
			best = Math.max(best, (Math.max(l, ground) + 0.05) / (Math.min(l, ground) + 0.05));
		}
		return { ground, best };
	}, png.toString('base64'));
}

/** Scrolls so only the top `visible` px of `target` show above the Save bar. */
async function peekAboveBar(target: Locator, visible: number) {
	const shown = await target.evaluate((el, visible) => {
		const bar = document.querySelector('form button[type="submit"]')!.parentElement!;
		for (let i = 0; i < 3; i++) {
			scrollBy(0, el.getBoundingClientRect().top - (bar.getBoundingClientRect().top - visible));
		}
		return bar.getBoundingClientRect().top - el.getBoundingClientRect().top;
	}, visible);
	expect(Math.abs(shown - visible), 'placed behind the bar').toBeLessThanOrEqual(1);
	return (await target.boundingBox())!;
}

test.describe('contrast from resolved tokens', () => {
	for (const scheme of SCHEMES) {
		test(`error toast text is at least 4.5:1 (${scheme})`, async ({ page }) => {
			await page.emulateMedia({ colorScheme: scheme });
			await signIn(page, `a11y-toast-${crypto.randomUUID()}`);
			await page.goto(`${APP}/admin/settings`);
			await page.getByLabel('Logo URL (optional)').fill('http://example.test/logo.svg');
			await page.getByRole('button', { name: 'Save settings' }).click();
			const toast = page.locator('[data-scope="toast"][data-part="root"][data-type="error"]');
			await expect(toast).toContainText('Logo and terms URLs must use https.');
			expect(await contrast(toast, 'color')).toBeGreaterThanOrEqual(4.5);
		});

		// F-261: an unselected Location tab's roundel code reads at 4.5:1 too, and
		// the selected roundel still looks different from the others.
		test(`unselected Location roundel codes are at least 4.5:1 (${scheme})`, async ({ page }) => {
			await page.emulateMedia({ colorScheme: scheme });
			await page.goto(`${APP}/`);
			const tabs = page.locator('[role="tab"][data-code]');
			await expect(tabs.nth(1)).toBeVisible();
			await page.mouse.move(0, 0);
			const results: { name: string; selected: boolean; ground: number; best: number }[] = [];
			for (const tab of await tabs.all()) {
				// The roundel is the trigger's ::before: the first flex item, centred.
				const clip = await tab.evaluate((el) => {
					const r = el.getBoundingClientRect();
					const s = getComputedStyle(el);
					const b = getComputedStyle(el, '::before');
					const [w, h] = [parseFloat(b.width), parseFloat(b.height)];
					const top = r.y + parseFloat(s.paddingTop) + parseFloat(s.borderTopWidth);
					const inner = r.height - parseFloat(s.paddingTop) - parseFloat(s.paddingBottom) - parseFloat(s.borderTopWidth) - parseFloat(s.borderBottomWidth);
					const x = r.x + parseFloat(s.paddingLeft) + parseFloat(s.borderLeftWidth);
					const y = top + (inner - h) / 2;
					// The code's glyphs, clear of the disc's edge.
					return { x: x + 7, y: y + 9, width: w - 14, height: h - 18 };
				});
				const name = (await tab.getAttribute('data-code')) ?? '';
				const selected = (await tab.getAttribute('aria-selected')) === 'true';
				results.push({ name, selected, ...(await painted(page, clip)) });
			}
			expect(results.filter((r) => r.selected)).toHaveLength(1);
			for (const r of results) expect(r.best, `${r.name} roundel code`).toBeGreaterThanOrEqual(4.5);
			const selected = results.find((r) => r.selected)!;
			for (const r of results.filter((r) => !r.selected)) {
				expect(r.ground, `${r.name} looks unselected`).not.toBeCloseTo(selected.ground, 2);
			}
		});

		test(`route-view hop numbers are at least 4.5:1 (${scheme})`, async ({ page }) => {
			await page.emulateMedia({ colorScheme: scheme });
			await page.goto(`${APP}/`);
			await page.getByRole('combobox', { name: 'Method' }).click();
			await page.getByRole('option', { name: 'MTR', exact: true }).click();
			// The Select hands focus back to its trigger a frame after it closes; a fill
			// that starts before then types into the trigger instead (seen in WebKit).
			await expect(page.getByRole('combobox', { name: 'Method' })).toBeFocused();
			await page.getByLabel('Target').fill('1.1.1.1');
			await expect(page.getByLabel('Target')).toHaveValue('1.1.1.1');
			await page.getByRole('button', { name: 'Run diagnostic' }).click();
			const hop = page.getByRole('row').filter({ hasText: '192.0.2.1' }).locator('td').nth(1);
			await expect(hop).toHaveText('1');
			expect(await contrast(hop, 'color')).toBeGreaterThanOrEqual(4.5);
		});

		// F-220: placeholders carry the format examples; they must read like text.
		test(`input placeholders are at least 4.5:1 (${scheme})`, async ({ page }) => {
			await page.emulateMedia({ colorScheme: scheme });
			await page.goto(`${APP}/`);
			const target = page.getByLabel('Target');
			await expect(target).toHaveAttribute('placeholder', /\S/);
			const ratio = await target.evaluate((el) => {
				const lum = (value: string) => {
					const [r, g, b] = (value.match(/[\d.]+/g) ?? []).map(Number).map((v) => {
						const s = v / 255;
						return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
					});
					return 0.2126 * r + 0.7152 * g + 0.0722 * b;
				};
				const a = lum(getComputedStyle(el, '::placeholder').color);
				const b = lum(getComputedStyle(el).backgroundColor);
				return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
			});
			expect(ratio).toBeGreaterThanOrEqual(4.5);
		});

		test(`input, select and checkbox boundaries are at least 3:1 (${scheme})`, async ({ page }) => {
			await page.emulateMedia({ colorScheme: scheme });
			await signIn(page, `a11y-borders-${crypto.randomUUID()}`);
			await page.goto(`${APP}/admin/settings`);
			for (const id of ['site-title', 'default-theme']) {
				const control = page.locator(`#${id}`);
				await expect(control).toBeVisible();
				expect(await contrast(control, 'border-top-color', control.locator('..'))).toBeGreaterThanOrEqual(3);
			}
			await page.goto(`${APP}/admin/locations/vie?tab=methods`);
			const box = page.locator('[data-scope="checkbox"][data-part="control"][data-state="unchecked"]').first();
			await expect(box).toBeVisible();
			expect(await contrast(box, 'border-top-color', box.locator('..'))).toBeGreaterThanOrEqual(3);
		});
	}
});

test('the More test IPs disclosure is one button, not a button in a button', async ({ page }) => {
	await page.goto(`${APP}/`);
	await page.getByRole('tab', { name: 'Vienna (AS64500)' }).click();
	const more = page.getByRole('button', { name: 'More test IPs' });
	await expect(more).toHaveAttribute('aria-expanded', 'false');
	await expect(page.locator('button button')).toHaveCount(0);
	await more.click();
	await expect(more).toHaveAttribute('aria-expanded', 'true');
	await expect(page.getByText('192.0.2.2', { exact: true })).toBeVisible();
});

test.describe('focus stays on the control that started the work', () => {
	test('Enter in Target keeps focus there through the run', async ({ page }) => {
		await page.goto(`${APP}/`);
		const target = page.getByLabel('Target');
		await target.fill('slow.test');
		await target.press('Enter');
		await expect(page.getByRole('button', { name: 'Cancel' })).toBeVisible();
		expect(await focusedId(page)).toBe('target');
		await expect(target).toHaveAttribute('aria-disabled', 'true');
		// A second Enter in the busy field must not cancel the run.
		await target.press('Enter');
		await expect(page.getByRole('button', { name: 'Cancel' })).toBeVisible();
		await page.getByRole('button', { name: 'Cancel' }).click();
		await expect(page.getByRole('button', { name: 'Run diagnostic' })).toBeVisible();
	});

	test('starting the speed test keeps focus on its button and announces the result', async ({
		page
	}) => {
		test.setTimeout(60_000);
		// Hold the upload so the busy state is observable against the fast fixture.
		const { promise: released, resolve: release } = Promise.withResolvers<void>();
		await page.route('**/api/locations/fra/speedtest/upload', async (route) => {
			await released;
			await route.continue();
		});
		// Stop the page clock so the run's own 10 s download and upload deadlines
		// never end the held run before a slowed runner sees it busy (F-172).
		await page.clock.install();
		await page.goto(`${APP}/`);
		await page.clock.pauseAt(Date.now() + 60_000);
		const start = page.getByRole('button', { name: 'Start speed test' });
		await start.focus();
		await page.keyboard.press('Enter');
		const busy = page.getByRole('button', { name: 'Testing…' });
		await expect(busy).toBeVisible();
		expect(await page.evaluate(() => document.activeElement?.textContent?.trim())).toBe('Testing…');
		await expect(busy).toHaveAttribute('aria-busy', 'true');
		// Let 9 s of page time pass with the upload still held, inside the 10 s
		// deadlines, so a blur on a timer or animation frame shows up here.
		await page.clock.runFor(9_000);
		await expect(busy).toBeFocused();
		release();
		await expect(start).toBeVisible({ timeout: 25_000 });
		// The clock is still paused: run it on so a blur the settled run left on a
		// timer or animation frame fires before the focus check.
		await page.clock.runFor(1_000);
		await expect(start).toBeFocused();
		await expect(page.locator('[aria-live="polite"]').filter({ hasText: /Download \d+ Mbps/ })).toHaveCount(1);
	});

	/** Presses the busy button by pointer, keys and Space; it keeps focus and does nothing. */
	async function pressBusy(page: Page, button: Locator) {
		await expect(button).toHaveAttribute('aria-busy', 'true');
		await expect(button).toHaveAttribute('aria-disabled', 'true');
		await expect(button).toBeFocused();
		await page.keyboard.press('Enter');
		await page.keyboard.press('Space');
		const box = (await button.boundingBox())!;
		await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2);
		await expect(button).toBeFocused();
	}

	test('saving settings keeps focus on the busy Save button', async ({ page }) => {
		const { promise: released, resolve: release } = Promise.withResolvers<void>();
		let puts = 0;
		await page.route('**/api/admin/settings', async (route) => {
			if (route.request().method() === 'PUT') {
				puts++;
				await released;
			}
			await route.continue();
		});
		await signIn(page, `a11y-busy-save-${crypto.randomUUID()}`);
		await page.goto(`${APP}/admin/settings`);
		const save = page.getByRole('button', { name: 'Save settings' });
		await save.focus();
		await page.keyboard.press('Enter');
		await pressBusy(page, save);
		release();
		await expect(page.getByText('Settings saved.')).toBeVisible();
		await expect(save).not.toHaveAttribute('aria-busy');
		await expect(save).toBeFocused();
		expect(puts).toBe(1);
	});

	test('a busy confirm keeps focus on its button', async ({ page }) => {
		const { promise: released, resolve: release } = Promise.withResolvers<void>();
		let deletes = 0;
		await page.route('**/api/admin/locations/fra', async (route) => {
			if (route.request().method() === 'DELETE') {
				deletes++;
				await released;
			}
			await route.continue();
		});
		await signIn(page, `a11y-busy-confirm-${crypto.randomUUID()}`);
		await page.goto(`${APP}/admin`);
		await page.getByRole('button', { name: 'Delete Frankfurt' }).click();
		// The dialog places its initial focus a frame after opening; move on only after that.
		await expect(page.getByRole('dialog').locator(':focus')).toHaveCount(1);
		const confirm = page.getByRole('dialog').getByRole('button', { name: 'Delete location' });
		await confirm.focus();
		await page.keyboard.press('Enter');
		await pressBusy(page, confirm);
		release();
		await expect(page.getByText('Deleted Frankfurt and everything under it.')).toBeVisible();
		expect(deletes).toBe(1);
		await expect(page.locator('main')).toBeFocused();
	});

	// The delete removes the dialog's opener, so focus cannot go back to it; it must
	// land somewhere visible (the main landmark), never on <body> or a hidden button.
	test('a confirmed delete returns focus to the page, not a hidden button', async ({ page }) => {
		await signIn(page, `a11y-delete-focus-${crypto.randomUUID()}`);
		await page.goto(`${APP}/admin`);
		const opener = page.getByRole('button', { name: 'Delete Frankfurt' });
		await opener.focus();
		// While the opener is still there, closing returns focus to it.
		await page.keyboard.press('Enter');
		await expect(page.getByRole('dialog').locator(':focus')).toHaveCount(1);
		await page.getByRole('dialog').getByRole('button', { name: 'Cancel' }).focus();
		await page.keyboard.press('Enter');
		await expect(opener).toBeFocused();
		await page.keyboard.press('Enter');
		await expect(page.getByRole('dialog').locator(':focus')).toHaveCount(1);
		await page.getByRole('dialog').getByRole('button', { name: 'Delete location' }).focus();
		await page.keyboard.press('Enter');
		await expect(page.getByText('Deleted Frankfurt and everything under it.')).toBeVisible();
		await expect(page.locator('main')).toBeFocused();
		const focused = await page.evaluate(() => {
			const el = document.activeElement;
			return { tag: el?.tagName, connected: !!el?.isConnected, visible: !!el?.checkVisibility() };
		});
		expect(focused).toEqual({ tag: 'MAIN', connected: true, visible: true });
	});

	// Safari does not focus a button on click, so the dialog can open with nothing focused.
	test('a dialog opened with nothing focused closes to the page, not a hidden button', async ({ page }) => {
		await signIn(page, `a11y-unfocused-open-${crypto.randomUUID()}`);
		await page.goto(`${APP}/admin`);
		await page.getByRole('button', { name: 'Delete Frankfurt' }).evaluate((el: HTMLElement) => {
			(document.activeElement as HTMLElement | null)?.blur();
			el.click();
		});
		await expect(page.getByRole('dialog').locator(':focus')).toHaveCount(1);
		await page.getByRole('dialog').getByRole('button', { name: 'Cancel' }).focus();
		await page.keyboard.press('Enter');
		await expect(page.locator('main')).toBeFocused();
	});

	// These forms pass disabled={!canSubmit} and canSubmit includes !submitting, so the
	// explicit disabled is on while loading; the busy button must still keep focus.
	const FORMS = [
		{
			name: 'Sign in',
			fixture: 'a11y-busy-login',
			url: '/login',
			post: '**/api/auth/login',
			fields: [['Username', 'brooke'], ['Password', 'fixture-password']],
			next: '/admin'
		},
		{
			name: 'Create account',
			fixture: 'fresh-install-a11y-busy',
			url: '/install',
			post: '**/api/setup',
			fields: [
				['Setup token', 'fixture-setup-token'],
				['Username', 'admin'],
				['Password', 'fixture-password'],
				['Confirm password', 'fixture-password']
			],
			next: '/login'
		},
		{
			name: 'Set password',
			fixture: 'a11y-busy-activate',
			url: '/activate/fixture-activation-token',
			post: '**/api/activate/fixture-activation-token',
			fields: [['Password', 'dana-activated-password'], ['Confirm password', 'dana-activated-password']],
			next: '/login'
		}
	];
	for (const form of FORMS) {
		test(`a busy ${form.name} button keeps focus and submits once`, async ({ page }) => {
			const { promise: released, resolve: release } = Promise.withResolvers<void>();
			let posts = 0;
			await page.route(form.post, async (route) => {
				if (route.request().method() === 'POST') {
					posts++;
					await released;
				}
				await route.continue();
			});
			await page.setExtraHTTPHeaders({ 'x-looking-glass-fixture': `${form.fixture}-${crypto.randomUUID()}` });
			await page.goto(`${APP}${form.url}`);
			for (const [label, value] of form.fields) await page.getByLabel(label, { exact: true }).fill(value);
			const submit = page.locator('form button[type="submit"]');
			await expect(submit).toHaveText(form.name);
			await submit.focus();
			await page.keyboard.press('Enter');
			await pressBusy(page, submit);
			// Enter in a busy field: the field is read-only, so it takes focus and sends nothing.
			const field = page.getByLabel(form.fields[0][0], { exact: true });
			await field.press('Enter');
			await expect(field).toBeFocused();
			release();
			await page.waitForURL(`${APP}${form.next}`);
			expect(posts).toBe(1);
		});
	}

	// Busy fields are read-only, not disabled: disabling the focused field drops focus
	// to <body>, and Enter in a field is the usual keyboard submit.
	const FIELD_FORMS: {
		name: string;
		fixture: string;
		url: string;
		signIn?: boolean;
		open?: string;
		method: string;
		request: string;
		fields: string[][];
		done: (page: Page) => Promise<unknown>;
	}[] = [
		...FORMS.map((form) => ({
			...form,
			method: 'POST',
			request: form.post,
			done: (page: Page) => page.waitForURL(`${APP}${form.next}`)
		})),
		{
			name: 'Create activation link',
			fixture: 'a11y-busy-admin-create',
			url: '/admin/administrators',
			signIn: true,
			method: 'POST',
			request: '**/api/admin/administrators',
			fields: [['Username', 'frank']],
			done: (page) => expect(page.getByRole('dialog', { name: /Activation link for frank/ })).toBeVisible()
		},
		{
			name: 'Change password',
			fixture: 'a11y-busy-password',
			url: '/admin/administrators',
			signIn: true,
			method: 'PUT',
			request: '**/api/admin/me/password',
			fields: [
				['Current password', 'fixture-password'],
				['New password', 'another-long-password'],
				['Confirm new password', 'another-long-password']
			],
			done: (page) => expect(page.getByText('Password changed.').first()).toBeVisible()
		},
		{
			name: 'Add location',
			fixture: 'a11y-busy-new-location',
			url: '/admin',
			signIn: true,
			open: 'Add location',
			method: 'POST',
			request: '**/api/admin/locations',
			fields: [
				['Display name', 'Oslo'],
				['Geographic label', 'Oslo, NO']
			],
			done: (page) => page.waitForURL(/\/admin\/locations\/[^/?]+\?tab=settings$/)
		}
	];
	for (const form of FIELD_FORMS) {
		test(`Enter in a busy ${form.name} field keeps focus there and submits once`, async ({ page }) => {
			const { promise: released, resolve: release } = Promise.withResolvers<void>();
			let sent = 0;
			await page.route(form.request, async (route) => {
				if (route.request().method() === form.method) {
					sent++;
					await released;
				}
				await route.continue();
			});
			const fixtureId = `${form.fixture}-${crypto.randomUUID()}`;
			if (form.signIn) await signIn(page, fixtureId);
			else await page.setExtraHTTPHeaders({ 'x-looking-glass-fixture': fixtureId });
			await page.goto(`${APP}${form.url}`);
			if (form.open) {
				await page.getByRole('button', { name: form.open }).first().click();
				await expect(page.getByRole('dialog').locator(':focus')).toHaveCount(1);
			}
			const fields = form.fields.map(([label]) => page.getByLabel(label, { exact: true }));
			for (const [i, [, value]] of form.fields.entries()) await fields[i].fill(value);
			const field = fields[fields.length - 1];
			const value = form.fields[fields.length - 1][1];
			const submit = page.locator('form').filter({ has: field }).locator('button[type="submit"]');
			await field.press('Enter');
			await expect(submit).toHaveAttribute('aria-busy', 'true');
			await expect(field).toBeFocused();
			await expect(page.locator('form').filter({ has: field })).toHaveAttribute('aria-busy', 'true');
			// Every field is natively read-only (not disabled, and not only aria-disabled),
			// so Enter from any of them keeps focus.
			for (const f of fields) expect(await f.evaluate((el: HTMLInputElement) => el.readOnly && !el.disabled)).toBe(true);
			// Typing in the busy field changes nothing, and a second Enter sends nothing.
			await page.keyboard.type('x');
			await page.keyboard.press('Enter');
			await expect(field).toHaveValue(value);
			await expect(field).toBeFocused();
			release();
			await form.done(page);
			expect(sent).toBe(1);
		});
	}
});

test('info icons in the Settings form open their tooltip without submitting it', async ({ page }) => {
	let puts = 0;
	page.on('request', (request) => {
		if (request.method() === 'PUT' && request.url().endsWith('/api/admin/settings')) puts++;
	});
	await signIn(page, `a11y-info-${crypto.randomUUID()}`);
	await page.goto(`${APP}/admin/settings`);
	const about = page.getByRole('button', { name: 'About Global concurrency cap' });
	await about.click();
	await page.getByLabel('Global concurrency cap').focus();
	await page.keyboard.press('Shift+Tab');
	await expect(about).toBeFocused();
	await expect(page.getByRole('tooltip')).toContainText('The most runs that can execute at the same time.');
	await page.keyboard.press('Enter');
	await page.keyboard.press('Space');
	// Only the real Save sends the settings; any earlier submit would have been first.
	await page.getByRole('button', { name: 'Save settings' }).click();
	await expect(page.getByText('Settings saved.').first()).toBeVisible();
	expect(puts, 'settings saves').toBe(1);
});

test('Run brings the output up just below the sticky header', async ({ page }) => {
	await page.setViewportSize({ width: 390, height: 844 });
	await page.emulateMedia({ reducedMotion: 'reduce' });
	await page.goto(`${APP}/`);
	await page.getByLabel('Target').fill('1.1.1.1');
	await page.getByRole('button', { name: 'Run diagnostic' }).click();
	const output = page.getByRole('region', { name: 'Output' });
	const gap = () =>
		output.evaluate((el) => el.getBoundingClientRect().top - document.querySelector('header')!.getBoundingClientRect().bottom);
	await expect.poll(gap).toBeLessThanOrEqual(24);
	expect(await gap()).toBeGreaterThanOrEqual(0);
});

test('Field errors and hints describe their control', async ({ page }) => {
	await page.goto(`${APP}/activate/fixture-activation-token`);
	const password = page.getByLabel('Password', { exact: true });
	await password.fill('short');
	await expect(password).toHaveAccessibleDescription('At least 12 characters.');

	await signIn(page, `a11y-describe-${crypto.randomUUID()}`);
	await page.goto(`${APP}/admin/settings`);
	await expect(page.locator('#default-theme')).toHaveAccessibleDescription(
		"This changes the default only. It does not replace a visitor's saved preference."
	);
});

// Each location tab mounts its own Target field; the error must describe the
// visible input after a switch, not the outgoing panel's one.
test('the Target error still describes the input after a location switch', async ({ page }) => {
	await page.goto(`${APP}/`);
	const target = page.locator('#target');
	await target.fill('10.0.0.1');
	await expect(target).toHaveAccessibleDescription(/publicly routable/);
	for (const name of ['Vienna (AS64500)', 'Frankfurt (AS64501)']) {
		await page.getByRole('tab', { name }).click();
		await expect(page.getByRole('tab', { name })).toHaveAttribute('aria-selected', 'true');
		await expect(target).toHaveCount(1);
		await expect(target).toHaveAccessibleDescription(/publicly routable/);
	}
});

test('the activate page names the upper length limit', async ({ page }) => {
	await page.goto(`${APP}/activate/fixture-activation-token`);
	await page.getByLabel('Password', { exact: true }).fill('x'.repeat(513));
	await expect(page.getByText('At most 512 characters.')).toBeVisible();
	await expect(page.getByText('At least 12 characters.')).toHaveCount(0);
});

// Beside the 220 px admin sidebar the Locations list's fixed columns must still
// fit: no sideways scroll, and every name cell at least as wide as its longest word.
for (const width of [1024, 1100, 1280]) {
	test(`the Locations list fits at ${width} px with readable names`, async ({ page }) => {
		await signIn(page, `a11y-list-${width}-${crypto.randomUUID()}`);
		await page.setViewportSize({ width, height: 800 });
		for (const scheme of SCHEMES) {
			await page.emulateMedia({ colorScheme: scheme });
			await page.goto(`${APP}/admin`);
			await expect(page.getByRole('button', { name: 'Delete Frankfurt' })).toBeVisible();
			expect.soft(await horizontalOverflow(page), scheme).toBe(0);
			const cramped = await page.locator('ul li h2').evaluateAll((titles) =>
				titles.flatMap((title) => {
					// Each word's unwrapped width in the title's own font.
					const probe = title.appendChild(document.createElement('span'));
					probe.style.cssText = 'position: absolute; white-space: nowrap';
					let widest = 0;
					for (const word of title.firstChild!.textContent!.split(' ')) {
						probe.textContent = word;
						widest = Math.max(widest, probe.getBoundingClientRect().width);
					}
					probe.remove();
					const cell = title.getBoundingClientRect().width;
					return cell + 0.5 < widest ? [`${title.textContent}: ${cell}px < ${widest}px`] : [];
				})
			);
			expect(cramped, scheme).toEqual([]);
		}
	});
}

test.describe('reflow at 320 px', () => {
	test.use({ viewport: { width: 320, height: 640 } });

	test('a long site title does not scroll Settings sideways', async ({ page }) => {
		await signIn(page, `a11y-title-${crypto.randomUUID()}`);
		await page.goto(`${APP}/admin/settings`);
		await page.getByLabel('Site title').fill('Example Networks Looking Glass Operations Center');
		await expect(page.getByText('Unsaved changes.')).toBeVisible();
		expect(await horizontalOverflow(page)).toBe(0);
	});

	test('a long username does not scroll Administrators sideways', async ({ page }) => {
		const fixtureId = `a11y-username-${crypto.randomUUID()}`;
		await signIn(page, fixtureId);
		const name = 'svc_monitoring_automation_backbone_ops_team';
		const created = await page.context().request.post(`${APP}/api/admin/administrators`, {
			data: { username: name },
			headers: { 'x-looking-glass-fixture': fixtureId }
		});
		expect(created.status()).toBe(201);
		await page.goto(`${APP}/admin/administrators`);
		await expect(page.getByText(name, { exact: true })).toBeVisible();
		expect(await horizontalOverflow(page)).toBe(0);
	});

	test('the "Not enrolled" status does not scroll Locations sideways', async ({ page }) => {
		await signIn(page, `a11y-locations-${crypto.randomUUID()}`);
		await page.goto(`${APP}/admin`);
		await expect(page.locator('li').getByText('Not enrolled', { exact: true })).toBeVisible();
		expect(await horizontalOverflow(page)).toBe(0);
	});

	test('gigabit speed readouts stay inside their card', async ({ page }) => {
		await page.goto(`${APP}/`);
		const results = page.getByRole('group', { name: 'Speed test results' });
		await expect(results).toBeVisible();
		// Layout only: put 4-digit values where a gigabit result would land.
		const [unitRight, cardInnerRight] = await results.evaluate((group) => {
			for (const row of group.querySelectorAll(':scope > div > div:last-child')) {
				row.firstElementChild!.textContent = '1000';
			}
			const card = group.parentElement!;
			const style = getComputedStyle(card);
			const inner =
				card.getBoundingClientRect().right - parseFloat(style.borderRightWidth) - parseFloat(style.paddingRight);
			const units = [...group.querySelectorAll(':scope > div > div:last-child > :last-child')];
			return [Math.max(...units.map((unit) => unit.getBoundingClientRect().right)), inner];
		});
		expect(unitRight).toBeLessThanOrEqual(cardInnerRight + 0.5);
	});

	test('no control on the public page is cut off at the side, More test IPs open', async ({ page }) => {
		await page.goto(`${APP}/`);
		await page.getByRole('tab', { name: 'Vienna (AS64500)' }).click();
		await page.getByRole('button', { name: 'More test IPs' }).click();
		await expect(page.getByText('192.0.2.2', { exact: true })).toBeVisible();
		// An overflow:hidden ancestor cuts a control without scrolling the page, so
		// horizontalOverflow cannot see it.
		const clipped = await page.evaluate(() => {
			const found: string[] = [];
			for (const el of document.querySelectorAll('button, a[href], input, select, textarea')) {
				const r = el.getBoundingClientRect();
				if (!el.checkVisibility() || r.width <= 1) continue; // visually-hidden natives
				for (let box = el.parentElement; box; box = box.parentElement) {
					if (!/hidden|clip/.test(getComputedStyle(box).overflowX)) continue;
					const c = box.getBoundingClientRect();
					if (r.left < c.left - 0.5 || r.right > c.right + 0.5) {
						const name = el.getAttribute('aria-label') || el.textContent?.trim();
						found.push(`${el.tagName} "${name}" ${r.left}..${r.right} in ${c.left}..${c.right}`);
					}
				}
			}
			return found;
		});
		expect(clipped).toEqual([]);
	});
});

test.describe('sticky header and Save bar', () => {
	test.use({ viewport: { width: 390, height: 844 } });

	// Focus rings reach at most 4px outside a control (2px outline, 2px offset).
	const RING = 4;

	async function openSettings(page: Page) {
		await signIn(page, `a11y-sticky-${crypto.randomUUID()}`);
		await page.goto(`${APP}/admin/settings`);
		await expect(page.getByLabel('Site title')).toBeVisible();
		expect(await page.evaluate(() => getComputedStyle(document.querySelector('header')!).position)).toBe('sticky');
	}

	/** Resolves once the page has stopped scrolling; WebKit scrolls to a focused field a frame late. */
	const settle = (page: Page) =>
		page.evaluate(
			() =>
				new Promise<void>((resolve) => {
					let last = scrollY;
					let still = 0;
					let frames = 0;
					const tick = () => {
						still = scrollY === last ? still + 1 : 0;
						last = scrollY;
						if (still >= 3 || ++frames > 60) resolve();
						else requestAnimationFrame(tick);
					};
					requestAnimationFrame(tick);
				})
		);

	/** Room between the focused control, ring included, and each sticky bar; negative means covered. */
	const clearance = (page: Page) =>
		page.evaluate((ring) => {
			const el = document.activeElement as HTMLElement;
			const bar = document.querySelector('form button[type="submit"]')!.parentElement!;
			const r = el.getBoundingClientRect();
			return {
				label: el.textContent?.trim() || el.id,
				inBar: bar.contains(el),
				belowHeader: r.top - ring - document.querySelector('header')!.getBoundingClientRect().bottom,
				aboveBar: bar.getBoundingClientRect().top - (r.bottom + ring)
			};
		}, RING);

	async function expectClear(page: Page) {
		await settle(page);
		const c = await clearance(page);
		if (c.inBar) return c;
		expect(c.belowHeader, `focused "${c.label}" is under the header`).toBeGreaterThanOrEqual(0);
		expect(c.aboveBar, `focused "${c.label}" is under the Save bar`).toBeGreaterThanOrEqual(0);
		return c;
	}

	for (const [width, height] of [
		[390, 844],
		[320, 568]
	] as const) {
		test(`Tab and Shift-Tab never leave the focused control under a bar (${width}x${height})`, async ({
			page
		}) => {
			await page.setViewportSize({ width, height });
			await openSettings(page);
			await page.getByLabel('Site title').focus();
			for (const [key, last] of [
				['Tab', 'Save settings'],
				['Shift+Tab', 'site-title']
			] as const) {
				for (let step = 0; ; step++) {
					expect(step, `never reached ${last}`).toBeLessThan(40);
					await page.keyboard.press(key);
					if ((await expectClear(page)).label === last) break;
				}
			}
		});
	}

	test('Tab into a control mostly behind the Save bar lifts all of it clear', async ({ page }) => {
		await page.setViewportSize({ width: 320, height: 568 });
		await openSettings(page);
		for (const [from, to, visible] of [
			[page.getByLabel('Terms-of-service URL (optional)'), page.getByLabel('Custom content block (optional)'), 30],
			[page.getByLabel('Custom content block (optional)'), page.locator('#default-theme'), 12],
			[page.getByLabel('Rate limit (runs)'), page.getByRole('button', { name: 'About Rate window (seconds)' }), 6]
		] as const) {
			await from.evaluate((el: HTMLElement) => el.focus({ preventScroll: true }));
			await settle(page);
			await peekAboveBar(to, visible);
			await page.keyboard.press('Tab');
			await expect(to).toBeFocused();
			await expectClear(page);
		}
	});

	test('a field taller than the gap keeps its top clear of the header', async ({ page }) => {
		await page.setViewportSize({ width: 320, height: 568 });
		await openSettings(page);
		const block = page.getByLabel('Custom content block (optional)');
		// As if the user dragged the textarea taller than the room between the bars.
		await block.evaluate((el) => (el.style.height = '600px'));
		await page.getByLabel('Terms-of-service URL (optional)').focus();
		await page.keyboard.press('Tab');
		await expect(block).toBeFocused();
		await settle(page);
		expect((await clearance(page)).belowHeader, 'the top of the tall field is under the header').toBeGreaterThanOrEqual(0);
	});

	test('Shift-Tab through Locations keeps each control clear of the header', async ({ page }) => {
		await page.setViewportSize({ width: 1280, height: 720 });
		await signIn(page, `a11y-sticky-${crypto.randomUUID()}`);
		await page.goto(`${APP}/admin`);
		await expect(page.getByRole('button', { name: 'Delete Frankfurt' })).toBeVisible();
		const focused = () =>
			page.evaluate((ring) => {
				const el = document.activeElement as HTMLElement;
				return {
					label: el.textContent?.trim() || el.id,
					inMain: !!el.closest('main'),
					belowHeader: el.getBoundingClientRect().top - ring - document.querySelector('header')!.getBoundingClientRect().bottom
				};
			}, RING);
		await page.locator('main').focus();
		let count = 0;
		while (count < 80) {
			await page.keyboard.press('Tab');
			if (!(await focused()).inMain) break;
			count++;
		}
		let checked = 0;
		for (; checked < count; checked++) {
			await page.keyboard.press('Shift+Tab');
			await settle(page);
			const c = await focused();
			if (!c.inMain) break;
			expect(c.belowHeader, `focused "${c.label}" is under the header`).toBeGreaterThanOrEqual(0);
		}
		expect(checked, 'controls checked on the Locations page').toBeGreaterThan(10);
	});

	// Presses land within 8px of the bar's top edge, where scrolling on press focus loses them.
	test('a press on the visible strip of a covered select opens it', async ({ page }) => {
		await openSettings(page);
		const box = await peekAboveBar(page.locator('#default-theme'), 12);
		await page.mouse.click(box.x + box.width / 2, box.y + 5);
		await expect(page.getByRole('listbox')).toBeVisible();
	});

	test('a press on the visible strip of a covered info button reaches it', async ({ page }) => {
		await openSettings(page);
		const about = page.getByRole('button', { name: 'About Rate window (seconds)' });
		await about.evaluate((el) => el.addEventListener('click', () => (el.dataset.clicked = 'yes')));
		const box = await peekAboveBar(about, 6);
		await page.mouse.click(box.x + box.width / 2, box.y + 3);
		await expect(about).toHaveAttribute('data-clicked', 'yes');
	});

	test('a press on the text line of a covered text field puts the caret there', async ({ page }) => {
		await openSettings(page);
		const cap = page.getByLabel('Global concurrency cap');
		const before = await cap.inputValue();
		const box = await peekAboveBar(cap, 12);
		await page.mouse.click(box.x + box.width - 30, box.y + 10);
		await expect(cap).toBeFocused();
		await page.keyboard.type('7');
		await expect(cap).toHaveValue(`${before}7`);
	});
});

test.describe('forced colours', () => {
	test.beforeEach(async ({ page }) => {
		await page.emulateMedia({ forcedColors: 'active' });
	});

	test('the selected location tab is marked differently from the others', async ({ page }) => {
		await page.goto(`${APP}/`);
		const selected = page.getByRole('tab', { name: 'Frankfurt (AS64501)' });
		await expect(selected).toHaveAttribute('aria-selected', 'true');
		const other = page.getByRole('tab', { name: 'Vienna (AS64500)' });
		const underline = (tab: Locator) => tab.evaluate((el) => getComputedStyle(el).borderBottomColor);
		expect(await underline(selected)).not.toBe(await underline(other));
	});

	// F-225: the Location editor's section stops fill the current one; forced
	// colours drop that fill, so the current stop needs its own mark there.
	for (const scheme of SCHEMES) {
		test(`the current Location editor section is marked differently from the others (${scheme})`, async ({ page }) => {
			await page.emulateMedia({ colorScheme: scheme, forcedColors: 'active' });
			await signIn(page, `a11y-forced-stops-${crypto.randomUUID()}`);
			await page.goto(`${APP}/admin/locations/fra?tab=methods`);
			const current = page.getByRole('tab', { name: 'Methods', selected: true });
			await expect(current).toBeVisible();
			await page.mouse.move(0, 0);
			const stop = (tab: Locator) =>
				tab.evaluate((el) => {
					const s = getComputedStyle(el, '::before');
					return `${s.backgroundColor} ${s.borderTopColor}`;
				});
			expect(await stop(current)).not.toBe(await stop(page.getByRole('tab', { name: 'Settings' })));
			// The label beside the marked stop must stay readable.
			expect(await contrast(current, 'color')).toBeGreaterThanOrEqual(4.5);
		});
	}

	// F-259: forced colours repaint the track and both fills Canvas, so the bar
	// showed nothing. The empty track keeps a visible edge and each finished
	// fill stands out from the Canvas the track and page show, in both schemes.
	for (const scheme of SCHEMES) {
		test(`the speed test bar shows its track and its fill (${scheme})`, async ({ page }) => {
			test.setTimeout(60_000);
			const ratio = (a: number, b: number) => (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
			// Pin the download near 100 Mbps (12.5 MB in about 1 s) so the finished
			// bar splits into two visible segments whatever the engine's local
			// upload rate. Uploads stay real: an intercepted POST reports no progress.
			await page.route('**/api/locations/fra/files/*/download', async (route) => {
				await new Promise((resolve) => setTimeout(resolve, 1_000));
				await route.fulfill({ status: 200, body: Buffer.alloc(12_500_000) });
			});
			await page.emulateMedia({ colorScheme: scheme, forcedColors: 'active' });
			await page.goto(`${APP}/`);
			const bar = page.getByRole('progressbar', { name: 'Speed test progress' });
			await bar.scrollIntoViewIfNeeded();
			await page.mouse.move(0, 0);
			const box = (await bar.boundingBox())!;
			const around = { x: box.x - 4, y: box.y - 4, width: box.width + 8, height: box.height + 8 };
			expect((await painted(page, around)).best, 'empty track edge').toBeGreaterThanOrEqual(3);

			await page.getByRole('button', { name: 'Start speed test' }).click();
			await expect(page.getByRole('button', { name: 'Start speed test' })).toBeEnabled({ timeout: 25_000 });
			await expect(bar).toHaveAttribute('aria-valuenow', '100');
			await expect(page.getByRole('alert')).toHaveCount(0);
			const ground = await painted(page, { x: box.x, y: box.y + box.height + 6, width: box.width, height: 4 });
			// Settled segments: download from the left, upload after it.
			await bar.evaluate((el) => Promise.all(el.getAnimations({ subtree: true }).map((a) => a.finished)));
			const segments = await bar.evaluate((el) =>
				[...el.children].map((child) => {
					const r = child.getBoundingClientRect();
					return { x: r.x, y: r.y, width: r.width, height: r.height };
				})
			);
			for (const [i, r] of segments.entries()) {
				const name = ['download', 'upload'][i];
				expect(r.width, `${name} fill width`).toBeGreaterThanOrEqual(3);
				const fill = await painted(page, { x: r.x + 1, y: r.y + 1, width: r.width - 2, height: r.height - 2 });
				expect(ratio(fill.ground, ground.ground), `${name} fill against the page`).toBeGreaterThanOrEqual(3);
			}
		});
	}

	test('the keyboard-highlighted option is marked differently from the others', async ({ page }) => {
		await page.goto(`${APP}/`);
		const method = page.getByRole('combobox', { name: 'Method' });
		await method.focus();
		await page.keyboard.press('ArrowDown');
		await expect(page.getByRole('listbox')).toBeVisible();
		await page.keyboard.press('ArrowDown');
		const look = (option: Locator) =>
			option.evaluate((el) => {
				let node: Element | null = el;
				let bg = '';
				// Transparent layers (any colour at alpha 0) show the painted ancestor.
				while (node && /,\s*0\)$/.test((bg = getComputedStyle(node).backgroundColor))) {
					node = node.parentElement;
				}
				const s = getComputedStyle(el);
				return `${bg} ${s.outlineStyle === 'none' ? 'none' : `${s.outlineStyle} ${s.outlineColor}`}`;
			});
		const highlighted = page.locator('[role="option"][data-highlighted]');
		await expect(highlighted).toHaveCount(1);
		const plain = page.locator('[role="option"]:not([data-highlighted]):not([data-state="checked"])').first();
		expect(await look(highlighted)).not.toBe(await look(plain));
	});

	/**
	 * Contrast of the painted pixels in `part` against the colour most of its box
	 * shows, i.e. what sits right behind the glyphs. Computed styles cannot see a
	 * forced-colours backplate; a plate the colour of the text reads about 1:1.
	 */
	async function inkContrast(page: Page, part: Locator) {
		const box = await part.evaluate((el) => {
			const range = document.createRange();
			range.selectNodeContents(el);
			const r = range.getBoundingClientRect();
			return { x: r.x, y: r.y, width: r.width, height: r.height };
		});
		// Inset so no edge of a plate, or of the option around it, is counted.
		const clip = { x: box.x + 2, y: box.y + 2, width: box.width - 4, height: box.height - 4 };
		return (await painted(page, clip)).best;
	}

	// F-233: Chromium draws a Canvas backplate behind forced-colour text, which
	// hid the HighlightText label on the Highlight option. 2.5:1 passes Firefox's
	// own selection pair (white on #3399ff, 2.9:1); a hidden label reads about 1:1.
	for (const scheme of SCHEMES) {
		for (const theme of SCHEMES) {
			test(`the highlighted option's label is readable (${scheme}, page theme ${theme})`, async ({ page }) => {
				await page.addInitScript((t) => localStorage.setItem('theme', t), theme);
				await page.emulateMedia({ colorScheme: scheme, forcedColors: 'active' });
				await page.goto(`${APP}/`);
				await page.getByRole('combobox', { name: 'Method' }).focus();
				await page.keyboard.press('ArrowDown');
				const highlighted = page.locator('[role="option"][data-highlighted]');
				await expect(highlighted).toHaveCount(1);
				// Opening highlights the chosen method, check mark included.
				await expect(highlighted).toHaveAttribute('data-state', 'checked');
				// Measure the settled list, not a frame of its pop-in.
				await page
					.getByRole('listbox')
					.evaluate((el) => Promise.all(el.getAnimations({ subtree: true }).map((a) => a.finished)));
				const label = highlighted.locator('[data-part="item-text"]');
				expect(await inkContrast(page, label), 'checked label').toBeGreaterThanOrEqual(2.5);
				expect(await inkContrast(page, highlighted.locator('[data-part="item-indicator"]')), 'check mark').toBeGreaterThanOrEqual(2.5);
				// The next option, keyboard-highlighted and then under the pointer too.
				await page.keyboard.press('ArrowDown');
				await expect(highlighted).not.toHaveAttribute('data-state', 'checked');
				expect(await inkContrast(page, label), 'highlighted label').toBeGreaterThanOrEqual(2.5);
				await highlighted.hover();
				await expect(highlighted).toHaveCount(1);
				expect(await inkContrast(page, label), 'hovered label').toBeGreaterThanOrEqual(2.5);
			});
		}
	}
});

test.describe('touch', () => {
	test.use({ viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true });

	test('tapping an info icon opens its tooltip and tapping again closes it', async ({ page }) => {
		await signIn(page, `a11y-touch-${crypto.randomUUID()}`);
		await page.goto(`${APP}/admin/settings`);
		const about = page.getByRole('button', { name: 'About Global concurrency cap' });
		await about.tap();
		const tip = page.getByRole('tooltip');
		await expect(tip).toBeVisible();
		await expect(tip).toContainText('The most runs that can execute at the same time.');
		await about.tap();
		await expect(tip).toBeHidden();
	});

	test('tapping the visible strip of an info icon behind the Save bar opens it', async ({ page }) => {
		await signIn(page, `a11y-touch-${crypto.randomUUID()}`);
		await page.goto(`${APP}/admin/settings`);
		const about = page.getByRole('button', { name: 'About Rate window (seconds)' });
		await expect(about).toBeVisible();
		const box = await peekAboveBar(about, 12);
		await page.touchscreen.tap(box.x + box.width / 2, box.y + 4);
		await expect(page.getByRole('tooltip')).toBeVisible();
		await expect(page.getByRole('tooltip')).toContainText('The period, in seconds, the rate limit is counted over.');
	});
});

test('delete and revoke confirms name the location', async ({ page }) => {
	await signIn(page, `a11y-confirm-${crypto.randomUUID()}`);
	await page.goto(`${APP}/admin`);
	await page.getByRole('button', { name: 'Delete Frankfurt' }).click();
	await expect(page.getByRole('dialog')).toContainText('Frankfurt and its test IPs');
	await page.getByRole('dialog').getByRole('button', { name: 'Cancel' }).click();
	await expect(page.getByRole('dialog')).toHaveCount(0);

	const vienna = page.getByRole('listitem').filter({ hasText: 'Vienna' });
	await vienna.getByRole('button', { name: 'Revoke' }).click();
	await expect(page.getByRole('dialog')).toContainText("Vienna's agent credential stops working");
});

// F-260: editing New password after Confirm was left keeps the mismatch a
// hint while typing; leaving New password shows it as one error.
test('a new password edit after Confirm keeps the mismatch a hint while typing', async ({ page }) => {
	await signIn(page, `a11y-cross-${crypto.randomUUID()}`);
	await page.goto(`${APP}/admin/administrators`);
	const next = page.getByLabel('New password', { exact: true });
	const confirm = page.getByLabel('Confirm new password');
	await next.fill('correct-horse-battery');
	await confirm.fill('correct-horse-battery');
	await next.focus();
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

	for (const key of ['End', 'Backspace', 'y', 'Backspace']) await next.press(key);
	await expect(confirm).toHaveAccessibleDescription('Passwords do not match.');
	await expect(confirm).not.toHaveAttribute('aria-invalid', 'true');
	await expect(page.getByRole('alert')).toHaveCount(0);
	expect(await alerts()).toEqual([]);

	await next.press('Tab');
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

// F-297: a session check that cannot reach the server says nothing about the
// session. The signed-in admin stays in the admin area, hears why, and can retry.
test('an unreachable server keeps a signed-in admin in place with an announced retry', async ({ page }) => {
	let down = true;
	let hold: Promise<void> | null = null;
	await page.route('**/api/admin/me', async (route) => {
		await hold;
		return down ? route.abort() : route.continue();
	});
	await signIn(page, `a11y-gate-${crypto.randomUUID()}`);
	await page.goto(`${APP}/admin`);
	const alert = page.getByRole('alert');
	await expect(alert).toContainText('Could not reach the server');
	await expect(page).toHaveURL(`${APP}/admin`);

	// A keyboard retry that fails again keeps focus on Try again (F-299).
	const retry = alert.getByRole('button', { name: 'Try again' });
	await retry.focus();
	await page.keyboard.press('Enter');
	await expect(alert).toContainText('Could not reach the server');
	await expect(retry).toBeFocused();

	// Focus the user moves while that check is pending stays put when it fails:
	// WebKit honoured a native autofocus there and stole it back.
	let release = () => {};
	hold = new Promise((resolve) => (release = resolve));
	await page.keyboard.press('Enter');
	await expect(alert).toHaveCount(0);
	await page.keyboard.press('Tab');
	const moved = await page.evaluateHandle(() => document.activeElement);
	expect(await moved.evaluate((el) => el !== null && el !== document.body)).toBe(true);
	release();
	hold = null;
	await expect(alert).toContainText('Could not reach the server');
	await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
	expect(await moved.evaluate((el) => el === document.activeElement)).toBe(true);
	await expect(retry).not.toBeFocused();

	down = false;
	await retry.click();
	await expect(page.getByRole('complementary', { name: 'Administration' })).toBeVisible();
	await expect(page.getByRole('alert')).toHaveCount(0);
});
