<script lang="ts">
	import type { HTMLAttributes } from 'svelte/elements';
	import type { Snippet } from 'svelte';
	import { cx } from 'styled-system/css';
	import { card } from 'styled-system/recipes';

	let {
		class: className,
		children,
		...rest
	}: Omit<HTMLAttributes<HTMLHeadingElement>, 'class'> & { class?: string; children?: Snippet } =
		$props();
</script>

<h2 class={cx(card().title, className)} {...rest}>
	{@render children?.()}
</h2>
