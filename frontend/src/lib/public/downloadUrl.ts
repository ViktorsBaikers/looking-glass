import type { LocationDetail, TestFile } from '../admin/types.js';

/// Direct-from-node download URL for a location's test file (range-capable).
export function downloadUrl(location: LocationDetail, file: TestFile): string {
	if (location.kind === 'remote') {
		if (!location.data_plane_origin) return '#';
		const origin = safeOrigin(location.data_plane_origin);
		if (!origin) return '#';
		const parts = file.source_ref.split('/');
		if (
			parts.length === 0 ||
			parts.some((part) => part.length === 0 || part === '.' || part === '..')
		) {
			return '#';
		}
		const source = parts.map(encodeURIComponent).join('/');
		return `${origin}/files/${source}`;
	}
	return `/api/locations/${location.id}/files/${file.id}/download`;
}

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
