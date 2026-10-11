<script lang="ts">
	import type { HTMLInputAttributes } from 'svelte/elements';
	import { cx } from 'styled-system/css';
	import { input } from 'styled-system/recipes';

	let {
		class: className,
		value = $bindable(),
		invalid = false,
		mono = false,
		...rest
	}: Omit<HTMLInputAttributes, 'class'> & {
		class?: string;
		/** Red border + error focus ring; also sets aria-invalid. */
		invalid?: boolean;
		/** Monospace face for IPs, ports, CIDR, commands. */
		mono?: boolean;
	} = $props();
</script>

<input
	bind:value
	class={cx(input({ invalid, mono }), className)}
	aria-invalid={invalid || undefined}
	{...rest}
/>
