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

	// Zag ignores touch pointers and opens on focus only when focus is visible,
	// so a tap toggles the tooltip itself: the state is read at pointerdown,
	// before zag's own close lands in the DOM, and preventDefault stops zag's
	// close-on-click from undoing the open.
	let tapped = false;
	let wasOpen = false;
</script>

<!-- Tabbing to an off-screen trigger scrolls it into view; with the default
     closeOnScroll that scroll closes the tooltip the instant focus opened it,
     so keyboard users never saw it. Positioning follows the trigger anyway. -->
<Tooltip.Root {disabled} closeOnScroll={false}>
	<Tooltip.Context>
		{#snippet render(api)}
			<!-- type="button": the triggers sit inside forms (Settings), where a bare
			     <button> would submit the form. -->
			<Tooltip.Trigger
				type="button"
				class={s.trigger}
				onpointerdown={(event) => {
					tapped = event.pointerType === 'touch';
					wasOpen = event.currentTarget.dataset.state === 'open';
				}}
				onclick={(event) => {
					if (tapped && !disabled) {
						event.preventDefault();
						api().setOpen(!wasOpen);
					}
					tapped = false;
				}}>{@render children()}</Tooltip.Trigger
			>
		{/snippet}
	</Tooltip.Context>
	<Portal>
		<Tooltip.Positioner class={s.positioner}>
			<Tooltip.Content class={s.content}>{content}</Tooltip.Content>
		</Tooltip.Positioner>
	</Portal>
</Tooltip.Root>
