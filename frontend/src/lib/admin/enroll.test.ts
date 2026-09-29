// Seam 3 (pure logic) for the Enrollment tab: the ticket expiry countdown.
import { cleanup, fireEvent, render, screen } from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { formatCountdown } from './editor.js';
import LocationEditor from './LocationEditor.svelte';

describe('enrollment countdown', () => {
	it('renders whole minutes and seconds as mm:ss', () => {
		expect(formatCountdown(125_000)).toBe('02:05');
		expect(formatCountdown(900_000)).toBe('15:00');
		expect(formatCountdown(5_000)).toBe('00:05');
	});

	it('clamps at zero once the ticket has expired', () => {
		expect(formatCountdown(0)).toBe('00:00');
		expect(formatCountdown(-5_000)).toBe('00:00');
	});

	it('truncates partial seconds', () => {
		expect(formatCountdown(1_999)).toBe('00:01');
	});
});

// Seam 1 (component) for the Enrollment tab inside the editor: every POST
// /enroll stores another live 15-minute token, so only an explicit Regenerate
// may mint again once one is shown, and the per-second countdown must
// not sit in a live region.
describe('enrollment tab lifecycle', () => {
	const location = {
		id: 'L1',
		name: 'Frankfurt',
		geo_label: 'Frankfurt, DE',
		map_query: null,
		facility: null,
		facility_url: null,
		kind: 'remote',
		data_plane_origin: null,
		asn: null,
		offered_methods: ['ping'],
		created_at: 0,
		last_seen: null,
		test_ips: [],
		iperf: [],
		files: []
	};

	function stubServer(server: { status: string; posts: number; ttl: number }) {
		vi.stubGlobal(
			'fetch',
			vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
				const url = String(input);
				if (url.endsWith('/enroll') && init?.method === 'POST') {
					server.posts++;
					return Response.json({
						install_command: `install --token T${server.posts}`,
						token: `T${server.posts}`,
						fingerprint: 'f',
						expires_at: Math.floor(Date.now() / 1000) + server.ttl
					});
				}
				if (url === '/api/admin/locations/L1') {
					return Response.json({ ...location, status: server.status });
				}
				return Response.json({}, { status: 404 });
			})
		);
	}

	// Real-time polling (setInterval is faked below, so testing-library's
	// waitFor would stall).
	async function until(check: () => boolean) {
		for (let i = 0; i < 200 && !check(); i++) await new Promise((r) => setTimeout(r, 10));
		expect(check()).toBe(true);
	}
	const command = () => document.querySelector('code')?.textContent ?? null;

	beforeEach(() => vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] }));
	afterEach(() => {
		cleanup();
		vi.useRealTimers();
		vi.unstubAllGlobals();
	});

	it('mints once across the agent-online reload and a tab revisit', async () => {
		const server = { status: 'offline', posts: 0, ttl: 900 };
		stubServer(server);
		const ontab = () => {};
		const view = render(LocationEditor, { props: { locationId: 'L1', tab: 'enrollment', ontab } });
		await until(() => command() === 'install --token T1');
		expect(server.posts).toBe(1);

		// The agent dials home: the 3 s poll sees it online and the editor reloads.
		server.status = 'online';
		vi.advanceTimersByTime(3000);
		await until(() => screen.queryByText(/Connected — Frankfurt is online/) !== null);
		expect(server.posts).toBe(1);
		expect(command()).toBe('install --token T1');

		await view.rerender({ locationId: 'L1', tab: 'settings', ontab });
		await until(() => screen.queryByRole('button', { name: 'Save location' }) !== null);
		await view.rerender({ locationId: 'L1', tab: 'enrollment', ontab });
		await until(() => command() !== null);
		expect(server.posts).toBe(1);
		expect(command()).toBe('install --token T1');
	});

	it('mints once when the tab is opened by a click and revisited mid-mint', async () => {
		// Every tab panel renders the panel snippet, and the outgoing panel lives
		// on for one pass after a switch; each mounted EnrollmentTab must share
		// the one in-flight mint for its location.
		let posts = 0;
		let release = () => {};
		const gate = new Promise<void>((resolve) => (release = resolve));
		vi.stubGlobal(
			'fetch',
			vi.fn(async (_input: RequestInfo | URL, init?: RequestInit) => {
				if (init?.method === 'POST') {
					posts++;
					await gate;
					return Response.json({
						install_command: 'install --token T1',
						token: 'T1',
						fingerprint: 'f',
						expires_at: Math.floor(Date.now() / 1000) + 900
					});
				}
				return Response.json({ ...location, status: 'offline' });
			})
		);
		render(LocationEditor, { props: { locationId: 'L1', tab: 'settings', ontab: () => {} } });
		await until(() => screen.queryByRole('tab', { name: 'Enrollment' }) !== null);
		await fireEvent.click(screen.getByRole('tab', { name: 'Enrollment' }));
		await until(() => posts > 0);
		await fireEvent.click(screen.getByRole('tab', { name: 'Settings' }));
		await fireEvent.click(screen.getByRole('tab', { name: 'Enrollment' }));
		release();
		await until(() => command() === 'install --token T1');
		await new Promise((r) => setTimeout(r, 50));
		expect(posts).toBe(1);
	});

	it('stores a late mint under the location that requested it', async () => {
		// L1's POST is held open while the editor moves to L2 in place (a client
		// navigation between two editor URLs); its late response must not replace
		// L2's token, and L1 keeps it for its next visit.
		const posts: string[] = [];
		let releaseL1 = () => {};
		const l1Gate = new Promise<void>((resolve) => (releaseL1 = resolve));
		vi.stubGlobal(
			'fetch',
			vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
				const id = String(input).split('/')[4];
				if (init?.method === 'POST') {
					posts.push(id);
					if (id === 'L1') await l1Gate;
					return Response.json({
						install_command: `install --token ${id}`,
						token: id,
						fingerprint: 'f',
						expires_at: Math.floor(Date.now() / 1000) + 900
					});
				}
				return Response.json({ ...location, id, name: id, status: 'offline' });
			})
		);
		const ontab = () => {};
		const view = render(LocationEditor, { props: { locationId: 'L1', tab: 'enrollment', ontab } });
		await until(() => posts.length === 1);
		await view.rerender({ locationId: 'L2', tab: 'enrollment', ontab });
		await until(() => command() === 'install --token L2');

		releaseL1();
		await new Promise((r) => setTimeout(r, 50));
		expect(command()).toBe('install --token L2');

		await view.rerender({ locationId: 'L1', tab: 'enrollment', ontab });
		await until(() => command() === 'install --token L1');
		expect(posts).toEqual(['L1', 'L2']);
	});

	it('keeps the ticking countdown out of the live region but announces expiry', async () => {
		stubServer({ status: 'offline', posts: 0, ttl: 900 });
		render(LocationEditor, { props: { locationId: 'L1', tab: 'enrollment', ontab: () => {} } });
		await until(() => screen.queryByText(/Expires in/) !== null);
		expect(screen.getByText(/Expires in/).closest('[aria-live]')).toBeNull();
		cleanup();

		stubServer({ status: 'offline', posts: 0, ttl: -1 });
		render(LocationEditor, { props: { locationId: 'L1', tab: 'enrollment', ontab: () => {} } });
		await until(() => screen.queryByText(/Token expired/) !== null);
		expect(screen.getByText(/Token expired/).closest('[aria-live="polite"]')).not.toBeNull();
	});

	// F-340: a refused mint must say why (central's message), or Try again
	// fails the same way with no clue; a network failure stays generic.
	function stubMint(mint: () => Promise<Response>) {
		vi.stubGlobal(
			'fetch',
			vi.fn(async (input: RequestInfo | URL, init?: RequestInit) =>
				String(input).endsWith('/enroll') && init?.method === 'POST'
					? mint()
					: Response.json({ ...location, status: 'offline' })
			)
		);
	}
	const alertText = () => screen.queryByRole('alert')?.textContent ?? '';

	for (const [status, error, message] of [
		[503, 'identity_mismatch', 'The tunnel certificate does not match the enrollment certificate.'],
		[422, 'validation', 'Agent binary SHA-256 must be 64 hex characters.']
	] as const) {
		it(`shows central's reason when it refuses to mint (${status} ${error})`, async () => {
			stubMint(async () => Response.json({ error, message }, { status }));
			render(LocationEditor, { props: { locationId: 'L1', tab: 'enrollment', ontab: () => {} } });
			await until(() => alertText() !== '');
			expect(alertText()).toContain(message);
		});
	}

	it('keeps a generic message when central cannot be reached', async () => {
		stubMint(() => Promise.reject(new TypeError('Failed to fetch')));
		render(LocationEditor, { props: { locationId: 'L1', tab: 'enrollment', ontab: () => {} } });
		await until(() => alertText() !== '');
		expect(alertText()).toContain('The enrollment token could not be generated.');
		expect(alertText()).toContain('Could not reach the server.');
		expect(screen.getByRole('button', { name: 'Try again' })).toBeTruthy();
	});

	// F-354, F-355: the pressed button stays mounted and busy while the mint
	// runs (focus never drops to <body>), and a polite status region says what
	// is happening and when a new token is ready.
	const ticketFor = (n: number) =>
		Response.json({
			install_command: `install --token T${n}`,
			token: `T${n}`,
			fingerprint: 'f',
			expires_at: Math.floor(Date.now() / 1000) + 900
		});
	const status = () => screen.getByRole('status');

	it('Try again keeps focus and announces the pending mint and the new token', async () => {
		let posts = 0;
		let release: (response: Response) => void = () => {};
		stubMint(() => {
			posts++;
			if (posts === 1) return Promise.resolve(Response.json({ error: 'x', message: 'Set LG_TUNNEL_URL.' }, { status: 422 }));
			return new Promise<Response>((resolve) => (release = resolve));
		});
		render(LocationEditor, { props: { locationId: 'L1', tab: 'enrollment', ontab: () => {} } });
		await until(() => screen.queryByRole('button', { name: 'Try again' }) !== null);
		const retry = screen.getByRole('button', { name: 'Try again' });
		retry.focus();

		await fireEvent.click(retry);
		await until(() => status().textContent?.includes('Generating enrollment token…') === true);
		expect(status().getAttribute('aria-live') ?? 'polite').toBe('polite');
		expect(retry.isConnected).toBe(true);
		expect(retry.getAttribute('aria-busy')).toBe('true');
		expect(document.activeElement).toBe(retry);

		release(ticketFor(2));
		await until(() => command() === 'install --token T2');
		expect(status().textContent).toContain('Enrollment token generated.');
		expect(document.activeElement).toBe(screen.getByRole('button', { name: /Regenerate/ }));
	});

	it('Regenerate keeps focus while it mints, and a failure hands focus to Try again', async () => {
		let posts = 0;
		let release: (response: Response) => void = () => {};
		stubMint(() => {
			posts++;
			if (posts === 1) return Promise.resolve(ticketFor(1));
			if (posts === 2) return new Promise<Response>((resolve) => (release = resolve));
			return Promise.reject(new TypeError('Failed to fetch'));
		});
		render(LocationEditor, { props: { locationId: 'L1', tab: 'enrollment', ontab: () => {} } });
		await until(() => command() === 'install --token T1');
		const regenerate = screen.getByRole('button', { name: /Regenerate/ });
		regenerate.focus();

		await fireEvent.click(regenerate);
		await until(() => status().textContent?.includes('Generating enrollment token…') === true);
		expect(regenerate.getAttribute('aria-busy')).toBe('true');
		expect(document.activeElement).toBe(regenerate);
		// The old token may be dead (a revoke purges it): not shown, not copyable.
		expect(command()).toBeNull();
		expect(screen.queryByRole('button', { name: /copy/i })).toBeNull();
		expect(screen.queryByText(/Expires in/)).toBeNull();

		release(ticketFor(2));
		await until(() => command() === 'install --token T2');
		expect(status().textContent).toContain('Enrollment token generated.');
		expect(document.activeElement).toBe(regenerate);

		await fireEvent.click(regenerate);
		await until(() => alertText() !== '');
		expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Try again' }));
	});

	// F-368: a revoke confirmed mid-mint would join the pre-revoke mint and show
	// a token the revoke purged, so Revoke waits for the mint.
	it('Revoke is not operable while a Regenerate mints', async () => {
		let posts = 0;
		let revokes = 0;
		let release: (response: Response) => void = () => {};
		vi.stubGlobal(
			'fetch',
			vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
				const url = String(input);
				if (url.endsWith('/enroll') && init?.method === 'POST') {
					posts++;
					if (posts === 1) return ticketFor(1);
					return new Promise<Response>((resolve) => (release = resolve));
				}
				if (init?.method === 'POST' || init?.method === 'DELETE') {
					revokes++;
					return new Response(null, { status: 204 });
				}
				return Response.json({ ...location, status: 'online' });
			})
		);
		render(LocationEditor, { props: { locationId: 'L1', tab: 'enrollment', ontab: () => {} } });
		await until(() => command() === 'install --token T1');
		const revoke = screen.getByRole('button', { name: 'Revoke agent' });
		expect(revoke.hasAttribute('aria-disabled')).toBe(false);

		await fireEvent.click(screen.getByRole('button', { name: /Regenerate/ }));
		await until(() => posts === 2);
		expect(revoke.getAttribute('aria-disabled')).toBe('true');
		await fireEvent.click(revoke);
		await new Promise((r) => setTimeout(r, 50));
		expect(screen.queryByRole('dialog')).toBeNull();
		expect(revokes).toBe(0);

		release(ticketFor(2));
		await until(() => command() === 'install --token T2');
		expect(revoke.hasAttribute('aria-disabled')).toBe(false);
		await fireEvent.click(revoke);
		await until(() => screen.queryByRole('dialog') !== null);
	});

	it('a revisit during a failing Regenerate ends in the alert with Try again', async () => {
		// The tab remounts on every visit (here: away to another location and
		// back); the new tab must join the in-flight mint and show its failure.
		let posts = 0;
		let fail: (error: Error) => void = () => {};
		vi.stubGlobal(
			'fetch',
			vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
				const id = String(input).split('/')[4];
				if (init?.method === 'POST') {
					posts++;
					if (posts === 1) return ticketFor(1);
					return new Promise<Response>((_resolve, reject) => (fail = reject));
				}
				return Response.json({ ...location, id, name: id, status: 'offline' });
			})
		);
		const ontab = () => {};
		const view = render(LocationEditor, { props: { locationId: 'L1', tab: 'enrollment', ontab } });
		await until(() => command() === 'install --token T1');
		await fireEvent.click(screen.getByRole('button', { name: /Regenerate/ }));
		await until(() => posts === 2);

		await view.rerender({ locationId: 'L2', tab: 'settings', ontab });
		await until(() => screen.queryByRole('heading', { level: 1 })?.textContent === 'L2');
		await view.rerender({ locationId: 'L1', tab: 'enrollment', ontab });
		await until(() => screen.queryByRole('heading', { level: 1 })?.textContent === 'L1');
		fail(new TypeError('Failed to fetch'));

		await until(() => screen.queryByRole('button', { name: 'Try again' }) !== null);
		expect(alertText()).toContain('Could not reach the server.');
		expect(screen.queryByText(/Generating enrollment token/, { selector: 'p:not([role])' })).toBeNull();
		expect(posts).toBe(2);
	});

	it('announces a repeat failure with the same message again', async () => {
		stubMint(async () => Response.json({ error: 'x', message: 'Set LG_TUNNEL_URL.' }, { status: 422 }));
		render(LocationEditor, { props: { locationId: 'L1', tab: 'enrollment', ontab: () => {} } });
		await until(() => screen.queryByRole('button', { name: 'Try again' }) !== null);
		const first = screen.getByRole('alert');
		const retry = screen.getByRole('button', { name: 'Try again' });

		await fireEvent.click(retry);
		await until(() => retry.getAttribute('aria-busy') !== 'true' && screen.queryByRole('alert') !== first);
		// A fresh alert node: screen readers announce an inserted alert, not an unchanged one.
		expect(alertText()).toContain('Set LG_TUNNEL_URL.');
	});

	it('keeps the install command when the refresh after the agent connects fails', async () => {
		// F-353: the poll sees the agent online, then the editor's refresh GET fails.
		let gets = 0;
		vi.stubGlobal(
			'fetch',
			vi.fn(async (_input: RequestInfo | URL, init?: RequestInit) => {
				if (init?.method === 'POST') return ticketFor(1);
				gets++;
				if (gets === 1) return Response.json({ ...location, status: 'offline' });
				if (gets === 2) return Response.json({ ...location, status: 'online' });
				throw new TypeError('Failed to fetch');
			})
		);
		render(LocationEditor, { props: { locationId: 'L1', tab: 'enrollment', ontab: () => {} } });
		await until(() => command() === 'install --token T1');
		vi.advanceTimersByTime(3000);
		await until(() => screen.queryByText(/This location could not be refreshed\./) !== null);
		expect(screen.queryByText('This location could not be loaded.')).toBeNull();
		expect(command()).toBe('install --token T1');
		expect(screen.getByRole('tab', { name: 'Settings' })).toBeTruthy();
	});
});
