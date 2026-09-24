// Hermetic mock of the central HTTP API for Playwright e2e (port 4173).
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
const APP_ORIGIN = 'http://127.0.0.1:4174';

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
		data_plane_origin: 'http://127.0.0.1:4173',
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

// ----- Per-fixture state ----------------------------------------------------------

const states = new Map();

function stateFor(fixtureId) {
	const key = typeof fixtureId === 'string' && fixtureId.length > 0 ? fixtureId : '(default)';
	let state = states.get(key);
	if (!state) {
		state = key.startsWith('fresh-install-')
			? { installed: false, administrators: [], authenticatedAs: null }
			: { installed: true, administrators: seededAdministrators(), authenticatedAs: null };
		states.set(key, state);
	}
	return state;
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

// ----- Server --------------------------------------------------------------------------

createServer(async (request, response) => {
	const path = new URL(request.url, 'http://127.0.0.1').pathname;
	const method = request.method;
	const state = stateFor(request.headers['x-looking-glass-fixture']);

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
		return json(response, locations.filter((location) => location.status === 'online'));
	}
	if (path === '/api/visitor') return json(response, { ip: '198.51.100.7' });
	if (path === '/api/public/settings') {
		return json(response, {
			site_title: 'Looking Glass',
			logo_url: null,
			default_theme: 'system',
			terms_url: null,
			custom_block: null
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

	// ----- admin: locations (read-only mirror of the seeded catalogue) -----
	if (path === '/api/admin/locations') {
		if (!signedIn()) return json(response, { error: 'unauthorized', message: 'Authentication required.' }, 401);
		return json(
			response,
			locations.map((location) => ({
				...location,
				last_seen: location.id === 'vie' ? now() : null
			}))
		);
	}
	const adminLocation = /^\/api\/admin\/locations\/([^/]+)$/.exec(path);
	if (method === 'GET' && adminLocation) {
		if (!signedIn()) return json(response, { error: 'unauthorized', message: 'Authentication required.' }, 401);
		const location = locations.find((candidate) => candidate.id === adminLocation[1]);
		if (!location) return json(response, { error: 'not_found', message: 'The requested item does not exist.' }, 404);
		return json(response, { ...location, last_seen: location.id === 'vie' ? now() : null });
	}

	// ----- run stream -----
	if (path === '/api/run/stream') {
		response.writeHead(200, {
			'content-type': 'text/event-stream',
			'cache-control': 'no-cache',
			connection: 'keep-alive'
		});
		setTimeout(() => {
			response.write('event: line\ndata: 64 bytes from 1.1.1.1: icmp_seq=1\n\n');
			setTimeout(() => {
				response.write('event: done\ndata: {"status":"completed","success":true,"elapsed_ms":100}\n\n');
				setTimeout(() => response.end(), 25);
			}, 750);
		}, 150);
		return;
	}

	// Unknown API paths answer as JSON; everything else is the SPA shell.
	if (path.startsWith('/api/')) {
		return json(response, { error: 'not_found', message: 'The requested item does not exist.' }, 404);
	}
	response.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
	response.end(fixture);
}).listen(4173, '127.0.0.1');

async function readJsonBytes(request) {
	const chunks = [];
	for await (const chunk of request) chunks.push(chunk);
	return Buffer.concat(chunks).length;
}
