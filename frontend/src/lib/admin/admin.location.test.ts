import { cleanup, render, screen, waitFor, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import LocationsPage from '../../routes/admin/+page.svelte';
import { fetchLocations } from '../public/api.js';
import { runnableMethods } from '../public/methods.js';
import LocationEditor from './LocationEditor.svelte';
import { OFFERED_METHODS } from './types.js';
import type { IperfEndpoint, LocationDetail, TestFile, TestIp } from './types.js';

type Fixture = {
	locations: Map<string, LocationDetail>;
	nextId: number;
};

let fixture: Fixture;

function location(): LocationDetail {
	return {
		id: 'fra',
		name: 'Frankfurt',
		geo_label: 'Frankfurt, DE',
		map_query: null,
		facility: null,
		facility_url: null,
		kind: 'local',
		data_plane_origin: null,
		offered_methods: ['ping', 'mtr'],
		status: 'online',
		created_at: 0,
		test_ips: [],
		iperf: [],
		files: []
	};
}

function response(body: unknown, status = 200) {
	return new Response(JSON.stringify(body), {
		status,
		headers: { 'content-type': 'application/json' }
	});
}

function invalidInput(message: string) {
	return response({ error: 'invalid_input', message }, 422);
}

function recordForChild(id: string, key: 'test_ips' | 'iperf' | 'files') {
	for (const record of fixture.locations.values()) {
		const item = record[key].find((candidate) => candidate.id === id);
		if (item) return [record, item] as const;
	}
	return null;
}

function body(init?: RequestInit) {
	return JSON.parse(String(init?.body)) as Record<string, unknown>;
}

function adminLocationList() {
	return [...fixture.locations.values()];
}

function publicLocationList() {
	return adminLocationList().filter((record) => record.status === 'online');
}

function requiredString(draft: Record<string, unknown>, key: string) {
	const value = draft[key];
	return typeof value === 'string' && value.length > 0 ? value : null;
}

function nullableString(draft: Record<string, unknown>, key: string) {
	const value = draft[key];
	return value === undefined || value === null ? null : typeof value === 'string' ? value : undefined;
}

function replacementLocation(record: LocationDetail, draft: Record<string, unknown>): LocationDetail | null {
	const name = requiredString(draft, 'name');
	const geoLabel = draft.geo_label;
	const mapQuery = nullableString(draft, 'map_query');
	const facility = nullableString(draft, 'facility');
	const facilityUrl = nullableString(draft, 'facility_url');
	const dataPlaneOrigin = nullableString(draft, 'data_plane_origin');
	if (
		!name ||
		typeof geoLabel !== 'string' ||
		mapQuery === undefined ||
		facility === undefined ||
		facilityUrl === undefined ||
		dataPlaneOrigin === undefined ||
		(draft.kind !== 'local' && draft.kind !== 'remote') ||
		!Array.isArray(draft.offered_methods) ||
		!draft.offered_methods.every(
			(method) => typeof method === 'string' && OFFERED_METHODS.includes(method as (typeof OFFERED_METHODS)[number])
		)
	) {
		return null;
	}
	return {
		...record,
		name,
		geo_label: geoLabel,
		map_query: mapQuery,
		facility,
		facility_url: facilityUrl,
		kind: draft.kind as LocationDetail['kind'],
		data_plane_origin: dataPlaneOrigin,
		status: draft.kind === 'local' ? 'online' : record.status,
		offered_methods: draft.offered_methods as LocationDetail['offered_methods']
	};
}

async function fetchFixture(input: RequestInfo | URL, init?: RequestInit) {
	const path = new URL(input.toString(), 'http://looking-glass.test').pathname;
	const method = init?.method ?? 'GET';

	if (method === 'GET' && path === '/api/admin/locations') return response(adminLocationList());
	if (method === 'GET' && path === '/api/locations') return response(publicLocationList());

	const locationMatch = path.match(/^\/api\/admin\/locations\/([^/]+)$/);
	if (locationMatch) {
		const record = fixture.locations.get(locationMatch[1]);
		if (method === 'GET') return record ? response(record) : response({}, 404);
		if (method === 'PUT') {
			if (!record) return response({}, 404);
			const updated = replacementLocation(record, body(init));
			if (!updated) return invalidInput('Complete location details are required.');
			fixture.locations.set(updated.id, updated);
			return response(updated);
		}
		if (method === 'DELETE') {
			fixture.locations.delete(locationMatch[1]);
			return new Response(null, { status: 204 });
		}
	}

	if (method === 'POST' && path === '/api/admin/locations') {
		const draft = body(init);
		if (!draft.name) return invalidInput('Display name is required.');
		const id = `location-${fixture.nextId++}`;
		const record: LocationDetail = {
			...location(),
			id,
			name: String(draft.name),
			geo_label: String(draft.geo_label ?? ''),
			kind: draft.kind === 'remote' ? 'remote' : 'local',
			offered_methods: []
		};
		fixture.locations.set(id, record);
		return response(record, 201);
	}

	const childMatch = path.match(/^\/api\/admin\/locations\/([^/]+)\/(test-ips|iperf|files)$/);
	if (method === 'POST' && childMatch) {
		const record = fixture.locations.get(childMatch[1]);
		if (!record) return response({}, 404);
		const id = `${childMatch[2]}-${fixture.nextId++}`;
		const draft = body(init);
		if (childMatch[2] === 'test-ips') {
			const address = requiredString(draft, 'address');
			const label = nullableString(draft, 'label');
			if (!address || label === undefined || (draft.family !== 'v4' && draft.family !== 'v6')) {
				return invalidInput('Complete Test IP details are required.');
			}
			const item: TestIp = {
				id,
				location_id: record.id,
				address,
				family: draft.family,
				label
			};
			record.test_ips.push(item);
			return response(item, 201);
		}
		if (childMatch[2] === 'iperf') {
			const item: IperfEndpoint = {
				id,
				location_id: record.id,
				label: String(draft.label),
				host: String(draft.host),
				port: Number(draft.port),
				cmd_incoming: String(draft.cmd_incoming),
				cmd_outgoing: String(draft.cmd_outgoing)
			};
			record.iperf.push(item);
			return response(item, 201);
		}
		const item: TestFile = {
			id,
			location_id: record.id,
			label: String(draft.label),
			declared_size: String(draft.declared_size),
			source_ref: String(draft.source_ref)
		};
		record.files.push(item);
		return response(item, 201);
	}

	const childUpdateMatch = path.match(/^\/api\/admin\/(test-ips|iperf|files)\/([^/]+)$/);
	if (childUpdateMatch) {
		const key: 'test_ips' | 'iperf' | 'files' =
			childUpdateMatch[1] === 'test-ips' ? 'test_ips' : childUpdateMatch[1] === 'iperf' ? 'iperf' : 'files';
		const found = recordForChild(childUpdateMatch[2], key);
		if (!found) return response({}, 404);
		const [record, item] = found;
		if (method === 'PUT') {
			const draft = body(init);
			if (key === 'test_ips') {
				const address = requiredString(draft, 'address');
				const label = nullableString(draft, 'label');
				if (!address || label === undefined || (draft.family !== 'v4' && draft.family !== 'v6')) {
					return invalidInput('Complete Test IP details are required.');
				}
				const updated: TestIp = {
					id: item.id,
					location_id: item.location_id,
					family: draft.family,
					address,
					label
				};
				record.test_ips = record.test_ips.map((candidate) =>
					candidate.id === item.id ? updated : candidate
				);
				return response(updated);
			}
			if (key === 'iperf') {
				const label = requiredString(draft, 'label');
				const host = requiredString(draft, 'host');
				const cmdIncoming = requiredString(draft, 'cmd_incoming');
				const cmdOutgoing = requiredString(draft, 'cmd_outgoing');
				const port = typeof draft.port === 'number' ? draft.port : Number.NaN;
				if (!label || !host || !cmdIncoming || !cmdOutgoing || !Number.isInteger(port) || port < 1) {
					return invalidInput('Complete iperf details are required.');
				}
				const updated: IperfEndpoint = {
					id: item.id,
					location_id: item.location_id,
					label,
					host,
					port,
					cmd_incoming: cmdIncoming,
					cmd_outgoing: cmdOutgoing
				};
				record.iperf = record.iperf.map((candidate) => (candidate.id === item.id ? updated : candidate));
				return response(updated);
			}
			const label = requiredString(draft, 'label');
			const declaredSize = requiredString(draft, 'declared_size');
			const sourceRef = requiredString(draft, 'source_ref');
			if (!label || !declaredSize || !sourceRef) return invalidInput('Complete file details are required.');
			const updated: TestFile = {
				id: item.id,
				location_id: item.location_id,
				label,
				declared_size: declaredSize,
				source_ref: sourceRef
			};
			record.files = record.files.map((candidate) => (candidate.id === item.id ? updated : candidate));
			return response(updated);
		}
		if (method === 'DELETE') {
			if (key === 'test_ips') record.test_ips = record.test_ips.filter((candidate) => candidate.id !== item.id);
			else if (key === 'iperf') record.iperf = record.iperf.filter((candidate) => candidate.id !== item.id);
			else record.files = record.files.filter((candidate) => candidate.id !== item.id);
			return new Response(null, { status: 204 });
		}
	}

	throw new Error(`unexpected fetch ${method} ${path}`);
}

async function publicLocations() {
	const result = await fetchLocations();
	if (!result.ok) throw new Error(result.message);
	return result.data;
}

async function editor() {
	render(LocationEditor, { locationId: 'fra', onclose: vi.fn() });
	await screen.findByRole('heading', { name: 'Frankfurt' });
	return userEvent.setup();
}

async function confirmEntryDelete() {
	const dialog = await screen.findByRole('dialog', { name: 'Delete this entry?' });
	await userEvent.setup().click(within(dialog).getByRole('button', { name: 'Delete' }));
}

describe('admin location CRUD', () => {
	beforeEach(() => {
		fixture = { locations: new Map([['fra', location()]]), nextId: 1 };
		if (!HTMLDialogElement.prototype.showModal) {
			HTMLDialogElement.prototype.showModal = function () {
				this.open = true;
			};
			HTMLDialogElement.prototype.close = function () {
				this.open = false;
				this.dispatchEvent(new Event('close'));
			};
		}
		vi.stubGlobal('fetch', vi.fn(fetchFixture));
	});

	afterEach(() => {
		cleanup();
		vi.unstubAllGlobals();
	});

	it('rejects an invalid location without a public partial write', async () => {
		fixture.locations.clear();
		const user = userEvent.setup();
		render(LocationsPage);
		await screen.findByText('No locations yet — add your first.');
		await user.click(screen.getAllByRole('button', { name: 'Add location' })[0]);
		await user.click(screen.getByRole('button', { name: 'Create' }));

		expect((await screen.findByRole('alert')).textContent).toContain('Display name is required.');
		expect(await publicLocations()).toEqual([]);
	});

	it('rejects a partial location update without changing public data', async () => {
		const before = await publicLocations();
		const result = await fetch('/api/admin/locations/fra', {
			method: 'PUT',
			headers: { 'content-type': 'application/json' },
			body: JSON.stringify({ name: 'Patch' })
		});

		expect(result.status).toBe(422);
		expect(await result.json()).toMatchObject({ error: 'invalid_input' });
		expect(await publicLocations()).toEqual(before);
	});

	it('rejects a Test IP without a family before writing public data', async () => {
		const result = await fetch('/api/admin/locations/fra/test-ips', {
			method: 'POST',
			headers: { 'content-type': 'application/json' },
			body: JSON.stringify({ address: '203.0.113.9', label: 'edge' })
		});

		expect(result.status).toBe(422);
		expect(await result.json()).toMatchObject({ error: 'invalid_input' });
		expect((await publicLocations())[0].test_ips).toEqual([]);
	});

	it('adds, edits, and deletes Test IPs in public data', async () => {
		const user = await editor();
		await user.click(screen.getByRole('tab', { name: 'Test IPs' }));
		await user.click(screen.getByRole('button', { name: 'Add IP' }));
		await user.type(screen.getByLabelText('Address'), '203.0.113.9');
		await user.type(screen.getByLabelText('Label (optional)'), 'edge');
		await user.click(screen.getByRole('button', { name: 'Save' }));

		await screen.findByText('203.0.113.9 · v4 · edge');
		expect((await publicLocations())[0].test_ips).toMatchObject([
			{ family: 'v4', address: '203.0.113.9', label: 'edge' }
		]);
		await user.click(screen.getByRole('button', { name: 'Edit Test IP addresses' }));
		await user.clear(screen.getByLabelText('Address'));
		await user.type(screen.getByLabelText('Address'), '198.51.100.7');
		await user.click(screen.getByRole('button', { name: 'Save' }));

		await screen.findByText('198.51.100.7 · v4 · edge');
		expect((await publicLocations())[0].test_ips).toMatchObject([
			{ family: 'v4', address: '198.51.100.7', label: 'edge' }
		]);
		await user.click(screen.getByRole('button', { name: 'Delete Test IP addresses' }));
		await confirmEntryDelete();
		await screen.findByText('Nothing here yet.');
		expect((await publicLocations())[0].test_ips).toEqual([]);
	});

	it('adds, edits, and deletes iperf endpoints in public data', async () => {
		const user = await editor();
		await user.click(screen.getByRole('tab', { name: 'iperf' }));
		await user.click(screen.getByRole('button', { name: 'Add endpoint' }));
		await user.type(screen.getByLabelText('Label'), 'Frankfurt 10G');
		await user.type(screen.getByLabelText('Host'), 'speed.example.test');
		await user.type(screen.getByLabelText('Port'), '5201');
		await user.type(screen.getByLabelText('Incoming command'), 'iperf3 -c speed.example.test');
		await user.type(screen.getByLabelText('Outgoing command'), 'iperf3 -c speed.example.test -R');
		await user.click(screen.getByRole('button', { name: 'Save' }));

		await screen.findByText('Frankfurt 10G · speed.example.test:5201');
		expect((await publicLocations())[0].iperf).toMatchObject([
			{
				label: 'Frankfurt 10G',
				host: 'speed.example.test',
				port: 5201,
				cmd_incoming: 'iperf3 -c speed.example.test',
				cmd_outgoing: 'iperf3 -c speed.example.test -R'
			}
		]);
		await user.click(screen.getByRole('button', { name: 'Edit iperf endpoints' }));
		await user.clear(screen.getByLabelText('Host'));
		await user.type(screen.getByLabelText('Host'), 'iperf.example.test');
		await user.click(screen.getByRole('button', { name: 'Save' }));

		await screen.findByText('Frankfurt 10G · iperf.example.test:5201');
		expect((await publicLocations())[0].iperf).toMatchObject([
			{
				label: 'Frankfurt 10G',
				host: 'iperf.example.test',
				port: 5201,
				cmd_incoming: 'iperf3 -c speed.example.test',
				cmd_outgoing: 'iperf3 -c speed.example.test -R'
			}
		]);
		await user.click(screen.getByRole('button', { name: 'Delete iperf endpoints' }));
		await confirmEntryDelete();
		await screen.findByText('Nothing here yet.');
		expect((await publicLocations())[0].iperf).toEqual([]);
	});

	it('adds, edits, and deletes downloadable files in public data', async () => {
		const user = await editor();
		await user.click(screen.getByRole('tab', { name: 'Files' }));
		await user.click(screen.getByRole('button', { name: 'Add file' }));
		await user.type(screen.getByLabelText('Label'), '1 GB');
		await user.type(screen.getByLabelText('Declared size'), '1 GB');
		await user.type(screen.getByLabelText('Source on node'), '/files/1g.bin');
		await user.click(screen.getByRole('button', { name: 'Save' }));

		await screen.findByText('1 GB · 1 GB');
		expect((await publicLocations())[0].files).toMatchObject([
			{ label: '1 GB', declared_size: '1 GB', source_ref: '/files/1g.bin' }
		]);
		await user.click(screen.getByRole('button', { name: 'Edit Downloadable test files' }));
		await user.clear(screen.getByLabelText('Declared size'));
		await user.type(screen.getByLabelText('Declared size'), '1024 MB');
		await user.click(screen.getByRole('button', { name: 'Save' }));

		await screen.findByText('1 GB · 1024 MB');
		expect((await publicLocations())[0].files).toMatchObject([
			{ label: '1 GB', declared_size: '1024 MB', source_ref: '/files/1g.bin' }
		]);
		await user.click(screen.getByRole('button', { name: 'Delete Downloadable test files' }));
		await confirmEntryDelete();
		await screen.findByText('Nothing here yet.');
		expect((await publicLocations())[0].files).toEqual([]);
	});

	it('removes a disabled offered method from the runnable public methods', async () => {
		const user = await editor();
		await user.click(screen.getByRole('tab', { name: 'Methods' }));
		await user.click(screen.getByLabelText('ping'));
		await user.click(screen.getByRole('button', { name: 'Save methods' }));

		await waitFor(async () => expect((await publicLocations())[0].offered_methods).toEqual(['mtr']));
		expect(runnableMethods((await publicLocations())[0])).toEqual([{ value: 'mtr', label: 'MTR' }]);
	});

	it('removes a confirmed location and its children from public data', async () => {
		const record = fixture.locations.get('fra')!;
		fixture.locations.set('offline', {
			...location(),
			id: 'offline',
			name: 'Offline sibling',
			kind: 'remote',
			status: 'offline'
		});
		record.test_ips.push({ id: 'ip-1', location_id: 'fra', address: '203.0.113.9', family: 'v4', label: null });
		record.iperf.push({
			id: 'iperf-1',
			location_id: 'fra',
			label: 'Frankfurt',
			host: 'iperf.example.test',
			port: 5201,
			cmd_incoming: 'iperf3 -c iperf.example.test',
			cmd_outgoing: 'iperf3 -c iperf.example.test -R'
		});
		record.files.push({
			id: 'file-1',
			location_id: 'fra',
			label: '1 GB',
			declared_size: '1 GB',
			source_ref: '/files/1g.bin'
		});
		const beforeDelete = await publicLocations();
		expect(beforeDelete.map((location) => location.id)).toEqual(['fra']);
		expect(beforeDelete).toMatchObject([
			{
				test_ips: [{ address: '203.0.113.9' }],
				iperf: [{ host: 'iperf.example.test' }],
				files: [{ source_ref: '/files/1g.bin' }]
			}
		]);
		const user = userEvent.setup();
		render(LocationsPage);
		await screen.findByText('Frankfurt');
		await user.click(screen.getByRole('button', { name: 'Delete Frankfurt' }));
		const dialog = await screen.findByRole('dialog', { name: 'Delete this location?' });
		await user.click(within(dialog).getByRole('button', { name: 'Delete location' }));

		await screen.findByText('Offline sibling');
		expect(await publicLocations()).toEqual([]);
	});
});
