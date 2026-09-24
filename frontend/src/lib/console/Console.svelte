<script lang="ts">
	import ContentCopy from '~icons/material-symbols/content-copy';
	import { cx } from 'styled-system/css';
	import { Button } from '$lib/components/ui/button/index.js';
	import {
		consoleHeader,
		consoleHeaderLeft,
		consoleHeading,
		chip,
		chipText,
		chipDot,
		chipDotTone,
		copyOutputIcon,
		terminal,
		titlebar,
		trafficDot,
		trafficRed,
		trafficYellow,
		trafficGreen,
		titlebarText,
		terminalBody,
		linePlain,
		lineBytes,
		lineTime,
		lineError,
		lineMeta,
		lineHint,
		cursor,
		tableWrap
	} from '$lib/public/styles.js';
	import type { RunController, RunStatus } from './run.svelte.js';
	import { colorize } from './colorize.js';
	import { parseMtr } from './mtr.js';
	import MtrTable from './MtrTable.svelte';

	let {
		controller,
		method,
		idleTitle,
		hint = 'Pick a location and method, enter a target, then Run Diagnostic.'
	}: { controller: RunController; method: string; idleTitle: string; hint?: string } = $props();

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

	const outputLines = $derived(controller.lines.filter((line) => line.kind === 'out'));
	const mtrRows = $derived(
		(method === 'mtr' || method === 'mtr6') && controller.status === 'done'
			? parseMtr(outputLines.map((line) => line.text))
			: null
	);

	function copyOutput() {
		navigator.clipboard?.writeText(controller.lines.map((line) => line.text).join('\n'));
	}
</script>

<div class={consoleHeader}>
	<div class={consoleHeaderLeft}>
		<span class={consoleHeading}>Live Console</span>
		{#if controller.status !== 'idle'}
			<span class={chip} role="status">
				<span class={cx(chipDot, chipDotTone[controller.status])} aria-hidden="true"></span>
				<span class={chipText}>{CHIP_LABEL[controller.status]}</span>
			</span>
		{/if}
	</div>
	<Button variant="secondary" size="sm" class={copyOutputIcon} onclick={copyOutput}>
		<ContentCopy aria-hidden="true" />
		Copy output
	</Button>
</div>

<div class={terminal}>
	<div class={titlebar}>
		<span class={cx(trafficDot, trafficRed)} aria-hidden="true"></span>
		<span class={cx(trafficDot, trafficYellow)} aria-hidden="true"></span>
		<span class={cx(trafficDot, trafficGreen)} aria-hidden="true"></span>
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
		{:else if mtrRows}
			<div class={tableWrap}>
				<MtrTable rows={mtrRows} />
			</div>
			{#each controller.lines.filter((line) => line.kind !== 'out') as line, index (index)}
				<p class={line.kind === 'error' ? cx(linePlain, lineError) : cx(linePlain, lineMeta)}>
					{line.text}
				</p>
			{/each}
		{:else}
			{#each controller.lines as line, index (index)}
				{#if line.kind === 'out'}
					<p class={linePlain}>{#each colorize(line.text) as segment, segmentIndex (segmentIndex)}<span class={toneClass[segment.tone]}>{segment.text}</span>{/each}</p>
				{:else}
					<p class={line.kind === 'error' ? cx(linePlain, lineError) : cx(linePlain, lineMeta)}>
						{line.text}
					</p>
				{/if}
			{/each}
			{#if controller.active}
				<p class={cursor} aria-hidden="true">▍</p>
			{/if}
		{/if}
	</div>
</div>
