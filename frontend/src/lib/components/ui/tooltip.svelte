<script lang="ts">
	import type { Snippet } from 'svelte';
	import { Tooltip, Portal } from '@ark-ui/svelte';
	import { tooltip } from 'styled-system/recipes';

	/**
	 * Wrap a focusable/hoverable trigger (children) with a text tooltip. Ark
	 * handles the hover/focus/Escape behaviour and ARIA.
	 */
	let {
		content,
		disabled = false,
		children
	}: { content: string; disabled?: boolean; children: Snippet } = $props();

	const s = tooltip();
</script>

<Tooltip.Root {disabled}>
	<Tooltip.Trigger class={s.trigger}>{@render children()}</Tooltip.Trigger>
	<Portal>
		<Tooltip.Positioner class={s.positioner}>
			<Tooltip.Content class={s.content}>{content}</Tooltip.Content>
		</Tooltip.Positioner>
	</Portal>
</Tooltip.Root>
