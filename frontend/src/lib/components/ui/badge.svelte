<script lang="ts" module>
	export type BadgeTone = 'neutral' | 'success' | 'warning' | 'danger';
</script>

<script lang="ts">
	import type { Snippet } from 'svelte';
	import { cx } from 'styled-system/css';
	import { badge } from 'styled-system/recipes';

	let {
		tone = 'neutral',
		class: className,
		children
	}: { tone?: BadgeTone; class?: string; children?: Snippet } = $props();
</script>

<span class={cx(badge({ tone }), className)}>
	{@render children?.()}
</span>
