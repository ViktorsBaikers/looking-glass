<script lang="ts">
	import { cn } from '$lib/utils.js';

	let {
		tabs,
		active = $bindable()
	}: {
		tabs: { id: string; label: string }[];
		active: string;
	} = $props();
</script>

<div
	role="tablist"
	class="flex flex-wrap gap-1 border-b border-border"
	aria-label="Location editor sections"
>
	{#each tabs as tab (tab.id)}
		<button
			type="button"
			role="tab"
			id="tab-{tab.id}"
			aria-selected={active === tab.id}
			aria-controls="panel-{tab.id}"
			class={cn(
				'-mb-px min-h-11 border-b-2 px-3 py-2 text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring',
				active === tab.id
					? 'border-primary text-foreground'
					: 'border-transparent text-muted-foreground hover:text-foreground'
			)}
			onclick={() => (active = tab.id)}
		>
			{tab.label}
		</button>
	{/each}
</div>
