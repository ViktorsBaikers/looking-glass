import { cleanup, render, screen } from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import Console from './Console.svelte';
import { RunController } from './run.svelte.js';

type Listener = (event: MessageEvent<string>) => void;

class EventSourceStub {
	static instance: EventSourceStub;

	readonly listeners = new Map<string, Listener[]>();
	onerror: (() => void) | null = null;

	constructor() {
		EventSourceStub.instance = this;
	}

	addEventListener(type: string, listener: Listener) {
		this.listeners.set(type, [...(this.listeners.get(type) ?? []), listener]);
	}

	close() {}

	emit(type: string, data: string) {
		for (const listener of this.listeners.get(type) ?? []) {
			listener(new MessageEvent(type, { data }));
		}
	}
}

function start(controller: RunController, method: string) {
	controller.start('fra', method, method === 'mtr' ? 'MTR' : 'Ping', '1.1.1.1');
	return EventSourceStub.instance;
}

describe('diagnostic console', () => {
	beforeEach(() => {
		vi.stubGlobal('EventSource', EventSourceStub);
	});

	afterEach(() => {
		cleanup();
		vi.unstubAllGlobals();
	});

	it('renders incremental stream lines', async () => {
		const controller = new RunController();
		render(Console, { controller, method: 'ping' });
		const source = start(controller, 'ping');
		source.emit('line', '64 bytes from 1.1.1.1');

		expect(await screen.findByText('64 bytes from 1.1.1.1')).toBeTruthy();
		expect(screen.getByText('Running')).toBeTruthy();
	});

	it('renders a parsed MTR table after a successful stream', async () => {
		const controller = new RunController();
		render(Console, { controller, method: 'mtr' });
		const source = start(controller, 'mtr');
		source.emit('line', '  1.|-- 1.1.1.1   0.0%   4   1.2   1.3   1.1   1.5   0.2');
		source.emit('done', JSON.stringify({ status: 'completed', success: true, elapsed_ms: 100 }));

		expect(await screen.findByRole('table')).toBeTruthy();
		expect(screen.getByRole('cell', { name: '1.1.1.1' })).toBeTruthy();
		expect(screen.getByText('Done')).toBeTruthy();
	});

	it('renders a clear stream error', async () => {
		const controller = new RunController();
		render(Console, { controller, method: 'ping' });
		const source = start(controller, 'ping');
		source.emit('run-error', 'The target was refused.');
		source.emit('done', JSON.stringify({ status: 'failed', success: false, elapsed_ms: 100 }));

		expect(await screen.findByText('The target was refused.')).toBeTruthy();
		expect(screen.getByText('Error')).toBeTruthy();
	});
});
