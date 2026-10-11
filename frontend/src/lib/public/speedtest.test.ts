import { cleanup, fireEvent, render, screen } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import SpeedtestBlock from './SpeedtestBlock.svelte';
import { barWidths, declaredSizeBytes, largestTestFile, mbps, runSpeedTest } from './speedtest.js';
import type { LocationDetail, TestFile } from '../admin/types.js';

function file(id: string, declared: string): TestFile {
	return { id, location_id: 'fra', label: id, declared_size: declared, source_ref: `${id}.bin` };
}

describe('declaredSizeBytes', () => {
	it('parses decimal units', () => {
		expect(declaredSizeBytes('100 MB')).toBe(100_000_000);
		expect(declaredSizeBytes('10 MB')).toBe(10_000_000);
		expect(declaredSizeBytes('1.5 GB')).toBe(1_500_000_000);
		expect(declaredSizeBytes('512 KB')).toBe(512_000);
	});

	it('parses binary units', () => {
		expect(declaredSizeBytes('1 GiB')).toBe(1024 ** 3);
		expect(declaredSizeBytes('2 MiB')).toBe(2 * 1024 ** 2);
	});

	it('returns 0 for unparseable declarations', () => {
		expect(declaredSizeBytes('')).toBe(0);
		expect(declaredSizeBytes('huge')).toBe(0);
		expect(declaredSizeBytes('10 parsecs')).toBe(0);
	});
});

describe('largestTestFile', () => {
	it('picks the largest declared size regardless of order', () => {
		const files = [file('small', '10 MB'), file('big', '100 MB'), file('mid', '50 MB')];
		expect(largestTestFile(files)?.id).toBe('big');
	});

	it('returns null when there are no files', () => {
		expect(largestTestFile([])).toBeNull();
	});
});

describe('mbps', () => {
	it('converts bytes over elapsed time to whole megabits per second', () => {
		expect(mbps(12_500_000, 1000)).toBe(100); // 100 Mbit in 1 s
		expect(mbps(1_250_000, 2000)).toBe(5); // 10 Mbit in 2 s
	});

	it('guards against zero or negative inputs', () => {
		expect(mbps(1000, 0)).toBe(0);
		expect(mbps(0, 1000)).toBe(0);
	});
});

describe('barWidths', () => {
	it('keeps each phase fill inside its own half of the track', () => {
		expect(barWidths(0)).toEqual({ download: 0, upload: 0 });
		// Halfway through the download phase: a quarter of the whole track.
		expect(barWidths(25)).toEqual({ download: 25, upload: 0 });
		// Download done: its half stays full while the upload half fills.
		expect(barWidths(50)).toEqual({ download: 50, upload: 0 });
		expect(barWidths(75)).toEqual({ download: 50, upload: 25 });
		expect(barWidths(100)).toEqual({ download: 50, upload: 50 });
	});

	it('clamps out-of-range progress', () => {
		expect(barWidths(-10)).toEqual({ download: 0, upload: 0 });
		expect(barWidths(150)).toEqual({ download: 50, upload: 50 });
	});
});

// ----- runSpeedTest orchestration (stubbed fetch/XHR; no real bytes) --------

const LOCAL = { id: 'fra', kind: 'local', files: [] } as unknown as LocationDetail;
const LOCAL_WITH_FILE = {
	id: 'fra',
	kind: 'local',
	files: [file('f1', '100 MB')]
} as unknown as LocationDetail;

function fetchStub(handler: (url: string, init: RequestInit, call: number) => Promise<unknown>) {
	const mock = vi.fn(async (url: string, init: RequestInit) => handler(url, init, mock.mock.calls.length));
	vi.stubGlobal('fetch', mock);
	return mock;
}

/// Upload POSTs go through XMLHttpRequest (for send progress). Each fake
/// request reports its whole body as sent, then answers `statusFor(call)`, or
/// fails at the network level for 'network-error'.
function xhrStub(statusFor: (call: number) => number | 'network-error') {
	const sends = vi.fn();
	class FakeXhr {
		status = 0;
		upload: { onprogress: ((event: { loaded: number }) => void) | null } = { onprogress: null };
		onload: (() => void) | null = null;
		onerror: (() => void) | null = null;
		onabort: (() => void) | null = null;
		open() {}
		abort() {
			this.onabort?.();
		}
		send(body: ArrayBuffer) {
			sends(body);
			const call = sends.mock.calls.length;
			queueMicrotask(() => {
				this.upload.onprogress?.({ loaded: body.byteLength });
				const status = statusFor(call);
				if (status === 'network-error') return this.onerror?.();
				this.status = status;
				this.onload?.();
			});
		}
	}
	vi.stubGlobal('XMLHttpRequest', FakeXhr);
	return sends;
}

afterEach(() => {
	cleanup();
	vi.unstubAllGlobals();
	vi.restoreAllMocks();
});

describe('runSpeedTest', () => {
	it('fails instead of measuring a refused download (404 body is not throughput)', async () => {
		const mock = fetchStub(async () => ({ ok: false, status: 404, body: null }));
		const sends = xhrStub(() => 200);
		const result = await runSpeedTest(LOCAL_WITH_FILE, () => {}, 10_000);
		expect(result.failed).toBe(true);
		expect(result.downloadMbps).toBe(0);
		expect(result.uploadMbps).toBe(0);
		// The refused download must not be followed by an upload phase.
		expect(mock).toHaveBeenCalledTimes(1);
		expect(sends).not.toHaveBeenCalled();
	});

	// A download that fails at the network level (node unreachable, TLS or
	// CORS failure) measured nothing: a failure, not a finished "0 Mbps".
	it('reports a download lost at the network level as a failure', async () => {
		fetchStub(async () => {
			throw new TypeError('Failed to fetch');
		});
		const sends = xhrStub(() => 200);
		const result = await runSpeedTest(LOCAL_WITH_FILE, () => {}, 10_000);
		expect(result).toEqual({ downloadMbps: 0, uploadMbps: 0, failed: true, uploadFailed: false });
		expect(sends).not.toHaveBeenCalled();
	});

	it('caps the upload phase at four requests against the shared run limiter', async () => {
		const sends = xhrStub(() => 200);
		const result = await runSpeedTest(LOCAL, () => {}, 10_000);
		expect(sends).toHaveBeenCalledTimes(4);
		expect(result.failed).toBe(false);
		expect(result.uploadMbps).toBeGreaterThan(0);
	}, 30_000);

	it('ends the upload phase gracefully on a 429 with data measured so far', async () => {
		const sends = xhrStub((call) => (call === 1 ? 200 : 429));
		const result = await runSpeedTest(LOCAL, () => {}, 10_000);
		expect(sends).toHaveBeenCalledTimes(2);
		expect(result.failed).toBe(false);
		expect(result.uploadMbps).toBeGreaterThan(0);
	}, 30_000);

	// A refused first upload (429 from the shared run limiter) measured
	// nothing; it must surface as an upload failure, not a finished "0 Mbps".
	it('reports an upload refused before any chunk landed as a failure', async () => {
		const sends = xhrStub(() => 429);
		const result = await runSpeedTest(LOCAL, () => {}, 10_000);
		expect(sends).toHaveBeenCalledTimes(1);
		expect(result.failed).toBe(false);
		expect(result.uploadFailed).toBe(true);
		expect(result.uploadMbps).toBe(0);
	}, 30_000);

	// An upload that fails at the network level before any chunk
	// landed measured nothing either.
	it('reports an upload lost before any chunk landed as a failure', async () => {
		const sends = xhrStub(() => 'network-error');
		const result = await runSpeedTest(LOCAL, () => {}, 10_000);
		expect(sends).toHaveBeenCalledTimes(1);
		expect(result.failed).toBe(false);
		expect(result.uploadFailed).toBe(true);
		expect(result.uploadMbps).toBe(0);
	}, 30_000);

	it('keeps the rate measured so far when a later upload is lost', async () => {
		const sends = xhrStub((call) => (call === 1 ? 200 : 'network-error'));
		const result = await runSpeedTest(LOCAL, () => {}, 10_000);
		expect(sends).toHaveBeenCalledTimes(2);
		expect(result.uploadFailed).toBe(false);
		expect(result.uploadMbps).toBeGreaterThan(0);
	}, 30_000);

	// A remote source_ref that downloadUrl refuses ('#') must not be
	// fetched (it resolves to the SPA page) and must not report a speed.
	it.each(['files//100mb.bin', 'files/./100mb.bin', 'files/100mb.bin/'])(
		'fails without fetching when the remote file ref %s has no download URL',
		async (sourceRef) => {
			const mock = fetchStub(async () => ({
				ok: true,
				status: 200,
				body: { getReader: () => ({ read: async () => ({ done: true, value: undefined }) }) }
			}));
			const sends = xhrStub(() => 200);
			const remote = {
				id: 'vie',
				kind: 'remote',
				data_plane_origin: 'https://vie.example.test:9443',
				files: [{ ...file('f1', '100 MB'), source_ref: sourceRef }]
			} as unknown as LocationDetail;
			const result = await runSpeedTest(remote, () => {}, 10_000);
			expect(mock).not.toHaveBeenCalled();
			expect(sends).not.toHaveBeenCalled();
			expect(result).toEqual({ downloadMbps: 0, uploadMbps: 0, failed: true, uploadFailed: false });
		}
	);

	// F-204: an unusable largest file must not hide a usable smaller one.
	it('measures the largest file that has a download URL', async () => {
		const mock = fetchStub(async () => ({
			ok: true,
			status: 200,
			body: { getReader: () => ({ read: async () => ({ done: true, value: undefined }) }) }
		}));
		xhrStub(() => 429);
		const remote = {
			id: 'vie',
			kind: 'remote',
			data_plane_origin: 'https://vie.example.test:9443',
			files: [
				{ ...file('bad', '100 MB'), source_ref: 'files/100mb.bin/' },
				{ ...file('tiny', '1 MB'), source_ref: 'files/1mb.bin' },
				{ ...file('ok', '10 MB'), source_ref: 'files/10mb.bin' }
			]
		} as unknown as LocationDetail;
		const result = await runSpeedTest(remote, () => {}, 10_000);
		expect(mock.mock.calls.map(([url]) => url)).toEqual(['https://vie.example.test:9443/files/files/10mb.bin']);
		expect(result.failed).toBe(false);
	}, 30_000);

	// F-205: a node that sends no byte before the deadline measured nothing:
	// a failure, not a finished "0 Mbps", and no upload phase follows.
	it.each(['the response never starts', 'the body sends no byte'])(
		'reports a download with no byte before the deadline as a failure (%s)',
		async (stall) => {
			const stalled = (signal: AbortSignal) => {
				const { promise, reject } = Promise.withResolvers<never>();
				signal.addEventListener('abort', () => reject(new DOMException('aborted', 'AbortError')));
				return promise;
			};
			fetchStub(async (_url, init) => {
				const signal = init.signal as AbortSignal;
				if (stall === 'the response never starts') return stalled(signal);
				return { ok: true, status: 206, body: { getReader: () => ({ read: () => stalled(signal) }) } };
			});
			const sends = xhrStub(() => 200);
			const result = await runSpeedTest(LOCAL_WITH_FILE, () => {}, 50);
			expect(result).toEqual({ downloadMbps: 0, uploadMbps: 0, failed: true, uploadFailed: false });
			expect(sends).not.toHaveBeenCalled();
		}
	);

	it('stops at the deadline and skips upload when the caller aborts', async () => {
		const controller = new AbortController();
		const sends = xhrStub(() => 200);
		const mock = fetchStub((_url, init) => {
			const { promise, reject } = Promise.withResolvers<unknown>();
			(init.signal as AbortSignal).addEventListener('abort', () => reject(new Error('aborted')));
			// Deterministic stand-in for a slow node: cancel while in flight.
			controller.abort();
			return promise;
		});
		const result = await runSpeedTest(LOCAL_WITH_FILE, () => {}, 10_000, controller.signal);
		// Only the in-flight download fetch happened; no upload POST followed.
		expect(mock).toHaveBeenCalledTimes(1);
		expect(sends).not.toHaveBeenCalled();
		expect(result.failed).toBe(false);
	});

	// The download rate comes from the bytes read, and the phase
	// deadline ends a body that stalls, keeping the rate measured so far.
	it('measures the bytes read and keeps that rate when the deadline cuts a stalled body', async () => {
		let now = 0;
		vi.spyOn(performance, 'now').mockImplementation(() => (now += 100));
		fetchStub(async (_url, init) => {
			let reads = 0;
			const read = () => {
				if (++reads === 1) return Promise.resolve({ done: false, value: new Uint8Array(1_250_000) });
				// Stalls until the deadline aborts the request, as a real body read would.
				const { promise, reject } = Promise.withResolvers<never>();
				(init.signal as AbortSignal).addEventListener('abort', () => reject(new Error('aborted')));
				return promise;
			};
			return { ok: true, status: 206, body: { getReader: () => ({ read }) } };
		});
		// Local upload is out of scope here: a refused sink ends that phase at once.
		xhrStub(() => 429);
		const result = await runSpeedTest(LOCAL_WITH_FILE, () => {}, 50);
		expect(result.failed).toBe(false);
		// 1.25 MB (10 Mbit) over the fake clock's 100 ms: 100 Mbps.
		expect(result.downloadMbps).toBe(100);
	}, 10_000);

	// The upload deadline aborts a POST still in
	// flight; the bytes it already sent are a partial rate, not a failure.
	it('keeps the partial rate when the deadline aborts the first upload mid-send', async () => {
		class StalledXhr {
			status = 0;
			upload: { onprogress: ((event: { loaded: number }) => void) | null } = { onprogress: null };
			onload: (() => void) | null = null;
			onerror: (() => void) | null = null;
			onabort: (() => void) | null = null;
			open() {}
			abort() {
				this.onabort?.();
			}
			send(body: ArrayBuffer) {
				queueMicrotask(() => this.upload.onprogress?.({ loaded: body.byteLength / 2 }));
			}
		}
		vi.stubGlobal('XMLHttpRequest', StalledXhr);
		const result = await runSpeedTest(LOCAL, () => {}, 200);
		expect(result.uploadFailed).toBe(false);
		expect(result.uploadMbps).toBeGreaterThan(0);
	}, 10_000);
});

// ----- SpeedtestBlock ---------------------------------------------------------

function block(over: Partial<LocationDetail>): LocationDetail {
	return { id: 'fra', kind: 'local', iperf: [], files: [file('f1', '100 MB')], ...over } as unknown as LocationDetail;
}

describe('SpeedtestBlock', () => {
	// A refused or lost upload shows an error, not a finished "0 Mbps"; the
	// wording must not blame the server for a network failure.
	it.each([429, 'network-error'] as const)(
		'shows an alert and no upload rate when the upload fails (%s)',
		async (outcome) => {
			fetchStub(async () => ({
				ok: true,
				status: 200,
				body: { getReader: () => ({ read: async () => ({ done: true, value: undefined }) }) }
			}));
			xhrStub(() => outcome);
			render(SpeedtestBlock, { props: { location: block({}) } });
			await fireEvent.click(screen.getByRole('button', { name: 'Start speed test' }));
			const alert = await screen.findByRole('alert', {}, { timeout: 10_000 });
			expect(alert.textContent).toBe('Upload test failed — the upload could not be completed.');
			const results = screen.getByRole('group', { name: 'Speed test results' });
			expect(results.textContent?.replace(/\s+/g, ' ')).toMatch(/Upload — Mbps$/);
		},
		30_000
	);

	// A download lost at the network level shows the failure, not "Download 0 Mbps".
	it('shows a failure and no download rate when the download is lost', async () => {
		fetchStub(async () => {
			throw new TypeError('Failed to fetch');
		});
		xhrStub(() => 200);
		render(SpeedtestBlock, { props: { location: block({}) } });
		await fireEvent.click(screen.getByRole('button', { name: 'Start speed test' }));
		const alert = await screen.findByRole('alert', {}, { timeout: 10_000 });
		expect(alert.textContent).toBe('Speed test failed — the test file could not be downloaded.');
		const results = screen.getByRole('group', { name: 'Speed test results' });
		expect(results.textContent?.replace(/\s+/g, ' ')).toMatch(/Download — Mbps/);
	}, 30_000);

	// F-206: a second run starts from an empty, unsettled bar, not the
	// previous run's finished split at 100%.
	it('resets the settled progress bar when the test runs again', async () => {
		let calls = 0;
		fetchStub((_url, init) => {
			if (++calls === 1) {
				return Promise.resolve({
					ok: true,
					status: 200,
					body: { getReader: () => ({ read: async () => ({ done: true, value: undefined }) }) }
				});
			}
			// The second download has not started yet while the bar is checked.
			const { promise, reject } = Promise.withResolvers<never>();
			(init.signal as AbortSignal).addEventListener('abort', () => reject(new Error('aborted')));
			return promise;
		});
		xhrStub(() => 429);
		render(SpeedtestBlock, { props: { location: block({}) } });
		const bar = screen.getByRole('progressbar', { name: 'Speed test progress' });
		await fireEvent.click(screen.getByRole('button', { name: 'Start speed test' }));
		await vi.waitFor(() => expect(bar.getAttribute('aria-valuenow')).toBe('100'), { timeout: 10_000 });
		expect(bar.dataset.settled).toBe('true');

		await fireEvent.click(await screen.findByRole('button', { name: 'Start speed test' }));
		await screen.findByRole('button', { name: 'Testing…' });
		expect(bar.getAttribute('aria-valuenow')).toBe('0');
		expect(bar.dataset.settled).toBe('false');
	}, 30_000);

	// F-249: a second run clears the previous result; it must not show the old
	// Mbps beside "Testing…" while it waits for the first byte.
	it('clears the previous rates when the test runs again', async () => {
		let calls = 0;
		fetchStub((_url, init) => {
			if (++calls === 1) {
				let sent = false;
				return Promise.resolve({
					ok: true,
					status: 200,
					body: {
						getReader: () => ({
							read: async () => {
								if (sent) return { done: true, value: undefined };
								sent = true;
								await new Promise((resolve) => setTimeout(resolve, 5));
								return { done: false, value: new Uint8Array(10_000_000) };
							}
						})
					}
				});
			}
			// The second download is still waiting for its first byte.
			const { promise, reject } = Promise.withResolvers<never>();
			(init.signal as AbortSignal).addEventListener('abort', () => reject(new Error('aborted')));
			return promise;
		});
		xhrStub(() => 200);
		render(SpeedtestBlock, { props: { location: block({}) } });
		const results = screen.getByRole('group', { name: 'Speed test results' });
		const readouts = () => results.textContent?.replace(/\s+/g, ' ').trim();
		await fireEvent.click(screen.getByRole('button', { name: 'Start speed test' }));
		await vi.waitFor(() => expect(readouts()).toMatch(/^Download [1-9]\d* Mbps Upload [1-9]\d* Mbps$/), {
			timeout: 10_000
		});

		await fireEvent.click(await screen.findByRole('button', { name: 'Start speed test' }));
		await screen.findByRole('button', { name: 'Testing…' });
		expect(readouts()).toBe('Download 0 Mbps Upload 0 Mbps');
	}, 30_000);

	// A file ref downloadUrl refuses ('#') gets no link; '#' would
	// download this page's HTML under the file's name.
	it('renders no download link for a remote file ref without a URL', () => {
		render(SpeedtestBlock, {
			props: {
				location: block({
					id: 'vie',
					kind: 'remote',
					data_plane_origin: 'https://vie.example.test:9443',
					files: [
						{ ...file('bad', '100 MB'), label: 'Unusable file', source_ref: 'files/100mb.bin/' },
						{ ...file('ok', '10 MB'), label: 'Usable file', source_ref: 'files/10mb.bin' }
					]
				})
			}
		});
		const links = screen.getAllByRole('link');
		expect(links.map((link) => link.getAttribute('href'))).toEqual([
			'https://vie.example.test:9443/files/files/10mb.bin'
		]);
		expect(links[0].textContent).toContain('Usable file');
	});
});
