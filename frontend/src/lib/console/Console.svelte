<script lang="ts">
	import ContentCopy from '~icons/material-symbols/content-copy';
	import { cx } from 'styled-system/css';
	import { Button } from '$lib/components/ui/button/index.js';
	import { toast } from '$lib/toast.svelte.js';
	import { lineCode, lineStyle } from '$lib/lines.js';
	import {
		consoleHeader,
		consoleHeaderLeft,
		consoleHeaderRight,
		consoleHeading,
		chip,
		chipText,
		chipDot,
		chipDotTone,
		copyOutputIcon,
		viewSwitch,
		viewOption,
		terminal,
		titlebar,
		roundel,
		roundelIdle,
		titlebarText,
		terminalBody,
		linePlain,
		lineBytes,
		lineTime,
		lineError,
		lineMeta,
		lineHint,
		cursor,
		metaGap,
		tableWrap
	} from '$lib/public/styles.js';
	import type { RunController, RunStatus } from './run.svelte.js';
	import { colorize } from './colorize.js';
	import { parseMtr } from './mtr.js';
	import { parseTraceroute } from './trace.js';
	import MtrTable from './MtrTable.svelte';
	import TraceTable from './TraceTable.svelte';

	let {
		controller,
		method,
		idleTitle,
		location,
		hint = 'Pick a location and method, enter a target, then Run diagnostic.'
	}: {
		controller: RunController;
		method: string;
		idleTitle: string;
		/** Location whose line colours the output (the run's, else the selected one). */
		location: { id: string; name: string } | null;
		hint?: string;
	} = $props();

	const CHIP_LABEL: Record<Exclude<RunStatus, 'idle'>, string> = {
		connecting: 'Connecting',
		streaming: 'Streaming',
		done: 'Completed',
		error: 'Failed',
		canceled: 'Canceled'
	};

	const toneClass = {
		plain: linePlain,
		bytes: cx(linePlain, lineBytes),
		time: cx(linePlain, lineTime)
	} as const;

	// controller.lines only grows until a new run replaces it, so each line is
	// split and filed once. The log renders in blocks of BLOCK lines and only the
	// growing block re-renders: a frame costs its new lines plus one step per
	// block, not a pass over the whole log (F-338).
	type Line = RunController['lines'][number];
	const BLOCK = 100;
	let seen: Line[] | null = null;
	let count = 0;
	let blocks: Line[][] = [];
	let output: Line[] = [];
	let other: Line[] = [];
	// A new object per change: everything below reads through `log`.
	const log = $derived.by(() => {
		const lines = controller.lines;
		if (lines !== seen) {
			seen = lines;
			count = 0;
			blocks = [];
			output = [];
			other = [];
		}
		for (; count < lines.length; count++) {
			const line = lines[count];
			(line.kind === 'out' ? output : other).push(line);
			const last = blocks.length - 1;
			// The growing block is replaced, never mutated, so its {#each} sees the change.
			if (last < 0 || blocks[last].length === BLOCK) blocks.push([line]);
			else blocks[last] = [...blocks[last], line];
		}
		return { blocks, output, other };
	});
	const mtrRows = $derived(
		(method === 'mtr' || method === 'mtr6') && controller.status === 'done'
			? parseMtr(log.output.map((line) => line.text))
			: null
	);
	const traceRows = $derived(
		method === 'traceroute' || method === 'traceroute6'
			? parseTraceroute(log.output.map((line) => line.text))
			: null
	);
	const hasRoute = $derived(mtrRows !== null || traceRows !== null);

	// Route view by default; Raw shows the tool's own text, byte for byte.
	let raw = $state(false);

	async function copyOutput() {
		try {
			await navigator.clipboard.writeText(controller.lines.map((line) => line.text).join('\n'));
			toast.success('Output copied.');
		} catch {
			// Denied, or no clipboard API outside a secure context.
			toast.error("Couldn't copy the output. Select it and copy it manually.");
		}
	}
</script>

<div class={consoleHeader}>
	<div class={consoleHeaderLeft}>
		<h2 class={consoleHeading}>Output</h2>
		{#if controller.status !== 'idle'}
			<span class={chip} role="status">
				<span class={cx(chipDot, chipDotTone[controller.status])} aria-hidden="true"></span>
				<span class={chipText}>{CHIP_LABEL[controller.status]}</span>
			</span>
		{/if}
	</div>
	<div class={consoleHeaderRight}>
		{#if hasRoute}
			<div class={viewSwitch} role="group" aria-label="Output view">
				<button type="button" class={viewOption} aria-pressed={!raw} onclick={() => (raw = false)}>Route</button>
				<button type="button" class={viewOption} aria-pressed={raw} onclick={() => (raw = true)}>Raw</button>
			</div>
		{/if}
		<Button variant="secondary" size="sm" class={copyOutputIcon} onclick={copyOutput}>
			<ContentCopy aria-hidden="true" />
			Copy output
		</Button>
	</div>
</div>

<div class={terminal} data-line={location ? '' : undefined} style={location ? lineStyle(location.id) : undefined}>
	<div class={titlebar}>
		{#if location}
			<span class={roundel} aria-hidden="true">{lineCode(location.name)}</span>
		{:else}
			<span class={roundelIdle} aria-hidden="true"></span>
		{/if}
		<span class={titlebarText}>{controller.runTitle || idleTitle}</span>
	</div>

	<div
		class={terminalBody}
		role="log"
		aria-live="polite"
		aria-atomic="false"
		aria-label="Diagnostic output"
	>
		{#if controller.status === 'idle' && controller.lines.length === 0}
			<p class={lineHint}>{hint}</p>
		{:else if controller.status === 'connecting' && controller.lines.length === 0}
			<p class={lineHint}>Connecting to the node…</p>
		{:else if hasRoute && !raw}
			{#each log.output.filter((line) => !/^\s*(\d+[\s.]|HOST:)/.test(line.text)) as line, index (index)}
				<p class={cx(linePlain, lineMeta)}>{line.text}</p>
			{/each}
			<div class={tableWrap}>
				{#if mtrRows}
					<MtrTable rows={mtrRows} origin={location?.name} />
				{:else if traceRows}
					<TraceTable rows={traceRows} live={controller.active} origin={location?.name} />
				{/if}
			</div>
			{#each log.other as line, index (index)}
				<p class={cx(linePlain, index === 0 ? metaGap : '', line.kind === 'error' ? lineError : lineMeta)}>
					{line.text}
				</p>
			{/each}
		{:else}
			{#each log.blocks as block, blockIndex (blockIndex)}
				{#each block as line, index (index)}
					{#if line.kind === 'out'}
						<p class={linePlain}>{#each colorize(line.text) as segment, segmentIndex (segmentIndex)}<span class={toneClass[segment.tone]}>{segment.text}</span>{/each}</p>
					{:else}
						<p class={line.kind === 'error' ? cx(linePlain, lineError) : cx(linePlain, lineMeta)}>
							{line.text}
						</p>
					{/if}
				{/each}
			{/each}
			{#if controller.active}
				<span class={cursor} aria-hidden="true"></span>
			{/if}
		{/if}
	</div>
</div>
