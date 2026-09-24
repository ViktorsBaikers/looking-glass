// Seam 3 (pure logic) for the admin Locations list: derived state, search and
// sort. The rendered page itself is covered by tests/locations.e2e.spec.ts.
import { describe, expect, it } from 'vitest';
import { filterLocations, locationState, sortLocations } from './locationList.js';
import type { Location } from './types.js';

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
