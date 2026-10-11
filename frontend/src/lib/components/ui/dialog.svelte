<script module lang="ts">
	// When frames are slow (WebKit stops painting while a modal is open), the
	// close animation can end before Ark listens for its end. The closed dialog
	// then stays in the page and onexitcomplete never runs (F-145, F-146, F-264),
	// so a close still pending after 500 ms is ended with the event Ark awaits.
	// Call during component init; the admin menu drawer shares it (F-329).
	export function endStuckClose(isOpen: () => boolean, parts: () => (Element | null)[]) {
		$effect(() => {
			if (isOpen()) return;
			const timer = setInterval(() => {
				const pending = parts().filter((el) => el instanceof HTMLElement && !el.hidden);
				if (pending.length === 0) clearInterval(timer);
				for (const el of pending) el?.dispatchEvent(new Event('animationcancel'));
			}, 500);
			return () => clearInterval(timer);
		});
	}
</script>

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
		onexitcomplete,
		preventClose = false,
		class: className,
		children
	}: {
		open?: boolean;
		title: string;
		description?: string;
		onclose?: () => void;
		/** Fires once the close animation and focus restore are done. */
		onexitcomplete?: () => void;
		preventClose?: boolean;
		class?: string;
		children: Snippet;
	} = $props();

	const s = dialog();

	// Focus goes back to whatever was focused when the dialog opened. When that is
	// gone or hidden (a confirmed delete re-renders the list without it), it goes
	// to the main landmark, never left on a hidden button or <body>.
	let opener: Element | null = null;
	$effect.pre(() => {
		if (open) opener = document.activeElement;
	});
	function finalFocusEl() {
		const el = opener;
		if (el instanceof HTMLElement && el !== document.body && el.isConnected && el.checkVisibility?.() !== false) {
			return el;
		}
		return document.querySelector<HTMLElement>('main');
	}

	let backdropEl = $state<Element | null>(null);
	let contentEl = $state<Element | null>(null);
	endStuckClose(() => open, () => [backdropEl, contentEl]);
</script>

<Dialog.Root
	bind:open
	closeOnEscape={!preventClose}
	closeOnInteractOutside={!preventClose}
	onOpenChange={(e) => {
		if (!e.open) onclose?.();
	}}
	onExitComplete={() => onexitcomplete?.()}
	{finalFocusEl}
>
	<Portal>
		<!-- A closing backdrop lets clicks through to the page (F-196). -->
		<Dialog.Backdrop bind:ref={backdropEl} class={s.backdrop} style={open ? undefined : 'pointer-events: none'} />
		<Dialog.Positioner class={s.positioner}>
			<Dialog.Content bind:ref={contentEl} class={cx(s.content, className)}>
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
