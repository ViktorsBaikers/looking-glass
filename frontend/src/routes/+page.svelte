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
	import {
		page,
		pageTitle,
		pageSubtitle,
		panel,
		panelLeft,
		inputsGrid,
		panelRight,
		runButton,
		panelNote,
		panelError,
		resultsSection
	} from '$lib/public/styles.js';
	import type { LocationDetail } from '$lib/admin/types.js';

	const controller = new RunController();

	let phase = $state<'loading' | 'ready' | 'error'>('loading');
	let locations = $state<LocationDetail[]>([]);
	let selectedId = $state('');
	let method = $state('');
	let runMethod = $state('');
	let target = $state('');
	let requiredTargetError = $state('');
	let detectedIp = $state<string | null>(null);
	let latencyMs = $state<number | null>(null);

	const selected = $derived(locations.find((location) => location.id === selectedId));
	const locationTabs = $derived(
		locations.map((location) => ({
			id: location.id,
			label: location.asn ? `${location.name} (AS${location.asn})` : location.name
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
		controller.start(selected.id, selected.name, method, trimmed);
	}

	function onsubmit(event: SubmitEvent) {
		event.preventDefault();
		if (controller.active) controller.cancel();
		else run();
	}
</script>

<div class={page}>
	<header>
		<h1 class={pageTitle}>Network diagnostics</h1>
		<p class={pageSubtitle}>Run connectivity and performance tests from any available location.</p>
	</header>

	{#if phase === 'error'}
		<p class={panelError} role="alert">Couldn't load locations. Refresh the page to try again.</p>
	{:else if phase === 'loading'}
		<p class={panelNote}>Loading locations…</p>
	{:else if !hasLocations}
		<p class={panelNote}>No locations online yet.</p>
	{:else}
		<Tabs tabs={locationTabs} bind:active={selectedId} label="Location" />

		<form class={panel} onsubmit={onsubmit}>
			<div class={panelLeft}>
				<div class={inputsGrid}>
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
						/>
					</Field>
				</div>
				{#if methodOptions.length === 0}
					<p class={panelNote}>This location has no runnable methods enabled yet.</p>
				{/if}
			</div>

			<div class={panelRight}>
				{#if selected}
					<StatusPanel location={selected} {detectedIp} {latencyMs} />
				{/if}
				<Button type="submit" size="lg" class={runButton} disabled={!controller.active && !canRun}>
					{controller.active ? 'Cancel' : 'Run Diagnostic'}
				</Button>
			</div>
		</form>
	{/if}

	<div class={resultsSection}>
		<Console
			{controller}
			method={runMethod || method}
			idleTitle={selected ? `${selected.name} ~ ${method}` : ''}
		/>
		{#if metrics.length > 0}
			<MetricsGrid {metrics} />
		{/if}
	</div>

	{#if selected}
		<SpeedtestBlock location={selected} />
	{/if}
</div>
