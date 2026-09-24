import { afterEach, describe, expect, it, vi } from 'vitest';
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
/// request reports its whole body as sent, then answers `statusFor(call)`.
function xhrStub(statusFor: (call: number) => number) {
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
				this.status = statusFor(call);
				this.onload?.();
			});
		}
	}
	vi.stubGlobal('XMLHttpRequest', FakeXhr);
	return sends;
}

afterEach(() => {
	vi.unstubAllGlobals();
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
});
