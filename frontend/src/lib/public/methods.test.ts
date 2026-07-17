import { describe, expect, it } from 'vitest';
import { runnableMethods, targetPlaceholder } from './methods.js';
import type { LocationDetail } from '../admin/types.js';

function location(offered: string[]): LocationDetail {
	return {
		id: 'loc-1',
		name: 'Node',
		geo_label: 'DE',
		map_query: null,
		facility: null,
		facility_url: null,
		kind: 'local',
		data_plane_origin: null,
		// Cast at the boundary: a real payload can carry a method the client has no
		// label for, which runnableMethods must filter out.
		offered_methods: offered as LocationDetail['offered_methods'],
		status: 'online',
		created_at: 0,
		test_ips: [],
		iperf: [],
		files: []
	};
}

describe('runnableMethods', () => {
	it('offers BGP alongside the diagnostics (Slice 11 — BGP is now runnable)', () => {
		const options = runnableMethods(location(['ping', 'bgp', 'bgp6']));
		expect(options).toEqual([
			{ value: 'ping', label: 'Ping' },
			{ value: 'bgp', label: 'BGP' },
			{ value: 'bgp6', label: 'BGP (IPv6)' }
		]);
	});

	it('drops a method with no known label so the UI never offers an unrunnable one', () => {
		const options = runnableMethods(location(['ping', 'iperf', 'telnet']));
		expect(options).toEqual([{ value: 'ping', label: 'Ping' }]);
	});
});

describe('targetPlaceholder', () => {
	it('hints a route prefix for BGP methods', () => {
		expect(targetPlaceholder('bgp')).toBe('e.g. 8.8.8.0/24 or 2001:db8::/32');
		expect(targetPlaceholder('bgp6')).toBe('e.g. 8.8.8.0/24 or 2001:db8::/32');
	});

	it('hints an IP or hostname for diagnostic methods', () => {
		expect(targetPlaceholder('ping')).toBe('e.g. 1.1.1.1 or example.com');
		expect(targetPlaceholder('traceroute')).toBe('e.g. 1.1.1.1 or example.com');
	});
});
