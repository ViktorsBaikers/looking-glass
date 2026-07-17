import { describe, expect, it } from 'vitest';
import { downloadUrl } from './downloadUrl.js';
import type { LocationDetail, TestFile } from '../admin/types.js';

const file: TestFile = {
	id: 'file-1',
	location_id: 'loc-1',
	label: 'Probe',
	declared_size: '16 B',
	source_ref: 'downloads/probe.bin'
};

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
		offered_methods: ['ping'],
		status: 'online',
		created_at: 0,
		test_ips: [],
		iperf: [],
		files: [file]
	};
}

describe('downloadUrl', () => {
	it('keeps local downloads on central', () => {
		expect(downloadUrl(location('local', null), file)).toBe(
			'/api/locations/loc-1/files/file-1/download'
		);
	});

	it('points remote downloads at the agent data-plane origin', () => {
		expect(downloadUrl(location('remote', 'https://remote.example.test:9443/'), file)).toBe(
			'https://remote.example.test:9443/files/downloads/probe.bin'
		);
	});

	it.each([
		'javascript:alert(1)',
		'https://remote.example.test/path',
		'https://remote.example.test?x=1',
		'https://user@remote.example.test'
	])('refuses stale or malicious remote data-plane origin %s', (origin) => {
		expect(downloadUrl(location('remote', origin), file)).toBe('#');
	});

	it('refuses remote source refs with dot segments', () => {
		expect(
			downloadUrl(location('remote', 'https://remote.example.test:9443'), {
				...file,
				source_ref: 'downloads/../secret.bin'
			})
		).toBe('#');
	});

	it('refuses absolute-style remote source refs', () => {
		expect(
			downloadUrl(location('remote', 'https://remote.example.test:9443'), {
				...file,
				source_ref: '/probe.bin'
			})
		).toBe('#');
	});
});
