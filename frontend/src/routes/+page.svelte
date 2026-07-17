<script lang="ts">
	import { onMount } from 'svelte';
	import LoaderCircle from '@lucide/svelte/icons/loader-circle';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import Console from '$lib/console/Console.svelte';
	import { RunController } from '$lib/console/run.svelte.js';
	import NetworkBlock from '$lib/public/NetworkBlock.svelte';
	import SpeedtestBlock from '$lib/public/SpeedtestBlock.svelte';
	import { fetchLocations, fetchVisitorIp } from '$lib/public/api.js';
	import { runnableMethods, targetPlaceholder } from '$lib/public/methods.js';
	import { measureLatency } from '$lib/public/latency.js';
	import type { LocationDetail } from '$lib/admin/types.js';

	const controller = new RunController();

	let phase = $state<'loading' | 'ready' | 'error'>('loading');
	let locations = $state<LocationDetail[]>([]);
	let selectedId = $state('');
	let method = $state('');
	let target = $state('');
	let requiredTargetError = $state('');
	let detectedIp = $state<string | null>(null);
	let latencyMs = $state<number | null>(null);

	function isClearlyNonPublicIpv4(value: string): boolean {
		const parts = value.split('.');
		if (parts.length !== 4 || parts.some((part) => !/^(0|[1-9]\d{0,2})$/.test(part))) return false;

		const octets = parts.map(Number);
		if (octets.some((octet) => octet > 255)) return false;

		const [first, second] = octets;
		return (
			first === 10 ||
			first === 127 ||
			(first === 169 && second === 254) ||
			(first === 172 && second >= 16 && second <= 31) ||
			(first === 192 && second === 168)
		);
	}

	const selected = $derived(locations.find((location) => location.id === selectedId));
	const methodOptions = $derived(selected ? runnableMethods(selected) : []);
	const hasLocations = $derived(locations.length > 0);
	const controlsDisabled = $derived(controller.active || !hasLocations);
	const targetPreflightError = $derived(
		method !== 'bgp' && method !== 'bgp6' && isClearlyNonPublicIpv4(target.trim())
			? 'Enter a publicly routable IPv4 address or hostname.'
			: ''
	);
	const targetError = $derived(targetPreflightError || requiredTargetError);
	const canRun = $derived(hasLocations && methodOptions.length > 0 && !targetPreflightError);

	// Keep the method selection valid as the chosen location (and its offered set)
	// changes — default to its first runnable method when the current one is gone.
	$effect(() => {
		const values = methodOptions.map((option) => option.value);
		if (values.length > 0 && !values.includes(method)) {
			method = values[0];
		}
	});

	const fieldClass =
		'flex h-11 w-full rounded-md border border-input bg-transparent px-3 py-1 text-sm shadow-sm transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background disabled:cursor-not-allowed disabled:opacity-50';

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
		if (targetPreflightError) {
			return;
		}
		const label = methodOptions.find((option) => option.value === method)?.label ?? method;
		controller.start(selectedId, method, label, trimmed);
	}

	function onsubmit(event: SubmitEvent) {
		event.preventDefault();
		if (controller.active) controller.cancel();
		else run();
	}
</script>

<section class="space-y-6">
	<div class="space-y-1">
		<h1 class="text-2xl font-semibold tracking-tight">Network diagnostics</h1>
		<p class="text-muted-foreground">
			Run ping, traceroute, MTR, or a read-only BGP route lookup from a location and watch the
			output stream in live.
		</p>
	</div>

	<form class="space-y-4" {onsubmit}>
		<div class="grid gap-4 sm:grid-cols-[minmax(0,1fr)_minmax(0,1fr)_minmax(0,1.5fr)_auto]">
			<div class="space-y-2">
				<Label for="location">Location</Label>
				<select id="location" class={fieldClass} bind:value={selectedId} disabled={controlsDisabled}>
					{#each locations as location (location.id)}
						<option value={location.id}>{location.name}</option>
					{/each}
				</select>
			</div>

			<div class="space-y-2">
				<Label for="method">Method</Label>
				<select
					id="method"
					class={fieldClass}
					bind:value={method}
					disabled={controlsDisabled || methodOptions.length === 0}
				>
					{#each methodOptions as option (option.value)}
						<option value={option.value}>{option.label}</option>
					{/each}
				</select>
			</div>

			<div class="space-y-2">
				<Label for="target">Target</Label>
				<Input
					id="target"
					placeholder={targetPlaceholder(method)}
					autocomplete="off"
					spellcheck={false}
					bind:value={target}
					disabled={controlsDisabled}
					aria-invalid={targetError ? 'true' : undefined}
					aria-describedby={targetError ? 'target-error' : undefined}
				/>
			</div>

			<div class="space-y-2">
				<span class="hidden text-sm sm:block sm:invisible" aria-hidden="true">Run</span>
				{#if controller.active}
					<Button type="submit" variant="destructive" class="w-full sm:w-auto">Cancel</Button>
				{:else}
					<Button type="submit" class="w-full sm:w-auto" disabled={!canRun}>
						{#if controller.status === 'connecting'}
							<LoaderCircle class="animate-spin" aria-hidden="true" />
							Running
						{:else}
							Run
						{/if}
					</Button>
				{/if}
			</div>
		</div>

		{#if targetError}
			<p id="target-error" class="text-sm text-destructive" role="alert">{targetError}</p>
		{/if}

		{#if phase === 'loading'}
			<p class="flex items-center gap-2 text-sm text-muted-foreground">
				<LoaderCircle class="size-4 animate-spin" aria-hidden="true" />
				Loading locations…
			</p>
		{:else if phase === 'error'}
			<p class="text-sm text-destructive" role="alert">
				Couldn't load locations. Refresh the page to try again.
			</p>
		{:else if !hasLocations}
			<p class="text-sm text-muted-foreground">No locations online yet.</p>
		{:else if methodOptions.length === 0}
			<p class="text-sm text-muted-foreground">
				This location has no runnable methods enabled yet.
			</p>
		{/if}
	</form>

	<Console {controller} {method} />

	{#if selected}
		<NetworkBlock location={selected} {detectedIp} {latencyMs} />
		<SpeedtestBlock location={selected} />
	{/if}
</section>
