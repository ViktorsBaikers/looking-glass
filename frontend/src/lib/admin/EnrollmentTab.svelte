<script lang="ts">
	import { Button } from '$lib/components/ui/button/index.js';
	import ConfirmDialog from '$lib/components/ui/confirm-dialog.svelte';
	import CopyButton from '$lib/components/ui/copy-button.svelte';
	import { toast } from '$lib/toast.svelte.js';
	import {
		enrollIntro,
		enrollTitle,
		retryBtn,
		cmdBox,
		cmdCopy,
		enrollRow,
		countdownText,
		countdownMono,
		expiredText,
		connectedRow,
		connectedOk,
		waitingMuted,
		revokeRow,
		loadingRow,
		errorText,
		spinIcon
	} from './editor-styles.js';
	import { createEnrollment, getLocation, revokeAgent } from './api.js';
	import { formatCountdown } from './editor.js';
	import type { EnrollmentTicket } from './types.js';
	import Spinner from '~icons/material-symbols/progress-activity';
	import Refresh from '~icons/material-symbols/refresh';
	import CheckCircle from '~icons/material-symbols/check-circle';
	import LinkOff from '~icons/material-symbols/link-off';

	let {
		locationId,
		locationName,
		kind,
		online,
		onchanged
	}: {
		locationId: string;
		locationName: string;
		kind: 'local' | 'remote';
		/** Live-derived status of the location; flips once the agent dials home. */
		online: boolean;
		onchanged: () => void;
	} = $props();

	let phase = $state<'loading' | 'ready' | 'error'>('loading');
	let ticket = $state<EnrollmentTicket | null>(null);
	let now = $state(Date.now());
	let revoking = $state(false);
	let askRevoke = $state(false);
	let requestGeneration = 0;

	// Mint a ticket when the tab first renders; tick the countdown while shown.
	$effect(() => {
		if (kind !== 'remote') return;
		if (phase === 'loading' && !ticket) void generate();
		const id = setInterval(() => (now = Date.now()), 1000);
		return () => clearInterval(id);
	});

	// Poll the derived status so "Connected" appears live once the agent enrolls;
	// the parent reloads the location, which flips `online`.
	$effect(() => {
		if (kind !== 'remote' || online) return;
		let cancelled = false;
		const id = setInterval(async () => {
			const result = await getLocation(locationId);
			if (!cancelled && result.ok && result.data.status === 'online') onchanged();
		}, 3000);
		return () => {
			cancelled = true;
			clearInterval(id);
		};
	});

	async function generate() {
		const currentGeneration = ++requestGeneration;
		phase = 'loading';
		const result = await createEnrollment(locationId);
		if (currentGeneration !== requestGeneration) return;
		if (result.ok) {
			ticket = result.data;
			now = Date.now();
			phase = 'ready';
		} else {
			phase = 'error';
		}
	}

	function regenerate() {
		requestGeneration += 1;
		ticket = null;
		phase = 'loading';
		void generate();
	}

	async function confirmRevoke() {
		if (revoking) return;
		revoking = true;
		const result = await revokeAgent(locationId);
		revoking = false;
		if (result.ok) {
			askRevoke = false;
			toast.success('Agent revoked.');
			onchanged();
		} else {
			toast.error(result.message);
		}
	}

	const remainingMs = $derived(ticket ? ticket.expires_at * 1000 - now : 0);
	const expired = $derived(ticket !== null && remainingMs <= 0);
</script>

{#if kind === 'local'}
	<p class={errorText}>Enrollment applies only to remote locations.</p>
{:else if phase === 'loading'}
	<p class={loadingRow}>
		<Spinner class={spinIcon} aria-hidden="true" />
		Generating enrollment token…
	</p>
{:else if phase === 'error'}
	<div>
		<p class={errorText} role="alert">The enrollment token could not be generated.</p>
		<Button variant="secondary" size="sm" onclick={regenerate} class={retryBtn}>Try again</Button>
	</div>
{:else if ticket}
	<h3 class={enrollTitle}>Agent Enrollment</h3>
	<p class={enrollIntro}>
		Use the following command to enroll a remote agent for this location. It carries a
		single-use token and central's identity — nothing to edit.
	</p>
	<div class={cmdBox}>
		<code>{ticket.install_command}</code>
		<CopyButton text={ticket.install_command} label="install command" class={cmdCopy} />
	</div>

	<div class={enrollRow} aria-live="polite">
		{#if expired}
			<span class={expiredText}>Token expired — regenerate to enroll.</span>
		{:else}
			<span class={countdownText}>
				Expires in <span class={countdownMono}>{formatCountdown(remainingMs)}</span>
			</span>
		{/if}
		<Button variant="ghost" size="sm" onclick={regenerate}>
			<Refresh aria-hidden="true" />
			Regenerate
		</Button>
	</div>

	<p class={connectedRow} aria-live="polite">
		{#if online}
			<span class={connectedOk}>
				<CheckCircle aria-hidden="true" />
				Connected — {locationName} is online.
			</span>
		{:else}
			<span class={waitingMuted}>
				<Spinner class={spinIcon} aria-hidden="true" />
				Waiting for the agent to connect… the location comes online once it enrolls.
			</span>
		{/if}
	</p>

	{#if online}
		<div class={revokeRow}>
			<Button variant="danger" onclick={() => (askRevoke = true)}>
				<LinkOff aria-hidden="true" />
				Revoke agent
			</Button>
		</div>
	{/if}
{/if}

<ConfirmDialog
	bind:open={askRevoke}
	title="Revoke agent?"
	message="The agent's credential is invalidated immediately and the location goes offline."
	confirmLabel="Revoke"
	danger
	busy={revoking}
	onconfirm={confirmRevoke}
/>
