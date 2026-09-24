// Pure list logic for the admin Locations page: the derived per-location state
// (Online / Offline / Not enrolled), the search filter (name, geo label, state,
// ASN) and the sort orders (name, status, last seen). Kept DOM-free so vitest
// can pin the behaviour without rendering.

import type { Location } from './types.js';

export type LocationState = 'online' | 'offline' | 'not_enrolled';

/** A remote that never heartbeated has no agent yet: "Not enrolled". */
export function locationState(location: Location): LocationState {
	if (location.kind === 'remote' && location.status === 'offline' && !location.last_seen) {
		return 'not_enrolled';
	}
	return location.status;
}

export const STATE_LABEL: Record<LocationState, string> = {
	online: 'Online',
	offline: 'Offline',
	not_enrolled: 'Not enrolled'
};

export type SortKey = 'name' | 'status' | 'recent';

const STATE_ORDER: Record<LocationState, number> = { online: 0, offline: 1, not_enrolled: 2 };
const byName = (a: Location, b: Location) => a.name.localeCompare(b.name);

/** Case-insensitive substring match over name, geo label, state label and ASN
 *  (both bare digits and the `AS{n}` form). An empty query keeps every row. */
export function filterLocations(locations: Location[], query: string): Location[] {
	const q = query.trim().toLowerCase();
	if (!q) return locations;
	return locations.filter((location) => {
		const state = STATE_LABEL[locationState(location)];
		const asn = location.asn == null ? '' : `${location.asn} as${location.asn}`;
		return `${location.name} ${location.geo_label} ${state} ${asn}`.toLowerCase().includes(q);
	});
}

/** Returns a new array; `recent` puts the freshest heartbeat first and
 *  never-seen locations last, with name as the tie-break everywhere. */
export function sortLocations(locations: Location[], key: SortKey): Location[] {
	const sorted = [...locations];
	if (key === 'name') sorted.sort(byName);
	else if (key === 'status') {
		sorted.sort(
			(a, b) => STATE_ORDER[locationState(a)] - STATE_ORDER[locationState(b)] || byName(a, b)
		);
	} else {
		sorted.sort((a, b) => (b.last_seen ?? -1) - (a.last_seen ?? -1) || byName(a, b));
	}
	return sorted;
}
