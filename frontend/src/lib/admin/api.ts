// Typed admin endpoint wrappers over the JSON helpers in $lib/api. Bodies match
// the *Input DTOs the server validates (crates/central/src/admin_api.rs).

import { writable } from 'svelte/store';
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

// Path parameters (often decoded route params) are encoded so a '/', '?' or
// '#' in one cannot steer the request to another endpoint.
const seg = encodeURIComponent;

export const listLocations = () => getJson<Location[]>('/api/admin/locations');
export const getLocation = (id: string) => getJson<LocationDetail>(`/api/admin/locations/${seg(id)}`);
export const createLocation = (body: LocationInput) =>
	postJsonReturning<Location>('/api/admin/locations', body);
export const updateLocation = (id: string, body: LocationInput) =>
	putJson<Location>(`/api/admin/locations/${seg(id)}`, body);
export const deleteLocation = (id: string) => del(`/api/admin/locations/${seg(id)}`);
/** Set the public tab order; `ids` must list every location exactly once (409 otherwise). */
export const reorderLocations = (ids: string[]) =>
	putJson<undefined>('/api/admin/locations/order', { ids });
export const revokeAgent = (id: string) =>
	postJsonReturning<Location>(`/api/admin/locations/${seg(id)}/agent/revoke`, {});

/** Mint a single-use enrollment token + install command for a remote location. */
export const createEnrollment = (locationId: string) =>
	postJsonReturning<EnrollmentTicket>(`/api/admin/locations/${seg(locationId)}/enroll`, {});

export const createTestIp = (locationId: string, body: TestIpInput) =>
	postJsonReturning<TestIp>(`/api/admin/locations/${seg(locationId)}/test-ips`, body);
export const updateTestIp = (id: string, body: TestIpInput) =>
	putJson<TestIp>(`/api/admin/test-ips/${seg(id)}`, body);
export const deleteTestIp = (id: string) => del(`/api/admin/test-ips/${seg(id)}`);

export const createIperf = (locationId: string, body: IperfInput) =>
	postJsonReturning<IperfEndpoint>(`/api/admin/locations/${seg(locationId)}/iperf`, body);
export const updateIperf = (id: string, body: IperfInput) =>
	putJson<IperfEndpoint>(`/api/admin/iperf/${seg(id)}`, body);
export const deleteIperf = (id: string) => del(`/api/admin/iperf/${seg(id)}`);

export const createTestFile = (locationId: string, body: TestFileInput) =>
	postJsonReturning<TestFile>(`/api/admin/locations/${seg(locationId)}/files`, body);
export const updateTestFile = (id: string, body: TestFileInput) =>
	putJson<TestFile>(`/api/admin/files/${seg(id)}`, body);
export const deleteTestFile = (id: string) => del(`/api/admin/files/${seg(id)}`);

export const getSettings = () => getJson<GlobalSettings>('/api/admin/settings');
/** The last saved settings, so the running shell (header, title, footer, theme) follows a save. */
export const savedSettings = writable<GlobalSettings | null>(null);
export const saveSettings = async (body: GlobalSettings) => {
	const result = await putJson<GlobalSettings>('/api/admin/settings', body);
	if (result.ok) savedSettings.set(result.data);
	return result;
};

// ----- Administrators (ADR-0001) ----------------------------------------------

/** The signed-in administrator (spec #1: `{id, username}`). */
export const getMe = () => getJson<Me>('/api/admin/me');

export const listAdministrators = () => getJson<Administrator[]>('/api/admin/administrators');
/** Create a pending peer; the activation URL in the result is shown once. */
export const createAdministrator = (username: string) =>
	postJsonReturning<ActivationLink>('/api/admin/administrators', { username });
/** Replace a pending peer's activation link; the previous one dies. */
export const regenerateActivation = (id: string) =>
	postJsonReturning<ActivationLink>(`/api/admin/administrators/${seg(id)}/activation`, {});
export const removeAdministrator = (id: string) => del(`/api/admin/administrators/${seg(id)}`);
/** Ends the caller's other sessions; the current one stays signed in. */
export const changePassword = (current_password: string, new_password: string) =>
	putJson<undefined>('/api/admin/me/password', { current_password, new_password });
/** Public activation page reads: whose link is this? 410 when invalid. */
export const getActivation = (token: string) =>
	getJson<{ username: string }>(`/api/activate/${seg(token)}`);
/** Public activation submit: sets the peer's password, single use. */
export const activate = (token: string, password: string) =>
	postJsonReturning<undefined>(`/api/activate/${seg(token)}`, { password });
