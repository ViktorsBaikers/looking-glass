import type { LocationDetail } from '../admin/types.js';
import { safeOrigin } from './downloadUrl.js';

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
