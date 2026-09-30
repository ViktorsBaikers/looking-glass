// The run controller: owns one diagnostic run's EventSource lifecycle and the
// reactive console state the UI renders. Cancel and browser-close both simply
// close the stream — the node observes the disconnect and kills the process.

export type RunStatus = 'idle' | 'connecting' | 'streaming' | 'done' | 'error' | 'canceled';

type ConsoleLine = { kind: 'out' | 'error' | 'meta'; text: string };

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
	errorText = $state('');
	/// '{Location} ~ {method}' title of the last started run (Live Console bar).
	runTitle = $state('');

	#source: EventSource | null = null;
	// Lines received since the last frame. Every update re-reconciles the whole
	// rendered list, so a burst (thousands of BGP lines) is appended once per
	// animation frame instead of once per line.
	#pending: ConsoleLine[] = [];
	#frame = 0;

	get active(): boolean {
		return this.status === 'connecting' || this.status === 'streaming';
	}

	start(locationId: string, locationName: string, method: string, target: string): void {
		this.#close();
		this.lines = [];
		this.errorText = '';
		this.status = 'connecting';
		this.runTitle = `${locationName} ~ ${method}`;

		const params = new URLSearchParams({ location: locationId, method, target });
		const source = new EventSource(`/api/run/stream?${params.toString()}`);
		this.#source = source;

		// Each line arrives as a JSON string, so a blank line still has a data field.
		source.addEventListener('line', (event) => {
			this.status = 'streaming';
			this.#pending.push({ kind: 'out', text: this.#decodeLine((event as MessageEvent<string>).data) });
			this.#frame ||= requestAnimationFrame(() => this.#flush());
		});

		// A server-sent failure/refusal (named "run-error" to avoid colliding with
		// EventSource's native `error` event).
		source.addEventListener('run-error', (event) => {
			const message = (event as MessageEvent<string>).data;
			if (message) {
				this.#flush();
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
		this.status = 'canceled';
		this.lines.push({ kind: 'meta', text: 'Run canceled.' });
	}

	#finish(payload: DonePayload | null): void {
		this.#close();
		// A tool that exits non-zero (ping with 100% loss) still ran to the end:
		// its output is the result. Central does not send the exit code.
		if (payload?.status === 'completed') {
			this.status = 'done';
			if (!payload.success) this.lines.push({ kind: 'meta', text: 'The command finished with a non-zero exit code.' });
			return;
		}
		this.status = 'error';
		if (!this.errorText) {
			this.errorText = FRIENDLY_FAILURE[payload?.status ?? 'failed'] ?? FRIENDLY_FAILURE.failed;
			this.lines.push({ kind: 'error', text: this.errorText });
		}
	}

	/// A central older than JSON-encoded lines sends raw text (a tab open across
	/// a rollback): show anything that is not a JSON string as it came.
	#decodeLine(data: string): string {
		try {
			const text: unknown = JSON.parse(data);
			if (typeof text === 'string') return text;
		} catch {
			// Raw text.
		}
		return data;
	}

	#parseDone(data: string): DonePayload | null {
		try {
			return JSON.parse(data) as DonePayload;
		} catch {
			return null;
		}
	}

	#flush(): void {
		cancelAnimationFrame(this.#frame);
		this.#frame = 0;
		if (this.#pending.length === 0) return;
		// Not push(...pending): a hidden tab's backlog can exceed the argument limit (F-156).
		for (const line of this.#pending) this.lines.push(line);
		this.#pending = [];
	}

	/// Closes the stream; lines still waiting for a frame land first, so any
	/// status line pushed next stays in order.
	#close(): void {
		this.#flush();
		this.#source?.close();
		this.#source = null;
	}
}
