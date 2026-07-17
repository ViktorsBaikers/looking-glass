// Domain shapes mirroring the central admin API (crates/central/src/admin_api.rs
// + store.rs). Kept in one place so the editor, list, and settings form agree.

export type NodeKind = 'local' | 'remote';
export type LocationStatus = 'online' | 'offline';
export type Family = 'v4' | 'v6';
export type Theme = 'system' | 'light' | 'dark';

export const OFFERED_METHODS = [
	'ping',
	'ping6',
	'mtr',
	'mtr6',
	'traceroute',
	'traceroute6',
	'bgp',
	'bgp6'
] as const;
export type OfferedMethod = (typeof OFFERED_METHODS)[number];

export interface Location {
	id: string;
	name: string;
	geo_label: string;
	map_query: string | null;
	facility: string | null;
	facility_url: string | null;
	kind: NodeKind;
	data_plane_origin: string | null;
	offered_methods: OfferedMethod[];
	status: LocationStatus;
	created_at: number;
	/** Most recent agent heartbeat (unix seconds), for the admin last-seen column.
	 * Present for a remote node; null/absent for a local node (Slice 8b). */
	last_seen?: number | null;
}

export interface TestIp {
	id: string;
	location_id: string;
	family: Family;
	address: string;
	label: string | null;
}

export interface IperfEndpoint {
	id: string;
	location_id: string;
	label: string;
	host: string;
	port: number;
	cmd_incoming: string;
	cmd_outgoing: string;
}

export interface TestFile {
	id: string;
	location_id: string;
	label: string;
	declared_size: string;
	source_ref: string;
}

export interface LocationDetail extends Location {
	test_ips: TestIp[];
	iperf: IperfEndpoint[];
	files: TestFile[];
}

/** What the admin gets after minting an enrollment token for a remote location
 * (mirrors the EnrollmentTicket in crates/central/src/enroll.rs). The token is
 * shown once, embedded in `install_command`; `expires_at` is unix seconds. */
export interface EnrollmentTicket {
	install_command: string;
	token: string;
	fingerprint: string;
	expires_at: number;
}

export interface GlobalSettings {
	site_title: string;
	logo_url: string | null;
	default_theme: Theme;
	terms_url: string | null;
	custom_block: string | null;
	exec_max_concurrent: number;
	exec_timeout_secs: number;
	exec_max_output_kib: number;
	exec_rate_max: number;
	exec_rate_window_secs: number;
}
