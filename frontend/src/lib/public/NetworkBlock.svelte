<script lang="ts">
	import MapPin from '@lucide/svelte/icons/map-pin';
	import Building2 from '@lucide/svelte/icons/building-2';
	import Gauge from '@lucide/svelte/icons/gauge';
	import ExternalLink from '@lucide/svelte/icons/external-link';
	import {
		Card,
		CardHeader,
		CardTitle,
		CardDescription,
		CardContent
	} from '$lib/components/ui/card/index.js';
	import CopyButton from './CopyButton.svelte';
	import type { LocationDetail } from '$lib/admin/types.js';

	let {
		location,
		detectedIp,
		latencyMs
	}: {
		location: LocationDetail;
		detectedIp: string | null;
		latencyMs: number | null;
	} = $props();

	// A location may carry a free-text map query or a full URL; only link out when
	// it is an absolute http(s) URL, otherwise show it as plain text.
	const mapHref = $derived(
		location.map_query && /^https?:\/\//.test(location.map_query) ? location.map_query : null
	);
</script>

<Card>
	<CardHeader>
		<CardTitle>Network</CardTitle>
		<CardDescription class="flex flex-wrap items-center gap-x-4 gap-y-1">
			<span class="flex items-center gap-1.5">
				<MapPin class="size-4 shrink-0" aria-hidden="true" />
				{location.geo_label || location.name}
			</span>
			{#if location.facility}
				<span class="flex items-center gap-1.5">
					<Building2 class="size-4 shrink-0" aria-hidden="true" />
					{#if location.facility_url}
						<a
							href={location.facility_url}
							target="_blank"
							rel="noopener noreferrer"
							class="inline-flex min-h-11 items-center gap-1 break-words text-primary underline-offset-4 hover:underline"
						>
							{location.facility}
							<ExternalLink class="size-3" aria-hidden="true" />
						</a>
					{:else}
						{location.facility}
					{/if}
				</span>
			{/if}
			{#if mapHref}
				<a
					href={mapHref}
					target="_blank"
					rel="noopener noreferrer"
						class="inline-flex min-h-11 min-w-11 items-center justify-center gap-1 text-primary underline-offset-4 hover:underline"
				>
					Map
					<ExternalLink class="size-3" aria-hidden="true" />
				</a>
			{/if}
		</CardDescription>
	</CardHeader>

	<CardContent class="space-y-4">
		<div class="space-y-2">
			<h3 class="text-sm font-medium">Test IPs</h3>
			{#if location.test_ips.length === 0}
				<p class="text-sm text-muted-foreground">No test IPs listed for this location.</p>
			{:else}
				<ul class="space-y-1">
					{#each location.test_ips as ip (ip.id)}
						<li class="flex items-center gap-2 rounded-md border border-border px-3 py-2">
							<span
								class="w-9 shrink-0 text-center text-[10px] font-medium uppercase tracking-wide text-muted-foreground"
							>
								{ip.family === 'v6' ? 'IPv6' : 'IPv4'}
							</span>
							<code class="min-w-0 flex-1 truncate font-mono text-sm">{ip.address}</code>
							{#if ip.label}
								<span class="hidden shrink-0 text-xs text-muted-foreground sm:inline">{ip.label}</span>
							{/if}
							<CopyButton text={ip.address} label={`test IP ${ip.address}`} />
						</li>
					{/each}
				</ul>
			{/if}
		</div>

		<dl class="grid gap-4 sm:grid-cols-2">
			<div class="space-y-1">
				<dt class="text-sm font-medium">Your IP</dt>
				<dd class="flex items-center gap-2">
					{#if detectedIp}
						<code class="min-w-0 flex-1 truncate font-mono text-sm">{detectedIp}</code>
						<CopyButton text={detectedIp} label="your IP" />
					{:else}
						<span class="text-sm text-muted-foreground">Not detected</span>
					{/if}
				</dd>
			</div>
			<div class="space-y-1">
				<dt class="flex items-center gap-1.5 text-sm font-medium">
					<Gauge class="size-4 shrink-0" aria-hidden="true" />
					Latency
				</dt>
				<dd class="font-mono text-sm">
					{#if latencyMs !== null}
						≈ {latencyMs} ms
					{:else}
						<span class="text-muted-foreground">Measuring…</span>
					{/if}
				</dd>
			</div>
		</dl>
	</CardContent>
</Card>
