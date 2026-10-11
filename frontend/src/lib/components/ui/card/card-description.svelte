<script lang="ts">
	import type { HTMLAttributes } from 'svelte/elements';
	import type { Snippet } from 'svelte';
	import { cx } from 'styled-system/css';
	import { card } from 'styled-system/recipes';

	let {
		class: className,
		children,
		...rest
	}: Omit<HTMLAttributes<HTMLParagraphElement>, 'class'> & { class?: string; children?: Snippet } =
		$props();
</script>

<p class={cx(card().description, className)} {...rest}>
	{@render children?.()}
</p>
