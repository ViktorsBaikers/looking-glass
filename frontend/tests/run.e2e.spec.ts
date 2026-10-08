import { APP } from './ports';
import { expect, test, type Locator, type Page } from '@playwright/test';

const metricsGroup = (page: Page) => page.getByRole('group', { name: 'Run metrics' });

async function selectMethod(page: Page, label: string) {
	await page.getByRole('combobox', { name: 'Method' }).click();
	await page.getByRole('option', { name: label, exact: true }).click();
}

async function run(page: Page, target: string) {
	await page.getByLabel('Target').fill(target);
	await page.getByRole('button', { name: 'Run diagnostic' }).click();
}

// Sets the page theme through its toggle.
async function useTheme(page: Page, theme: 'light' | 'dark') {
	const other = theme === 'light' ? 'dark' : 'light';
	const toTheme = page.getByRole('button', { name: `Switch to ${theme} theme` });
	if (await toTheme.isVisible()) await toTheme.click();
	await expect(page.getByRole('button', { name: `Switch to ${other} theme` })).toBeVisible();
}

// Forced colours hand the pressed option the system selection pair, which
// the app cannot pick: Firefox's default Highlight/HighlightText measures
// 2.94:1, Chromium's 9-11:1. A label painted in its own background colour
// still measures 1.5-1.6:1 from antialiasing (the old hover bug 1.6-1.95:1),
// so 2.5:1 separates a real system pair from an invisible label.
const MIN_FORCED_CONTRAST = 2.5;

// WCAG contrast ratio of two sRGB pixels.
function contrast(a: number[], b: number[]) {
	const lum = (p: number[]) =>
		p
			.map((v) => (v / 255 <= 0.03928 ? v / 255 / 12.92 : ((v / 255 + 0.055) / 1.055) ** 2.4))
			.reduce((sum, v, i) => sum + v * [0.2126, 0.7152, 0.0722][i], 0);
	const [hi, lo] = [lum(a), lum(b)].sort((x, y) => y - x);
	return (hi + 0.05) / (lo + 0.05);
}

// A screenshot taken with scale 'css' (one pixel per CSS pixel, whatever the
// device scale), as rows of [r, g, b].
async function pixels(page: Page, png: Buffer): Promise<number[][][]> {
	return page.evaluate(async (b64) => {
		const image = new Image();
		image.src = `data:image/png;base64,${b64}`;
		await image.decode();
		const canvas = document.createElement('canvas');
		canvas.width = image.width;
		canvas.height = image.height;
		const context = canvas.getContext('2d')!;
		context.drawImage(image, 0, 0);
		const data = context.getImageData(0, 0, image.width, image.height).data;
		const at = (x: number, y: number) => (y * image.width + x) * 4;
		return Array.from({ length: image.height }, (_, y) =>
			Array.from({ length: image.width }, (_, x) => [...data.slice(at(x, y), at(x, y) + 3)])
		);
	}, png.toString('base64'));
}

// Reads an option's label from its pixels: whether its text box shows the
// computed text colour and the option's own background (sampled from the
// left padding; a text backplate hides it), and the strongest contrast between
// a text-box pixel and that background, near 1 when the glyphs match it.
async function label(page: Page, option: Locator) {
	const { color, text } = await option.evaluate((el) => {
		const range = document.createRange();
		range.selectNodeContents(el);
		const [r, o] = [range.getBoundingClientRect(), el.getBoundingClientRect()];
		const text = { x: r.x - o.x + 2, y: r.y - o.y + 2, width: r.width - 4, height: r.height - 4 };
		return { color: getComputedStyle(el).color, text };
	});
	const rows = await pixels(page, await option.screenshot({ scale: 'css' }));
	const background = rows[Math.floor(rows.length / 2)][3];
	const ink = color.match(/\d+/g)!.slice(0, 3).map(Number);
	const near = (a: number[], b: number[]) => a.every((c, i) => Math.abs(c - b[i]) <= 32);
	const seen = { text: false, background: false, contrast: 1 };
	for (let y = Math.ceil(text.y); y < text.y + text.height; y++) {
		for (let x = Math.ceil(text.x); x < text.x + text.width; x++) {
			const p = rows[y][x];
			seen.text ||= near(p, ink);
			seen.background ||= near(p, background);
			seen.contrast = Math.max(seen.contrast, contrast(p, background));
		}
	}
	return seen;
}

// Contrast between an option's focus ring (2px outline at a 2px offset, drawn
// over the group's 1px border) and the page's Canvas: the ring is sampled
// 3-4px above the option's top edge, just outside the group's border, and the
// Canvas 7-8px above it, outside the group.
async function ringContrast(page: Page, option: Locator) {
	const box = (await option.boundingBox())!;
	const clip = { x: box.x - 8, y: box.y - 8, width: box.width + 16, height: box.height + 16 };
	const rows = await pixels(page, await page.screenshot({ clip, scale: 'css' }));
	const mid = Math.floor(rows[0].length / 2);
	return contrast(rows[4][mid], rows[0][mid]);
}

test.describe('diagnostic runs', () => {
	test.beforeEach(async ({ page }) => {
		await page.goto(`${APP}/`);
		await expect(page.getByRole('tab', { name: 'Frankfurt (AS64501)' })).toBeVisible();
	});

	test('runs a ping: streams, completes, and derives metric cards', async ({ page }) => {
		const streamRequest = page.waitForRequest(
			(request) => new URL(request.url()).pathname === '/api/run/stream'
		);
		await run(page, '1.1.1.1');

		const request = await streamRequest;
		expect(request.resourceType()).toBe('eventsource');
		const stream = new URL(request.url());
		expect(stream.origin).toBe(APP);
		expect(stream.searchParams.get('location')).toBe('fra');
		expect(stream.searchParams.get('method')).toBe('ping');
		expect(stream.searchParams.get('target')).toBe('1.1.1.1');

		await expect(page.getByRole('log')).toContainText('64 bytes from 1.1.1.1: icmp_seq=1');
		await expect(page.getByRole('status')).toHaveText('Completed');
		await expect(page.getByText('Frankfurt ~ ping')).toBeVisible();
		await expect(page.getByRole('button', { name: 'Run diagnostic' })).toBeEnabled();

		// Metric cards derived from the finished output.
		const metrics = metricsGroup(page);
		await expect(metrics).toBeVisible();
		await expect(metrics.getByText('Latency', { exact: true })).toBeVisible();
		await expect(metrics.getByText('12.1', { exact: true })).toBeVisible();
		await expect(metrics.getByText('Packet loss', { exact: true })).toBeVisible();
		await expect(metrics.getByText('0/4', { exact: true })).toBeVisible();
		await expect(metrics.getByText('Jitter', { exact: true })).toBeVisible();
		await expect(metrics.getByText('0.2', { exact: true })).toBeVisible();
		await expect(metrics.getByText('TTL', { exact: true })).toBeVisible();
		await expect(metrics.getByText('56', { exact: true })).toBeVisible();

		// Each card explains itself through its info tooltip. Zag's pointer
		// tracking needs a real two-step mouse move; a teleport hover won't open it.
		const about = page.getByRole('button', { name: 'About Latency' });
		await about.scrollIntoViewIfNeeded();
		const box = await about.boundingBox();
		if (!box) throw new Error('About Latency trigger has no bounding box');
		await page.mouse.move(box.x + box.width / 2 - 40, box.y + box.height / 2 - 40);
		await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
		await expect(about).toHaveAttribute('data-state', 'open');
		await expect(page.getByText('Average round-trip time across all replies.')).toBeVisible();
	});

	// Keyboard users reach the same explanations. Tabbing to a trigger below
	// the fold scrolls it into view, and that scroll once closed the tooltip
	// the instant focus opened it.
	test('metric info tooltips open on keyboard focus', async ({ page }) => {
		await run(page, '1.1.1.1');
		await expect(page.getByRole('status')).toHaveText('Completed');
		const about = page.getByRole('button', { name: 'About Latency' });
		await page.getByRole('button', { name: 'Copy output' }).focus();
		for (let i = 0; i < 12 && !(await about.evaluate((el) => el === document.activeElement)); i++) {
			await page.keyboard.press('Tab');
		}
		await expect(about).toBeFocused();
		await expect(about).toHaveAttribute('data-state', 'open');
		await expect(page.getByRole('tooltip')).toHaveText('Average round-trip time across all replies.');
	});

	test('Run diagnostic morphs to Cancel and cancelling ends the run', async ({ page }) => {
		await run(page, 'slow.test');
		const cancel = page.getByRole('button', { name: 'Cancel' });
		await expect(cancel).toBeVisible();
		await expect(page.getByRole('status')).toHaveText('Streaming');

		await cancel.click();
		await expect(page.getByRole('status')).toHaveText('Canceled');
		await expect(page.getByRole('log')).toContainText('Run canceled.');
		await expect(page.getByRole('button', { name: 'Run diagnostic' })).toBeEnabled();
		// No metric cards for a run that never finished.
		await expect(metricsGroup(page)).toHaveCount(0);
	});

	test('a refused run shows a friendly failure', async ({ page }) => {
		await run(page, 'fail.test');
		await expect(page.getByRole('status')).toHaveText('Failed');
		await expect(page.getByRole('log')).toContainText('The node refused the run.');
	});

	test('an mtr run renders the hop table and final-hop metrics', async ({ page }) => {
		await selectMethod(page, 'MTR');
		await run(page, '1.1.1.1');

		await expect(page.getByRole('status')).toHaveText('Completed');
		await expect(page.getByText('Frankfurt ~ mtr')).toBeVisible();
		const table = page.getByRole('table');
		await expect(table).toBeVisible();
		await expect(table.getByRole('cell', { name: '192.0.2.1' })).toBeVisible();
		await expect(table.getByRole('cell', { name: '1.1.1.1' })).toBeVisible();

		const metrics = metricsGroup(page);
		await expect(metrics.getByText('12.2', { exact: true })).toBeVisible();
		await expect(metrics.getByText('final hop avg', { exact: true })).toBeVisible();
		await expect(metrics.getByText('Hops', { exact: true })).toBeVisible();
		await expect(metrics.getByText('3', { exact: true })).toBeVisible();
	});

	test('a traceroute run shows only the hop-count card', async ({ page }) => {
		await selectMethod(page, 'Traceroute');
		await run(page, '1.1.1.1');

		await expect(page.getByRole('status')).toHaveText('Completed');
		const metrics = metricsGroup(page);
		await expect(metrics).toBeVisible();
		await expect(metrics.getByText('Hops', { exact: true })).toBeVisible();
		await expect(metrics.getByText('3', { exact: true })).toBeVisible();
		await expect(metrics.locator(':scope > div')).toHaveCount(1);
	});

	// F-224: forced colours replace the pressed option's ink fill, so the
	// pressed Route | Raw option needs a look of its own there, and both labels
	// must stay readable on the light and dark system palettes, also under the
	// pointer that just pressed it (F-232), with a keyboard focus ring that
	// shows on Canvas when the page theme differs from the system palette.
	for (const scheme of ['light', 'dark'] as const) {
		test(`the pressed output view stands out in forced colours (${scheme})`, async ({ page }) => {
			await page.emulateMedia({ forcedColors: 'active', colorScheme: scheme });
			// The page theme follows the system palette first, as it does by default.
			await useTheme(page, scheme);
			await selectMethod(page, 'Traceroute');
			await run(page, '1.1.1.1');
			await expect(page.getByRole('status')).toHaveText('Completed');
			const group = page.getByRole('group', { name: 'Output view' });
			const route = group.getByRole('button', { name: 'Route' });
			const raw = group.getByRole('button', { name: 'Raw' });
			await expect(route).toHaveAttribute('aria-pressed', 'true');
			await expect(raw).toHaveAttribute('aria-pressed', 'false');
			await page.mouse.move(0, 0);
			// The colour an option paints: its own background, or the first opaque
			// one behind it (the page's Canvas when every ancestor is transparent).
			const paint = (name: string) =>
				group.getByRole('button', { name }).evaluate((option) => {
					const opaque = (color: string) => color !== 'transparent' && !/^rgba\(.*,\s*0\)$/.test(color);
					for (let node: Element | null = option; node; node = node.parentElement) {
						const color = getComputedStyle(node).backgroundColor;
						if (opaque(color)) return color;
					}
					const probe = document.body.appendChild(document.createElement('div'));
					probe.style.background = 'Canvas';
					const canvas = getComputedStyle(probe).backgroundColor;
					probe.remove();
					return canvas;
				});
			expect(await paint('Route')).not.toBe(await paint('Raw'));
			// Computed colours miss the text backplate the browser may paint behind
			// a label and say nothing about how the glyphs meet the fill: read pixels.
			const readable = async (option: Locator, name: string) => {
				const seen = await label(page, option);
				expect.soft(seen, name).toMatchObject({ text: true, background: true });
				expect.soft(seen.contrast, `${name} contrast`).toBeGreaterThanOrEqual(MIN_FORCED_CONTRAST);
			};
			await readable(route, 'pressed label');
			await readable(raw, 'unpressed label');
			// Pressing Route again leaves the pointer resting on the pressed option.
			await raw.click();
			await route.click();
			await expect(route).toHaveAttribute('aria-pressed', 'true');
			await readable(route, 'hovered pressed label');
			// Page theme opposite to the system palette, then keyboard focus.
			await useTheme(page, scheme === 'light' ? 'dark' : 'light');
			await page.mouse.move(0, 0);
			await raw.focus();
			await page.keyboard.press('Shift+Tab');
			await expect(route).toBeFocused();
			expect.soft(await ringContrast(page, route), 'pressed focus ring on Canvas').toBeGreaterThanOrEqual(3);
		});
	}

	// F-232: the pointer that presses an option stays on it, so the pressed
	// colours must win over the hover colour.
	for (const theme of ['light', 'dark'] as const) {
		test(`hovering the pressed output view keeps its label readable (${theme})`, async ({ page }) => {
			await useTheme(page, theme);
			await selectMethod(page, 'Traceroute');
			await run(page, '1.1.1.1');
			await expect(page.getByRole('status')).toHaveText('Completed');
			const group = page.getByRole('group', { name: 'Output view' });
			const route = group.getByRole('button', { name: 'Route' });
			await group.getByRole('button', { name: 'Raw' }).click();
			await route.click();
			await expect(route).toHaveAttribute('aria-pressed', 'true');
			// The app's own colours here: WCAG AA for text.
			expect((await label(page, route)).contrast).toBeGreaterThanOrEqual(4.5);
		});
	}

	// F-190: ping's blank line before its statistics reaches the console and
	// Copy output, in order.
	test('a blank output line reaches the console and Copy output', async ({ page }) => {
		await page.evaluate(() => {
			const copied = window as unknown as { __copied: string };
			Object.defineProperty(navigator, 'clipboard', {
				configurable: true,
				value: { writeText: async (text: string) => void (copied.__copied = text) }
			});
		});
		await run(page, '1.1.1.1');
		await expect(page.getByRole('status')).toHaveText('Completed');

		const lines = page.getByRole('log').locator('p');
		const statistics = lines.filter({ hasText: '--- 1.1.1.1 ping statistics ---' });
		await expect(statistics).toHaveCount(1);
		const shown = await lines.allTextContents();
		const at = shown.indexOf('--- 1.1.1.1 ping statistics ---');
		expect(shown.slice(at - 2, at + 1)).toEqual([
			'64 bytes from 1.1.1.1: icmp_seq=4 ttl=56 time=12.0 ms',
			'',
			'--- 1.1.1.1 ping statistics ---'
		]);
		// The blank row is visible: as tall as the line before it, not collapsed to 0.
		const heights = await lines.evaluateAll((rows) => rows.map((row) => row.getBoundingClientRect().height));
		expect(heights[at - 2]).toBeGreaterThan(0);
		expect(heights[at - 1]).toBe(heights[at - 2]);

		await page.getByRole('button', { name: 'Copy output' }).click();
		await expect
			.poll(() => page.evaluate(() => (window as unknown as { __copied?: string }).__copied))
			.toContain('icmp_seq=4 ttl=56 time=12.0 ms\n\n--- 1.1.1.1 ping statistics ---\n');
	});

	test('a BGP run shows no metric cards', async ({ page }) => {
		await page.getByRole('tab', { name: 'Vienna (AS64500)' }).click();
		await selectMethod(page, 'BGP');
		await run(page, '8.8.8.0/24');

		await expect(page.getByRole('status')).toHaveText('Completed');
		await expect(page.getByRole('log')).toContainText('BGP routing table entry for 8.8.8.0/24');
		await expect(metricsGroup(page)).toHaveCount(0);
	});
});

// In-app navigation unmounts the diagnostics page; the run's
// EventSource must close with it so the node stops the run.
test('leaving the diagnostics page in-app closes the run stream', async ({ page }) => {
	const fixtureId = `run-leave-${crypto.randomUUID()}`;
	const login = await page.context().request.post(`${APP}/api/auth/login`, {
		data: { username: 'brooke', password: 'fixture-password' },
		headers: { 'x-looking-glass-fixture': fixtureId }
	});
	expect(login.status()).toBe(204);
	await page.setExtraHTTPHeaders({ 'x-looking-glass-fixture': fixtureId });
	await page.addInitScript(() => {
		const Native = window.EventSource;
		const opened: EventSource[] = [];
		(window as unknown as { __streams: EventSource[] }).__streams = opened;
		window.EventSource = class extends Native {
			constructor(url: string | URL, init?: EventSourceInit) {
				super(url, init);
				opened.push(this);
			}
		} as typeof EventSource;
	});
	const streamStates = () =>
		page.evaluate(() => (window as unknown as { __streams: EventSource[] }).__streams.map((s) => s.readyState));

	await page.goto(`${APP}/`);
	await expect(page.getByRole('tab', { name: 'Frankfurt (AS64501)' })).toBeVisible();
	await run(page, 'slow.test');
	await expect(page.getByRole('status')).toHaveText('Streaming');
	expect(await streamStates()).toEqual([1]); // OPEN

	await page.getByRole('link', { name: 'Administration' }).click();
	await expect(page).toHaveURL(`${APP}/admin`);
	await expect.poll(streamStates, { timeout: 2000 }).toEqual([2]); // CLOSED
});
