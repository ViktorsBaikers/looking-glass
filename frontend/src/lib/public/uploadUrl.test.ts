import { describe, expect, it } from 'vitest';
import { speedtestUploadUrl } from './uploadUrl.js';
import { downloadUrl } from './downloadUrl.js';
import type { LocationDetail } from '../admin/types.js';

function location(kind: 'local' | 'remote', data_plane_origin: string | null): LocationDetail {
	return {
		id: 'loc-1',
		name: 'Node',
		geo_label: 'DE',
		map_query: null,
		facility: null,
		facility_url: null,
		kind,
		data_plane_origin,
		asn: null,
		offered_methods: ['ping'],
		status: 'online',
		created_at: 0,
		test_ips: [],
		iperf: [],
		files: []
	};
}

describe('speedtestUploadUrl', () => {
	it('keeps local uploads on central', () => {
		expect(speedtestUploadUrl(location('local', null))).toBe(
			'/api/locations/loc-1/speedtest/upload'
		);
	});

	it('points remote uploads at the agent data-plane origin', () => {
		expect(speedtestUploadUrl(location('remote', 'https://remote.example.test:9443/'))).toBe(
			'https://remote.example.test:9443/speedtest/upload'
		);
	});

	it('refuses a remote location without a data-plane origin', () => {
		expect(speedtestUploadUrl(location('remote', null))).toBeNull();
	});

	it.each([
		'javascript:alert(1)',
		'https://remote.example.test/path',
		'https://remote.example.test?x=1',
		'https://user@remote.example.test',
		'not a url'
	])('refuses stale or malicious remote data-plane origin %s', (origin) => {
		expect(speedtestUploadUrl(location('remote', origin))).toBeNull();
	});
});

// The agent serves its data plane over HTTPS only, so a plain http origin
// (stale data, or a downgrade) never becomes a speed-test or download URL.
describe('remote data-plane origins are https only', () => {
	it.each(['http://remote.example.test', 'http://remote.example.test:443', 'ws://remote.example.test'])(
		'refuses %s',
		(origin) => {
			const remote = location('remote', origin);
			expect(speedtestUploadUrl(remote)).toBeNull();
			const file = { id: 'f', location_id: 'loc-1', label: 'f', declared_size: '1 MB', source_ref: 'a.bin' };
			expect(downloadUrl(remote, file)).toBe('#');
		}
	);

	it('accepts an https origin', () => {
		expect(speedtestUploadUrl(location('remote', 'https://203.0.113.10'))).toBe(
			'https://203.0.113.10/speedtest/upload'
		);
	});
});
