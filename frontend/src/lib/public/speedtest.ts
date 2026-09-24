// Browser-measured Speed test (spec #1, ADR-0002: no Ookla). Download streams
// the largest Test file with a Range request for up to 10 s; upload POSTs
// random bytes to the sink for up to 10 s. The pure size/Mbps helpers are
// vitest-pinned in speedtest.test.ts; the orchestration is covered by e2e.

import type { LocationDetail, TestFile } from '../admin/types.js';
import { downloadUrl } from './downloadUrl.js';
import { speedtestUploadUrl } from './uploadUrl.js';

const SIZE_UNITS: Record<string, number> = {
	b: 1,
	kb: 1e3,
	mb: 1e6,
	gb: 1e9,
	kib: 1024,
	mib: 1024 ** 2,
	gib: 1024 ** 3
};

/// Declared sizes are operator free text ("100 MB"); parse to bytes, 0 when
/// unparseable, so "largest file" degrades to the first file instead of failing.
export function declaredSizeBytes(declared: string): number {
	const match = /^\s*([\d.]+)\s*([a-z]+)\s*$/i.exec(declared);
	if (!match) return 0;
	const unit = SIZE_UNITS[match[2].toLowerCase()];
	const value = Number(match[1]);
	if (!unit || !Number.isFinite(value)) return 0;
	return Math.round(value * unit);
}

export function largestTestFile(files: TestFile[]): TestFile | null {
	let best: TestFile | null = null;
	let bestBytes = -1;
	for (const file of files) {
		const bytes = declaredSizeBytes(file.declared_size);
		if (bytes > bestBytes) {
			bestBytes = bytes;
			best = file;
		}
	}
	return best;
}

/// Whole megabits per second for `bytes` transferred in `elapsedMs`.
export function mbps(bytes: number, elapsedMs: number): number {
	if (bytes <= 0 || elapsedMs <= 0) return 0;
	return Math.round((bytes * 8) / (elapsedMs / 1000) / 1e6);
}

/// Split progress-bar widths (percent of the whole track) for a live run:
/// `progress` sweeps 0–50 while downloading and 50–100 while uploading, and
/// each phase's fill stays inside its own half of the track.
export function barWidths(progress: number): { download: number; upload: number } {
	const clamped = Math.min(100, Math.max(0, progress));
	return { download: Math.min(clamped, 50), upload: Math.max(clamped - 50, 0) };
}

export interface SpeedSample {
	phase: 'download' | 'upload';
	downloadMbps: number;
	uploadMbps: number;
	/// Overall 0–100 across both phases (download is the first half).
	progress: number;
}

export interface SpeedResult {
	downloadMbps: number;
	uploadMbps: number;
	/// True when the test-file download was refused (a 404 or other
	/// non-200/206): no rate was measured and the UI shows a failure instead
	/// of bogus numbers.
	failed: boolean;
}

// Every upload POST counts against central's shared per-client run rate limiter
// (20 per 60 s by default), so the phase sends as few requests as it can:
// chunks just under the sinks' 25 MB per-request cap, at most four of them.
// ponytail: 4 × 24 MiB caps the measurable upload near ~80 Mbps over the 10 s
// phase and spends at most 4 of the 20-call run allowance; revisit only if the
// sink cap or the limiter budget grows.
const UPLOAD_CHUNK = 24 * 1024 * 1024;
const UPLOAD_MAX_REQUESTS = 4;

function uploadChunk(): ArrayBuffer {
	const buffer = new ArrayBuffer(UPLOAD_CHUNK);
	const chunk = new Uint8Array(buffer);
	// crypto.getRandomValues caps at 65536 bytes per call.
	for (let offset = 0; offset < chunk.length; offset += 65536) {
		crypto.getRandomValues(chunk.subarray(offset, offset + 65536));
	}
	return buffer;
}

/// POST a buffered body with byte-level send progress. `fetch` cannot report
/// upload progress for a buffered body, and streamed request bodies need
/// HTTP/2 in Chrome and are unsupported in Firefox. Resolves the HTTP status;
/// rejects on abort or network failure.
function postWithProgress(
	url: string,
	body: ArrayBuffer,
	signal: AbortSignal,
	onProgress: (sent: number) => void
): Promise<number> {
	const { promise, resolve, reject } = Promise.withResolvers<number>();
	if (signal.aborted) {
		reject(new Error('aborted'));
		return promise;
	}
	const xhr = new XMLHttpRequest();
	xhr.open('POST', url);
	xhr.upload.onprogress = (event) => onProgress(event.loaded);
	xhr.onload = () => resolve(xhr.status);
	xhr.onerror = xhr.onabort = () => reject(new Error('upload failed'));
	signal.addEventListener('abort', () => xhr.abort(), { once: true });
	xhr.send(body);
	return promise;
}

async function measureDownload(
	location: LocationDetail,
	durationMs: number,
	onSample: (sample: SpeedSample) => void,
	current: SpeedResult,
	signal?: AbortSignal
): Promise<number | null> {
	const file = largestTestFile(location.files);
	if (!file) return 0;

	const controller = new AbortController();
	const timer = setTimeout(() => controller.abort(), durationMs);
	const onAbort = () => controller.abort();
	signal?.addEventListener('abort', onAbort);
	const startedAt = performance.now();
	let bytes = 0;
	let value = 0;
	try {
		const response = await fetch(downloadUrl(location, file), {
			headers: { range: 'bytes=0-' },
			signal: controller.signal
		});
		// A missing test file answers 404 (and other refusals are possible);
		// counting an error body as throughput would report a rate that never
		// happened — refuse anything but a served (200) or ranged (206) body.
		if (response.status !== 200 && response.status !== 206) return null;
		const reader = response.body?.getReader();
		if (!reader) return 0;
		for (;;) {
			const { done, value: chunk } = await reader.read();
			if (done) break;
			bytes += chunk.length;
			value = mbps(bytes, performance.now() - startedAt);
			onSample({
				phase: 'download',
				downloadMbps: value,
				uploadMbps: current.uploadMbps,
				progress: Math.min(50, ((performance.now() - startedAt) / durationMs) * 50)
			});
		}
	} catch {
		// Aborted at the deadline (expected) or a network failure: keep whatever
		// throughput was measured so far.
	} finally {
		clearTimeout(timer);
		signal?.removeEventListener('abort', onAbort);
	}
	return value;
}

async function measureUpload(
	url: string,
	durationMs: number,
	onSample: (sample: SpeedSample) => void,
	current: SpeedResult,
	signal?: AbortSignal
): Promise<number> {
	const chunk = uploadChunk();
	// One deadline for the whole phase: it aborts whichever POST is in flight
	// when the time is up, instead of leaving a stalled sink pending forever.
	const controller = new AbortController();
	const timer = setTimeout(() => controller.abort(), durationMs);
	const onAbort = () => controller.abort();
	signal?.addEventListener('abort', onAbort);
	const startedAt = performance.now();
	let bytes = 0; // committed by completed requests
	let sent = 0; // sent so far by the in-flight request
	let aborted = false;
	const sample = () => {
		onSample({
			phase: 'upload',
			downloadMbps: current.downloadMbps,
			uploadMbps: mbps(bytes + sent, performance.now() - startedAt),
			progress: Math.min(100, 50 + ((performance.now() - startedAt) / durationMs) * 50)
		});
	};
	try {
		for (let request = 0; request < UPLOAD_MAX_REQUESTS; request++) {
			sent = 0;
			try {
				const status = await postWithProgress(url, chunk, controller.signal, (loaded) => {
					sent = loaded;
					sample();
				});
				// A 429 (the shared run limiter) or any other refusal ends the
				// phase gracefully, keeping what was measured so far.
				if (status < 200 || status >= 300) break;
			} catch {
				// Deadline or visitor cancellation mid-request; a plain network
				// failure did not actually ship the counted bytes.
				aborted = controller.signal.aborted;
				break;
			}
			bytes += sent;
			sample();
		}
	} finally {
		clearTimeout(timer);
		signal?.removeEventListener('abort', onAbort);
	}
	// Bytes pushed before a deadline abort still went over the wire.
	if (aborted) bytes += sent;
	return mbps(bytes, Math.max(performance.now() - startedAt, 1));
}

/// Run the full download-then-upload measurement. `onSample` receives live
/// Mbps readings for the progress UI. Upload is skipped (reported as 0) when
/// the location has no reachable sink URL or `signal` was aborted. Pass
/// `signal` to cancel the whole run (location switch, unmount); stale samples
/// stop as soon as it fires.
export async function runSpeedTest(
	location: LocationDetail,
	onSample: (sample: SpeedSample) => void,
	durationMs = 10_000,
	signal?: AbortSignal
): Promise<SpeedResult> {
	const result: SpeedResult = { downloadMbps: 0, uploadMbps: 0, failed: false };
	const download = await measureDownload(location, durationMs, onSample, result, signal);
	if (download === null) {
		result.failed = true;
		return result;
	}
	result.downloadMbps = download;
	const uploadUrl = speedtestUploadUrl(location);
	if (uploadUrl && !signal?.aborted) {
		result.uploadMbps = await measureUpload(uploadUrl, durationMs, onSample, result, signal);
	}
	if (!signal?.aborted) {
		onSample({
			phase: 'upload',
			downloadMbps: result.downloadMbps,
			uploadMbps: result.uploadMbps,
			progress: 100
		});
	}
	return result;
}
