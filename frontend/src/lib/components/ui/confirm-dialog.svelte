<script lang="ts">
	import Dialog from './dialog.svelte';
	import Button from './button/button.svelte';
	import { confirmActions as actions } from '$lib/styles.js';

	let {
		open = $bindable(false),
		title,
		message,
		confirmLabel = 'Confirm',
		cancelLabel = 'Cancel',
		danger = false,
		busy = false,
		onconfirm
	}: {
		open?: boolean;
		title: string;
		message?: string;
		confirmLabel?: string;
		cancelLabel?: string;
		danger?: boolean;
		busy?: boolean;
		onconfirm: () => void;
	} = $props();

</script>

<Dialog bind:open {title} description={message}>
	<div class={actions}>
		<Button variant="ghost" onclick={() => (open = false)} disabled={busy}>{cancelLabel}</Button>
		<Button variant={danger ? 'danger' : 'primary'} onclick={onconfirm} loading={busy}>
			{confirmLabel}
		</Button>
	</div>
</Dialog>
