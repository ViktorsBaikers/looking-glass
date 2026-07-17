// The run controller: owns one diagnostic run's EventSource lifecycle and the
// reactive console state the UI renders. Cancel and browser-close both simply
// close the stream — the node observes the disconnect and kills the process.

export type RunStatus = 'idle' | 'connecting' | 'streaming' | 'done' | 'error';

export type ConsoleLine = { kind: 'out' | 'error' | 'meta'; text: string };

type DonePayload = { status: string; success: boolean; elapsed_ms: number };

const FRIENDLY_FAILURE: Record<string, string> = {
	timeout: 'The run timed out.',
	truncated: 'The output was too large, so the run was stopped.',
	canceled: 'The run was canceled.',
	failed: 'The run did not complete.'
};

export class RunController {
	status = $state<RunStatus>('idle');
	lines = $state<ConsoleLine[]>([]);
	summary = $state('');
	errorText = $state('');

	#source: EventSource | null = null;
	#methodLabel = '';
	#target = '';

	get active(): boolean {
		return this.status === 'connecting' || this.status === 'streaming';
	}

	start(location: string, method: string, methodLabel: string, target: string): void {
		this.#close();
		this.lines = [];
		this.summary = '';
		this.errorText = '';
		this.status = 'connecting';
		this.#methodLabel = methodLabel;
		this.#target = target;

		const params = new URLSearchParams({ location, method, target });
		const source = new EventSource(`/api/run/stream?${params.toString()}`);
		this.#source = source;

		source.addEventListener('line', (event) => {
			this.status = 'streaming';
			this.lines.push({ kind: 'out', text: (event as MessageEvent<string>).data });
		});

		// A server-sent failure/refusal (named "run-error" to avoid colliding with
		// EventSource's native `error` event).
		source.addEventListener('run-error', (event) => {
			const message = (event as MessageEvent<string>).data;
			if (message) {
				this.errorText = message;
				this.lines.push({ kind: 'error', text: message });
			}
		});

		source.addEventListener('done', (event) => {
			this.#finish(this.#parseDone((event as MessageEvent<string>).data));
		});

		// Native transport error (connection lost, or a non-200 like the 403 a
		// cross-origin request gets). EventSource retries by default; closing here
		// stops the retry loop.
		source.onerror = () => {
			if (!this.active) return;
			this.#close();
			this.status = 'error';
			if (!this.errorText) {
				this.errorText = 'The connection to the node was lost.';
				this.lines.push({ kind: 'error', text: this.errorText });
			}
		};
	}

	cancel(): void {
		if (!this.#source) return;
		this.#close();
		this.status = 'idle';
		this.lines.push({ kind: 'meta', text: 'Run canceled.' });
	}

	#finish(payload: DonePayload | null): void {
		this.#close();
		if (payload?.status === 'completed' && payload.success) {
			this.status = 'done';
			const seconds = ((payload.elapsed_ms ?? 0) / 1000).toFixed(1);
			this.summary = `${this.#methodLabel} · ${this.#target} · ${seconds}s`;
			return;
		}
		this.status = 'error';
		if (!this.errorText) {
			this.errorText = FRIENDLY_FAILURE[payload?.status ?? 'failed'] ?? FRIENDLY_FAILURE.failed;
			this.lines.push({ kind: 'error', text: this.errorText });
		}
	}

	#parseDone(data: string): DonePayload | null {
		try {
			return JSON.parse(data) as DonePayload;
		} catch {
			return null;
		}
	}

	#close(): void {
		this.#source?.close();
		this.#source = null;
	}
}
