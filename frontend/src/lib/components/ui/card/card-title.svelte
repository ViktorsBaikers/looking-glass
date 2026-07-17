<script lang="ts">
	import type { HTMLAttributes } from 'svelte/elements';
	import { cn } from '$lib/utils.js';

	let { class: className, children, ...rest }: HTMLAttributes<HTMLHeadingElement> = $props();
</script>

<h1 class={cn('text-lg font-semibold leading-none tracking-tight', className)} {...rest}>
	{@render children?.()}
</h1>
