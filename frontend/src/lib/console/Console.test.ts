import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { flushSync } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import Console from './Console.svelte';
import CopyButton from '$lib/components/ui/copy-button.svelte';
import { RunController } from './run.svelte.js';
import { toast } from '$lib/toast.svelte.js';

const toastError = vi.spyOn(toast, 'error');
const toastSuccess = vi.spyOn(toast, 'success');

function stubClipboard(clipboard: { writeText: (text: string) => Promise<void> } | undefined) {
	vi.stubGlobal('navigator', { ...navigator, clipboard });
}

function renderConsole(controller = new RunController(), method = 'ping') {
	return render(Console, { props: { controller, method, idleTitle: 'Idle', location: null } });
}

afterEach(() => {
	cleanup();
	vi.unstubAllGlobals();
	toastError.mockClear();
	toastSuccess.mockClear();
});

// A denied or missing clipboard must tell the visitor instead of
// failing silently with an unhandled rejection.
describe('Console copy output', () => {
	it('confirms a successful copy', async () => {
		const writeText = vi.fn(() => Promise.resolve());
		stubClipboard({ writeText });
		renderConsole();
		await fireEvent.click(screen.getByRole('button', { name: 'Copy output' }));
		await waitFor(() => expect(toastSuccess).toHaveBeenCalledWith('Output copied.'));
		expect(toastError).not.toHaveBeenCalled();
	});

	it('reports a denied clipboard without an unhandled rejection', async () => {
		stubClipboard({ writeText: () => Promise.reject(new DOMException('denied', 'NotAllowedError')) });
		const unhandled = vi.fn();
		process.on('unhandledRejection', unhandled);
		try {
			renderConsole();
			await fireEvent.click(screen.getByRole('button', { name: 'Copy output' }));
			await waitFor(() =>
				expect(toastError).toHaveBeenCalledWith("Couldn't copy the output. Select it and copy it manually.")
			);
			expect(toastSuccess).not.toHaveBeenCalled();
			expect(unhandled).not.toHaveBeenCalled();
		} finally {
			process.off('unhandledRejection', unhandled);
		}
	});

	it('reports a missing clipboard API (plain http)', async () => {
		stubClipboard(undefined);
		renderConsole();
		await fireEvent.click(screen.getByRole('button', { name: 'Copy output' }));
		await waitFor(() =>
			expect(toastError).toHaveBeenCalledWith("Couldn't copy the output. Select it and copy it manually.")
		);
	});
});

describe('CopyButton', () => {
	it('reports a denied clipboard and stays in the copy state', async () => {
		stubClipboard({ writeText: () => Promise.reject(new DOMException('denied', 'NotAllowedError')) });
		render(CopyButton, { props: { text: 'iperf3 -c 192.0.2.21', label: '192.0.2.21 standard command' } });
		await fireEvent.click(screen.getByRole('button', { name: 'Copy 192.0.2.21 standard command' }));
		await waitFor(() =>
			expect(toastError).toHaveBeenCalledWith("Couldn't copy. Select the text and copy it manually.")
		);
		expect(screen.getByRole('button', { name: 'Copy 192.0.2.21 standard command' })).toBeTruthy();
	});

	it('reports a missing clipboard API (plain http)', async () => {
		stubClipboard(undefined);
		render(CopyButton, { props: { text: 'iperf3 -c 192.0.2.21', label: '192.0.2.21 standard command' } });
		await fireEvent.click(screen.getByRole('button', { name: 'Copy 192.0.2.21 standard command' }));
		await waitFor(() =>
			expect(toastError).toHaveBeenCalledWith("Couldn't copy. Select the text and copy it manually.")
		);
	});
});

// Each render re-reconciles the whole log, so a burst must render once
// per frame, not once per streamed line.
describe('Console streaming render count', () => {
	it('renders a burst of lines in one update', async () => {
		class FakeEventSource {
			static last: FakeEventSource;
			onerror = null;
			#line: ((event: MessageEvent<string>) => void) | undefined;
			constructor() {
				FakeEventSource.last = this;
			}
			addEventListener(type: string, listener: (event: MessageEvent<string>) => void) {
				if (type === 'line') this.#line = listener;
			}
			line(text: string) {
				this.#line?.({ data: JSON.stringify(text) } as MessageEvent<string>);
			}
			close() {}
		}
		vi.stubGlobal('EventSource', FakeEventSource);
		const controller = new RunController();
		const view = renderConsole(controller, 'bgp');
		controller.start('vie', 'Vienna', 'bgp', '192.0.2.0/24');
		const log = view.container.querySelector('[role="log"]')!;
		const counts = new Set<number>();
		for (let i = 0; i < 200; i++) {
			FakeEventSource.last.line(`route ${i}`);
			flushSync(); // each SSE event is its own task
			counts.add(log.querySelectorAll('p').length);
		}
		await new Promise((resolve) => requestAnimationFrame(resolve));
		flushSync();
		counts.add(log.querySelectorAll('p').length);
		// No line renders while the burst arrives; the frame renders all 200.
		expect([...counts]).toEqual([0, 200]);
	});
});

// F-351: the log is split into output and other lines once, as lines arrive.
// A new run starts that split over, and each line is filed exactly once.
describe('Console line split', () => {
	const texts = (root: HTMLElement) =>
		[...root.querySelectorAll('[role="log"] p')].map((p) => p.textContent?.trim());

	it('shows only the new run after a restart', () => {
		const controller = new RunController();
		const view = renderConsole(controller, 'ping');
		controller.status = 'streaming';
		for (const text of ['first 1', 'first 2', 'first 3']) controller.lines.push({ kind: 'out', text });
		flushSync();
		expect(texts(view.container)).toEqual(['first 1', 'first 2', 'first 3']);
		// What RunController.start() does for the next run.
		controller.lines = [];
		controller.lines.push({ kind: 'out', text: 'second 1' });
		flushSync();
		expect(texts(view.container)).toEqual(['second 1']);
	});

	it('keeps traceroute hops in the table and the error line below it', () => {
		const controller = new RunController();
		const view = renderConsole(controller, 'traceroute');
		controller.status = 'streaming';
		controller.lines.push({ kind: 'out', text: 'traceroute to 1.1.1.1 (1.1.1.1), 30 hops max, 60 byte packets' });
		controller.lines.push({ kind: 'out', text: ' 1  192.0.2.1 (192.0.2.1)  1.234 ms  1.100 ms  1.050 ms' });
		controller.lines.push({ kind: 'out', text: ' 2  198.51.100.1 (198.51.100.1)  5.001 ms  5.100 ms  5.200 ms' });
		controller.lines.push({ kind: 'error', text: 'The run timed out.' });
		controller.status = 'error';
		flushSync();
		// No hop is repeated as a text line.
		expect(texts(view.container).filter((text) => /^\d+\s/.test(text ?? ''))).toEqual([]);
		const table = view.container.querySelector('[role="log"] table')!;
		expect(table).not.toBeNull();
		const error = [...view.container.querySelectorAll('[role="log"] p')].find((p) =>
			p.textContent?.includes('The run timed out.')
		)!;
		expect(table.compareDocumentPosition(error) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
	});
});

// F-338: a long run must cost about linearly in its output. Svelte's reactive
// reads and keyed {#each} lookups go through Map.get, so its call count tracks
// the rendering work; re-rendering the whole log every frame makes 4x the
// lines cost about 16x the work.
describe('Console rendering work', () => {
	function stream(lines: number) {
		const controller = new RunController();
		const view = renderConsole(controller, 'ping');
		controller.status = 'streaming';
		flushSync();
		const get = Map.prototype.get;
		let work = 0;
		Map.prototype.get = function (this: Map<unknown, unknown>, key: unknown) {
			work++;
			return get.call(this, key);
		};
		try {
			for (let i = 0; i < lines; i += 5) {
				for (let j = i; j < i + 5; j++) controller.lines.push({ kind: 'out', text: `reply ${j}` });
				flushSync(); // one frame per 5 lines
			}
		} finally {
			Map.prototype.get = get;
		}
		const rendered = [...view.container.querySelectorAll('[role="log"] p')].map((p) => p.textContent);
		cleanup();
		return { work, rendered };
	}

	// ~4 s on an idle machine; the default 5 s timeout flakes on a loaded runner (F-348).
	it('grows about linearly with the line count', { timeout: 60_000 }, () => {
		const small = stream(400);
		const large = stream(1600);
		expect(large.rendered).toEqual(Array.from({ length: 1600 }, (_, i) => `reply ${i}`));
		expect(large.work / small.work).toBeLessThan(6);
	});
});
