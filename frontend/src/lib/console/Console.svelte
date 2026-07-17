<script lang="ts">
	import type { RunController } from './run.svelte.js';
	import { parseMtr } from './mtr.js';
	import MtrTable from './MtrTable.svelte';

	let {
		controller,
		method,
		hint = 'Pick a location and method, enter a target, then Run.'
	}: { controller: RunController; method: string; hint?: string } = $props();

	const outputLines = $derived(controller.lines.filter((line) => line.kind === 'out'));
	const mtrRows = $derived(
		(method === 'mtr' || method === 'mtr6') && controller.status === 'done'
			? parseMtr(outputLines.map((line) => line.text))
			: null
	);

	const statusLabel = $derived(
		{
			idle: 'Ready',
			connecting: 'Connecting',
			streaming: 'Running',
			done: 'Done',
			error: 'Error'
		}[controller.status]
	);

	const dotClass = $derived(
		{
			idle: 'bg-zinc-500',
			connecting: 'bg-amber-400 animate-pulse',
			streaming: 'bg-emerald-400 animate-pulse',
			done: 'bg-emerald-400',
			error: 'bg-red-400'
		}[controller.status]
	);
</script>

<div class="overflow-hidden rounded-lg border border-zinc-800 bg-zinc-950 text-zinc-50 shadow-sm">
	<div
		class="flex items-center justify-between border-b border-zinc-800 px-3 py-2 text-xs text-zinc-400"
	>
		<span class="flex items-center gap-2">
			<span class={`inline-block size-2 rounded-full ${dotClass}`} aria-hidden="true"></span>
			<span>{statusLabel}</span>
		</span>
		{#if controller.status === 'done' && controller.summary}
			<span class="font-mono text-zinc-300">{controller.summary}</span>
		{/if}
	</div>

	<div
		class="max-h-96 overflow-auto px-3 py-3 font-mono text-xs leading-relaxed"
		role="log"
		aria-live="polite"
		aria-atomic="false"
		aria-label="Diagnostic output"
	>
		{#if controller.status === 'idle' && controller.lines.length === 0}
			<p class="text-zinc-500">{hint}</p>
		{:else if controller.status === 'connecting' && controller.lines.length === 0}
			<p class="text-zinc-400">Connecting to the node…</p>
		{:else if mtrRows}
			<MtrTable rows={mtrRows} />
			{#each controller.lines.filter((line) => line.kind !== 'out') as line, index (index)}
				<p class={line.kind === 'error' ? 'mt-2 text-red-400' : 'mt-2 text-zinc-400'}>{line.text}</p>
			{/each}
		{:else}
			<div class="min-w-0 whitespace-pre-wrap break-words">
				{#each controller.lines as line, index (index)}
					<p
						class={line.kind === 'error'
							? 'text-red-400'
							: line.kind === 'meta'
								? 'text-zinc-400'
								: 'text-zinc-100'}
					>
						{line.text}
					</p>
				{/each}
				{#if controller.active}
					<p class="text-emerald-400" aria-hidden="true">
						<span class="animate-pulse">▍</span>
					</p>
				{/if}
			</div>
		{/if}
	</div>
</div>
