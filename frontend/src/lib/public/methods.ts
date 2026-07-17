// The methods a visitor can actually run at a location. A location's
// offered_methods maps to the labels below; BGP (bgp/bgp6) is runnable as of
// Slice 11 (it shells to the node's read-only routing-daemon CLI), so it is
// offered in the public selector alongside the diagnostics. A method with no
// label is filtered out, so the UI never presents one the run path would reject.

import type { LocationDetail } from '$lib/admin/types.js';

export interface MethodOption {
	value: string;
	label: string;
}

const RUNNABLE_METHOD_LABELS: Record<string, string> = {
	ping: 'Ping',
	ping6: 'Ping (IPv6)',
	mtr: 'MTR',
	mtr6: 'MTR (IPv6)',
	traceroute: 'Traceroute',
	traceroute6: 'Traceroute (IPv6)',
	bgp: 'BGP',
	bgp6: 'BGP (IPv6)'
};

const BGP_METHODS = new Set(['bgp', 'bgp6']);

export function runnableMethods(location: LocationDetail): MethodOption[] {
	return location.offered_methods
		.filter((method) => method in RUNNABLE_METHOD_LABELS)
		.map((method) => ({ value: method, label: RUNNABLE_METHOD_LABELS[method] }));
}

// BGP takes a route prefix (IP or CIDR), not a hostname, so the input hint
// switches to a prefix example when a BGP method is selected.
export function targetPlaceholder(method: string): string {
	return BGP_METHODS.has(method)
		? 'e.g. 8.8.8.0/24 or 2001:db8::/32'
		: 'e.g. 1.1.1.1 or example.com';
}
