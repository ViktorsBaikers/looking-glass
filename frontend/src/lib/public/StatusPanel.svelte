<script lang="ts">
	import OpenInNew from '~icons/material-symbols/open-in-new';
	import Collapsible from '$lib/components/ui/collapsible.svelte';
	import CopyButton from '$lib/components/ui/copy-button.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import {
		statusList,
		statusRow,
		statusValue,
		statusMono,
		statusValueRow,
		statusDotOnline,
		statusDotOffline,
		iconLink,
		geoValue,
		moreIpList,
		moreIpRow,
		moreIpMeta,
		moreIps as moreWrap,
		moreToggle
	} from './styles.js';
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

	// A map_query is either a full URL (linked as-is) or a free-text query,
	// which links out to an OpenStreetMap search.
	const mapHref = $derived(
		location.map_query
			? /^https?:\/\//.test(location.map_query)
				? location.map_query
				: `https://www.openstreetmap.org/search?query=${encodeURIComponent(location.map_query)}`
			: null
	);

	const firstV4 = $derived(location.test_ips.find((ip) => ip.family === 'v4') ?? null);
	const firstV6 = $derived(location.test_ips.find((ip) => ip.family === 'v6') ?? null);
	const moreIps = $derived(location.test_ips.filter((ip) => ip !== firstV4 && ip !== firstV6));
</script>

<div class={statusList} role="group" aria-label="Location status">
	<div class={statusRow}>
		<span>Status</span>
		<span class={statusValueRow}>
			<span
				class={location.status === 'online' ? statusDotOnline : statusDotOffline}
				aria-hidden="true"></span>
			<span class={statusValue}>{location.status === 'online' ? 'Online' : 'Offline'}</span>
		</span>
	</div>

	<div class={statusRow}>
		<span>Network</span>
		<span class={statusMono}>{location.asn ? `AS${location.asn}` : '—'}</span>
	</div>

	<div class={statusRow}>
		<span>Location</span>
		<span class={geoValue}>
			<span>{location.geo_label}</span>
			{#if mapHref}
				<a class={iconLink} href={mapHref} target="_blank" rel="noreferrer" aria-label={`Open map for ${location.geo_label}`}>
					<OpenInNew aria-hidden="true" />
				</a>
			{/if}
		</span>
	</div>

	{#if location.facility}
		<div class={statusRow}>
			<span>Facility</span>
			<span class={geoValue}>
				<span>{location.facility}</span>
				{#if location.facility_url}
					<a class={iconLink} href={location.facility_url} target="_blank" rel="noreferrer" aria-label={`Open facility page for ${location.facility}`}>
						<OpenInNew aria-hidden="true" />
					</a>
				{/if}
			</span>
		</div>
	{/if}

	<div class={statusRow}>
		<span>IPv4</span>
		<span class={statusValueRow}>
			{#if firstV4}
				<span class={statusMono}>{firstV4.address}</span>
				<CopyButton text={firstV4.address} label="IPv4 test IP" />
			{:else}
				<span class={statusValue}>—</span>
			{/if}
		</span>
	</div>

	<div class={statusRow}>
		<span>IPv6</span>
		<span class={statusValueRow}>
			{#if firstV6}
				<span class={statusMono}>{firstV6.address}</span>
				<CopyButton text={firstV6.address} label="IPv6 test IP" />
			{:else}
				<span class={statusValue}>—</span>
			{/if}
		</span>
	</div>

	<div class={statusRow}>
		<span>Your IP</span>
		<span class={statusMono}>{detectedIp ?? '—'}</span>
	</div>

	<div class={statusRow}>
		<span>Latency ≈</span>
		<span class={statusValue}>{latencyMs === null ? '—' : `${latencyMs} ms`}</span>
	</div>
	{#if moreIps.length > 0}
		<div class={moreWrap}>
		<Collapsible>
			{#snippet trigger()}
				<Button variant="ghost" size="sm" class={moreToggle}>More test IPs</Button>
			{/snippet}
			<div class={moreIpList}>
				{#each moreIps as ip (ip.id)}
					<div class={moreIpRow}>
						<span class={statusMono}>{ip.address}</span>
						<span class={moreIpMeta}>{ip.family === 'v4' ? 'IPv4' : 'IPv6'}{ip.label ? ` · ${ip.label}` : ''}</span>
						<CopyButton text={ip.address} label={`test IP ${ip.address}`} />
					</div>
				{/each}
			</div>
		</Collapsible>
		</div>
	{/if}
</div>
