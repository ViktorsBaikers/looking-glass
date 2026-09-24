<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import Field from '$lib/components/ui/field.svelte';
	import Select from '$lib/components/ui/select.svelte';
	import StatusBadge from '$lib/components/ui/status-badge.svelte';
	import Dialog from '$lib/components/ui/dialog.svelte';
	import ConfirmDialog from '$lib/components/ui/confirm-dialog.svelte';
	import { confirmActions, srOnly } from '$lib/styles.js';
	import {
		pageHead,
		pageTitle,
		pageSub,
		toolbar,
		searchField,
		searchWrap,
		searchInput,
		sortField,
		fieldLabel,
		stack,
		grid,
		card,
		cardHead,
		cardTitle,
		metaRow,
		geoChip,
		metrics,
		metricLabel,
		metricValue,
		cardFoot,
		outlineAction,
		trailingAction,
		stateCard,
		stateText,
		errorCard,
		skeleton,
		formStack,
		formError
	} from '$lib/admin/list-styles.js';
	import { toast } from '$lib/toast.svelte.js';
	import {
		listLocations,
		createLocation,
		deleteLocation,
		revokeAgent,
		type LocationInput
	} from '$lib/admin/api.js';
	import { formatLastSeen } from '$lib/admin/lastSeen.js';
	import {
		filterLocations,
		locationState,
		sortLocations,
		STATE_LABEL,
		type LocationState,
		type SortKey
	} from '$lib/admin/locationList.js';
	import type { Location, NodeKind } from '$lib/admin/types.js';
	import Add from '~icons/material-symbols/add';
	import Search from '~icons/material-symbols/search';
	import VpnKey from '~icons/material-symbols/vpn-key';
	import Pencil from '~icons/material-symbols/edit';
	import Trash from '~icons/material-symbols/delete';
	import LinkOff from '~icons/material-symbols/link-off';

	let phase = $state<'loading' | 'ready' | 'error'>('loading');
	let locations = $state<Location[]>([]);
	let query = $state('');
	let sortValue = $state('name');

	// Ticks once a minute so the relative last-seen labels stay current without a
	// per-second re-render; the online/offline state itself comes from the API.
	let nowMs = $state(Date.now());
	$effect(() => {
		const id = setInterval(() => (nowMs = Date.now()), 60_000);
		return () => clearInterval(id);
	});

	const visible = $derived(
		sortLocations(filterLocations(locations, query), sortValue as SortKey)
	);

	const tone: Record<LocationState, 'success' | 'danger' | 'neutral'> = {
		online: 'success',
		offline: 'danger',
		not_enrolled: 'neutral'
	};

	let showCreate = $state(false);
	let draft = $state({ name: '', geo_label: '', kind: 'local' });
	let creating = $state(false);
	let createError = $state('');

	let pendingDelete = $state<Location | null>(null);
	let showDelete = $state(false);
	let deleting = $state(false);
	let pendingRevoke = $state<Location | null>(null);
	let showRevoke = $state(false);
	let revoking = $state(false);

	const kindItems = [
		{ label: 'Local (built-in node)', value: 'local' },
		{ label: 'Remote (enrolled agent)', value: 'remote' }
	];
	const sortItems = [
		{ label: 'Name', value: 'name' },
		{ label: 'Status', value: 'status' },
		{ label: 'Recent', value: 'recent' }
	];

	onMount(load);

	async function load() {
		phase = 'loading';
		const result = await listLocations();
		if (result.ok) {
			locations = result.data;
			phase = 'ready';
		} else {
			phase = 'error';
		}
	}

	function openCreate() {
		draft = { name: '', geo_label: '', kind: 'local' };
		createError = '';
		showCreate = true;
	}

	async function submitCreate(event: SubmitEvent) {
		event.preventDefault();
		if (creating) return;
		creating = true;
		createError = '';
		const body: LocationInput = {
			name: draft.name,
			geo_label: draft.geo_label,
			map_query: null,
			facility: null,
			facility_url: null,
			kind: draft.kind as NodeKind,
			data_plane_origin: null,
			offered_methods: []
		};
		const result = await createLocation(body);
		creating = false;
		if (result.ok) {
			showCreate = false;
			toast.success('Location created.');
			// A fresh remote needs its agent enrolled first; a local node runs on
			// the built-in node, so it goes straight to its settings.
			const tab = result.data.kind === 'remote' ? 'enrollment' : 'settings';
			await goto(`/admin/locations/${result.data.id}?tab=${tab}`);
		} else {
			createError = result.message;
		}
	}

	async function confirmDelete() {
		if (!pendingDelete || deleting) return;
		const location = pendingDelete;
		deleting = true;
		const result = await deleteLocation(location.id);
		deleting = false;
		if (result.ok) {
			showDelete = false;
			pendingDelete = null;
			toast.success(`Deleted ${location.name} and everything under it.`);
			await load();
		} else {
			toast.error(result.message);
		}
	}

	async function confirmRevoke() {
		if (!pendingRevoke || revoking) return;
		const location = pendingRevoke;
		revoking = true;
		const result = await revokeAgent(location.id);
		revoking = false;
		if (result.ok) {
			showRevoke = false;
			pendingRevoke = null;
			toast.success(`Revoked ${location.name}'s agent.`);
			await load();
		} else {
			toast.error(result.message);
		}
	}
</script>

<div class={stack}>
	<header class={pageHead}>
		<div>
			<h1 class={pageTitle}>Locations</h1>
			<p class={pageSub}>Find a diagnostic location, check its state, or continue its setup.</p>
		</div>
		<Button onclick={openCreate}>
			<Add aria-hidden="true" />
			Add location
		</Button>
	</header>

	<section class={toolbar}>
		<div class={searchField}>
			<Label for="location-search" class={fieldLabel}>Search locations</Label>
			<div class={searchWrap}>
				<Search aria-hidden="true" />
				<Input
					id="location-search"
					class={searchInput}
					bind:value={query}
					placeholder="Name, place, or status"
				/>
			</div>
		</div>
		<div class={sortField}>
			<Label for="location-sort" class={fieldLabel}>Sort locations</Label>
			<Select id="location-sort" items={sortItems} bind:value={sortValue} />
		</div>
	</section>

	{#if phase === 'loading'}
		<div class={grid} aria-hidden="true">
			{#each { length: 4 } as _, i (i)}
				<div class={skeleton}></div>
			{/each}
		</div>
		<p class={srOnly} aria-live="polite">Loading locations…</p>
	{:else if phase === 'error'}
		<div class={errorCard} role="alert">
			<p>Locations could not be loaded.</p>
			<Button variant="secondary" onclick={load}>Try again</Button>
		</div>
	{:else if locations.length === 0}
		<div class={stateCard}>
			<p class={stateText}>No locations yet — add your first.</p>
			<Button onclick={openCreate}>
				<Add aria-hidden="true" />
				Add location
			</Button>
		</div>
	{:else if visible.length === 0}
		<div class={stateCard}>
			<p class={stateText}>No locations match “{query}”.</p>
			<Button variant="secondary" onclick={() => (query = '')}>Clear search</Button>
		</div>
	{:else}
		<ul class={grid}>
			{#each visible as location (location.id)}
				{@const state = locationState(location)}
				<li>
					<article class={card}>
						<div class={cardHead}>
							<div>
								<h2 class={cardTitle}>{location.name}</h2>
								<div class={metaRow}>
									{#if location.geo_label}
										<span class={geoChip}>{location.geo_label}</span>
										<span aria-hidden="true">•</span>
									{/if}
									<span>{location.kind === 'remote' ? 'Remote' : 'Local'}</span>
								</div>
							</div>
							<StatusBadge tone={tone[state]}>{STATE_LABEL[state]}</StatusBadge>
						</div>
						<div class={metrics}>
							<div>
								<p class={metricLabel}>Last Seen</p>
								<p class={metricValue}>
									{location.kind === 'local' ? '—' : formatLastSeen(location.last_seen, nowMs)}
								</p>
							</div>
							<div>
								<p class={metricLabel}>Diagnostic Methods</p>
								<p class={metricValue}>
									{location.offered_methods.length} method{location.offered_methods.length === 1
										? ''
										: 's'} configured
								</p>
							</div>
						</div>
						<div class={cardFoot}>
							{#if location.kind === 'remote'}
								<Button
									variant="secondary"
									onclick={() => goto(`/admin/locations/${location.id}?tab=enrollment`)}
								>
									<VpnKey aria-hidden="true" />
									Enroll
								</Button>
							{/if}
							<Button
								variant="ghost"
								class={outlineAction}
								onclick={() => goto(`/admin/locations/${location.id}?tab=settings`)}
							>
								<Pencil aria-hidden="true" />
								Edit
							</Button>
							{#if location.kind === 'remote' && state !== 'not_enrolled'}
								<Button
									variant="ghost"
									class={outlineAction}
									onclick={() => {
										pendingRevoke = location;
										showRevoke = true;
									}}
								>
									<LinkOff aria-hidden="true" />
									Revoke
								</Button>
							{/if}
							<Button
								variant="danger"
								size="icon"
								class={trailingAction}
								aria-label="Delete {location.name}"
								onclick={() => {
									pendingDelete = location;
									showDelete = true;
								}}
							>
								<Trash aria-hidden="true" />
							</Button>
						</div>
					</article>
				</li>
			{/each}
		</ul>
	{/if}
</div>

<Dialog bind:open={showCreate} title="Add location" description="Name it; configure it next.">
	<form class={formStack} onsubmit={submitCreate} novalidate>
		<Field label="Display name" for="new-name">
			<Input id="new-name" bind:value={draft.name} disabled={creating} required />
		</Field>
		<Field label="Geographic label" for="new-geo">
			<Input id="new-geo" bind:value={draft.geo_label} placeholder="Frankfurt, DE" disabled={creating} />
		</Field>
		<Field label="Node kind" for="new-kind">
			<Select id="new-kind" items={kindItems} bind:value={draft.kind} disabled={creating} portaled={false} />
		</Field>
		{#if createError}
			<p class={formError} role="alert">{createError}</p>
		{/if}
		<div class={confirmActions}>
			<Button type="button" variant="ghost" onclick={() => (showCreate = false)} disabled={creating}>
				Cancel
			</Button>
			<Button type="submit" loading={creating}>Create</Button>
		</div>
	</form>
</Dialog>

<ConfirmDialog
	bind:open={showDelete}
	title="Delete this location?"
	message="Its test IPs, iperf endpoints, files, agent, and tokens are all removed. This cannot be undone."
	confirmLabel="Delete location"
	danger
	busy={deleting}
	onconfirm={confirmDelete}
/>

<ConfirmDialog
	bind:open={showRevoke}
	title="Revoke this agent?"
	message="The live tunnel is dropped and the location returns to not enrolled until a new agent enrolls."
	confirmLabel="Revoke agent"
	danger
	busy={revoking}
	onconfirm={confirmRevoke}
/>
