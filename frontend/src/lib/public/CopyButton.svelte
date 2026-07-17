<script lang="ts">
	import Copy from '@lucide/svelte/icons/copy';
	import Check from '@lucide/svelte/icons/check';
	import { cn } from '$lib/utils.js';

	let {
		text,
		label = 'Copy',
		class: className = ''
	}: { text: string; label?: string; class?: string } = $props();

	let copied = $state(false);
	let timer: ReturnType<typeof setTimeout> | undefined;

	async function copy() {
		try {
			await navigator.clipboard.writeText(text);
			copied = true;
			clearTimeout(timer);
			timer = setTimeout(() => (copied = false), 1500);
		} catch {
			// Clipboard is unavailable (older browser or insecure context) — the
			// value stays visible for a manual copy, so this fails quietly.
		}
	}
</script>

<button
	type="button"
	onclick={copy}
	class={cn(
		'inline-flex size-11 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-accent hover:text-accent-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring',
		className
	)}
	aria-label={copied ? `Copied ${label}` : `Copy ${label}`}
>
	{#if copied}
		<Check class="size-4 text-status-online" aria-hidden="true" />
	{:else}
		<Copy class="size-4" aria-hidden="true" />
	{/if}
</button>
<span class="sr-only" aria-live="polite">{copied ? 'Copied' : ''}</span>
