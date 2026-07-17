import { cleanup, render, screen, waitFor, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import LocationsPage from '../../routes/admin/+page.svelte';
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
		offered_methods: ['ping'],
		status: 'online',
		last_seen: Math.floor(Date.now() / 1000),
		created_at: 0,
		...overrides
	};
}

function response(body: unknown, status = 200) {
	return new Response(JSON.stringify(body), {
		status,
		headers: { 'content-type': 'application/json' }
	});
}

describe('admin agent status and revoke', () => {
	let locations: Location[];

	beforeEach(() => {
		locations = [
			location(),
			location({ id: 'vie', name: 'Vienna', status: 'offline', last_seen: 1_700_000_000 })
		];
		if (!HTMLDialogElement.prototype.showModal) {
			HTMLDialogElement.prototype.showModal = function () {
				this.open = true;
			};
			HTMLDialogElement.prototype.close = function () {
				this.open = false;
				this.dispatchEvent(new Event('close'));
			};
		}
		vi.stubGlobal(
			'fetch',
			vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
				const path = new URL(input.toString(), 'http://looking-glass.test').pathname;
				if ((init?.method ?? 'GET') === 'GET' && path === '/api/admin/locations') {
					return response(locations);
				}
				if (init?.method === 'POST' && path === '/api/admin/locations/fra/agent/revoke') {
					locations = locations.map((record) =>
						record.id === 'fra' ? { ...record, status: 'offline', last_seen: null } : record
					);
					return response(locations[0]);
				}
				throw new Error(`unexpected fetch ${init?.method ?? 'GET'} ${path}`);
			})
		);
	});

	afterEach(() => {
		cleanup();
		vi.unstubAllGlobals();
	});

	it('renders heartbeat-derived online and offline status dots and readouts', async () => {
		render(LocationsPage);

		const frankfurt = (await screen.findByText('Frankfurt')).closest('li')!;
		const vienna = screen.getByText('Vienna').closest('li')!;
		expect(within(frankfurt).getByText('Online')).toBeTruthy();
		expect(frankfurt.querySelector('.bg-status-online')).not.toBeNull();
		expect(within(vienna).getByText('Offline')).toBeTruthy();
		expect(vienna.querySelector('.bg-status-offline')).not.toBeNull();
	});

	it('moves a successfully revoked agent to offline and not enrolled after confirmation', async () => {
		const user = userEvent.setup();
		render(LocationsPage);
		const frankfurt = (await screen.findByText('Frankfurt')).closest('li')!;
		expect(within(frankfurt).getByText('Online')).toBeTruthy();

		await user.click(within(frankfurt).getByRole('button', { name: 'Revoke' }));
		const dialog = await screen.findByRole('dialog', { name: 'Revoke this agent?' });
		await user.click(within(dialog).getByRole('button', { name: 'Revoke agent' }));

		await waitFor(() => {
			const updated = screen.getByText('Frankfurt').closest('li')!;
			expect(within(updated).getByText('Not enrolled')).toBeTruthy();
			expect(updated.querySelector('.bg-status-offline')).not.toBeNull();
		});
		expect(fetch).toHaveBeenCalledWith('/api/admin/locations/fra/agent/revoke', expect.objectContaining({
			method: 'POST'
		}));
	});
});
