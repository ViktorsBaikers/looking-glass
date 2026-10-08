<script lang="ts">
	import { Button } from '$lib/components/ui/button/index.js';
	import ConfirmDialog from '$lib/components/ui/confirm-dialog.svelte';
	import CopyButton from '$lib/components/ui/copy-button.svelte';
	import { untrack } from 'svelte';
	import { toast } from '$lib/toast.svelte.js';
	import { srOnly } from '$lib/styles.js';
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
	import { getLocation, revokeAgent } from './api.js';
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
		onchanged,
		ticket,
		mint
	}: {
		locationId: string;
		locationName: string;
		kind: 'local' | 'remote';
		/** Live-derived status of the location; flips once the agent dials home. */
		online: boolean;
		onchanged: () => void;
		/** Owned by the editor so the tab remounting (every visit, every tab switch)
		 * reuses the live ticket instead of minting another. */
		ticket?: EnrollmentTicket;
		/** Mints under the location that requested it (a late mint never becomes
		 * another location's) and joins the one already in flight for it; a new
		 * mint drops the shown ticket. Resolves null once a ticket is stored, or
		 * why it was not (central's refusal, or a generic network message). */
		mint: (locationId: string) => Promise<string | null>;
	} = $props();

	let failed = $state<string | null>(null);
	// A mint in flight: the pressed button stays mounted and busy, so keyboard
	// focus never drops to <body>; the status region says what is happening.
	let busy = $state(false);
	let retried = $state(false);
	// Bumped per failure: the alert is re-inserted, so a repeat is announced.
	let failures = $state(0);
	let announcement = $state('');
	let now = $state(Date.now());
	let revoking = $state(false);
	let askRevoke = $state(false);
	let requestGeneration = 0;
	// The shown command's box, and its height when a Regenerate drops it: the
	// empty space stays, so Regenerate does not move under the pointer.
	let box = $state<HTMLElement | null>(null);
	let heldHeight = $state(0);

	// Mint a ticket exactly once when the tab first renders and the editor holds
	// none for this location (one-shot: the Regenerate button is the only other
	// path to `generate`, so a click can never double-POST); tick the countdown
	// while shown.
	let generated = false;
	$effect(() => {
		if (kind !== 'remote') return;
		if (!generated) {
			generated = true;
			if (!untrack(() => ticket)) void generate();
		}
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
		busy = true;
		if (retried) announcement = 'Generating enrollment token…';
		const failure = await mint(locationId);
		if (currentGeneration !== requestGeneration) return;
		busy = false;
		failed = failure;
		if (failure === null) now = Date.now();
		else failures++;
		// A failure is announced by its alert.
		if (retried) announcement = failure === null ? 'Enrollment token generated.' : '';
	}

	function regenerate() {
		heldHeight = box?.offsetHeight ?? 0;
		retried = true;
		void generate();
	}

	// A mint that settles in the other branch swaps Try again and Regenerate:
	// the new button takes the focus the pressed one had.
	const refocus = (button: HTMLElement) => {
		if (retried && (document.activeElement ?? document.body) === document.body) button.focus();
	};

	async function confirmRevoke() {
		if (revoking) return;
		revoking = true;
		const result = await revokeAgent(locationId);
		revoking = false;
		if (result.ok) {
			askRevoke = false;
			toast.success(`Revoked ${locationName}'s agent.`);
			// The shown ticket predates the revoke (and may be spent): offer a fresh one.
			regenerate();
			onchanged();
		} else {
			toast.error(result.message);
		}
	}

	const remainingMs = $derived(ticket ? ticket.expires_at * 1000 - now : 0);
	const expired = $derived(ticket !== undefined && remainingMs <= 0);
</script>

{#if kind === 'remote'}
	<p class={srOnly} role="status">{announcement}</p>
{/if}
{#if kind === 'local'}
	<p class={errorText}>Enrollment applies only to remote locations.</p>
{:else if failed !== null}
	<div>
		{#key failures}
			<p class={errorText} role="alert">The enrollment token could not be generated. {failed}</p>
		{/key}
		<Button
			variant="secondary"
			size="sm"
			onclick={regenerate}
			class={retryBtn}
			loading={busy}
			{@attach refocus}>Try again</Button
		>
	</div>
{:else if ticket || (busy && retried)}
	<!-- A Regenerate in flight keeps this branch (and its focused button) but
	     hides the old command: the mint has dropped it. -->
	<h3 class={enrollTitle}>Agent enrollment</h3>
	<p class={enrollIntro}>
		Run this install command on the remote node to enroll its agent with this location. It already
		holds a single-use token and this server's identity, so run it as is.
	</p>
	{#if ticket}
		<div class={cmdBox} bind:this={box}>
			<code>{ticket.install_command}</code>
			<CopyButton text={ticket.install_command} label="install command" class={cmdCopy} />
		</div>
	{:else if heldHeight}
		<div class={cmdBox} style="visibility: hidden; height: {heldHeight}px"></div>
	{/if}

	<div class={enrollRow}>
		<!-- The countdown ticks every second, so it stays outside the live region;
		     only the expiry itself is announced. -->
		<span>
			{#if ticket && !expired}
				<span class={countdownText}>
					Expires in <span class={countdownMono}>{formatCountdown(remainingMs)}</span>
				</span>
			{/if}
			<span aria-live="polite">
				{#if expired}<span class={expiredText}>Token expired — regenerate to enroll.</span>{/if}
			</span>
		</span>
		<Button variant="ghost" size="sm" onclick={regenerate} loading={busy} {@attach refocus}>
			{#if !busy}<Refresh aria-hidden="true" />{/if}
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
			<!-- Not while a mint runs: a revoke would join the pre-revoke mint and
			     show a token the revoke purged. Busy, not `disabled`, which blurs
			     a focused button; pointer-events keeps a click landing here. -->
			<Button
				variant="danger"
				style="pointer-events: auto"
				aria-disabled={busy || undefined}
				onclick={() => (askRevoke = !busy)}
			>
				<LinkOff aria-hidden="true" />
				Revoke agent
			</Button>
		</div>
	{/if}
{:else}
	<p class={loadingRow}>
		<Spinner class={spinIcon} aria-hidden="true" />
		Generating enrollment token…
	</p>
{/if}

<ConfirmDialog
	bind:open={askRevoke}
	title="Revoke this agent?"
	message="Its credential stops working and its tunnel closes now. The location shows Not enrolled until you enroll a new agent."
	confirmLabel="Revoke agent"
	danger
	busy={revoking}
	onconfirm={confirmRevoke}
/>
