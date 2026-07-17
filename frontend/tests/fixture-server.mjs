import { readFileSync } from 'node:fs';
import { createServer } from 'node:http';

const fixture = readFileSync(new URL('./fixture/index.html', import.meta.url));
const location = {
	id: 'fra',
	name: 'Frankfurt',
	geo_label: 'Frankfurt, DE',
	map_query: null,
	facility: null,
	facility_url: null,
	kind: 'local',
	data_plane_origin: null,
	offered_methods: ['ping'],
	status: 'online',
	created_at: 0,
	test_ips: [],
	iperf: [],
	files: []
};

function json(response, body, status = 200) {
	response.writeHead(status, { 'content-type': 'application/json' });
	response.end(JSON.stringify(body));
}

const freshInstalls = new Map();

createServer(async (request, response) => {
	const path = new URL(request.url, 'http://127.0.0.1').pathname;
	const fixtureId = request.headers['x-looking-glass-fixture'];
	const freshInstall =
		typeof fixtureId === 'string' && fixtureId.startsWith('fresh-install-')
			? freshInstalls.get(fixtureId) ?? { installed: false, admin: null }
			: null;
	if (path === '/api/locations') return json(response, [location]);
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
	if (path === '/api/setup/status') {
		return json(response, { installed: freshInstall?.installed ?? true });
	}
	if (request.method === 'POST' && path === '/api/setup') {
		if (!freshInstall || freshInstall.installed) {
			return json(response, { error: 'already_installed', message: 'Setup already completed.' }, 409);
		}
		const chunks = [];
		for await (const chunk of request) chunks.push(chunk);
		const body = JSON.parse(Buffer.concat(chunks).toString());
		if (body.setup_token !== 'fixture-setup-token') {
			return json(response, { error: 'invalid_setup_token', message: 'Invalid setup token.' }, 401);
		}
		freshInstalls.set(fixtureId, {
			installed: true,
			admin: { username: body.username, password: body.password }
		});
		response.writeHead(204).end();
		return;
	}
	if (request.method === 'POST' && path === '/api/auth/login') {
		const chunks = [];
		for await (const chunk of request) chunks.push(chunk);
		const body = JSON.parse(Buffer.concat(chunks).toString());
		if (
			freshInstall?.admin &&
			body.username === freshInstall.admin.username &&
			body.password === freshInstall.admin.password
		) {
			response.writeHead(204).end();
			return;
		}
		return json(response, { error: 'invalid_credentials', message: 'Invalid username or password.' }, 401);
	}
	if (path === '/api/run/stream') {
		response.writeHead(200, {
			'content-type': 'text/event-stream',
			'cache-control': 'no-cache',
			connection: 'keep-alive'
		});
		response.write('event: line\ndata: 64 bytes from 1.1.1.1: icmp_seq=1\n\n');
		setTimeout(() => {
			response.write('event: done\ndata: {"status":"completed","success":true,"elapsed_ms":100}\n\n');
			setTimeout(() => response.end(), 25);
		}, 200);
		return;
	}

	response.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
	response.end(fixture);
}).listen(4173, '127.0.0.1');
