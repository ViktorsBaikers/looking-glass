<script lang="ts">
	import LoaderCircle from '@lucide/svelte/icons/loader-circle';
	import RefreshCw from '@lucide/svelte/icons/refresh-cw';
	import CircleCheck from '@lucide/svelte/icons/circle-check';
	import { Button } from '$lib/components/ui/button/index.js';
	import Dialog from '$lib/components/ui/dialog.svelte';
	import CopyButton from '$lib/public/CopyButton.svelte';
	import { createEnrollment, getLocation } from './api.js';
	import type { EnrollmentTicket } from './types.js';

	let {
		open = $bindable(false),
		locationId,
		locationName,
		onclose
	}: {
		open?: boolean;
		locationId: string;
		locationName: string;
		onclose: () => void;
	} = $props();

	let phase = $state<'loading' | 'ready' | 'error'>('loading');
	let ticket = $state<EnrollmentTicket | null>(null);
	let now = $state(Date.now());
	let connected = $state(false);
	let requestGeneration = 0;

	// Fetch a ticket the first time the dialog opens; tick the countdown while it is.
	$effect(() => {
		if (open && phase === 'loading' && !ticket) void generate();
	});

	$effect(() => {
		if (!open) return;
		const id = setInterval(() => (now = Date.now()), 1000);
		return () => clearInterval(id);
	});

	// While the dialog is open, poll the location's derived status; it flips online
	// once the agent dials home over the tunnel (Slice 8b). Stop once connected.
	$effect(() => {
		if (!open || connected) return;
		let cancelled = false;
		const id = setInterval(async () => {
			const result = await getLocation(locationId);
			if (!cancelled && result.ok && result.data.status === 'online') connected = true;
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
		if (!open || currentGeneration !== requestGeneration) return;
		if (result.ok) {
			ticket = result.data;
			now = Date.now();
			phase = 'ready';
		} else {
			phase = 'error';
		}
	}

	function regenerate() {
		ticket = null;
		void generate();
	}

	function cleanup() {
		requestGeneration += 1;
		ticket = null;
		phase = 'loading';
		connected = false;
		onclose();
	}

	const remainingMs = $derived(ticket ? ticket.expires_at * 1000 - now : 0);
	const expired = $derived(ticket !== null && remainingMs <= 0);
	const countdown = $derived.by(() => {
		const total = Math.max(0, Math.floor(remainingMs / 1000));
		const mm = String(Math.floor(total / 60)).padStart(2, '0');
		const ss = String(total % 60).padStart(2, '0');
		return `${mm}:${ss}`;
	});
</script>

<Dialog
	bind:open
	title="Enroll {locationName}"
	description="Run this command on the remote host. It carries a single-use token and central's identity — nothing to edit."
	onclose={cleanup}
>
	{#if phase === 'loading'}
		<div class="flex items-center gap-2 py-6 text-muted-foreground">
			<LoaderCircle class="size-5 animate-spin" aria-hidden="true" />
			<span>Generating enrollment token…</span>
		</div>
	{:else if phase === 'error'}
		<div class="space-y-4 py-2">
			<p class="text-sm text-destructive" role="alert">
				The enrollment token could not be generated.
			</p>
			<Button variant="outline" size="sm" onclick={generate}>Try again</Button>
		</div>
	{:else if ticket}
		<div class="space-y-4">
			<div class="space-y-2">
				<div class="flex items-center justify-between">
					<span class="text-sm font-medium">Install command</span>
					<CopyButton text={ticket.install_command} label="install command" />
				</div>
				<pre
					class="overflow-x-auto rounded-md border border-border bg-muted/50 p-3 font-mono text-xs leading-relaxed"><code
						>{ticket.install_command}</code
					></pre>
			</div>

			<div
				class="flex items-center justify-between rounded-md border border-border px-3 py-2 text-sm"
				aria-live="polite"
			>
				{#if expired}
					<span class="text-destructive">Token expired — regenerate to enroll.</span>
				{:else}
					<span class="text-muted-foreground">
						Expires in <span class="font-mono tabular-nums">{countdown}</span>
					</span>
				{/if}
				<Button variant="ghost" size="sm" onclick={regenerate}>
					<RefreshCw class="size-4" aria-hidden="true" />
					Regenerate
				</Button>
			</div>

			<p class="flex items-center gap-2 text-sm" aria-live="polite">
				{#if connected}
					<CircleCheck class="size-4 text-status-online" aria-hidden="true" />
					<span class="text-status-online">Connected — {locationName} is online.</span>
				{:else}
					<LoaderCircle class="size-4 animate-spin text-muted-foreground" aria-hidden="true" />
					<span class="text-muted-foreground">
						Waiting for the agent to connect… the location comes online once it enrolls.
					</span>
				{/if}
			</p>

			<div class="flex justify-end">
					<Button type="button" onclick={() => (open = false)}>Done</Button>
			</div>
		</div>
	{/if}
</Dialog>
