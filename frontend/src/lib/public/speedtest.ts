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
}

// 2 MB per request — comfortably under the sink's 25 MB per-request cap.
const UPLOAD_CHUNK = 2 * 1024 * 1024;

function uploadChunk(): ArrayBuffer {
	const buffer = new ArrayBuffer(UPLOAD_CHUNK);
	const chunk = new Uint8Array(buffer);
	// crypto.getRandomValues caps at 65536 bytes per call.
	for (let offset = 0; offset < chunk.length; offset += 65536) {
		crypto.getRandomValues(chunk.subarray(offset, offset + 65536));
	}
	return buffer;
}

async function measureDownload(
	location: LocationDetail,
	durationMs: number,
	onSample: (sample: SpeedSample) => void,
	current: SpeedResult
): Promise<number> {
	const file = largestTestFile(location.files);
	if (!file) return 0;

	const controller = new AbortController();
	const timer = setTimeout(() => controller.abort(), durationMs);
	const startedAt = performance.now();
	let bytes = 0;
	let value = 0;
	try {
		const response = await fetch(downloadUrl(location, file), {
			headers: { range: 'bytes=0-' },
			signal: controller.signal
		});
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
	}
	return value;
}

async function measureUpload(
	url: string,
	durationMs: number,
	onSample: (sample: SpeedSample) => void,
	current: SpeedResult
): Promise<number> {
	const chunk = uploadChunk();
	const startedAt = performance.now();
	let bytes = 0;
	let value = 0;
	while (performance.now() - startedAt < durationMs) {
		try {
			const response = await fetch(url, { method: 'POST', body: chunk });
			if (!response.ok) break;
		} catch {
			break;
		}
		bytes += chunk.byteLength;
		value = mbps(bytes, performance.now() - startedAt);
		onSample({
			phase: 'upload',
			downloadMbps: current.downloadMbps,
			uploadMbps: value,
			progress: Math.min(
				100,
				50 + ((performance.now() - startedAt) / durationMs) * 50
			)
		});
	}
	return value;
}

/// Run the full download-then-upload measurement. `onSample` receives live
/// Mbps readings for the progress UI. Upload is skipped (reported as 0) when
/// the location has no reachable sink URL.
export async function runSpeedTest(
	location: LocationDetail,
	onSample: (sample: SpeedSample) => void,
	durationMs = 10_000
): Promise<SpeedResult> {
	const result: SpeedResult = { downloadMbps: 0, uploadMbps: 0 };
	result.downloadMbps = await measureDownload(location, durationMs, onSample, result);
	const uploadUrl = speedtestUploadUrl(location);
	if (uploadUrl) {
		result.uploadMbps = await measureUpload(uploadUrl, durationMs, onSample, result);
	}
	onSample({ phase: 'upload', ...result, progress: 100 });
	return result;
}
