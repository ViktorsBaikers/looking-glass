<script lang="ts">
	import { cx } from 'styled-system/css';
	import { copyBtn as btn, copyOk as ok, srOnly } from '$lib/styles.js';
	import CopyIcon from '~icons/material-symbols/content-copy';
	import CheckIcon from '~icons/material-symbols/check';

	/**
	 * Copy-to-clipboard button with a transient "copied" confirmation. Uses
	 * navigator.clipboard directly (reliable + testable); the aria-label flips and
	 * a polite live region announces success for screen readers.
	 */
	let {
		text,
		label = 'Copy',
		class: className
	}: { text: string; label?: string; class?: string } = $props();

	let copied = $state(false);
	let timer: ReturnType<typeof setTimeout> | undefined;

	async function copy() {
		try {
			await navigator.clipboard.writeText(text);
			copied = true;
			clearTimeout(timer);
			timer = setTimeout(() => (copied = false), 1500);
		} catch {
			// Clipboard unavailable (insecure context or denied): stay silent.
		}
	}
</script>

<button
	type="button"
	class={cx(btn, copied ? ok : '', className)}
	onclick={copy}
	aria-label={copied ? `Copied ${label}` : `Copy ${label}`}
>
	{#if copied}
		<CheckIcon aria-hidden="true" />
	{:else}
		<CopyIcon aria-hidden="true" />
	{/if}
</button>
<span class={srOnly} aria-live="polite">{copied ? 'Copied' : ''}</span>
