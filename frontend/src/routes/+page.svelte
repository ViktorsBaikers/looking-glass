<script lang="ts">
	import { onMount } from 'svelte';
	import Tabs from '$lib/components/ui/tabs.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import Field from '$lib/components/ui/field.svelte';
	import Select from '$lib/components/ui/select.svelte';
	import Console from '$lib/console/Console.svelte';
	import MetricsGrid from '$lib/console/MetricsGrid.svelte';
	import { RunController } from '$lib/console/run.svelte.js';
	import { metricsFor } from '$lib/console/metrics.js';
	import StatusPanel from '$lib/public/StatusPanel.svelte';
	import SpeedtestBlock from '$lib/public/SpeedtestBlock.svelte';
	import { fetchLocations, fetchVisitorIp } from '$lib/public/api.js';
	import { runnableMethods, targetPlaceholder, targetPreflightError } from '$lib/public/methods.js';
	import { measureLatency } from '$lib/public/latency.js';
	import { lineCode, lineStyle } from '$lib/lines.js';
	import {
		page,
		pageHead,
		pageTitle,
		pageSubtitle,
		band,
		panel as controlPanel,
		runButton,
		panelNote,
		pageNote,
		resultsSection
	} from '$lib/public/styles.js';
	import type { LocationDetail } from '$lib/admin/types.js';

	const controller = new RunController();

	let phase = $state<'loading' | 'ready' | 'error'>('loading');
	let locations = $state<LocationDetail[]>([]);
	let selectedId = $state('');
	let method = $state('');
	let runMethod = $state('');
	let runLocation = $state<LocationDetail | null>(null);
	let outputSection = $state<HTMLElement>();
	let target = $state('');
	let requiredTargetError = $state('');
	let detectedIp = $state<string | null>(null);
	let latencyMs = $state<number | null>(null);

	const selected = $derived(locations.find((location) => location.id === selectedId));
	const locationTabs = $derived(
		locations.map((location) => ({
			id: location.id,
			label: location.name,
			meta: location.asn ? `AS${location.asn}` : undefined,
			code: lineCode(location.name),
			line: lineStyle(location.id)
		}))
	);
	const methodOptions = $derived(selected ? runnableMethods(selected) : []);
	const hasLocations = $derived(locations.length > 0);
	const preflightError = $derived(targetPreflightError(method, target));
	const targetError = $derived(preflightError || requiredTargetError);
	const canRun = $derived(hasLocations && methodOptions.length > 0 && preflightError === '');

	// Metric cards derive from the finished run's output only (Completed).
	const metrics = $derived(
		controller.status === 'done'
			? metricsFor(
					runMethod,
					controller.lines.filter((line) => line.kind === 'out').map((line) => line.text)
				)
			: []
	);

	// Keep the method selection valid as the chosen location (and its offered
	// set) changes — default to its first runnable method when the current one
	// is gone.
	$effect(() => {
		const values = methodOptions.map((option) => option.value);
		if (values.length > 0 && !values.includes(method)) {
			method = values[0];
		}
	});

	onMount(async () => {
		const [catalogue, ip] = await Promise.all([fetchLocations(), fetchVisitorIp()]);
		detectedIp = ip;
		if (catalogue.ok) {
			locations = catalogue.data;
			selectedId = catalogue.data[0]?.id ?? '';
			phase = 'ready';
		} else {
			phase = 'error';
		}
		measureLatency().then((ms) => (latencyMs = ms));
	});

	function run() {
		requiredTargetError = '';
		const trimmed = target.trim();
		if (!trimmed) {
			requiredTargetError = 'Enter a target IP address or hostname.';
			return;
		}
		if (preflightError || !selected) return;
		runMethod = method;
		runLocation = selected;
		controller.start(selected.id, selected.name, method, trimmed);
		// On short screens the output starts below the fold: bring it up.
		const top = outputSection?.getBoundingClientRect().top ?? 0;
		if (top > window.innerHeight - 120) {
			const reduce = matchMedia('(prefers-reduced-motion: reduce)').matches;
			outputSection?.scrollIntoView({ block: 'start', behavior: reduce ? 'auto' : 'smooth' });
		}
	}

	function onsubmit(event: SubmitEvent) {
		event.preventDefault();
		if (controller.active) controller.cancel();
		else run();
	}
</script>

<div class={page}>
	<header class={pageHead}>
		<h1 class={pageTitle}>Network diagnostics</h1>
		<p class={pageSubtitle}>Run connectivity and performance tests from any available location.</p>
	</header>

	{#if phase === 'error'}
		<p class={pageNote} role="alert">Couldn't load locations. Refresh the page to try again.</p>
	{:else if phase === 'loading'}
		<p class={pageNote}>Loading locations…</p>
	{:else if !hasLocations}
		<p class={pageNote}>No locations online yet.</p>
	{:else}
		<Tabs tabs={locationTabs} bind:active={selectedId} label="Location" variant="routes">
			{#snippet panel(tab)}
				<div class={band} data-line style={tab.line}>
					<form class={controlPanel} onsubmit={onsubmit}>
						<Field label="Method" for="method">
							<Select
								id="method"
								items={methodOptions}
								bind:value={method}
								placeholder="Select method"
								disabled={controller.active || methodOptions.length === 0}
							/>
						</Field>
						<Field label="Target" for="target" error={targetError}>
							<Input
								id="target"
								bind:value={target}
								mono
								placeholder={targetPlaceholder(method)}
								disabled={controller.active}
								invalid={targetError !== ''}
								aria-describedby={targetError ? 'target-error' : undefined}
							/>
						</Field>
						<Button type="submit" class={runButton} disabled={!controller.active && !canRun}>
							{controller.active ? 'Cancel' : 'Run Diagnostic'}
						</Button>
						{#if methodOptions.length === 0}
							<p class={panelNote}>This location has no runnable methods enabled yet.</p>
						{/if}
					</form>
					{#if selected}
						<StatusPanel location={selected} {detectedIp} {latencyMs} />
					{/if}
				</div>
			{/snippet}
		</Tabs>
	{/if}

	<section class={resultsSection} aria-label="Output" bind:this={outputSection}>
		<Console
			{controller}
			method={runMethod || method}
			idleTitle={selected ? `${selected.name} ~ ${method}` : ''}
			location={runLocation ?? selected ?? null}
		/>
		{#if metrics.length > 0}
			<MetricsGrid {metrics} />
		{/if}
	</section>

	{#if selected}
		<SpeedtestBlock location={selected} />
	{/if}
</div>
