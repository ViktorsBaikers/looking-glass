// Seam 3 (pure logic) for the admin Locations list: derived state, search and
// sort. The rendered page itself is covered by tests/locations.e2e.spec.ts.
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { tick } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { JsonResult } from '$lib/api.js';
import LocationsPage from '../../routes/admin/+page.svelte';
import CrudSection from './CrudSection.svelte';
import { filterLocations, locationState, sortLocations } from './locationList.js';
import type { Location } from './types.js';

vi.mock('$app/navigation', () => ({ goto: vi.fn() }));

function location(overrides: Partial<Location> = {}): Location {
	return {
		id: 'fra',
		name: 'Frankfurt',
		geo_label: 'Frankfurt, DE',
		map_query: null,
		facility: null,
		facility_url: null,
		kind: 'remote',
		data_plane_origin: null,
		asn: 64501,
		offered_methods: ['ping'],
		status: 'online',
		created_at: 0,
		last_seen: 1_700_000_000,
		...overrides
	};
}

const fleet: Location[] = [
	location(),
	location({
		id: 'vie',
		name: 'Vienna',
		geo_label: 'Vienna, AT',
		asn: 64500,
		status: 'offline',
		last_seen: 1_700_000_500
	}),
	location({
		id: 'sfo',
		name: 'San Francisco Hub',
		geo_label: 'SF-01',
		asn: null,
		offered_methods: [],
		status: 'offline',
		last_seen: null
	}),
	location({ id: 'lon', name: 'London', geo_label: 'London, UK', kind: 'local', asn: 64502 })
];

describe('locationState', () => {
	it('reads the heartbeat-derived state the badges show', () => {
		expect(locationState(fleet[0])).toBe('online');
		expect(locationState(fleet[1])).toBe('offline');
		expect(locationState(fleet[2])).toBe('not_enrolled');
	});

	it('treats a local node by its reported status, never as unenrolled', () => {
		expect(locationState(fleet[3])).toBe('online');
		expect(locationState(location({ kind: 'local', status: 'offline', last_seen: null }))).toBe(
			'offline'
		);
	});
});

describe('filterLocations', () => {
	it('keeps everything on an empty or blank query', () => {
		expect(filterLocations(fleet, '')).toEqual(fleet);
		expect(filterLocations(fleet, '   ')).toEqual(fleet);
	});

	it('matches name and geo label case-insensitively', () => {
		expect(filterLocations(fleet, 'frank').map((l) => l.id)).toEqual(['fra']);
		expect(filterLocations(fleet, 'VIENNA, AT').map((l) => l.id)).toEqual(['vie']);
	});

	it('matches the displayed state', () => {
		expect(filterLocations(fleet, 'not enrolled').map((l) => l.id)).toEqual(['sfo']);
		expect(filterLocations(fleet, 'online')).toEqual(
			fleet.filter((l) => l.status === 'online')
		);
	});

	it('matches the ASN as bare digits and as the AS form', () => {
		expect(filterLocations(fleet, '64500').map((l) => l.id)).toEqual(['vie']);
		expect(filterLocations(fleet, 'as64502').map((l) => l.id)).toEqual(['lon']);
		expect(filterLocations(fleet, '64999')).toEqual([]);
	});
});

describe('sortLocations', () => {
	it('sorts by name alphabetically', () => {
		expect(sortLocations(fleet, 'name').map((l) => l.name)).toEqual([
			'Frankfurt',
			'London',
			'San Francisco Hub',
			'Vienna'
		]);
	});

	it('groups by state (online, offline, not enrolled) with name as tie-break', () => {
		expect(sortLocations(fleet, 'status').map((l) => l.id)).toEqual([
			'fra',
			'lon',
			'vie',
			'sfo'
		]);
	});

	it('puts the freshest heartbeat first and never-seen last', () => {
		expect(sortLocations(fleet, 'recent').map((l) => l.id)).toEqual([
			'vie',
			'fra',
			'lon',
			'sfo'
		]);
	});

	it('does not mutate the input array', () => {
		const input = [...fleet];
		sortLocations(input, 'name');
		expect(input).toEqual(fleet);
	});
});

// F-195: reopening Add while a create is still pending must show that create
// busy, not a blank form whose Save sends a duplicate.
describe('reopening Add while a create is pending', () => {
	afterEach(cleanup);

	it('shows the pending create and sends no second one', async () => {
		const creates: unknown[] = [];
		let release = () => {};
		const held = new Promise<void>((resolve) => (release = resolve));
		render(CrudSection, {
			props: {
				title: 'Test IPs',
				description: '',
				addLabel: 'Add test IP',
				itemLabel: 'test IP',
				items: [],
				columns: [],
				// render() infers the form type as unknown, which has no keys.
				fields: [{ key: 'address' as never, label: 'IP address' }],
				rowName: () => '',
				create: async (draft: unknown) => {
					creates.push(draft);
					await held;
					return { ok: true, data: {} };
				},
				update: async () => ({ ok: true, data: {} }),
				remove: async () => ({ ok: true, data: {} }),
				onchanged: () => {},
				savedMessage: 'Saved.',
				deletedMessage: 'Deleted.'
			}
		});

		await fireEvent.click(screen.getByRole('button', { name: 'Add test IP' }));
		const first = (await screen.findByLabelText('IP address')) as HTMLInputElement;
		await fireEvent.input(first, { target: { value: '198.51.100.9' } });
		await fireEvent.submit(first.closest('form')!);
		expect(creates).toEqual([{ address: '198.51.100.9' }]);
		await fireEvent.click(screen.getByRole('button', { name: 'Close dialog' }));
		await waitFor(() => expect(first.closest('[role="dialog"]')?.getAttribute('data-state')).toBe('closed'));

		await fireEvent.click(screen.getByRole('button', { name: 'Add test IP' }));
		const reopened = (await screen.findByLabelText('IP address')) as HTMLInputElement;
		await tick();
		const form = reopened.closest('form')!;
		expect(reopened.value).toBe('198.51.100.9');
		expect(form.getAttribute('aria-busy')).toBe('true');
		await fireEvent.submit(form);
		await tick();
		expect(creates).toHaveLength(1);

		release();
		await waitFor(() => expect(form.closest('[role="dialog"]')?.getAttribute('data-state')).toBe('closed'));
		expect(creates).toHaveLength(1);
	});
});

// F-320: the pending create belongs to its location. Location B's Add form
// must not adopt a create still pending on location A, while reopening A's
// section (remounted, as a tab or location switch does) still adopts it.
describe('a pending create is scoped to its location', () => {
	afterEach(cleanup);

	function section(scope: string, creates: unknown[], result: Promise<JsonResult<unknown>>) {
		return render(CrudSection, {
			props: {
				title: 'Test IPs',
				description: '',
				addLabel: 'Add test IP',
				itemLabel: 'test IP',
				scope,
				items: [],
				columns: [],
				fields: [{ key: 'address' as never, label: 'IP address' }],
				rowName: () => '',
				create: async (draft: unknown) => {
					creates.push(draft);
					return result;
				},
				update: async () => ({ ok: true, data: {} }),
				remove: async () => ({ ok: true, data: {} }),
				onchanged: () => {},
				savedMessage: 'Saved.',
				deletedMessage: 'Deleted.'
			}
		});
	}

	async function openAdd() {
		await fireEvent.click(screen.getByRole('button', { name: 'Add test IP' }));
		const input = (await screen.findByLabelText('IP address')) as HTMLInputElement;
		await tick();
		return input;
	}

	it('B opens blank and idle while A has a create pending; A still adopts it', async () => {
		let fail = () => {};
		const pending = new Promise<JsonResult<unknown>>(
			(resolve) => (fail = () => resolve({ ok: false, error: 'bad_request', message: 'A failed' }))
		);
		const onA: unknown[] = [];
		const onB: unknown[] = [];

		const a = section('fra', onA, pending);
		const first = await openAdd();
		await fireEvent.input(first, { target: { value: '198.51.100.9' } });
		await fireEvent.submit(first.closest('form')!);
		a.unmount();

		const b = section('vie', onB, Promise.resolve({ ok: true, data: {} }));
		const onBInput = await openAdd();
		expect(onBInput.value).toBe('');
		expect(onBInput.closest('form')!.getAttribute('aria-busy')).toBeNull();
		b.unmount();

		section('fra', onA, pending);
		const reopened = await openAdd();
		const form = reopened.closest('form')!;
		expect(reopened.value).toBe('198.51.100.9');
		expect(form.getAttribute('aria-busy')).toBe('true');
		await fireEvent.submit(form);
		await tick();
		expect(onA).toHaveLength(1);

		fail();
		await waitFor(() => expect(form.getAttribute('aria-busy')).toBeNull());
		expect(onA).toHaveLength(1);
		expect(onB).toHaveLength(0);
	});
});

// F-335: a create adopted by a reopened Add must, when it answers, leave alone
// an Edit dialog opened after it (the generation guard in open()).
describe('a late adopted create and a newer edit', () => {
	afterEach(cleanup);

	it('leaves an Edit opened after the adoption open', async () => {
		let release = () => {};
		const held = new Promise<void>((resolve) => (release = resolve));
		// A variable, not a literal: render() infers the row type as { id: string }.
		const row = { id: 'ip-1', address: '198.51.100.1' };
		render(CrudSection, {
			props: {
				title: 'Test IPs',
				description: '',
				addLabel: 'Add test IP',
				itemLabel: 'test IP',
				scope: 'f-335',
				items: [row],
				columns: [],
				fields: [{ key: 'address' as never, label: 'IP address' }],
				rowName: () => 'Probe',
				create: async () => {
					await held;
					return { ok: true, data: {} };
				},
				update: async () => ({ ok: true, data: {} }),
				remove: async () => ({ ok: true, data: {} }),
				onchanged: () => {},
				savedMessage: 'Saved.',
				deletedMessage: 'Deleted.'
			}
		});
		const close = async (input: HTMLInputElement) => {
			await fireEvent.click(screen.getByRole('button', { name: 'Close dialog' }));
			await waitFor(() => expect(input.closest('[role="dialog"]')?.getAttribute('data-state')).toBe('closed'));
		};

		await fireEvent.click(screen.getByRole('button', { name: 'Add test IP' }));
		const add = (await screen.findByLabelText('IP address')) as HTMLInputElement;
		await fireEvent.input(add, { target: { value: '198.51.100.9' } });
		await fireEvent.submit(add.closest('form')!);
		await close(add);

		// Reopening Add adopts the pending create; close it and edit the row instead.
		await fireEvent.click(screen.getByRole('button', { name: 'Add test IP' }));
		const adopted = (await screen.findByLabelText('IP address')) as HTMLInputElement;
		await tick();
		expect(adopted.closest('form')!.getAttribute('aria-busy')).toBe('true');
		await close(adopted);

		await fireEvent.click(screen.getByRole('button', { name: 'Edit Probe' }));
		const edit = (await screen.findByLabelText('IP address')) as HTMLInputElement;
		await tick();
		const dialog = edit.closest('[role="dialog"]')!;
		expect(edit.value).toBe('198.51.100.1');
		expect(dialog.getAttribute('data-state')).toBe('open');

		release();
		await new Promise((resolve) => setTimeout(resolve, 50));
		await tick();
		expect(dialog.getAttribute('data-state')).toBe('open');
		expect(edit.value).toBe('198.51.100.1');
	});
});

// F-351: a burst of reorder moves saves one order at a time, so the server
// ends on the last order even when an earlier save answers late.
describe('reordering locations', () => {
	afterEach(() => {
		cleanup();
		vi.unstubAllGlobals();
	});

	it('sends the next order only after the previous save answered', async () => {
		const saves: string[][] = [];
		let release = () => {};
		const held = new Promise<void>((resolve) => (release = resolve));
		vi.stubGlobal(
			'fetch',
			vi.fn(async (path: string, init?: RequestInit) => {
				if (path === '/api/admin/locations/order') {
					saves.push(JSON.parse(init!.body as string).ids);
					if (saves.length === 1) await held;
					return new Response(null, { status: 204 });
				}
				return new Response(JSON.stringify(fleet), { headers: { 'content-type': 'application/json' } });
			})
		);
		render(LocationsPage);
		const grip = () => screen.getByRole('button', { name: 'Reorder Frankfurt' });
		await fireEvent.keyDown(await screen.findByRole('button', { name: 'Reorder Frankfurt' }), { key: 'ArrowDown' });
		await waitFor(() => expect(saves).toHaveLength(1));
		await fireEvent.keyDown(grip(), { key: 'ArrowDown' });
		await new Promise((resolve) => setTimeout(resolve, 50));
		expect(saves).toHaveLength(1);

		release();
		await waitFor(() => expect(saves).toHaveLength(2));
		expect(saves).toEqual([
			['vie', 'fra', 'sfo', 'lon'],
			['vie', 'sfo', 'fra', 'lon']
		]);
	});
});
