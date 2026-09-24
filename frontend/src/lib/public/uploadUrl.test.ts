import { describe, expect, it } from 'vitest';
import { speedtestUploadUrl } from './uploadUrl.js';
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
