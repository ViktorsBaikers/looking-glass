import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import Layout from './+layout.svelte';

const { goto, navigated } = vi.hoisted(() => ({ goto: vi.fn(), navigated: { to: (_path: string) => {} } }));

vi.mock('$app/navigation', () => ({
	goto,
	afterNavigate: (callback: (navigation: unknown) => void) => {
		navigated.to = (path) =>
			callback({ type: 'link', from: { url: new URL('http://x/admin') }, to: { url: new URL('http://x' + path) } });
		queueMicrotask(() => callback({ type: 'enter' }));
	}
}));
vi.mock('$app/state', () => ({ page: { url: new URL('http://looking-glass.test/admin') } }));

const json = (body: unknown, status = 200) =>
	new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json' } });
const signedIn = () => Promise.resolve(json({ id: 1, username: 'admin' }));
const signedOut = () => Promise.resolve(json({ error: 'unauthorized', message: 'Authentication required.' }, 401));

// F-297: only central's "unauthorized" answer means signed out. Anything else
// on /api/admin/me says nothing about the session, so the admin stays put and
// can try again.
describe('admin gate', () => {
	beforeEach(() => {
		goto.mockReset();
		// jsdom has no scrollIntoView (the failed-logout alert calls it).
		Element.prototype.scrollIntoView = vi.fn();
	});
	afterEach(() => {
		cleanup();
		vi.unstubAllGlobals();
	});

	it('opens the admin area for a signed-in session', async () => {
		vi.stubGlobal('fetch', vi.fn(signedIn));
		render(Layout);
		expect(await screen.findByRole('complementary', { name: 'Administration' })).toBeTruthy();
		expect(goto).not.toHaveBeenCalled();
	});

	it('sends a signed-out session to sign-in', async () => {
		vi.stubGlobal('fetch', vi.fn(signedOut));
		render(Layout);
		await waitFor(() => expect(goto).toHaveBeenCalledWith('/login'));
		expect(screen.queryByRole('alert')).toBeNull();
	});

	// F-316: only a network error means the server was not reached; any other
	// answer came from a server that could not confirm the session.
	const unreachable: [string, () => Promise<Response>, RegExp][] = [
		['a network error', () => Promise.reject(new TypeError('Failed to fetch')), /could not reach the server/i],
		['a 429', () => Promise.resolve(json({ error: 'rate_limited', message: 'Too many attempts. Try again later.' }, 429)), /server could not check your session/i],
		['a 500', () => Promise.resolve(json({ error: 'internal_error', message: 'Something went wrong.' }, 500)), /server could not check your session/i],
		['a 503', () => Promise.resolve(json({ error: 'unavailable', message: 'Service unavailable.' }, 503)), /server could not check your session/i],
		['a 502 proxy page', () => Promise.resolve(new Response('<html>Bad Gateway</html>', { status: 502 })), /server could not check your session/i],
		['a 200 proxy page', () => Promise.resolve(new Response('<html>maintenance</html>', { status: 200 })), /server could not check your session/i]
	];
	for (const [name, reply, message] of unreachable) {
		it(`keeps the admin on ${name} and signs in again only when a retry says signed out`, async () => {
			const fetchMock = vi.fn(reply);
			vi.stubGlobal('fetch', fetchMock);
			render(Layout);

			const alert = await screen.findByRole('alert');
			expect(alert.textContent).toMatch(message);
			if (name !== 'a network error') expect(alert.textContent).not.toMatch(/could not reach/i);
			expect(goto).not.toHaveBeenCalled();

			fetchMock.mockImplementation(signedOut);
			await fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
			await waitFor(() => expect(goto).toHaveBeenCalledWith('/login'));
		});
	}

	// F-299: a retry that fails again puts a fresh alert up (announced once)
	// and hands focus back to its Try again button instead of <body>.
	it('returns keyboard focus to Try again when a retry fails again and focus was left on <body>', async () => {
		const fetchMock = vi.fn((): Promise<Response> => Promise.reject(new TypeError('Failed to fetch')));
		vi.stubGlobal('fetch', fetchMock);
		render(Layout);
		await screen.findByRole('alert');
		expect(document.activeElement).toBe(document.body);

		const alerts: Element[] = [];
		new MutationObserver((records) => {
			for (const record of records)
				for (const node of record.addedNodes)
					if (node instanceof Element)
						alerts.push(...[node, ...node.querySelectorAll('*')].filter((el) => el.getAttribute('role') === 'alert'));
		}).observe(document.body, { childList: true, subtree: true });

		const button = screen.getByRole('button', { name: 'Try again' });
		button.focus();
		await fireEvent.click(button);
		await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(2));
		await waitFor(() => expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Try again' })));
		expect(alerts).toHaveLength(1);
	});

	it('leaves focus where the user moved it while a failed retry was pending', async () => {
		const fetchMock = vi.fn((): Promise<Response> => Promise.reject(new TypeError('Failed to fetch')));
		vi.stubGlobal('fetch', fetchMock);
		render(Layout);
		await screen.findByRole('alert');

		let fail = () => {};
		fetchMock.mockImplementationOnce(() => new Promise((_, reject) => (fail = () => reject(new TypeError('Failed to fetch')))));
		const button = screen.getByRole('button', { name: 'Try again' });
		button.focus();
		await fireEvent.click(button);
		const elsewhere = document.body.appendChild(document.createElement('button'));
		elsewhere.focus();
		fail();
		await screen.findByRole('alert');
		await new Promise((resolve) => setTimeout(resolve, 20));
		expect(document.activeElement).toBe(elsewhere);
		elsewhere.remove();
	});

	// F-307: only the newest session check decides; an older answer that
	// arrives late is ignored.
	it('keeps the admin area when an older failing check answers after a newer success', async () => {
		const fetchMock = vi.fn(signedIn);
		vi.stubGlobal('fetch', fetchMock);
		render(Layout);
		await screen.findByRole('complementary', { name: 'Administration' });

		let failOlder = () => {};
		fetchMock.mockImplementationOnce(
			() => new Promise((_, reject) => (failOlder = () => reject(new TypeError('Failed to fetch'))))
		);
		navigated.to('/admin/settings');
		navigated.to('/admin/administrators');
		await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(3));
		failOlder();
		await new Promise((resolve) => setTimeout(resolve, 20));
		expect(screen.queryByRole('alert')).toBeNull();
		expect(screen.getByRole('complementary', { name: 'Administration' })).toBeTruthy();
	});

	it('opens the admin area when a retry reaches the server', async () => {
		const fetchMock = vi.fn((): Promise<Response> => Promise.reject(new TypeError('Failed to fetch')));
		vi.stubGlobal('fetch', fetchMock);
		render(Layout);
		await screen.findByRole('alert');

		fetchMock.mockImplementation(signedIn);
		await fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
		expect(await screen.findByRole('complementary', { name: 'Administration' })).toBeTruthy();
		expect(screen.queryByRole('alert')).toBeNull();
		expect(fetchMock).toHaveBeenCalledTimes(2);
		expect(goto).not.toHaveBeenCalled();
	});
	// F-314: the check and a restored session are status messages, spoken
	// through one polite live region that stays in the page across states.
	it('announces the session check and a restored session through a polite status region', async () => {
		const fetchMock = vi.fn((): Promise<Response> => Promise.reject(new TypeError('Failed to fetch')));
		vi.stubGlobal('fetch', fetchMock);
		render(Layout);
		await screen.findByRole('alert');
		const status = screen.getByRole('status');

		let answer = () => {};
		fetchMock.mockImplementationOnce(() => new Promise((resolve) => (answer = () => resolve(json({ id: 1, username: 'admin' })))));
		await fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
		expect(screen.getByRole('status')).toBe(status);
		expect(status.textContent).toMatch(/checking your session/i);

		answer();
		await screen.findByRole('complementary', { name: 'Administration' });
		expect(screen.getByRole('status')).toBe(status);
		expect(status.textContent).toMatch(/session confirmed/i);
	});

	// F-306: a route-change check that fails takes the panel (and the focused
	// link) away; Try again takes focus when it was left on <body>.
	it('moves focus to Try again when a route-change check fails and focus was on <body>', async () => {
		const fetchMock = vi.fn(signedIn);
		vi.stubGlobal('fetch', fetchMock);
		render(Layout);
		await screen.findByRole('complementary', { name: 'Administration' });

		fetchMock.mockImplementation(() => Promise.reject(new TypeError('Failed to fetch')));
		(document.activeElement as HTMLElement | null)?.blur();
		navigated.to('/admin/settings');
		await screen.findByRole('alert');
		await waitFor(() => expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Try again' })));
	});

	// F-315: a logout the server did not confirm leaves the session alive, so
	// the admin stays on the page and is told.
	for (const [name, reply] of [
		['a network error', () => Promise.reject(new TypeError('Failed to fetch'))],
		['a 500', () => Promise.resolve(json({ error: 'internal_error', message: 'Something went wrong.' }, 500))]
	] as [string, () => Promise<Response>][]) {
		it(`keeps the admin on the page with an error when logout fails with ${name}`, async () => {
			const fetchMock = vi.fn(signedIn);
			vi.stubGlobal('fetch', fetchMock);
			render(Layout);
			await screen.findByRole('complementary', { name: 'Administration' });

			fetchMock.mockImplementation(reply);
			await fireEvent.click(screen.getAllByRole('button', { name: /log out/i })[0]);
			expect((await screen.findByRole('alert')).textContent).toMatch(/could not log out/i);
			expect(goto).not.toHaveBeenCalled();
			expect(screen.getByRole('complementary', { name: 'Administration' })).toBeTruthy();
		});
	}

	// F-326: the failed-logout alert is scrolled into view (Log out sits in a
	// sticky sidebar, so the page may be scrolled) and does not follow the admin
	// to the next page.
	it('scrolls a failed logout into view and clears it on the next navigation', async () => {
		const fetchMock = vi.fn(signedIn);
		vi.stubGlobal('fetch', fetchMock);
		render(Layout);
		await screen.findByRole('complementary', { name: 'Administration' });

		fetchMock.mockImplementation(() => Promise.reject(new TypeError('Failed to fetch')));
		await fireEvent.click(screen.getAllByRole('button', { name: /log out/i })[0]);
		const alert = await screen.findByRole('alert');
		expect(vi.mocked(Element.prototype.scrollIntoView).mock.contexts).toContain(alert);

		fetchMock.mockImplementation(signedIn);
		navigated.to('/admin/settings');
		await waitFor(() => expect(screen.queryByRole('alert')).toBeNull());
	});

	it('goes to sign-in after a logout the server confirmed', async () => {
		const fetchMock = vi.fn(signedIn);
		vi.stubGlobal('fetch', fetchMock);
		render(Layout);
		await screen.findByRole('complementary', { name: 'Administration' });

		fetchMock.mockImplementation(() => Promise.resolve(new Response(null, { status: 204 })));
		await fireEvent.click(screen.getAllByRole('button', { name: /log out/i })[0]);
		await waitFor(() => expect(goto).toHaveBeenCalledWith('/login'));
	});

	// F-317: an answer that arrives after the admin area unmounted (the user
	// already left for /) does not pull them onto sign-in.
	it('does not navigate when a signed-out answer arrives after the layout unmounted', async () => {
		let answer = () => {};
		vi.stubGlobal('fetch', vi.fn(() => new Promise<Response>((resolve) => (answer = () => resolve(json({ error: 'unauthorized', message: 'Authentication required.' }, 401))))));
		render(Layout);
		await new Promise((resolve) => setTimeout(resolve, 10));
		cleanup();
		answer();
		await new Promise((resolve) => setTimeout(resolve, 20));
		expect(goto).not.toHaveBeenCalled();
	});
});
