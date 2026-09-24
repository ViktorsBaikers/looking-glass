import type { LocationDetail } from '../admin/types.js';

/// Browser speed-test upload sink URL (spec #1): a local node uploads to
/// central's own sink; a remote node uploads to its agent's data-plane, whose
/// CORS layer allows the cross-origin POST. `null` when a remote location has
/// no usable data-plane origin — the speed test then cannot measure upload.
export function speedtestUploadUrl(location: LocationDetail): string | null {
	if (location.kind === 'remote') {
		if (!location.data_plane_origin) return null;
		const origin = safeOrigin(location.data_plane_origin);
		return origin ? `${origin}/speedtest/upload` : null;
	}
	return `/api/locations/${location.id}/speedtest/upload`;
}

// ponytail: mirrors downloadUrl.ts's private safeOrigin (Foundation owns that
// file); fold into one shared helper when both live in the same hands.
function safeOrigin(value: string): string | null {
	try {
		const url = new URL(value);
		if (
			(url.protocol !== 'http:' && url.protocol !== 'https:') ||
			url.username !== '' ||
			url.password !== '' ||
			url.pathname !== '/' ||
			url.search !== '' ||
			url.hash !== ''
		) {
			return null;
		}
		return url.origin;
	} catch {
		return null;
	}
}
