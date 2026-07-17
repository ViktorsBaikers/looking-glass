<script lang="ts">
	import { onMount } from 'svelte';
	import LoaderCircle from '@lucide/svelte/icons/loader-circle';
	import Plus from '@lucide/svelte/icons/plus';
	import Pencil from '@lucide/svelte/icons/pencil';
	import Trash2 from '@lucide/svelte/icons/trash-2';
	import Unplug from '@lucide/svelte/icons/unplug';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import Dialog from '$lib/components/ui/dialog.svelte';
	import LocationEditor from '$lib/admin/LocationEditor.svelte';
	import EnrollDialog from '$lib/admin/EnrollDialog.svelte';
	import { toaster } from '$lib/toast.svelte.js';
	import {
		listLocations,
		createLocation,
		deleteLocation,
		revokeAgent,
		type LocationInput
	} from '$lib/admin/api.js';
	import { formatLastSeen } from '$lib/admin/lastSeen.js';
	import type { Location, NodeKind } from '$lib/admin/types.js';

	let phase = $state<'loading' | 'ready' | 'error'>('loading');
	let locations = $state<Location[]>([]);
	let editingId = $state<string | null>(null);

	// Ticks once a minute so the relative last-seen labels stay current without a
	// per-second re-render; the derived online/offline status itself comes from the API.
	let nowMs = $state(Date.now());
	$effect(() => {
		const id = setInterval(() => (nowMs = Date.now()), 60_000);
		return () => clearInterval(id);
	});

	let showCreate = $state(false);
	let draft = $state<{ name: string; geo_label: string; kind: NodeKind }>({
		name: '',
		geo_label: '',
		kind: 'local'
	});
	let creating = $state(false);
	let createError = $state('');

	let pendingDelete = $state<Location | null>(null);
	let showDelete = $state(false);
	let deleting = $state(false);
	let deleteError = $state('');
	let pendingRevoke = $state<Location | null>(null);
	let showRevoke = $state(false);
	let revoking = $state(false);
	let revokeError = $state('');

	let enrollTarget = $state<{ id: string; name: string } | null>(null);
	let showEnroll = $state(false);

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
			kind: draft.kind,
			data_plane_origin: null,
			offered_methods: []
		};
		const result = await createLocation(body);
		creating = false;
		if (result.ok) {
			showCreate = false;
			toaster.success('Location created.');
			await load();
			// A remote location needs an agent — show its enrollment command before
			// dropping into the editor. A local node runs on the built-in node, so it
			// goes straight to configuration.
			if (result.data.kind === 'remote') {
				enrollTarget = { id: result.data.id, name: result.data.name };
				showEnroll = true;
			} else {
				editingId = result.data.id;
			}
		} else {
			createError = result.message;
		}
	}

	function askDelete(location: Location) {
		pendingDelete = location;
		deleteError = '';
		showDelete = true;
	}

	function askRevoke(location: Location) {
		pendingRevoke = location;
		revokeError = '';
		showRevoke = true;
	}

	async function confirmDelete() {
		if (!pendingDelete || deleting) return;
		const location = pendingDelete;
		deleting = true;
		try {
			const result = await deleteLocation(location.id);
			if (result.ok) {
				showDelete = false;
				pendingDelete = null;
				toaster.success(`Deleted ${location.name} and everything under it.`);
				await load();
			} else {
				deleteError = result.message;
			}
		} catch {
			deleteError = 'The request could not be completed.';
		} finally {
			deleting = false;
		}
	}

	async function confirmRevoke() {
		if (!pendingRevoke || revoking) return;
		const location = pendingRevoke;
		revoking = true;
		try {
			const result = await revokeAgent(location.id);
			if (result.ok) {
				showRevoke = false;
				pendingRevoke = null;
				toaster.success(`Revoked ${location.name}'s agent.`);
				await load();
			} else {
				revokeError = result.message;
			}
		} catch {
			revokeError = 'The request could not be completed.';
		} finally {
			revoking = false;
		}
	}

	function statusLabel(location: Location) {
		if (location.kind === 'remote' && location.status === 'offline' && !location.last_seen) {
			return 'Not enrolled';
		}
		return location.status === 'online' ? 'Online' : 'Offline';
	}
</script>

{#if editingId}
	<LocationEditor
		locationId={editingId}
		onclose={() => {
			editingId = null;
			void load();
		}}
	/>
{:else}
	<div class="space-y-6">
		<div class="flex flex-col items-stretch gap-3 sm:flex-row sm:items-center sm:justify-between">
			<div>
				<h1 class="text-xl font-semibold tracking-tight">Locations</h1>
				<p class="text-sm text-muted-foreground">Nodes visitors can run diagnostics from.</p>
			</div>
			<Button class="sm:w-auto" onclick={openCreate}>
				<Plus class="size-4" aria-hidden="true" />
				Add location
			</Button>
		</div>

		{#if phase === 'loading'}
			<div class="space-y-2" aria-hidden="true">
				{#each { length: 3 } as _, i (i)}
					<div class="h-16 animate-pulse rounded-md border border-border bg-muted/40"></div>
				{/each}
			</div>
			<p class="sr-only" aria-live="polite">Loading locations…</p>
		{:else if phase === 'error'}
			<div class="rounded-md border border-destructive/40 px-4 py-6 text-sm text-destructive" role="alert">
				<p>Locations could not be loaded.</p>
				<Button variant="outline" size="sm" class="mt-3" onclick={load}>Try again</Button>
			</div>
		{:else if locations.length === 0}
			<div class="rounded-md border border-dashed border-border px-4 py-12 text-center">
				<p class="text-sm text-muted-foreground">No locations yet — add your first.</p>
				<Button class="mt-4" onclick={openCreate}>
					<Plus class="size-4" aria-hidden="true" />
					Add location
				</Button>
			</div>
		{:else}
			<ul class="space-y-2">
				{#each locations as location (location.id)}
					<li
						class="grid grid-cols-1 gap-3 rounded-md border border-border p-4 sm:grid-cols-[1fr_auto_auto_auto_auto] sm:items-center"
					>
						<div class="min-w-0">
							<p class="truncate font-medium">{location.name}</p>
							<p class="truncate text-sm text-muted-foreground">
								{location.geo_label || '—'} · {location.kind}
							</p>
						</div>
						<span class="flex items-center gap-2 text-sm">
							<span
								class="inline-block size-2 rounded-full"
								class:bg-status-online={location.status === 'online'}
								class:bg-status-offline={location.status === 'offline'}
								aria-hidden="true"
							></span>
							{statusLabel(location)}
						</span>
						<span class="text-sm text-muted-foreground tabular-nums">
							{location.kind === 'local' ? '—' : formatLastSeen(location.last_seen, nowMs)}
						</span>
						<span class="text-sm text-muted-foreground">
							{location.offered_methods.length} method{location.offered_methods.length === 1
								? ''
								: 's'}
						</span>
						<div class="flex gap-1 justify-self-end">
							{#if location.kind === 'remote'}
								<Button
									size="sm"
									variant="outline"
									onclick={() => askRevoke(location)}
								>
									<Unplug class="size-4" aria-hidden="true" />
									Revoke
								</Button>
							{/if}
							<Button
								size="sm"
								variant="outline"
								onclick={() => (editingId = location.id)}
							>
								<Pencil class="size-4" aria-hidden="true" />
								Edit
							</Button>
							<Button
								size="icon"
								variant="ghost"
								onclick={() => askDelete(location)}
								aria-label="Delete {location.name}"
							>
								<Trash2 class="size-4 text-destructive" aria-hidden="true" />
							</Button>
						</div>
					</li>
				{/each}
			</ul>
		{/if}
	</div>
{/if}

<Dialog
	bind:open={showCreate}
	title="Add location"
	description="Name the location; add its IPs, files, and methods next."
>
	<form class="space-y-4" onsubmit={submitCreate} novalidate>
		<div class="space-y-2">
			<Label for="new-name">Display name</Label>
			<Input id="new-name" bind:value={draft.name} disabled={creating} required />
		</div>
		<div class="space-y-2">
			<Label for="new-geo">Geographic label</Label>
			<Input id="new-geo" bind:value={draft.geo_label} placeholder="Frankfurt, DE" disabled={creating} />
		</div>
		<div class="space-y-2">
			<Label for="new-kind">Node kind</Label>
			<select
				id="new-kind"
				bind:value={draft.kind}
				disabled={creating}
				class="h-11 w-full rounded-md border border-input bg-background px-3 text-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
			>
				<option value="local">Local (built-in node)</option>
				<option value="remote">Remote (enrolled agent)</option>
			</select>
		</div>
		{#if createError}
			<p class="text-sm text-destructive" role="alert">{createError}</p>
		{/if}
		<div class="flex justify-end gap-2">
			<Button type="button" variant="ghost" onclick={() => (showCreate = false)} disabled={creating}>
				Cancel
			</Button>
			<Button type="submit" disabled={creating}>{creating ? 'Creating…' : 'Create'}</Button>
		</div>
	</form>
</Dialog>

{#if enrollTarget}
	<EnrollDialog
		bind:open={showEnroll}
		locationId={enrollTarget.id}
		locationName={enrollTarget.name}
		onclose={() => {
			const id = enrollTarget?.id ?? null;
			enrollTarget = null;
			if (id) editingId = id;
		}}
	/>
{/if}

<Dialog
	bind:open={showDelete}
	preventClose={deleting}
	title="Delete this location?"
	description="Its test IPs, iperf endpoints, files, agent, and tokens are all removed. This cannot be undone."
>
	<div class="space-y-4">
		{#if deleteError}
			<p class="text-sm text-destructive" role="alert">{deleteError}</p>
		{/if}
		<div class="flex justify-end gap-2">
		<Button type="button" variant="ghost" onclick={() => (showDelete = false)} disabled={deleting}>
			Cancel
		</Button>
		<Button type="button" variant="destructive" onclick={confirmDelete} disabled={deleting}>
			{deleting ? 'Deleting…' : 'Delete location'}
		</Button>
		</div>
	</div>
</Dialog>

<Dialog
	bind:open={showRevoke}
	preventClose={revoking}
	title="Revoke this agent?"
	description="The live tunnel is dropped and the location returns to not enrolled until a new agent enrolls."
>
	<div class="space-y-4">
		{#if revokeError}
			<p class="text-sm text-destructive" role="alert">{revokeError}</p>
		{/if}
		<div class="flex justify-end gap-2">
		<Button type="button" variant="ghost" onclick={() => (showRevoke = false)} disabled={revoking}>
			Cancel
		</Button>
		<Button type="button" variant="destructive" onclick={confirmRevoke} disabled={revoking}>
			{revoking ? 'Revoking…' : 'Revoke agent'}
		</Button>
		</div>
	</div>
</Dialog>
