<script lang="ts">
	import X from '@lucide/svelte/icons/x';
	import { toaster } from '$lib/toast.svelte.js';
	import { cn } from '$lib/utils.js';
</script>

<div
	class="pointer-events-none fixed inset-x-0 bottom-4 z-50 flex flex-col items-center gap-2 px-4"
	aria-live="polite"
	aria-atomic="false"
>
	{#each toaster.items as toast (toast.id)}
		<div
			class={cn(
				'pointer-events-auto flex w-full max-w-sm items-start gap-3 rounded-md border px-4 py-3 text-sm shadow-md',
				toast.kind === 'success'
					? 'border-border bg-card text-card-foreground'
					: 'border-destructive/40 bg-card text-destructive'
			)}
			role={toast.kind === 'error' ? 'alert' : 'status'}
		>
			<span class="flex-1">{toast.message}</span>
			<button
				type="button"
				class="rounded text-muted-foreground hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
				onclick={() => toaster.dismiss(toast.id)}
				aria-label="Dismiss notification"
			>
				<X class="size-4" aria-hidden="true" />
			</button>
		</div>
	{/each}
</div>
