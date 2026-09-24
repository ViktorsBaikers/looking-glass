// Hermetic mock of the central HTTP API for Playwright e2e (port E2E_FIXTURE_PORT, default 4173).
// Mirrors the endpoints the SPA consumes, seeded with the design/stitch sample
// data. Mutable state is isolated per test through the `x-looking-glass-fixture`
// header: each distinct header value gets its own bucket; ids starting with
// `fresh-install-` begin uninstalled. Requests without the header share one
// default bucket (read-only specs like run.e2e).
import { randomUUID } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { createServer } from 'node:http';

const fixture = readFileSync(new URL('./fixture/index.html', import.meta.url));

const now = () => Math.floor(Date.now() / 1000);
const DAY = 24 * 3600;
const FIXTURE_PORT = Number(process.env.E2E_FIXTURE_PORT ?? 4173);
const APP_ORIGIN = `http://127.0.0.1:${process.env.E2E_APP_PORT ?? 4174}`;

// ----- Seeded catalogue (design/stitch sample data) ----------------------------
// Frankfurt is first: run.e2e expects the location select to default to 'fra'.

const ALL_METHODS = ['ping', 'ping6', 'mtr', 'mtr6', 'traceroute', 'traceroute6'];

function ips(loc, v4, v6) {
	return [
		{ id: `${loc}-ip4`, location_id: loc, family: 'v4', address: v4, label: 'primary' },
		{ id: `${loc}-ip6`, location_id: loc, family: 'v6', address: v6, label: 'primary' }
	];
}

function iperf(loc, host) {
	return [
		{
			id: `${loc}-iperf`,
			location_id: loc,
			label: 'Standard',
			host,
			port: 5201,
			cmd_incoming: `iperf3 -c ${host} -R`,
			cmd_outgoing: `iperf3 -c ${host}`
		}
	];
}

function files(loc, ref) {
	return [
		{
			id: `${loc}-file-100m`,
			location_id: loc,
			label: '100 MB test file',
			declared_size: '100 MB',
			source_ref: `${ref}/speedtest-100mb.bin`
		},
		{
			id: `${loc}-file-10m`,
			location_id: loc,
			label: '10 MB test file',
			declared_size: '10 MB',
			source_ref: `${ref}/speedtest-10mb.bin`
		}
	];
}

const locations = [
	{
		id: 'fra',
		name: 'Frankfurt',
		geo_label: 'Frankfurt, DE',
		map_query: 'Frankfurt',
		facility: 'Equinix FR2',
		facility_url:
			'https://www.equinix.com/data-centers/emea-colocation/germany-colocation/frankfurt-data-centers/fr2',
		kind: 'local',
		data_plane_origin: null,
		asn: 64501,
		offered_methods: ALL_METHODS,
		status: 'online',
		created_at: 0,
		test_ips: ips('fra', '192.0.2.21', '2001:db8::21'),
		iperf: iperf('fra', '192.0.2.21'),
		files: files('fra', 'frankfurt')
	},
	{
		// The design's Vienna: a remote node whose data-plane points back at this
		// fixture server, so cross-origin download/upload e2e exercises real fetches.
		id: 'vie',
		name: 'Vienna',
		geo_label: 'Vienna, AT',
		map_query: 'Vienna',
		facility: 'Interxion VIE1',
		facility_url: 'https://www.digitalrealty.com/data-centers/vienna/vie1',
		kind: 'remote',
		data_plane_origin: `http://127.0.0.1:${FIXTURE_PORT}`,
		asn: 64500,
		offered_methods: ['ping', 'ping6', 'mtr', 'traceroute', 'bgp'],
		status: 'online',
		created_at: 1,
		test_ips: [
			...ips('vie', '192.0.2.1', '2001:db8::1'),
			{ id: 'vie-ip4b', location_id: 'vie', family: 'v4', address: '192.0.2.2', label: 'secondary' },
			{ id: 'vie-ip6b', location_id: 'vie', family: 'v6', address: '2001:db8::2', label: 'secondary' }
		],
		iperf: iperf('vie', '192.0.2.1'),
		files: files('vie', 'vienna')
	},
	{
		id: 'lon',
		name: 'London',
		geo_label: 'London, UK',
		map_query: 'London',
		facility: 'Equinix LD8',
		facility_url: 'https://www.equinix.com/data-centers/emea-colocation/uk-colocation/london-data-centers/ld8',
		kind: 'local',
		data_plane_origin: null,
		asn: 64502,
		offered_methods: ['ping', 'mtr', 'traceroute'],
		status: 'online',
		created_at: 2,
		test_ips: ips('lon', '192.0.2.41', '2001:db8::41'),
		iperf: iperf('lon', '192.0.2.41'),
		files: files('lon', 'london')
	},
	{
		id: 'nyc',
		name: 'New York',
		geo_label: 'New York, US',
		map_query: 'New York',
		facility: 'Digital Realty JFK10',
		facility_url: 'https://www.digitalrealty.com/data-centers/northern-america/new-york-city/jfk10',
		kind: 'local',
		data_plane_origin: null,
		asn: 64503,
		offered_methods: ['ping', 'ping6', 'mtr', 'traceroute'],
		status: 'online',
		created_at: 3,
		test_ips: ips('nyc', '192.0.2.61', '2001:db8::61'),
		iperf: iperf('nyc', '192.0.2.61'),
		files: files('nyc', 'newyork')
	},
	{
		id: 'sin',
		name: 'Singapore',
		geo_label: 'Singapore, SG',
		map_query: 'Singapore',
		facility: 'Equinix SG1',
		facility_url: 'https://www.equinix.com/data-centers/asia-colocation/singapore-colocation/singapore-data-centers/sg1',
		kind: 'local',
		data_plane_origin: null,
		asn: 64504,
		offered_methods: ['ping', 'mtr', 'traceroute', 'bgp'],
		status: 'online',
		created_at: 4,
		test_ips: ips('sin', '192.0.2.81', '2001:db8::81'),
		iperf: iperf('sin', '192.0.2.81'),
		files: files('sin', 'singapore')
	},
	{
		// The design's admin-list card: a remote location awaiting enrollment.
		id: 'sfo',
		name: 'San Francisco Hub',
		geo_label: 'SF-01',
		map_query: 'San Francisco',
		facility: null,
		facility_url: null,
		kind: 'remote',
		data_plane_origin: null,
		asn: null,
		offered_methods: [],
		status: 'offline',
		created_at: 5,
		test_ips: [],
		iperf: [],
		files: []
	}
];

// ----- Seeded administrators ----------------------------------------------------
// brooke signs in with `fixture-password`. dana holds the valid activation token
// `fixture-activation-token`; erin's `fixture-expired-token` is expired.

const seededAdministrators = () => [
	{
		id: 'admin-brooke',
		username: 'brooke',
		status: 'active',
		created_at: Math.floor(Date.UTC(2026, 7, 5) / 1000),
		password: 'fixture-password',
		activation_token: null,
		activation_expires_at: null
	},
	{
		id: 'admin-dana',
		username: 'dana',
		status: 'pending',
		created_at: now() - 3600,
		password: null,
		activation_token: 'fixture-activation-token',
		activation_expires_at: now() + DAY
	},
	{
		id: 'admin-erin',
		username: 'erin',
		status: 'pending',
		created_at: now() - 2 * DAY,
		password: null,
		activation_token: 'fixture-expired-token',
		activation_expires_at: now() - DAY
	}
];

// Global settings (branding + execution limits), mutable per bucket.
const seededSettings = () => ({
	site_title: 'Looking Glass',
	logo_url: null,
	default_theme: 'system',
	terms_url: null,
	custom_block: null,
	exec_max_concurrent: 8,
	exec_timeout_secs: 30,
	exec_max_output_kib: 256,
	exec_rate_max: 20,
	exec_rate_window_secs: 60
});

// ----- Per-fixture state ----------------------------------------------------------

const states = new Map();

function stateFor(fixtureId) {
	const key = typeof fixtureId === 'string' && fixtureId.length > 0 ? fixtureId : '(default)';
	let state = states.get(key);
	if (!state) {
		state = key.startsWith('fresh-install-')
			? { installed: false, administrators: [], authenticatedAs: null, settings: seededSettings() }
			: {
					installed: true,
					administrators: seededAdministrators(),
					authenticatedAs: null,
					settings: seededSettings()
				};
		states.set(key, state);
	}
	return state;
}

// Per-state mutable clone of the seeded catalogue: admin location CRUD (list
// create/update/delete/revoke + sub-resources) mutates only the requesting
// test's bucket. Sub-resource arrays stay nested inside each location object.
function locationsFor(state) {
	if (!state.locations) {
		state.locations = structuredClone(locations);
		for (const location of state.locations) {
			location.last_seen = location.id === 'vie' ? now() : null;
		}
	}
	return state.locations;
}

// ----- Helpers ----------------------------------------------------------------------

function json(response, body, status = 200) {
	response.writeHead(status, { 'content-type': 'application/json' });
	response.end(JSON.stringify(body));
}

async function readJson(request) {
	const chunks = [];
	for await (const chunk of request) chunks.push(chunk);
	const raw = Buffer.concat(chunks).toString();
	return raw.length > 0 ? JSON.parse(raw) : {};
}

function adminPayload(admin) {
	return {
		id: admin.id,
		username: admin.username,
		status: admin.status,
		created_at: admin.created_at,
		activation_expires_at: admin.activation_expires_at
	};
}

function activationLink(admin) {
	return {
		administrator: adminPayload(admin),
		activation_url: `${APP_ORIGIN}/activate/${admin.activation_token}`,
		expires_at: admin.activation_expires_at
	};
}

function usernameAllowed(username) {
	return (
		typeof username === 'string' &&
		username.length >= 1 &&
		username.length <= 64 &&
		/^[A-Za-z0-9._-]+$/.test(username)
	);
}

/// Deterministic stand-in file content; served with Range support so the
/// speed-test download path (ranged fetch) exercises real 206 handling.
const FILE_BYTES = Buffer.alloc(65536);
for (let i = 0; i < FILE_BYTES.length; i++) FILE_BYTES[i] = (i * 7 + 13) % 251;

function serveBytes(request, response) {
	const range = request.headers.range;
	const match = range ? /bytes=(\d*)-(\d*)/.exec(range) : null;
	if (match) {
		const start = match[1] ? parseInt(match[1], 10) : 0;
		const end = match[2] ? Math.min(parseInt(match[2], 10), FILE_BYTES.length - 1) : FILE_BYTES.length - 1;
		if (start >= FILE_BYTES.length || start > end) {
			response.writeHead(416, { 'content-range': `bytes */${FILE_BYTES.length}` });
			return response.end();
		}
		response.writeHead(206, {
			'content-type': 'application/octet-stream',
			'content-range': `bytes ${start}-${end}/${FILE_BYTES.length}`,
			'content-length': end - start + 1,
			'accept-ranges': 'bytes'
		});
		return response.end(FILE_BYTES.subarray(start, end + 1));
	}
	response.writeHead(200, {
		'content-type': 'application/octet-stream',
		'content-length': FILE_BYTES.length,
		'accept-ranges': 'bytes'
	});
	response.end(FILE_BYTES);
}
// ----- Sub-resource CRUD specs (issue #7): test IPs, iperf endpoints, files -----
// Mirrors central's admin_api validation closely enough for e2e: required fields
// present and typed, else 422 invalid_input.
const SUB_RESOURCES = {
	'test-ips': {
		key: 'test_ips',
		prefix: 'ip',
		validate: (body) =>
			(body.family === 'v4' || body.family === 'v6') &&
			typeof body.address === 'string' &&
			body.address.length > 0,
		apply: (body) => ({
			family: body.family,
			address: body.address,
			label: typeof body.label === 'string' && body.label.length > 0 ? body.label : null
		})
	},
	iperf: {
		key: 'iperf',
		prefix: 'iperf',
		validate: (body) =>
			typeof body.label === 'string' &&
			body.label.length > 0 &&
			typeof body.host === 'string' &&
			body.host.length > 0 &&
			typeof body.port === 'number',
		apply: (body) => ({
			label: body.label,
			host: body.host,
			port: body.port,
			cmd_incoming: typeof body.cmd_incoming === 'string' ? body.cmd_incoming : '',
			cmd_outgoing: typeof body.cmd_outgoing === 'string' ? body.cmd_outgoing : ''
		})
	},
	files: {
		key: 'files',
		prefix: 'file',
		validate: (body) =>
			typeof body.label === 'string' &&
			body.label.length > 0 &&
			typeof body.declared_size === 'string' &&
			body.declared_size.length > 0 &&
			typeof body.source_ref === 'string' &&
			body.source_ref.length > 0,
		apply: (body) => ({
			label: body.label,
			declared_size: body.declared_size,
			source_ref: body.source_ref
		})
	}
};

/// Realistic per-method run output for the diagnostics e2e (the design's sample
/// ping run, a 3-hop mtr report, a 3-hop traceroute, a BGP table entry).
const RUN_OUTPUT = {
	ping: (target) => [
		`$ ping -c 4 ${target}`,
		`PING ${target} (1.1.1.1) 56(84) bytes of data.`,
		'64 bytes from 1.1.1.1: icmp_seq=1 ttl=56 time=12.3 ms',
		'64 bytes from 1.1.1.1: icmp_seq=2 ttl=56 time=12.1 ms',
		'64 bytes from 1.1.1.1: icmp_seq=3 ttl=55 time=11.9 ms',
		'64 bytes from 1.1.1.1: icmp_seq=4 ttl=56 time=12.0 ms',
		`--- ${target} ping statistics ---`,
		'4 packets transmitted, 4 received, 0% packet loss, time 3004ms',
		'rtt min/avg/max/mdev = 11.9/12.1/12.3/0.2 ms'
	],
	mtr: (target) => [
		`HOST: mtr --report ${target}    Loss%   Snt   Last   Avg  Best  Wrst StDev`,
		'  1.|-- 192.0.2.1             0.0%    10    1.1   1.2   1.0   1.5   0.2',
		'  2.|-- 198.51.100.1          0.0%    10    4.3   4.5   4.1   5.0   0.3',
		'  3.|-- 1.1.1.1               0.0%    10   12.0  12.2  11.8  12.9   0.4'
	],
	traceroute: (target) => [
		`traceroute to ${target} (1.1.1.1), 30 hops max, 60 byte packets`,
		' 1  192.0.2.1 (192.0.2.1)  1.045 ms  1.012 ms  0.998 ms',
		' 2  198.51.100.1 (198.51.100.1)  4.221 ms  4.190 ms  4.177 ms',
		' 3  1.1.1.1 (1.1.1.1)  12.043 ms  12.001 ms  11.987 ms'
	],
	bgp: (target) => [
		`BGP routing table entry for ${target}`,
		'Paths: (1 available, best #1)',
		'  64500 64511 13335',
		'    192.0.2.1 from 192.0.2.1 (192.0.2.1)',
		'      Origin IGP, localpref 100, valid, external, best'
	]
};

// ----- Server --------------------------------------------------------------------------

createServer(async (request, response) => {
	const path = new URL(request.url, 'http://127.0.0.1').pathname;
	const method = request.method;
	const state = stateFor(request.headers['x-looking-glass-fixture']);
	const locations = locationsFor(state);

	// Fail closed before setup, exactly like central's require_setup gate.
	if (
		!state.installed &&
		path.startsWith('/api/') &&
		path !== '/api/setup' &&
		path !== '/api/setup/status'
	) {
		return json(
			response,
			{ error: 'setup_required', message: 'First-run setup must be completed before this action.' },
			403
		);
	}

	const signedIn = () => state.administrators.find((admin) => admin.id === state.authenticatedAs);

	// ----- public catalogue -----
	if (path === '/api/locations') {
		const online = locations.filter((location) => location.status === 'online');
		// `no-files-` buckets blank the Test files so the disabled Speed test e2e
		// has seeded data to work against.
		const fixtureId = request.headers['x-looking-glass-fixture'];
		const noFiles = typeof fixtureId === 'string' && fixtureId.startsWith('no-files-');
		return json(response, noFiles ? online.map((location) => ({ ...location, files: [] })) : online);
	}
	if (path === '/api/visitor') return json(response, { ip: '198.51.100.7' });
	if (path === '/api/public/settings') {
		const settings = state.settings;
		return json(response, {
			site_title: settings.site_title,
			logo_url: settings.logo_url,
			default_theme: settings.default_theme,
			terms_url: settings.terms_url,
			custom_block: settings.custom_block
		});
	}

	// ----- file download (central sink + agent data-plane mirror) -----
	const centralDownload = /^\/api\/locations\/([^/]+)\/files\/([^/]+)\/download$/.exec(path);
	if (centralDownload) {
		const [, locationId, fileId] = centralDownload;
		const location = locations.find((candidate) => candidate.id === locationId);
		const known = location?.files.some((file) => file.id === fileId);
		if (!known) return json(response, { error: 'not_found', message: 'The requested item does not exist.' }, 404);
		return serveBytes(request, response);
	}
	if (path.startsWith('/files/')) return serveBytes(request, response);

	// ----- speed-test upload sinks (bytes discarded, count echoed) -----
	const centralUpload = /^\/api\/locations\/([^/]+)\/speedtest\/upload$/.exec(path);
	if (method === 'POST' && (centralUpload || path === '/speedtest/upload')) {
		const body = await readJsonBytes(request);
		if (centralUpload && !locations.some((candidate) => candidate.id === centralUpload[1])) {
			return json(response, { error: 'not_found', message: 'The requested item does not exist.' }, 404);
		}
		return json(response, { bytes: body });
	}

	// ----- setup + auth -----
	if (path === '/api/setup/status') return json(response, { installed: state.installed });
	if (method === 'POST' && path === '/api/setup') {
		const body = await readJson(request);
		if (state.installed) {
			return json(response, { error: 'already_installed', message: 'Setup already completed.' }, 409);
		}
		if (body.setup_token !== 'fixture-setup-token') {
			return json(response, { error: 'invalid_setup_token', message: 'Invalid setup token.' }, 401);
		}
		state.installed = true;
		state.administrators = [
			{
				id: `admin-${randomUUID()}`,
				username: body.username,
				status: 'active',
				created_at: now(),
				password: body.password,
				activation_token: null,
				activation_expires_at: null
			}
		];
		response.writeHead(204);
		return response.end();
	}
	if (method === 'POST' && path === '/api/auth/login') {
		const body = await readJson(request);
		const admin = state.administrators.find(
			(candidate) =>
				candidate.status === 'active' &&
				candidate.username === body.username &&
				candidate.password === body.password
		);
		if (!admin) {
			return json(response, { error: 'invalid_credentials', message: 'Invalid username or password.' }, 401);
		}
		state.authenticatedAs = admin.id;
		response.writeHead(204);
		return response.end();
	}
	if (method === 'POST' && path === '/api/auth/logout') {
		state.authenticatedAs = null;
		response.writeHead(204);
		return response.end();
	}

	// ----- activation (public, token-gated) -----
	const activation = /^\/api\/activate\/([^/]+)$/.exec(path);
	if (activation) {
		const pending = state.administrators.find(
			(candidate) =>
				candidate.status === 'pending' &&
				candidate.activation_token === decodeURIComponent(activation[1]) &&
				candidate.activation_expires_at >= now()
		);
		if (method === 'GET') {
			if (!pending) {
				return json(
					response,
					{
						error: 'activation_invalid',
						message: 'This activation link is no longer valid — ask a peer for a new one.'
					},
					410
				);
			}
			return json(response, { username: pending.username });
		}
		if (method === 'POST') {
			if (!pending) {
				return json(
					response,
					{
						error: 'activation_invalid',
						message: 'This activation link is no longer valid — ask a peer for a new one.'
					},
					410
				);
			}
			const body = await readJson(request);
			if (typeof body.password !== 'string' || body.password.length < 12 || body.password.length > 512) {
				return json(response, { error: 'invalid_input', message: 'password: invalid length: expected 12 <= length <= 512' }, 422);
			}
			pending.status = 'active';
			pending.password = body.password;
			pending.activation_token = null;
			pending.activation_expires_at = null;
			response.writeHead(204);
			return response.end();
		}
	}

	// ----- admin: me -----
	if (path === '/api/admin/me') {
		const me = signedIn();
		if (!me) return json(response, { error: 'unauthorized', message: 'Authentication required.' }, 401);
		return json(response, { id: me.id, username: me.username });
	}
	if (method === 'PUT' && path === '/api/admin/me/password') {
		const me = signedIn();
		if (!me) return json(response, { error: 'unauthorized', message: 'Authentication required.' }, 401);
		const body = await readJson(request);
		if (typeof body.new_password !== 'string' || body.new_password.length < 12 || body.new_password.length > 512) {
			return json(response, { error: 'invalid_input', message: 'new_password: invalid length: expected 12 <= length <= 512' }, 422);
		}
		if (body.current_password !== me.password) {
			return json(response, { error: 'invalid_credentials', message: 'The current password is incorrect.' }, 403);
		}
		me.password = body.new_password;
		response.writeHead(204);
		return response.end();
	}

	// ----- admin: global settings -----
	if (path === '/api/admin/settings') {
		const me = signedIn();
		if (!me) return json(response, { error: 'unauthorized', message: 'Authentication required.' }, 401);
		if (method === 'GET') return json(response, state.settings);
		if (method === 'PUT') {
			const next = parseSettings(await readJson(request));
			if (!next) {
				return json(response, { error: 'invalid_input', message: 'Settings payload is invalid.' }, 422);
			}
			state.settings = next;
			return json(response, next);
		}
	}

	// ----- admin: administrators -----
	if (path === '/api/admin/administrators') {
		const me = signedIn();
		if (!me) return json(response, { error: 'unauthorized', message: 'Authentication required.' }, 401);
		if (method === 'GET') {
			return json(response, state.administrators.map(adminPayload));
		}
		if (method === 'POST') {
			const body = await readJson(request);
			if (!usernameAllowed(body.username)) {
				return json(response, { error: 'invalid_input', message: 'Username may contain only letters, digits, and . _ -' }, 422);
			}
			if (state.administrators.some((candidate) => candidate.username.toLowerCase() === body.username.toLowerCase())) {
				return json(response, { error: 'username_taken', message: 'That username is already taken.' }, 409);
			}
			const admin = {
				id: `admin-${randomUUID()}`,
				username: body.username,
				status: 'pending',
				created_at: now(),
				password: null,
				activation_token: `act-${randomUUID()}`,
				activation_expires_at: now() + DAY
			};
			state.administrators.push(admin);
			return json(response, activationLink(admin), 201);
		}
	}
	const regenerate = /^\/api\/admin\/administrators\/([^/]+)\/activation$/.exec(path);
	if (method === 'POST' && regenerate) {
		const me = signedIn();
		if (!me) return json(response, { error: 'unauthorized', message: 'Authentication required.' }, 401);
		const admin = state.administrators.find((candidate) => candidate.id === regenerate[1]);
		if (!admin) return json(response, { error: 'not_found', message: 'The requested item does not exist.' }, 404);
		if (admin.status !== 'pending') {
			return json(response, { error: 'not_pending', message: 'Only a pending administrator has an activation link to regenerate.' }, 409);
		}
		admin.activation_token = `act-${randomUUID()}`;
		admin.activation_expires_at = now() + DAY;
		return json(response, activationLink(admin));
	}
	const removeAdmin = /^\/api\/admin\/administrators\/([^/]+)$/.exec(path);
	if (method === 'DELETE' && removeAdmin) {
		const me = signedIn();
		if (!me) return json(response, { error: 'unauthorized', message: 'Authentication required.' }, 401);
		const target = state.administrators.find((candidate) => candidate.id === removeAdmin[1]);
		if (!target) return json(response, { error: 'not_found', message: 'The requested item does not exist.' }, 404);
		if (target.id === me.id) {
			return json(response, { error: 'cannot_remove_self', message: 'You cannot remove your own account.' }, 409);
		}
		if (target.status === 'active' && state.administrators.filter((candidate) => candidate.status === 'active').length <= 1) {
			return json(response, { error: 'last_active_administrator', message: 'The last active administrator cannot be removed.' }, 409);
		}
		state.administrators = state.administrators.filter((candidate) => candidate.id !== target.id);
		// Removal ends the removed peer's sessions immediately.
		if (state.authenticatedAs === target.id) state.authenticatedAs = null;
		response.writeHead(204);
		return response.end();
	}

	// ----- admin: locations (per-state mutable mirror of the seeded catalogue) -----
	if (path === '/api/admin/locations') {
		if (!signedIn()) return json(response, { error: 'unauthorized', message: 'Authentication required.' }, 401);
		if (method === 'GET') return json(response, locations);
		if (method === 'POST') {
			const body = await readJson(request);
			if (typeof body.name !== 'string' || body.name.length === 0) {
				return json(response, { error: 'invalid_input', message: 'name: invalid length: expected 1 <= length' }, 422);
			}
			const location = {
				id: `loc-${randomUUID()}`,
				name: body.name,
				geo_label: typeof body.geo_label === 'string' ? body.geo_label : '',
				map_query: body.map_query ?? null,
				facility: body.facility ?? null,
				facility_url: body.facility_url ?? null,
				kind: body.kind === 'remote' ? 'remote' : 'local',
				data_plane_origin: body.data_plane_origin ?? null,
				asn: body.asn ?? null,
				offered_methods: Array.isArray(body.offered_methods) ? body.offered_methods : [],
				status: body.kind === 'remote' ? 'offline' : 'online',
				created_at: now(),
				last_seen: null,
				test_ips: [],
				iperf: [],
				files: []
			};
			locations.push(location);
			return json(response, location, 201);
		}
	}
	const adminLocation = /^\/api\/admin\/locations\/([^/]+)$/.exec(path);
	if (adminLocation) {
		if (!signedIn()) return json(response, { error: 'unauthorized', message: 'Authentication required.' }, 401);
		const location = locations.find((candidate) => candidate.id === adminLocation[1]);
		if (!location) return json(response, { error: 'not_found', message: 'The requested item does not exist.' }, 404);
		if (method === 'GET') return json(response, location);
		if (method === 'DELETE') {
			locations.splice(locations.indexOf(location), 1);
			response.writeHead(204);
			return response.end();
		}
	}
	const revokeAgent = /^\/api\/admin\/locations\/([^/]+)\/agent\/revoke$/.exec(path);
	if (method === 'POST' && revokeAgent) {
		if (!signedIn()) return json(response, { error: 'unauthorized', message: 'Authentication required.' }, 401);
		const location = locations.find((candidate) => candidate.id === revokeAgent[1]);
		if (!location) return json(response, { error: 'not_found', message: 'The requested item does not exist.' }, 404);
		location.status = 'offline';
		location.last_seen = null;
		return json(response, location);
	}
	// ----- admin: location update + enrollment (issue #7) -----
	if (method === 'PUT' && adminLocation) {
		const location = locations.find((candidate) => candidate.id === adminLocation[1]);
		if (!location) return json(response, { error: 'not_found', message: 'The requested item does not exist.' }, 404);
		const body = await readJson(request);
		if (typeof body.name !== 'string' || body.name.length === 0 || typeof body.geo_label !== 'string') {
			return json(response, { error: 'invalid_input', message: 'name: invalid length: expected 1 <= length' }, 422);
		}
		// Mirrors central's clean_asn: any non-whole or out-of-range value is the
		// coded 400 invalid_asn, judged here rather than by a body extractor.
		if (body.asn !== null && body.asn !== undefined) {
			if (!Number.isInteger(body.asn) || body.asn < 1 || body.asn > 4294967295) {
				return json(response, { error: 'invalid_asn', message: 'ASN must be a whole number between 1 and 4294967295.' }, 400);
			}
		}
		location.name = body.name;
		location.geo_label = body.geo_label;
		location.map_query = body.map_query ?? null;
		location.facility = body.facility ?? null;
		location.facility_url = body.facility_url ?? null;
		location.kind = body.kind === 'remote' ? 'remote' : 'local';
		location.data_plane_origin = body.data_plane_origin ?? null;
		location.asn = body.asn ?? null;
		if (Array.isArray(body.offered_methods)) location.offered_methods = body.offered_methods;
		if (location.kind === 'local') location.status = 'online';
		return json(response, location);
	}
	const enrollLocation = /^\/api\/admin\/locations\/([^/]+)\/enroll$/.exec(path);
	if (method === 'POST' && enrollLocation) {
		if (!signedIn()) return json(response, { error: 'unauthorized', message: 'Authentication required.' }, 401);
		const target = locations.find((candidate) => candidate.id === enrollLocation[1]);
		if (!target) return json(response, { error: 'not_found', message: 'The requested item does not exist.' }, 404);
		if (target.kind !== 'remote') {
			return json(response, { error: 'invalid_input', message: 'Enrollment applies only to remote locations.' }, 422);
		}
		const token = randomUUID();
		return json(response, {
			install_command: `curl -fsSL ${APP_ORIGIN}/install-agent.sh | sh -s -- --token ${token}`,
			token,
			fingerprint: 'sha256:fixture-central-fingerprint',
			expires_at: now() + 900
		}, 201);
	}

	// ----- admin: sub-resource CRUD (issue #7) -----
	for (const [slug, spec] of Object.entries(SUB_RESOURCES)) {
		const created = new RegExp(`^/api/admin/locations/([^/]+)/${slug}$`).exec(path);
		if (method === 'POST' && created) {
			if (!signedIn()) return json(response, { error: 'unauthorized', message: 'Authentication required.' }, 401);
			const owner = locations.find((candidate) => candidate.id === created[1]);
			if (!owner) return json(response, { error: 'not_found', message: 'The requested item does not exist.' }, 404);
			const body = await readJson(request);
			if (!spec.validate(body)) {
				return json(response, { error: 'invalid_input', message: 'A required field is missing or invalid.' }, 422);
			}
			const record = { id: `${spec.prefix}-${randomUUID()}`, location_id: owner.id, ...spec.apply(body) };
			owner[spec.key].push(record);
			return json(response, record, 201);
		}
		const addressed = new RegExp(`^/api/admin/${slug}/([^/]+)$`).exec(path);
		if (addressed) {
			if (!signedIn()) return json(response, { error: 'unauthorized', message: 'Authentication required.' }, 401);
			let owner = null;
			let record = null;
			for (const candidate of locations) {
				record = candidate[spec.key].find((entry) => entry.id === addressed[1]) ?? null;
				if (record) {
					owner = candidate;
					break;
				}
			}
			if (!record) return json(response, { error: 'not_found', message: 'The requested item does not exist.' }, 404);
			if (method === 'PUT') {
				const body = await readJson(request);
				if (!spec.validate(body)) {
					return json(response, { error: 'invalid_input', message: 'A required field is missing or invalid.' }, 422);
				}
				Object.assign(record, spec.apply(body));
				return json(response, record);
			}
			if (method === 'DELETE') {
				owner[spec.key].splice(owner[spec.key].indexOf(record), 1);
				response.writeHead(204);
				return response.end();
			}
		}
	}

	// ----- run stream -----
	if (path === '/api/run/stream') {
		const query = new URL(request.url, 'http://127.0.0.1').searchParams;
		const streamMethod = query.get('method') ?? 'ping';
		const streamTarget = query.get('target') ?? '1.1.1.1';
		response.writeHead(200, {
			'content-type': 'text/event-stream',
			'cache-control': 'no-cache',
			connection: 'keep-alive'
		});
		const send = (event, data) => response.write(`event: ${event}\ndata: ${data}\n\n`);

		// `slow.test` never finishes: keeps emitting until the client disconnects,
		// so the cancel e2e can stop a live run. `fail.test` refuses the run.
		if (streamTarget === 'slow.test') {
			let seq = 0;
			const timer = setInterval(
				() => send('line', `64 bytes from ${streamTarget}: icmp_seq=${++seq} ttl=56 time=12.0 ms`),
				300
			);
			response.on('close', () => clearInterval(timer));
			return;
		}
		if (streamTarget === 'fail.test') {
			setTimeout(() => {
				send('run-error', 'The node refused the run.');
				send('done', JSON.stringify({ status: 'failed', success: false, elapsed_ms: 10 }));
				setTimeout(() => response.end(), 25);
			}, 150);
			return;
		}

		const family = streamMethod.replace(/6$/, '');
		const lines = RUN_OUTPUT[family]
			? RUN_OUTPUT[family](streamTarget)
			: [`$ ${streamMethod} ${streamTarget}`];
		setTimeout(() => {
			for (const line of lines) send('line', line);
			send('done', JSON.stringify({ status: 'completed', success: true, elapsed_ms: 100 }));
			setTimeout(() => response.end(), 25);
		}, 150);
		return;
	}

	// Unknown API paths answer as JSON; everything else is the SPA shell.
	if (path.startsWith('/api/')) {
		return json(response, { error: 'not_found', message: 'The requested item does not exist.' }, 404);
	}
	response.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
	response.end(fixture);
}).listen(FIXTURE_PORT, '127.0.0.1');

async function readJsonBytes(request) {
	const chunks = [];
	for await (const chunk of request) chunks.push(chunk);
	return Buffer.concat(chunks).length;
}

/// Mirrors central's GlobalSettings validation closely enough for e2e: rejects
/// payloads the real server would 422.
function parseSettings(body) {
	if (!body || typeof body !== 'object' || Array.isArray(body)) return null;
	const positiveInt = (value) => Number.isInteger(value) && value >= 1;
	const optionalString = (value, max) =>
		value === null || (typeof value === 'string' && value.length <= max);
	if (typeof body.site_title !== 'string' || body.site_title.length < 1 || body.site_title.length > 100) return null;
	if (body.default_theme !== 'system' && body.default_theme !== 'light' && body.default_theme !== 'dark') return null;
	if (!optionalString(body.logo_url, 500) || !optionalString(body.terms_url, 300)) return null;
	if (!optionalString(body.custom_block, 5000)) return null;
	if (
		!positiveInt(body.exec_max_concurrent) ||
		!positiveInt(body.exec_timeout_secs) ||
		!positiveInt(body.exec_max_output_kib) ||
		!positiveInt(body.exec_rate_max) ||
		!positiveInt(body.exec_rate_window_secs)
	) {
		return null;
	}
	return {
		site_title: body.site_title,
		logo_url: body.logo_url,
		default_theme: body.default_theme,
		terms_url: body.terms_url,
		custom_block: body.custom_block,
		exec_max_concurrent: body.exec_max_concurrent,
		exec_timeout_secs: body.exec_timeout_secs,
		exec_max_output_kib: body.exec_max_output_kib,
		exec_rate_max: body.exec_rate_max,
		exec_rate_window_secs: body.exec_rate_window_secs
	};
}
