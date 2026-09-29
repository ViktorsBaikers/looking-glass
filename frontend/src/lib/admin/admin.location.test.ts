// Seam 3 (pure logic) for the Location editor: ASN client validation must agree
// word-for-word with central's 400 `invalid_asn`, and the Methods grid must split
// the eight supported methods by family.
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { tick } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import LocationEditor from './LocationEditor.svelte';
import { toast } from '$lib/toast.svelte.js';
import { asnError, asnValue, methodsByFamily, nullIfBlank, ASN_MESSAGE } from './editor.js';

describe('ASN client validation', () => {
	it('accepts the whole 1..4294967295 range and trims whitespace', () => {
		expect(asnValue('1')).toBe(1);
		expect(asnValue('64501')).toBe(64501);
		expect(asnValue(' 4294967295 ')).toBe(4294967295);
		expect(asnError('64501')).toBeNull();
	});

	it('treats an empty value as unset', () => {
		expect(asnValue('')).toBeNull();
		expect(asnValue('   ')).toBeNull();
		expect(asnError('')).toBeNull();
	});

	it.each(['0', '4294967296', '-3', '1.5', 'abc', '64 501', '0x10'])(
		'rejects %s with the coded server message',
		(raw) => {
			expect(asnValue(raw)).toBeNull();
			expect(asnError(raw)).toBe(ASN_MESSAGE);
		}
	);
});

describe('method families', () => {
	it('splits the eight supported methods into v4 and v6 groups', () => {
		expect(methodsByFamily()).toEqual({
			v4: ['ping', 'mtr', 'traceroute', 'bgp'],
			v6: ['ping6', 'mtr6', 'traceroute6', 'bgp6']
		});
	});
});

describe('nullIfBlank', () => {
	it('maps empty optional fields to null and keeps the rest', () => {
		expect(nullIfBlank('')).toBeNull();
		expect(nullIfBlank('  ')).toBeNull();
		expect(nullIfBlank('Equinix FR2')).toBe('Equinix FR2');
	});
});

// Seam 1 (component): a successful save refreshes the location in place. The
// editor must not swap itself for the "Loading location…" placeholder, which
// would destroy the focused Save button and reset the scroll.
describe('saving the location', () => {
	const location = {
		id: 'L1',
		name: 'Frankfurt',
		geo_label: 'Frankfurt, DE',
		map_query: null,
		facility: null,
		facility_url: null,
		kind: 'local',
		data_plane_origin: null,
		asn: null,
		offered_methods: ['ping'],
		created_at: 0,
		last_seen: null,
		status: 'online',
		test_ips: [],
		iperf: [],
		files: []
	};

	/** The PUT succeeds at once; every GET after it answers with `reload()`. */
	function stubSave(reload: () => Promise<Response>) {
		let saved = false;
		vi.stubGlobal(
			'fetch',
			vi.fn(async (_input: RequestInfo | URL, init?: RequestInit) => {
				if (init?.method === 'PUT') {
					saved = true;
					return Response.json(location);
				}
				return saved ? reload() : Response.json(location);
			})
		);
	}

	afterEach(() => {
		cleanup();
		vi.unstubAllGlobals();
		vi.restoreAllMocks();
	});

	it('refreshes in place: no loading state and the Save button keeps focus', async () => {
		const success = vi.spyOn(toast, 'success');
		let saved = false;
		let puts = 0;
		let release = () => {};
		const reloadGate = new Promise<void>((resolve) => (release = resolve));
		let finishPut = () => {};
		const putGate = new Promise<void>((resolve) => (finishPut = resolve));
		vi.stubGlobal(
			'fetch',
			vi.fn(async (_input: RequestInfo | URL, init?: RequestInit) => {
				if (init?.method === 'PUT') {
					puts++;
					await putGate;
					saved = true;
					return Response.json(location);
				}
				// Hold the post-save reload open so its in-flight state is observable.
				if (saved) await reloadGate;
				return Response.json(location);
			})
		);
		render(LocationEditor, { props: { locationId: 'L1', tab: 'settings', ontab: () => {} } });
		const save = await screen.findByRole('button', { name: 'Save location' });
		save.focus();

		await fireEvent.submit(save.closest('form')!);
		// While the PUT is in flight the focused button must stay enabled: a real
		// browser blurs a focused control the moment it becomes disabled (jsdom
		// does not), so busy is signalled with aria-busy and a double submit is
		// swallowed by the saving guard instead.
		await waitFor(() => expect(save.getAttribute('aria-busy')).toBe('true'));
		expect(save.hasAttribute('disabled')).toBe(false);
		// ...and still looks busy: aria-disabled hits the button recipe's _disabled look.
		expect(save.getAttribute('aria-disabled')).toBe('true');
		await fireEvent.submit(save.closest('form')!);
		expect(puts).toBe(1);
		finishPut();
		await waitFor(() => expect(saved).toBe(true));
		await tick();
		expect(screen.queryByText('Loading location…')).toBeNull();
		expect(save.isConnected).toBe(true);
		// Until the refresh lands the form stays busy and read-only (the refresh
		// overwrites every field), and success is announced only after it.
		const asn = screen.getByLabelText(/ASN/);
		expect(success).not.toHaveBeenCalled();
		expect(save.closest('form')!.getAttribute('aria-busy')).toBe('true');
		expect(asn.hasAttribute('readonly')).toBe(true);
		expect(asn.getAttribute('aria-disabled')).toBe('true');
		expect(save.getAttribute('aria-busy')).toBe('true');

		release();
		await waitFor(() => expect(success).toHaveBeenCalledWith('Location saved.'));
		expect(screen.getByRole('button', { name: 'Save location' })).toBe(save);
		expect(document.activeElement).toBe(save);
		expect(save.hasAttribute('aria-disabled')).toBe(false);
		expect(save.closest('form')!.hasAttribute('aria-busy')).toBe(false);
		expect(asn.hasAttribute('readonly')).toBe(false);
	});

	// Locked, not natively disabled: `disabled` would blur a focused control
	// (R-TS-05). The controls stay focusable and refuse changes instead.
	it('keeps the Methods checkboxes locked until the refresh lands', async () => {
		const success = vi.spyOn(toast, 'success');
		let release = () => {};
		const reloadGate = new Promise<void>((resolve) => (release = resolve));
		stubSave(async () => {
			await reloadGate;
			return Response.json(location);
		});
		render(LocationEditor, { props: { locationId: 'L1', tab: 'methods', ontab: () => {} } });
		const save = await screen.findByRole('button', { name: 'Save methods' });
		const bgp = screen.getByRole('checkbox', { name: 'bgp' }) as HTMLInputElement;

		await fireEvent.submit(save.closest('form')!);
		await waitFor(() => expect(save.closest('form')!.getAttribute('aria-busy')).toBe('true'));
		expect(bgp.disabled).toBe(false);
		expect(bgp.closest('[role="group"]')!.getAttribute('aria-disabled')).toBe('true');
		await fireEvent.click(bgp);
		expect(bgp.checked).toBe(false);
		expect(success).not.toHaveBeenCalled();

		release();
		await waitFor(() => expect(success).toHaveBeenCalledWith('Methods saved.'));
		expect(bgp.closest('[role="group"]')!.hasAttribute('aria-disabled')).toBe(false);
		await fireEvent.click(bgp);
		expect(bgp.checked).toBe(true);
	});

	it('keeps the Node kind select focusable but unchangeable until the refresh lands', async () => {
		let release = () => {};
		const reloadGate = new Promise<void>((resolve) => (release = resolve));
		stubSave(async () => {
			await reloadGate;
			return Response.json(location);
		});
		render(LocationEditor, { props: { locationId: 'L1', tab: 'settings', ontab: () => {} } });
		const save = await screen.findByRole('button', { name: 'Save location' });
		const kind = screen.getByRole('combobox');

		await fireEvent.submit(save.closest('form')!);
		await waitFor(() => expect(save.closest('form')!.getAttribute('aria-busy')).toBe('true'));
		expect(kind.hasAttribute('disabled')).toBe(false);
		// ...but exposed as unavailable, like the other settings controls.
		expect(kind.closest('[role="group"]')?.getAttribute('aria-disabled')).toBe('true');
		await fireEvent.click(kind);
		await fireEvent.keyDown(kind, { key: 'ArrowDown' });
		await tick();
		expect(kind.getAttribute('aria-expanded')).toBe('false');

		release();
		await waitFor(() => expect(save.closest('form')!.hasAttribute('aria-busy')).toBe(false));
		expect(kind.closest('[role="group"]')?.hasAttribute('aria-disabled')).toBe(false);
		await fireEvent.click(kind);
		await waitFor(() => expect(kind.getAttribute('aria-expanded')).toBe('true'));
	});

	// F-353: only a first load has no editor to keep. A failed refresh leaves the
	// editor in place and offers a retry instead of a dead-end page.
	it('reports the save and the failed refresh when the reload errors', async () => {
		const success = vi.spyOn(toast, 'success');
		let reloadOk = false;
		stubSave(async () =>
			reloadOk ? Response.json(location) : Response.json({ error: 'internal' }, { status: 500 })
		);
		render(LocationEditor, { props: { locationId: 'L1', tab: 'settings', ontab: () => {} } });
		const save = await screen.findByRole('button', { name: 'Save location' });

		await fireEvent.submit(save.closest('form')!);
		await waitFor(() =>
			expect(screen.getByRole('alert').textContent).toContain('This location could not be refreshed.')
		);
		expect(success).toHaveBeenCalledWith('Location saved.');
		expect(save.isConnected).toBe(true);
		expect(screen.getByRole('tab', { name: 'Settings' })).toBeTruthy();

		reloadOk = true;
		const retry = screen.getByRole('button', { name: 'Try again' });
		retry.focus();
		await fireEvent.click(retry);
		await waitFor(() => expect(screen.queryByRole('alert')).toBeNull());
		// The retry button goes with the alert; focus lands on the page heading, not <body>.
		expect(document.activeElement).toBe(screen.getByRole('heading', { level: 1 }));
	});

	// F-363: like the Enrollment tab's retry, Try again is busy while its GET
	// runs, a repeat press sends nothing, and a repeat failure is a fresh alert.
	it('the refresh Try again is busy, ignores repeat presses and re-announces a repeat failure', async () => {
		let gets = 0;
		let release = () => {};
		stubSave(async () => {
			gets++;
			if (gets === 2) await new Promise<void>((resolve) => (release = resolve));
			return Response.json({ error: 'internal' }, { status: 500 });
		});
		render(LocationEditor, { props: { locationId: 'L1', tab: 'settings', ontab: () => {} } });
		const save = await screen.findByRole('button', { name: 'Save location' });
		await fireEvent.submit(save.closest('form')!);
		await waitFor(() => expect(screen.queryByRole('alert')).not.toBeNull());
		const first = screen.getByRole('alert');
		const retry = screen.getByRole('button', { name: 'Try again' });
		retry.focus();

		await fireEvent.click(retry);
		await waitFor(() => expect(gets).toBe(2));
		expect(retry.getAttribute('aria-busy')).toBe('true');
		expect(retry.getAttribute('aria-disabled')).toBe('true');
		await fireEvent.click(retry);
		await fireEvent.click(retry);
		expect(gets).toBe(2);

		release();
		await waitFor(() => expect(retry.hasAttribute('aria-busy')).toBe(false));
		// A fresh alert node: screen readers announce an inserted alert, not an unchanged one.
		expect(screen.getByRole('alert')).not.toBe(first);
		expect(screen.getByRole('alert').textContent).toContain('This location could not be refreshed.');
		expect(document.activeElement).toBe(retry);
	});

	it('shows the full-page error only when the first load fails', async () => {
		vi.stubGlobal(
			'fetch',
			vi.fn(async () => Response.json({ error: 'internal' }, { status: 500 }))
		);
		render(LocationEditor, { props: { locationId: 'L1', tab: 'settings', ontab: () => {} } });
		await waitFor(() =>
			expect(screen.getByRole('alert').textContent).toBe('This location could not be loaded.')
		);
		expect(screen.queryByRole('tab', { name: 'Settings' })).toBeNull();
	});
});

// F-202, F-203: the Methods save sends the whole form, and a sub-resource save
// refreshes the location. Neither may lose or hide unsaved Settings edits.
describe('unsaved edits across tabs', () => {
	const location = {
		id: 'L1',
		name: 'Frankfurt',
		geo_label: 'Frankfurt, DE',
		map_query: null,
		facility: null,
		facility_url: null,
		kind: 'local',
		data_plane_origin: null,
		asn: null,
		offered_methods: ['ping', 'mtr'],
		created_at: 0,
		last_seen: null,
		status: 'online',
		test_ips: [{ id: 'ip1', label: 'Probe', address: '203.0.113.10', family: 'v4' }],
		iperf: [],
		files: []
	};
	const TABS: Record<string, string> = { settings: 'Settings', methods: 'Methods', 'test-ips': 'Test IPs' };
	let calls: string[] = [];

	/** Every GET after the first answers with `after`; DELETE and PUT succeed. */
	function stub(after: typeof location = location) {
		calls = [];
		vi.stubGlobal(
			'fetch',
			vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
				const method = init?.method ?? 'GET';
				calls.push(`${method} ${String(input)}`);
				if (method === 'DELETE') return new Response(null, { status: 204 });
				const gets = calls.filter((call) => call.startsWith('GET')).length;
				return Response.json(gets > 1 ? after : location);
			})
		);
	}

	async function gotoTab(id: string) {
		await fireEvent.click(screen.getByRole('tab', { name: TABS[id] }));
		await waitFor(() => expect(screen.getByRole('tab', { name: TABS[id], selected: true })).toBeTruthy());
		await tick();
	}

	afterEach(() => {
		cleanup();
		vi.unstubAllGlobals();
		vi.restoreAllMocks();
	});

	it('Save methods names the invalid ASN on Settings that blocks it', async () => {
		stub();
		render(LocationEditor, { props: { locationId: 'L1', tab: 'settings', ontab: () => {} } });
		const asn = await screen.findByLabelText(/ASN/);
		await fireEvent.input(asn, { target: { value: 'abc' } });
		await gotoTab('methods');
		const save = await screen.findByRole('button', { name: 'Save methods' });
		await fireEvent.click(screen.getByRole('checkbox', { name: 'bgp' }));

		await fireEvent.submit(save.closest('form')!);
		const alert = await within(save.closest('form')!).findByRole('alert');
		expect(alert.textContent).toContain('Settings');
		expect(alert.textContent).toContain(ASN_MESSAGE);
		expect(calls.filter((call) => call.startsWith('PUT'))).toEqual([]);
	});

	it('a save takes the saved copy of every field, edited ones included', async () => {
		// Central answers with its own copy (here a trimmed name): the form shows it.
		stub({ ...location, name: 'Frankfurt Main' });
		render(LocationEditor, { props: { locationId: 'L1', tab: 'settings', ontab: () => {} } });
		const name = (await screen.findByLabelText('Display name')) as HTMLInputElement;
		await fireEvent.input(name, { target: { value: ' Frankfurt Main ' } });
		await fireEvent.submit(name.closest('form')!);
		await waitFor(() => expect(calls.filter((call) => call.startsWith('GET'))).toHaveLength(2));
		await waitFor(() => expect(name.value).toBe('Frankfurt Main'));
	});

	it('a test IP delete refreshes only the fields left untouched', async () => {
		// The refresh brings a new geo label and drops mtr; neither was edited here.
		stub({ ...location, geo_label: 'Frankfurt am Main, DE', offered_methods: ['ping'] });
		render(LocationEditor, { props: { locationId: 'L1', tab: 'settings', ontab: () => {} } });
		const name = (await screen.findByLabelText('Display name')) as HTMLInputElement;
		await fireEvent.input(name, { target: { value: 'Unsaved name' } });
		await gotoTab('methods');
		await fireEvent.click(screen.getByRole('checkbox', { name: 'bgp' }));
		await gotoTab('test-ips');
		await fireEvent.click(await screen.findByRole('button', { name: 'Delete Probe' }));
		await fireEvent.click(await screen.findByRole('button', { name: 'Delete' }));
		await waitFor(() => expect(calls.filter((call) => call.startsWith('GET'))).toHaveLength(2));
		await new Promise((resolve) => setTimeout(resolve, 20));

		await gotoTab('methods');
		const checked = (method: string) => (screen.getByRole('checkbox', { name: method }) as HTMLInputElement).checked;
		expect(checked('bgp')).toBe(true);
		expect(checked('ping')).toBe(true);
		expect(checked('mtr')).toBe(false);
		await gotoTab('settings');
		expect((screen.getByLabelText('Display name') as HTMLInputElement).value).toBe('Unsaved name');
		expect((screen.getByLabelText('Geographic label') as HTMLInputElement).value).toBe('Frankfurt am Main, DE');
	});

	it('a failed refresh after a test IP delete keeps the editor and its unsaved edits', async () => {
		// F-353: the refresh GET hits a 502; the unsaved name must stay reachable.
		let deleted = false;
		vi.stubGlobal(
			'fetch',
			vi.fn(async (_input: RequestInfo | URL, init?: RequestInit) => {
				if (init?.method === 'DELETE') {
					deleted = true;
					return new Response(null, { status: 204 });
				}
				return deleted ? new Response('bad gateway', { status: 502 }) : Response.json(location);
			})
		);
		render(LocationEditor, { props: { locationId: 'L1', tab: 'settings', ontab: () => {} } });
		const name = (await screen.findByLabelText('Display name')) as HTMLInputElement;
		await fireEvent.input(name, { target: { value: 'Unsaved name' } });
		await gotoTab('test-ips');
		await fireEvent.click(await screen.findByRole('button', { name: 'Delete Probe' }));
		await fireEvent.click(await screen.findByRole('button', { name: 'Delete' }));
		await waitFor(() =>
			expect(screen.getByText(/This location could not be refreshed\./)).toBeTruthy()
		);
		expect(screen.queryByText('This location could not be loaded.')).toBeNull();
		expect(screen.getByRole('button', { name: 'Try again' })).toBeTruthy();

		await gotoTab('settings');
		expect((screen.getByLabelText('Display name') as HTMLInputElement).value).toBe('Unsaved name');
	});
});

// F-248: the tabs unmount the panel they leave, so a save in flight must be
// remembered above the Test IPs section. Reopening the row after a tab switch
// still shows the values being saved with Save busy, and a second Save sends
// nothing that could revert the first.
describe('a test IP save in flight across a tab switch', () => {
	afterEach(() => {
		cleanup();
		vi.unstubAllGlobals();
	});

	it('reopens the row on the values being saved and adopts that save', async () => {
		const location = {
			id: 'L1',
			name: 'Frankfurt',
			geo_label: 'F',
			map_query: null,
			facility: null,
			facility_url: null,
			kind: 'local',
			data_plane_origin: null,
			asn: null,
			offered_methods: ['ping'],
			created_at: 0,
			last_seen: null,
			status: 'online',
			test_ips: [{ id: 'ip-f248', location_id: 'L1', label: 'Probe', address: '198.51.100.1', family: 'v4' }],
			iperf: [],
			files: []
		};
		const puts: string[] = [];
		let release = () => {};
		const held = new Promise<void>((resolve) => (release = resolve));
		vi.stubGlobal(
			'fetch',
			vi.fn(async (_input: RequestInfo | URL, init?: RequestInit) => {
				if (init?.method === 'PUT') {
					puts.push(JSON.parse(String(init.body)).address);
					await held;
					return Response.json({});
				}
				return Response.json(location);
			})
		);
		render(LocationEditor, { props: { locationId: 'L1', tab: 'test-ips', ontab: () => {} } });
		const tab = async (name: string) => {
			await fireEvent.click(screen.getByRole('tab', { name }));
			await waitFor(() => expect(screen.getByRole('tab', { name, selected: true })).toBeTruthy());
			await tick();
		};

		await fireEvent.click(await screen.findByRole('button', { name: 'Edit Probe' }));
		const first = (await screen.findByLabelText('IP address')) as HTMLInputElement;
		await fireEvent.input(first, { target: { value: '198.51.100.77' } });
		await fireEvent.submit(first.closest('form')!);
		await waitFor(() => expect(puts).toEqual(['198.51.100.77']));
		await fireEvent.click(screen.getByRole('button', { name: 'Close dialog' }));
		await waitFor(() => expect(first.closest('[role="dialog"]')?.getAttribute('data-state')).toBe('closed'));

		await tab('Methods');
		// The Test IPs panel, and its section with it, is gone from the DOM.
		await waitFor(() => expect(document.querySelector('[aria-label="Edit Probe"]')).toBeNull());
		await tab('Test IPs');

		await fireEvent.click(await screen.findByRole('button', { name: 'Edit Probe' }));
		const reopened = (await screen.findByLabelText('IP address')) as HTMLInputElement;
		await tick();
		const form = reopened.closest('form')!;
		expect(reopened.value).toBe('198.51.100.77');
		expect(form.getAttribute('aria-busy')).toBe('true');
		await fireEvent.submit(form);
		await tick();
		expect(puts).toEqual(['198.51.100.77']);

		release();
		await waitFor(() => expect(form.getAttribute('aria-busy')).toBeNull());
		expect(reopened.value).toBe('198.51.100.77');
		expect(puts).toEqual(['198.51.100.77']);
	});
});

// F-284: a 2xx save whose body is not JSON (a proxy page, a truncated body)
// is a failed request; request() settles it as one instead of rejecting. The
// form, or the one that adopted the save after a tab switch, must leave busy
// and say so, and the row must not stay remembered as in flight: reopening it
// is editable and a retry is sent. Soft checks, so a failure reports every
// symptom.
describe('a test IP save whose response cannot be parsed', () => {
	const location = {
		id: 'L1',
		name: 'Frankfurt',
		geo_label: 'F',
		map_query: null,
		facility: null,
		facility_url: null,
		kind: 'local',
		data_plane_origin: null,
		asn: null,
		offered_methods: ['ping'],
		created_at: 0,
		last_seen: null,
		status: 'online',
		test_ips: [{ id: 'ip-f284', location_id: 'L1', label: 'Probe', address: '198.51.100.1', family: 'v4' }],
		iperf: [],
		files: []
	};
	const message = 'The request could not be completed.';
	const settle = () => new Promise((resolve) => setTimeout(resolve, 50));
	let puts: string[];
	let hold: Promise<void>;
	let unhandled: unknown[];
	const onUnhandled = (reason: unknown) => unhandled.push(reason);

	beforeEach(() => {
		puts = [];
		hold = Promise.resolve();
		unhandled = [];
		process.on('unhandledRejection', onUnhandled);
		vi.stubGlobal(
			'fetch',
			vi.fn(async (_input: RequestInfo | URL, init?: RequestInit) => {
				if (init?.method === 'PUT') {
					puts.push(JSON.parse(String(init.body)).address);
					await hold;
					return new Response('<html>proxy</html>', { status: 200, headers: { 'content-type': 'text/html' } });
				}
				return Response.json(location);
			})
		);
		render(LocationEditor, { props: { locationId: 'L1', tab: 'test-ips', ontab: () => {} } });
	});

	afterEach(() => {
		process.off('unhandledRejection', onUnhandled);
		cleanup();
		vi.unstubAllGlobals();
		vi.restoreAllMocks();
	});

	const tab = async (name: string) => {
		await fireEvent.click(screen.getByRole('tab', { name }));
		await waitFor(() => expect(screen.getByRole('tab', { name, selected: true })).toBeTruthy());
		await tick();
	};
	const openProbe = async () => {
		await fireEvent.click(await screen.findByRole('button', { name: 'Edit Probe' }));
		return ((await screen.findByLabelText('IP address')) as HTMLInputElement).closest('form')!;
	};
	const closeAndSwitchTabs = async (form: HTMLFormElement) => {
		await fireEvent.click(screen.getByRole('button', { name: 'Close dialog' }));
		await waitFor(() => expect(form.closest('[role="dialog"]')?.getAttribute('data-state')).toBe('closed'));
		await tab('Methods');
		await waitFor(() => expect(document.querySelector('[aria-label="Edit Probe"]')).toBeNull());
		await tab('Test IPs');
	};
	const saveProbe = async (form: HTMLFormElement) => {
		await fireEvent.input(within(form).getByLabelText('IP address'), { target: { value: '198.51.100.77' } });
		await fireEvent.submit(form);
		await waitFor(() => expect(puts).toHaveLength(1));
	};

	it('leaves busy, shows the error and lets the row be saved again', async () => {
		const errors = vi.spyOn(toast, 'error');
		const form = await openProbe();
		await saveProbe(form);
		await settle();
		expect.soft(form.getAttribute('aria-busy'), 'busy after the failed save').toBeNull();
		expect.soft(within(form).queryByRole('alert')?.textContent, 'error in the form').toBe(message);
		expect.soft(errors, 'error toast').toHaveBeenCalledWith(message);

		await closeAndSwitchTabs(form);
		const reopened = await openProbe();
		await settle();
		expect.soft(reopened.getAttribute('aria-busy'), 'busy after a tab switch and reopen').toBeNull();
		expect.soft((within(reopened).getByLabelText('IP address') as HTMLInputElement).value, 'reopen shows the stored address').toBe('198.51.100.1');
		expect.soft(within(reopened).queryByRole('alert'), 'no stale error after reopen').toBeNull();
		await fireEvent.submit(reopened);
		await settle();
		expect.soft(puts, 'the retry sends a request').toHaveLength(2);
		expect.soft(unhandled, 'unhandled rejections').toEqual([]);
	});

	it('a form that adopted the save leaves busy with the error', async () => {
		let release = () => {};
		hold = new Promise<void>((resolve) => (release = resolve));
		const form = await openProbe();
		await saveProbe(form);
		await closeAndSwitchTabs(form);
		const adopted = await openProbe();
		expect(adopted.getAttribute('aria-busy')).toBe('true');

		release();
		await settle();
		expect.soft(adopted.getAttribute('aria-busy'), 'adopting form busy after the failed save').toBeNull();
		expect.soft(within(adopted).queryByRole('alert')?.textContent, 'error in the adopting form').toBe(message);
		await fireEvent.submit(adopted);
		await settle();
		expect.soft(puts, 'the retry sends a request').toHaveLength(2);
		expect.soft(unhandled, 'unhandled rejections').toEqual([]);
	});
});
