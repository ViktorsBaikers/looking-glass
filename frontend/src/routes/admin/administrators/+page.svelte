<script lang="ts">
	import { onMount } from 'svelte';
	import Add from '~icons/material-symbols/add';
	import Refresh from '~icons/material-symbols/refresh';
	import Trash from '~icons/material-symbols/delete';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import Field from '$lib/components/ui/field.svelte';
	import {
		Card,
		CardContent,
		CardDescription,
		CardHeader,
		CardTitle
	} from '$lib/components/ui/card/index.js';
	import Badge from '$lib/components/ui/badge.svelte';
	import StatusBadge from '$lib/components/ui/status-badge.svelte';
	import Dialog from '$lib/components/ui/dialog.svelte';
	import ConfirmDialog from '$lib/components/ui/confirm-dialog.svelte';
	import CopyButton from '$lib/components/ui/copy-button.svelte';
	import { toast } from '$lib/toast.svelte.js';
	import {
		changePassword,
		createAdministrator,
		getMe,
		listAdministrators,
		regenerateActivation,
		removeAdministrator
	} from '$lib/admin/api.js';
	import type { ActivationLink, Administrator } from '$lib/admin/types.js';
	import { activationLink, activationNote, activationRow } from '$lib/auth/styles.js';
	import {
		accountCount,
		addField,
		addRow,
		pageHeader,
		pageSub,
		pageTitle,
		pageWrap,
		passwordActions,
		passwordGrid,
		peerActions,
		peerAdded,
		peerList,
		peerMain,
		peerName,
		peerNameRow,
		peerRow,
		peersHead,
		sectionDesc
	} from './styles.js';

	const MIN_PASSWORD = 12;

	let meId = $state('');
	let admins = $state<Administrator[]>([]);
	let phase = $state<'loading' | 'ready' | 'error'>('loading');

	// The one-time link from create/regenerate; shown once, discarded on close.
	let link = $state<ActivationLink | null>(null);
	let showLink = $state(false);

	let newUsername = $state('');
	let creating = $state(false);

	let target = $state<Administrator | null>(null);
	let showRegenerate = $state(false);
	let regenerating = $state(false);
	let showRemove = $state(false);
	let removing = $state(false);

	let current = $state('');
	let next = $state('');
	let confirmNext = $state('');
	let changing = $state(false);

	const nextError = $derived(
		next.length > 0 && next.length < MIN_PASSWORD ? `At least ${MIN_PASSWORD} characters.` : ''
	);
	const confirmError = $derived(confirmNext.length > 0 && confirmNext !== next ? 'Passwords do not match.' : '');

	const countLabel = $derived(`${admins.length} account${admins.length === 1 ? '' : 's'}`);

	onMount(async () => {
		const [me, list] = await Promise.all([getMe(), listAdministrators()]);
		if (!me.ok || !list.ok) {
			phase = 'error';
			return;
		}
		meId = me.data.id;
		admins = list.data;
		phase = 'ready';
	});

	function addedLabel(created_at: number) {
		return `Added ${new Date(created_at * 1000).toLocaleDateString()}`;
	}

	async function reload() {
		const list = await listAdministrators();
		if (list.ok) admins = list.data;
	}

	async function submitCreate(event: SubmitEvent) {
		event.preventDefault();
		if (creating || newUsername.length === 0) return;
		creating = true;
		const result = await createAdministrator(newUsername);
		creating = false;
		if (!result.ok) {
			toast.error(result.message);
			return;
		}
		newUsername = '';
		link = result.data;
		showLink = true;
		await reload();
	}

	function openRegenerate(admin: Administrator) {
		target = admin;
		showRegenerate = true;
	}

	async function confirmRegenerate() {
		if (!target) return;
		regenerating = true;
		const result = await regenerateActivation(target.id);
		regenerating = false;
		showRegenerate = false;
		if (!result.ok) {
			toast.error(result.message);
			return;
		}
		// The closing confirm dialog restores focus on its next tick; opening the
		// link dialog in the same tick makes its dismissable layer read that
		// restore as an outside interaction and close it at once.
		setTimeout(() => {
			link = result.data;
			showLink = true;
		}, 0);
		await reload();
	}

	function openRemove(admin: Administrator) {
		target = admin;
		showRemove = true;
	}

	async function confirmRemove() {
		if (!target) return;
		removing = true;
		const result = await removeAdministrator(target.id);
		removing = false;
		showRemove = false;
		if (!result.ok) {
			toast.error(result.message);
			return;
		}
		toast.success(`Removed ${target.username}.`);
		target = null;
		await reload();
	}

	async function submitPassword(event: SubmitEvent) {
		event.preventDefault();
		if (changing || current.length === 0 || nextError || confirmError || confirmNext !== next) return;
		changing = true;
		const result = await changePassword(current, next);
		changing = false;
		if (!result.ok) {
			toast.error(result.message);
			return;
		}
		current = '';
		next = '';
		confirmNext = '';
		toast.success('Password changed.');
	}
</script>

<div class={pageWrap}>
	<header class={pageHeader}>
		<h1 class={pageTitle}>Administrators</h1>
		<p class={pageSub}>Manage equal peers and your own password. One-time links disappear when closed.</p>
	</header>

	<Card>
		<CardHeader>
			<CardTitle>Add an administrator</CardTitle>
			<CardDescription>They choose their password from the one-time activation link.</CardDescription>
		</CardHeader>
		<CardContent>
			<form class={addRow} onsubmit={submitCreate}>
				<div class={addField}>
					<Field label="Username" for="new-admin-username">
						<Input
							id="new-admin-username"
							placeholder="Enter username"
							autocomplete="off"
							bind:value={newUsername}
							disabled={creating}
							required
						/>
					</Field>
				</div>
				<Button type="submit" loading={creating} disabled={newUsername.length === 0}>
					<Add aria-hidden="true" />
					Create pending
				</Button>
			</form>
		</CardContent>
	</Card>

	<Card>
		<div class={peersHead}>
			<CardTitle>Administrator peers</CardTitle>
			<span class={accountCount}>{countLabel}</span>
		</div>
		{#if phase === 'loading'}
			<p class={sectionDesc}>Loading…</p>
		{:else if phase === 'error'}
			<p class={sectionDesc}>Could not load administrators.</p>
		{:else}
			<ul class={peerList}>
				{#each admins as admin (admin.id)}
					<li class={peerRow}>
						<div class={peerMain}>
							<div class={peerNameRow}>
								<span class={peerName}>{admin.username}</span>
								{#if admin.id === meId}
									<Badge>You</Badge>
								{/if}
								{#if admin.status === 'active'}
									<StatusBadge tone="success">Active</StatusBadge>
								{:else}
									<StatusBadge tone="warning">Pending</StatusBadge>
								{/if}
							</div>
							<span class={peerAdded}>{addedLabel(admin.created_at)}</span>
						</div>
						<div class={peerActions}>
							{#if admin.status === 'pending'}
								<Button variant="secondary" size="sm" onclick={() => openRegenerate(admin)}>
									<Refresh aria-hidden="true" />
									Regenerate
								</Button>
							{/if}
							<Button variant="danger" size="sm" onclick={() => openRemove(admin)}>
								<Trash aria-hidden="true" />
								Remove
							</Button>
						</div>
					</li>
				{/each}
			</ul>
		{/if}
	</Card>

	<Card>
		<CardHeader>
			<CardTitle>Change your password</CardTitle>
			<CardDescription>Your current password is required.</CardDescription>
		</CardHeader>
		<CardContent>
			<form onsubmit={submitPassword} novalidate>
				<div class={passwordGrid}>
					<Field label="Current password" for="current-password">
						<Input
							id="current-password"
							type="password"
							autocomplete="current-password"
							bind:value={current}
							disabled={changing}
							required
						/>
					</Field>
					<Field label="New password" for="new-password" error={nextError}>
						<Input
							id="new-password"
							type="password"
							autocomplete="new-password"
							bind:value={next}
							disabled={changing}
							invalid={!!nextError}
							required
						/>
					</Field>
					<Field label="Confirm new password" for="confirm-password" error={confirmError}>
						<Input
							id="confirm-password"
							type="password"
							autocomplete="new-password"
							bind:value={confirmNext}
							disabled={changing}
							invalid={!!confirmError}
							required
						/>
					</Field>
				</div>
				<div class={passwordActions}>
					<Button type="submit" loading={changing}>Change password</Button>
				</div>
			</form>
		</CardContent>
	</Card>
</div>

<Dialog
	bind:open={showLink}
	title={link ? `Activation link for ${link.administrator.username}` : 'Activation link'}
	description="Share it now — this is the only time it is shown."
	onclose={() => (link = null)}
>
	{#if link}
		<p class={activationLink}>{link.activation_url}</p>
		<div class={activationRow}>
			<span class={activationNote}>Expires in 24 hours and works once.</span>
			<CopyButton text={link.activation_url} label="activation link" />
		</div>
	{/if}
</Dialog>

<ConfirmDialog
	bind:open={showRegenerate}
	title="Regenerate activation link?"
	message={target ? `The previous link for ${target.username} stops working immediately.` : ''}
	confirmLabel="Regenerate"
	busy={regenerating}
	onconfirm={confirmRegenerate}
/>

<ConfirmDialog
	bind:open={showRemove}
	title={target ? `Remove ${target.username}?` : 'Remove administrator?'}
	message="Their sessions end immediately. This cannot be undone."
	confirmLabel="Remove"
	danger
	busy={removing}
	onconfirm={confirmRemove}
/>
