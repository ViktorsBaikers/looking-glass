<script lang="ts">
	import type { Snippet } from 'svelte';
	import { cn } from '$lib/utils.js';

	let {
		open = $bindable(false),
		title,
		description,
		onclose: onDialogClose = () => {},
		preventClose = false,
		class: className,
		children
	}: {
		open?: boolean;
		title: string;
		description?: string;
		onclose?: () => void;
		preventClose?: boolean;
		class?: string;
		children: Snippet;
	} = $props();

	let el: HTMLDialogElement | undefined = $state();

	// Drive the native <dialog> from `open` so it brings the focus trap, Escape
	// handling, and inert background that an accessible modal needs for free.
	$effect(() => {
		if (!el) return;
		if (open && !el.open) el.showModal();
		else if (!open && el.open) el.close();
	});
</script>

<dialog
	bind:this={el}
	oncancel={(event) => {
		if (preventClose) event.preventDefault();
	}}
	onclose={() => {
		open = false;
		onDialogClose();
	}}
	aria-label={title}
	class={cn(
		'w-full max-w-lg rounded-lg border border-border bg-card p-6 text-card-foreground shadow-lg backdrop:bg-black/50',
		className
	)}
>
	{#if open}
		<div class="mb-4 space-y-1">
			<h2 class="text-lg font-semibold tracking-tight">{title}</h2>
			{#if description}
				<p class="text-sm text-muted-foreground">{description}</p>
			{/if}
		</div>
		{@render children()}
	{/if}
</dialog>
