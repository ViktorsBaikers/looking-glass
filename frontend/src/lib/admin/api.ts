// Typed admin endpoint wrappers over the JSON helpers in $lib/api. Bodies match
// the *Input DTOs the server validates (crates/central/src/admin_api.rs).

import { getJson, postJsonReturning, putJson, del } from '$lib/api.js';
import type {
	ActivationLink,
	Administrator,
	EnrollmentTicket,
	GlobalSettings,
	IperfEndpoint,
	Location,
	LocationDetail,
	Me,
	TestFile,
	TestIp
} from './types.js';

export type LocationInput = Pick<
	Location,
	| 'name'
	| 'geo_label'
	| 'map_query'
	| 'facility'
	| 'facility_url'
	| 'kind'
	| 'offered_methods'
	| 'data_plane_origin'
	| 'asn'
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

// ----- Administrators (ADR-0001) ----------------------------------------------

/** The signed-in administrator (spec #1: `{id, username}`). */
export const getMe = () => getJson<Me>('/api/admin/me');

export const listAdministrators = () => getJson<Administrator[]>('/api/admin/administrators');
/** Create a pending peer; the activation URL in the result is shown once. */
export const createAdministrator = (username: string) =>
	postJsonReturning<ActivationLink>('/api/admin/administrators', { username });
/** Replace a pending peer's activation link; the previous one dies. */
export const regenerateActivation = (id: string) =>
	postJsonReturning<ActivationLink>(`/api/admin/administrators/${id}/activation`, {});
export const removeAdministrator = (id: string) => del(`/api/admin/administrators/${id}`);
/** Ends the caller's other sessions; the current one stays signed in. */
export const changePassword = (current_password: string, new_password: string) =>
	putJson<undefined>('/api/admin/me/password', { current_password, new_password });
/** Public activation page reads: whose link is this? 410 when invalid. */
export const getActivation = (token: string) =>
	getJson<{ username: string }>(`/api/activate/${token}`);
/** Public activation submit: sets the peer's password, single use. */
export const activate = (token: string, password: string) =>
	postJsonReturning<undefined>(`/api/activate/${token}`, { password });
