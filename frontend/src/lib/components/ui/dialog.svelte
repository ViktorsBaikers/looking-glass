<script lang="ts">
	import type { Snippet } from 'svelte';
	import { Dialog, Portal } from '@ark-ui/svelte';
	import { cx } from 'styled-system/css';
	import { dialog } from 'styled-system/recipes';
	import Close from '~icons/material-symbols/close';

	let {
		open = $bindable(false),
		title,
		description,
		onclose,
		preventClose = false,
		class: className,
		children
	}: {
		open?: boolean;
		title: string;
		description?: string;
		onclose?: () => void;
		preventClose?: boolean;
		class?: string;
		children: Snippet;
	} = $props();

	const s = dialog();
</script>

<Dialog.Root
	bind:open
	closeOnEscape={!preventClose}
	closeOnInteractOutside={!preventClose}
	onOpenChange={(e) => {
		if (!e.open) onclose?.();
	}}
>
	<Portal>
		<Dialog.Backdrop class={s.backdrop} />
		<Dialog.Positioner class={s.positioner}>
			<Dialog.Content class={cx(s.content, className)}>
				<Dialog.Title class={s.title}>{title}</Dialog.Title>
				{#if description}
					<Dialog.Description class={s.description}>{description}</Dialog.Description>
				{/if}
				<Dialog.CloseTrigger class={s.closeTrigger} aria-label="Close dialog">
					<Close />
				</Dialog.CloseTrigger>
				{@render children()}
			</Dialog.Content>
		</Dialog.Positioner>
	</Portal>
</Dialog.Root>
