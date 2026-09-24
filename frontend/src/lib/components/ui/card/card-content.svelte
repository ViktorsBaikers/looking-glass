<script lang="ts">
	import type { HTMLAttributes } from 'svelte/elements';
	import type { Snippet } from 'svelte';
	import { cx } from 'styled-system/css';
	import { card } from 'styled-system/recipes';

	let {
		class: className,
		children,
		...rest
	}: Omit<HTMLAttributes<HTMLDivElement>, 'class'> & { class?: string; children?: Snippet } =
		$props();
</script>

<div class={cx(card().content, className)} {...rest}>
	{@render children?.()}
</div>
