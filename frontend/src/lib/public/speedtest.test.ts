import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import SpeedtestBlock from './SpeedtestBlock.svelte';
import type { LocationDetail } from '../admin/types.js';

const location: LocationDetail = {
	id: 'fra',
	name: 'Frankfurt',
	geo_label: 'Frankfurt, DE',
	map_query: null,
	facility: null,
	facility_url: null,
	kind: 'local',
	data_plane_origin: null,
	offered_methods: ['ping'],
	status: 'online',
	created_at: 0,
	test_ips: [],
	iperf: [
		{
			id: 'iperf-1',
			location_id: 'fra',
			label: 'Primary',
			host: 'fra.example.test',
			port: 5201,
			cmd_incoming: 'iperf3 -c fra.example.test -p 5201',
			cmd_outgoing: 'iperf3 -c fra.example.test -p 5201 -R'
		}
	],
	files: [
		{
			id: 'file-1',
			location_id: 'fra',
			label: '100 MB',
			declared_size: '100 MB',
			source_ref: '100mb.bin'
		}
	]
};

describe('public speedtest information', () => {
	let writeText: ReturnType<typeof vi.fn>;

	beforeEach(() => {
		writeText = vi.fn().mockResolvedValue(undefined);
		Object.defineProperty(navigator, 'clipboard', {
			configurable: true,
			value: { writeText }
		});
		vi.stubGlobal('fetch', vi.fn());
		vi.stubGlobal('EventSource', vi.fn());
	});

	afterEach(() => {
		cleanup();
		vi.unstubAllGlobals();
	});

	it('links to the real download route and keeps iperf commands display-only', async () => {
		render(SpeedtestBlock, { location });

		const download = screen.getByRole('link', { name: /100 MB/ });
		expect(download.getAttribute('href')).toBe('/api/locations/fra/files/file-1/download');
		expect(screen.getByText('iperf3 -c fra.example.test -p 5201')).toBeTruthy();
		expect(screen.getByText('iperf3 -c fra.example.test -p 5201 -R')).toBeTruthy();

		await fireEvent.click(screen.getByRole('button', { name: 'Copy Primary Incoming command' }));
		await waitFor(() =>
			expect(writeText).toHaveBeenCalledWith('iperf3 -c fra.example.test -p 5201')
		);
		expect(fetch).not.toHaveBeenCalled();
		expect(EventSource).not.toHaveBeenCalled();
	});
});
