// Public read endpoints the looking-glass page consumes: the online-location
// catalogue (its network + speedtest data) and the visitor's detected IP. Both
// are unauthenticated and reuse the shared JSON helper.

import { getJson } from '$lib/api.js';
import type { LocationDetail } from '$lib/admin/types.js';
export { downloadUrl } from './downloadUrl.js';

export const fetchLocations = () => getJson<LocationDetail[]>('/api/locations');

export async function fetchVisitorIp(): Promise<string | null> {
	const result = await getJson<{ ip: string | null }>('/api/visitor');
	return result.ok ? result.data.ip : null;
}
