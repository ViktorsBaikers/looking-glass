<script lang="ts">
	import { Toaster as ArkToaster, Toast } from '@ark-ui/svelte';
	import { toast as toastRecipe } from 'styled-system/recipes';
	import { arkToaster } from '$lib/toast.svelte.js';
	import { toastOkIcon as okIcon, toastBody as body } from '$lib/styles.js';
	import CheckCircle from '~icons/material-symbols/check-circle';
	import ErrorIcon from '~icons/material-symbols/error';
	import Close from '~icons/material-symbols/close';

	const s = toastRecipe();
</script>

<ArkToaster toaster={arkToaster} class={s.group}>
	{#snippet children(t)}
		<Toast.Root class={s.root}>
			{#if t().type === 'success'}
				<CheckCircle class={okIcon} aria-hidden="true" />
			{:else if t().type === 'error'}
				<ErrorIcon aria-hidden="true" />
			{/if}
			<div class={body}>
				<Toast.Title class={s.title}>{t().title}</Toast.Title>
				{#if t().description}
					<Toast.Description class={s.description}>{t().description}</Toast.Description>
				{/if}
			</div>
			<Toast.CloseTrigger class={s.closeTrigger} aria-label="Dismiss notification">
				<Close />
			</Toast.CloseTrigger>
		</Toast.Root>
	{/snippet}
</ArkToaster>
