import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import PublicPage from './+page.svelte';
import type { LocationDetail } from '$lib/admin/types.js';

const location: LocationDetail = {
	id: 'fra',
	name: 'Frankfurt',
	geo_label: 'Frankfurt, DE',
	map_query: null,
	facility: null,
	facility_url: null,
	kind: 'local',
	data_plane_origin: null,
	offered_methods: ['ping', 'mtr'],
	status: 'online',
	created_at: 0,
	test_ips: [],
	iperf: [],
	files: []
};

const vienna: LocationDetail = {
	...location,
	id: 'vie',
	name: 'Vienna',
	geo_label: 'Vienna, AT',
	offered_methods: ['traceroute']
};

const bgpLocation: LocationDetail = {
	...location,
	id: 'bgp',
	name: 'BGP',
	offered_methods: ['bgp']
};

const bgp6Location: LocationDetail = {
	...location,
	id: 'bgp6',
	name: 'BGP6',
	offered_methods: ['bgp6']
};

type Listener = (event: MessageEvent<string>) => void;

class EventSourceStub {
	static instances: EventSourceStub[] = [];

	readonly close = vi.fn();
	readonly listeners = new Map<string, Listener[]>();
	onerror: (() => void) | null = null;

	constructor(readonly url: string) {
		EventSourceStub.instances.push(this);
	}

	addEventListener(type: string, listener: Listener) {
		this.listeners.set(type, [...(this.listeners.get(type) ?? []), listener]);
	}

	emit(type: string, data: string) {
		for (const listener of this.listeners.get(type) ?? []) {
			listener(new MessageEvent(type, { data }));
		}
	}
}

let locations: LocationDetail[];

function response(body: unknown) {
	return new Response(JSON.stringify(body), {
		headers: { 'content-type': 'application/json' }
	});
}

describe('public diagnostic run', () => {
	beforeEach(() => {
		locations = [location];
		EventSourceStub.instances = [];
		vi.stubGlobal(
			'fetch',
			vi.fn((input: RequestInfo | URL) => {
				const path = new URL(input.toString(), 'http://looking-glass.test').pathname;
				if (path === '/api/locations') return Promise.resolve(response(locations));
				if (path === '/api/visitor') return Promise.resolve(response({ ip: '198.51.100.7' }));
				throw new Error(`unexpected request ${path}`);
			})
		);
		vi.stubGlobal('EventSource', EventSourceStub);
	});

	afterEach(() => {
		cleanup();
		vi.unstubAllGlobals();
	});

	it('lists only online locations and re-filters methods after a location change', async () => {
		locations = [location, vienna];
		render(PublicPage);
		const user = userEvent.setup();
		await screen.findByRole('option', { name: 'Frankfurt' });

		const locationSelect = screen.getByLabelText('Location') as HTMLSelectElement;
		const methodSelect = screen.getByLabelText('Method') as HTMLSelectElement;
		expect(Array.from(locationSelect.options, (option) => option.text)).toEqual(['Frankfurt', 'Vienna']);
		expect(Array.from(methodSelect.options, (option) => option.text)).toEqual(['Ping', 'MTR']);

		await user.selectOptions(locationSelect, 'vie');
		expect(Array.from(methodSelect.options, (option) => option.text)).toEqual(['Traceroute']);
	});

	it('refuses a non-public IPv4 literal through the form handler', async () => {
		render(PublicPage);
		const user = userEvent.setup();
		await screen.findByRole('option', { name: 'Frankfurt' });
		await user.type(screen.getByLabelText('Target'), '10.0.0.1');

		expect((screen.getByRole('button', { name: 'Run' }) as HTMLButtonElement).disabled).toBe(true);
		expect(screen.getByRole('alert').textContent).toBe(
			'Enter a publicly routable IPv4 address or hostname.'
		);
		await fireEvent.submit(screen.getByLabelText('Target').closest('form')!);
		expect(EventSourceStub.instances).toHaveLength(0);
	});

	it('lets BGP prefixes reach the server validator', async () => {
		locations = [bgpLocation];
		render(PublicPage);
		const user = userEvent.setup();
		await waitFor(() =>
			expect((screen.getByLabelText('Method') as HTMLSelectElement).value).toBe('bgp')
		);
		await user.type(screen.getByLabelText('Target'), '10.0.0.1');

		expect((screen.getByRole('button', { name: 'Run' }) as HTMLButtonElement).disabled).toBe(false);
		await user.click(screen.getByRole('button', { name: 'Run' }));
		expect(EventSourceStub.instances).toHaveLength(1);
		expect(EventSourceStub.instances[0].url).toContain('method=bgp');
	});

	it('lets BGP6 prefixes reach the server validator', async () => {
		locations = [bgp6Location];
		render(PublicPage);
		const user = userEvent.setup();
		await waitFor(() =>
			expect((screen.getByLabelText('Method') as HTMLSelectElement).value).toBe('bgp6')
		);
		await user.type(screen.getByLabelText('Target'), 'fc00::/7');

		expect((screen.getByRole('button', { name: 'Run' }) as HTMLButtonElement).disabled).toBe(false);
		expect(screen.queryByRole('alert')).toBeNull();
		await user.click(screen.getByRole('button', { name: 'Run' }));
		expect(EventSourceStub.instances).toHaveLength(1);
		const stream = new URL(EventSourceStub.instances[0].url, 'http://looking-glass.test');
		expect(stream.searchParams.get('method')).toBe('bgp6');
		expect(stream.searchParams.get('target')).toBe('fc00::/7');
	});

	it('starts a stream for a valid target', async () => {
		render(PublicPage);
		const user = userEvent.setup();
		await screen.findByRole('option', { name: 'Frankfurt' });
		await user.type(screen.getByLabelText('Target'), '1.1.1.1');
		await user.click(screen.getByRole('button', { name: 'Run' }));

		expect(EventSourceStub.instances).toHaveLength(1);
		expect(EventSourceStub.instances[0].url).toContain('target=1.1.1.1');
		expect(screen.getByRole('button', { name: 'Cancel' })).toBeTruthy();
	});

	it('cancels an active stream visibly', async () => {
		render(PublicPage);
		const user = userEvent.setup();
		await screen.findByRole('option', { name: 'Frankfurt' });
		await user.type(screen.getByLabelText('Target'), '1.1.1.1');
		await user.click(screen.getByRole('button', { name: 'Run' }));
		await user.click(screen.getByRole('button', { name: 'Cancel' }));

		expect(EventSourceStub.instances[0].close).toHaveBeenCalledOnce();
		expect(screen.getByRole('log').textContent).toContain('Run canceled.');
		expect((screen.getByRole('button', { name: 'Run' }) as HTMLButtonElement).disabled).toBe(false);
	});
});
