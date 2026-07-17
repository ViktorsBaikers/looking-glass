// Typed admin endpoint wrappers over the JSON helpers in $lib/api. Bodies match
// the *Input DTOs the server validates (crates/central/src/admin_api.rs).

import { getJson, postJsonReturning, putJson, del } from '$lib/api.js';
import type {
	EnrollmentTicket,
	GlobalSettings,
	IperfEndpoint,
	Location,
	LocationDetail,
	TestFile,
	TestIp
} from './types.js';

export type LocationInput = Pick<
	Location,
	'name' | 'geo_label' | 'map_query' | 'facility' | 'facility_url' | 'kind' | 'offered_methods'
	| 'data_plane_origin'
>;
export type TestIpInput = Pick<TestIp, 'family' | 'address' | 'label'>;
export type IperfInput = Pick<
	IperfEndpoint,
	'label' | 'host' | 'port' | 'cmd_incoming' | 'cmd_outgoing'
>;
export type TestFileInput = Pick<TestFile, 'label' | 'declared_size' | 'source_ref'>;

export const listLocations = () => getJson<Location[]>('/api/admin/locations');
export const getLocation = (id: string) => getJson<LocationDetail>(`/api/admin/locations/${id}`);
export const createLocation = (body: LocationInput) =>
	postJsonReturning<Location>('/api/admin/locations', body);
export const updateLocation = (id: string, body: LocationInput) =>
	putJson<Location>(`/api/admin/locations/${id}`, body);
export const deleteLocation = (id: string) => del(`/api/admin/locations/${id}`);
export const revokeAgent = (id: string) =>
	postJsonReturning<Location>(`/api/admin/locations/${id}/agent/revoke`, {});

/** Mint a single-use enrollment token + install command for a remote location. */
export const createEnrollment = (locationId: string) =>
	postJsonReturning<EnrollmentTicket>(`/api/admin/locations/${locationId}/enroll`, {});

export const createTestIp = (locationId: string, body: TestIpInput) =>
	postJsonReturning<TestIp>(`/api/admin/locations/${locationId}/test-ips`, body);
export const updateTestIp = (id: string, body: TestIpInput) =>
	putJson<TestIp>(`/api/admin/test-ips/${id}`, body);
export const deleteTestIp = (id: string) => del(`/api/admin/test-ips/${id}`);

export const createIperf = (locationId: string, body: IperfInput) =>
	postJsonReturning<IperfEndpoint>(`/api/admin/locations/${locationId}/iperf`, body);
export const updateIperf = (id: string, body: IperfInput) =>
	putJson<IperfEndpoint>(`/api/admin/iperf/${id}`, body);
export const deleteIperf = (id: string) => del(`/api/admin/iperf/${id}`);

export const createTestFile = (locationId: string, body: TestFileInput) =>
	postJsonReturning<TestFile>(`/api/admin/locations/${locationId}/files`, body);
export const updateTestFile = (id: string, body: TestFileInput) =>
	putJson<TestFile>(`/api/admin/files/${id}`, body);
export const deleteTestFile = (id: string) => del(`/api/admin/files/${id}`);

export const getSettings = () => getJson<GlobalSettings>('/api/admin/settings');
export const saveSettings = (body: GlobalSettings) =>
	putJson<GlobalSettings>('/api/admin/settings', body);
