import { afterEach, describe, expect, it, vi } from 'vitest';
import { RunController } from './run.svelte.js';

// A controllable EventSource: tests emit the server's named events by hand.
class FakeEventSource {
	static last: FakeEventSource;
	readyState = 0;
	onerror: (() => void) | null = null;
	#listeners = new Map<string, ((event: MessageEvent<string>) => void)[]>();
	constructor(public url: string) {
		FakeEventSource.last = this;
	}
	addEventListener(type: string, listener: (event: MessageEvent<string>) => void) {
		this.#listeners.set(type, [...(this.#listeners.get(type) ?? []), listener]);
	}
	emit(type: string, data: string) {
		for (const listener of this.#listeners.get(type) ?? []) listener({ data } as MessageEvent<string>);
	}
	close() {
		this.readyState = 2;
	}
}

const nextFrame = () => new Promise((resolve) => requestAnimationFrame(resolve));
const done = (status: string, success: boolean) => JSON.stringify({ status, success, elapsed_ms: 1 });
const shown = (controller: RunController) => controller.lines.map((line) => `${line.kind}:${line.text}`);

function startRun(target = '192.0.2.0/24') {
	vi.stubGlobal('EventSource', FakeEventSource);
	const controller = new RunController();
	controller.start('fra', 'Frankfurt', 'bgp', target);
	return { controller, source: FakeEventSource.last };
}

afterEach(() => {
	vi.unstubAllGlobals();
});

// A burst of streamed lines (thousands for BGP) is appended once per
// animation frame, not once per line, and batching never reorders output
// against the status lines that end a run.
describe('RunController line batching', () => {
	it('appends a burst of lines once, on the next frame, in order', async () => {
		const { controller, source } = startRun();
		for (let i = 0; i < 500; i++) source.emit('line', JSON.stringify(`l${i}`));
		expect(controller.lines).toHaveLength(0);
		expect(controller.status).toBe('streaming');
		await nextFrame();
		expect(controller.lines.map((line) => line.text)).toEqual(Array.from({ length: 500 }, (_, i) => `l${i}`));
	});

	it('lands pending lines before the failure line when the run ends', () => {
		const { controller, source } = startRun();
		source.emit('line', JSON.stringify('a'));
		source.emit('line', JSON.stringify('b'));
		source.emit('done', done('timeout', false));
		expect(shown(controller)).toEqual(['out:a', 'out:b', 'error:The run timed out.']);
		expect(controller.status).toBe('error');
	});

	// Each failed done status gets its own message.
	it.each([
		['timeout', 'The run timed out.'],
		['truncated', 'The output was too large, so the run was stopped.'],
		['canceled', 'The run was canceled.'],
		['failed', 'The run did not complete.']
	])('ends a %s run with its own message', (status, message) => {
		const { controller, source } = startRun();
		source.emit('done', done(status, false));
		expect(shown(controller)).toEqual([`error:${message}`]);
		expect(controller.status).toBe('error');
		expect(source.readyState).toBe(2);
	});

	it('lands pending lines on a successful finish even if no frame ever runs', () => {
		// A hidden tab never fires animation frames.
		vi.stubGlobal('requestAnimationFrame', () => 1);
		vi.stubGlobal('cancelAnimationFrame', () => {});
		const { controller, source } = startRun();
		for (let i = 0; i < 100; i++) source.emit('line', JSON.stringify(`l${i}`));
		expect(controller.lines).toHaveLength(0);
		source.emit('done', done('completed', true));
		expect(controller.lines.map((line) => line.text)).toEqual(Array.from({ length: 100 }, (_, i) => `l${i}`));
		expect(controller.status).toBe('done');
	});

	// F-156: a spread push overflows the call stack past ~100k arguments, so
	// a long hidden-tab backlog used to throw from the done handler and leave
	// the run streaming with its EventSource open.
	it('lands a very large hidden-tab backlog and still finishes the run', () => {
		vi.stubGlobal('requestAnimationFrame', () => 1);
		vi.stubGlobal('cancelAnimationFrame', () => {});
		const { controller, source } = startRun();
		const line = JSON.stringify('x');
		for (let i = 0; i < 200_000; i++) source.emit('line', line);
		source.emit('done', done('completed', true));
		expect(controller.lines).toHaveLength(200_000);
		expect(controller.status).toBe('done');
		expect(source.readyState).toBe(2);
	});

	// F-239: a tool that exits non-zero (ping with 100% loss) still completed.
	it('shows a completed run with a non-zero exit as completed, with a neutral note', () => {
		const { controller, source } = startRun();
		source.emit('line', JSON.stringify('3 packets transmitted, 0 received, 100% packet loss'));
		source.emit('done', done('completed', false));
		expect(shown(controller)).toEqual([
			'out:3 packets transmitted, 0 received, 100% packet loss',
			'meta:The command finished with a non-zero exit code.'
		]);
		expect(controller.status).toBe('done');
		expect(controller.errorText).toBe('');
		expect(source.readyState).toBe(2);
	});

	it('keeps output before a server-sent run error', () => {
		const { controller, source } = startRun();
		source.emit('line', JSON.stringify('a'));
		source.emit('run-error', 'The node refused the run.');
		source.emit('done', done('failed', false));
		expect(shown(controller)).toEqual(['out:a', 'error:The node refused the run.']);
	});

	it('keeps output before a lost-connection error', () => {
		const { controller, source } = startRun();
		source.emit('line', JSON.stringify('a'));
		source.onerror?.();
		expect(shown(controller)).toEqual(['out:a', 'error:The connection to the node was lost.']);
		expect(controller.status).toBe('error');
		expect(source.readyState).toBe(2);
	});

	it('keeps output before the cancel notice', () => {
		const { controller, source } = startRun();
		source.emit('line', JSON.stringify('a'));
		controller.cancel();
		expect(shown(controller)).toEqual(['out:a', 'meta:Run canceled.']);
		expect(source.readyState).toBe(2);
	});

	// F-190: central sends each line as a JSON string, so a blank line still
	// carries a data field EventSource dispatches.
	it('decodes each line, blank ones included, in order', () => {
		const { controller, source } = startRun();
		for (const text of ['PING 8.8.8.8', '', '--- stats ---']) source.emit('line', JSON.stringify(text));
		source.emit('done', done('completed', true));
		expect(shown(controller)).toEqual(['out:PING 8.8.8.8', 'out:', 'out:--- stats ---']);
	});

	// F-209: a tab open across a rollback gets raw text lines from the older
	// central; they show as sent, next to JSON-encoded ones (blank included).
	it('falls back to the raw text of a line that is not a JSON string', () => {
		const { controller, source } = startRun();
		const errors: unknown[] = [];
		const sent = ['PING 1.1.1.1 (1.1.1.1) 56(84) bytes of data.', '64', JSON.stringify(''), JSON.stringify('--- stats ---')];
		for (const data of sent) {
			// A browser reports a throwing listener and drops that event.
			try {
				source.emit('line', data);
			} catch (error) {
				errors.push(error);
			}
		}
		source.emit('done', done('completed', true));
		expect(controller.lines.map((line) => line.text)).toEqual([
			'PING 1.1.1.1 (1.1.1.1) 56(84) bytes of data.',
			'64',
			'',
			'--- stats ---'
		]);
		expect(errors).toEqual([]);
	});

	it('never leaks pending lines of a replaced run into the next one', async () => {
		const { controller, source } = startRun('192.0.2.0/24');
		source.emit('line', JSON.stringify('old'));
		controller.start('fra', 'Frankfurt', 'bgp', '198.51.100.0/24');
		expect(source.readyState).toBe(2);
		FakeEventSource.last.emit('line', JSON.stringify('new'));
		await nextFrame();
		await nextFrame();
		expect(shown(controller)).toEqual(['out:new']);
	});
});
