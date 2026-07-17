import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import PublicPage from '../../routes/+page.svelte';
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
	test_ips: [
		{ id: 'ip-1', location_id: 'fra', family: 'v4', address: '192.0.2.10', label: 'Probe' }
	],
	iperf: [],
	files: []
};

function response(body: unknown) {
	return new Response(JSON.stringify(body), {
		headers: { 'content-type': 'application/json' }
	});
}

describe('public network information', () => {
	let writeText: ReturnType<typeof vi.fn>;

	beforeEach(() => {
		writeText = vi.fn().mockResolvedValue(undefined);
		Object.defineProperty(navigator, 'clipboard', {
			configurable: true,
			value: { writeText }
		});
		vi.stubGlobal(
			'fetch',
			vi.fn((input: RequestInfo | URL) => {
				const path = new URL(input.toString(), 'http://looking-glass.test').pathname;
				if (path === '/api/locations') return Promise.resolve(response([location]));
				if (path === '/api/visitor') return Promise.resolve(response({ ip: '203.0.113.9' }));
				throw new Error(`unexpected request ${path}`);
			})
		);
	});

	afterEach(() => {
		cleanup();
		vi.unstubAllGlobals();
	});

	it('renders the visitor identity from /api/visitor and copies the exact test IP', async () => {
		render(PublicPage);

		expect(await screen.findByText('203.0.113.9')).toBeTruthy();
		expect(fetch).toHaveBeenCalledWith('/api/visitor', expect.anything());
		await fireEvent.click(screen.getByRole('button', { name: 'Copy test IP 192.0.2.10' }));

		await waitFor(() => expect(writeText).toHaveBeenCalledWith('192.0.2.10'));
	});
});
