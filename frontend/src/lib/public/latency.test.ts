import { afterEach, describe, expect, it, vi } from 'vitest';
import { measureLatency } from './latency.js';

afterEach(() => {
	vi.restoreAllMocks();
	vi.unstubAllGlobals();
});

// The "Your latency" readout is the fastest of the probe round trips.
describe('measureLatency', () => {
	it('keeps the fastest round trip, rounded', async () => {
		// start/end pairs: 30 ms, 10.4 ms, 25 ms, 12 ms.
		const clock = [0, 30, 100, 110.4, 200, 225, 300, 312];
		vi.spyOn(performance, 'now').mockImplementation(() => clock.shift() ?? 0);
		const fetchMock = vi.fn(async () => new Response('{}'));
		vi.stubGlobal('fetch', fetchMock);
		expect(await measureLatency()).toBe(10);
		expect(fetchMock).toHaveBeenCalledTimes(4);
		expect(fetchMock).toHaveBeenCalledWith('/api/visitor', { cache: 'no-store' });
	});

	it('keeps what it measured when a later probe fails, and is null when none landed', async () => {
		const clock = [0, 20, 100];
		vi.spyOn(performance, 'now').mockImplementation(() => clock.shift() ?? 0);
		const fetchMock = vi
			.fn()
			.mockResolvedValueOnce(new Response('{}'))
			.mockRejectedValue(new TypeError('offline'));
		vi.stubGlobal('fetch', fetchMock);
		expect(await measureLatency()).toBe(20);
		expect(await measureLatency()).toBeNull();
	});

	// The partial result is rounded like the full one.
	it('rounds what it measured when a later probe fails', async () => {
		const clock = [0, 20.37, 100];
		vi.spyOn(performance, 'now').mockImplementation(() => clock.shift() ?? 0);
		vi.stubGlobal(
			'fetch',
			vi.fn().mockResolvedValueOnce(new Response('{}')).mockRejectedValue(new TypeError('offline'))
		);
		expect(await measureLatency()).toBe(20);
	});
});

// F-319 (M15, M16): the readout rounds to the nearest ms, and a failed probe
// ends the measurement instead of skipping to the next one.
describe('measureLatency (F-319)', () => {
	it('rounds a fractional round trip up to the nearest millisecond', async () => {
		const clock = [0, 10.6, 100, 130, 200, 230, 300, 330];
		vi.spyOn(performance, 'now').mockImplementation(() => clock.shift() ?? 0);
		vi.stubGlobal('fetch', vi.fn(async () => new Response('{}')));
		expect(await measureLatency()).toBe(11);
	});

	it('stops probing at the first failed request', async () => {
		const clock = [0, 100, 105, 200, 205, 300, 305];
		vi.spyOn(performance, 'now').mockImplementation(() => clock.shift() ?? 0);
		const fetchMock = vi
			.fn()
			.mockRejectedValueOnce(new TypeError('offline'))
			.mockResolvedValue(new Response('{}'));
		vi.stubGlobal('fetch', fetchMock);
		expect(await measureLatency()).toBeNull();
		expect(fetchMock).toHaveBeenCalledTimes(1);
	});
});
