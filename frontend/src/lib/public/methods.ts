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

// A clearly non-public IPv4 literal (private/loopback/link-local ranges) is
// refused in the browser before the run starts; anything ambiguous is left to
// the server's SSRF validation. BGP takes route prefixes, so it is exempt.
export function targetPreflightError(method: string, target: string): string {
	if (BGP_METHODS.has(method)) return '';
	return isClearlyNonPublicIpv4(target.trim())
		? 'Enter a publicly routable IPv4 address or hostname.'
		: '';
}

function isClearlyNonPublicIpv4(value: string): boolean {
	const parts = value.split('.');
	if (parts.length !== 4 || parts.some((part) => !/^(0|[1-9]\d{0,2})$/.test(part))) return false;

	const octets = parts.map(Number);
	if (octets.some((octet) => octet > 255)) return false;

	const [first, second] = octets;
	return (
		first === 10 ||
		first === 127 ||
		(first === 169 && second === 254) ||
		(first === 172 && second >= 16 && second <= 31) ||
		(first === 192 && second === 168)
	);
}
