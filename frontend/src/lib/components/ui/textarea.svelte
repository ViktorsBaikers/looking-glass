<script lang="ts">
	import type { HTMLTextareaAttributes } from 'svelte/elements';
	import { cx } from 'styled-system/css';
	import { input } from 'styled-system/recipes';
	import { textareaArea as area } from '$lib/styles.js';

	let {
		class: className,
		value = $bindable(),
		invalid = false,
		mono = false,
		...rest
	}: Omit<HTMLTextareaAttributes, 'class'> & { class?: string; invalid?: boolean; mono?: boolean } =
		$props();
</script>

<textarea
	bind:value
	class={cx(input({ invalid, mono }), area, className)}
	aria-invalid={invalid || undefined}
	{...rest}
></textarea>
