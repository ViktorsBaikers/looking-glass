<script lang="ts">
	import { onMount } from 'svelte';
	import LoaderCircle from '@lucide/svelte/icons/loader-circle';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import { toaster } from '$lib/toast.svelte.js';
	import { getSettings, saveSettings } from '$lib/admin/api.js';
	import type { GlobalSettings } from '$lib/admin/types.js';

	let phase = $state<'loading' | 'ready' | 'error'>('loading');
	let form = $state<GlobalSettings | null>(null);
	let saving = $state(false);
	let formError = $state('');

	onMount(load);

	async function load() {
		phase = 'loading';
		const result = await getSettings();
		if (result.ok) {
			form = result.data;
			phase = 'ready';
		} else {
			phase = 'error';
		}
	}

	function nullIfBlank(value: string | null): string | null {
		return value && value.trim() !== '' ? value : null;
	}

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		if (!form || saving) return;
		saving = true;
		formError = '';
		// Coerce the numeric fields: form inputs surface strings, and the server
		// expects integers, so build the payload explicitly.
		const payload: GlobalSettings = {
			site_title: form.site_title,
			logo_url: nullIfBlank(form.logo_url),
			default_theme: form.default_theme,
			terms_url: nullIfBlank(form.terms_url),
			custom_block: nullIfBlank(form.custom_block),
			exec_max_concurrent: Number(form.exec_max_concurrent),
			exec_timeout_secs: Number(form.exec_timeout_secs),
			exec_max_output_kib: Number(form.exec_max_output_kib),
			exec_rate_max: Number(form.exec_rate_max),
			exec_rate_window_secs: Number(form.exec_rate_window_secs)
		};
		const result = await saveSettings(payload);
		saving = false;
		if (result.ok) {
			form = result.data;
			toaster.success('Settings saved.');
		} else {
			formError = result.message;
		}
	}
</script>

<div class="max-w-2xl space-y-6">
	<div>
		<h1 class="text-xl font-semibold tracking-tight">Settings</h1>
		<p class="text-sm text-muted-foreground">Branding and the limits every run is held to.</p>
	</div>

	{#if phase === 'loading'}
		<div class="flex items-center gap-2 text-muted-foreground">
			<LoaderCircle class="size-5 animate-spin" aria-hidden="true" />
			Loading settings…
		</div>
	{:else if phase === 'error'}
		<div class="rounded-md border border-destructive/40 px-4 py-6 text-sm text-destructive" role="alert">
			<p>Settings could not be loaded.</p>
			<Button variant="outline" size="sm" class="mt-3" onclick={load}>Try again</Button>
		</div>
	{:else if form}
		<form class="space-y-8" onsubmit={submit} novalidate>
			<fieldset class="space-y-4">
				<legend class="text-sm font-semibold text-muted-foreground">Branding</legend>
				<div class="space-y-2">
					<Label for="site-title">Site title</Label>
					<Input id="site-title" bind:value={form.site_title} disabled={saving} required />
				</div>
				<div class="space-y-2">
					<Label for="logo-url">Logo URL (optional)</Label>
					<Input id="logo-url" bind:value={form.logo_url} disabled={saving} />
				</div>
				<div class="space-y-2">
					<Label for="theme">Default theme</Label>
					<select
						id="theme"
						bind:value={form.default_theme}
						disabled={saving}
						class="h-11 w-full rounded-md border border-input bg-background px-3 text-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
					>
						<option value="system">Follow system</option>
						<option value="light">Light</option>
						<option value="dark">Dark</option>
					</select>
				</div>
				<div class="space-y-2">
					<Label for="terms-url">Terms-of-service URL (optional)</Label>
					<Input id="terms-url" bind:value={form.terms_url} disabled={saving} />
				</div>
				<div class="space-y-2">
					<Label for="custom-block">Custom content block (optional)</Label>
					<textarea
						id="custom-block"
						bind:value={form.custom_block}
						disabled={saving}
						rows="3"
						class="w-full rounded-md border border-input bg-transparent px-3 py-2 text-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
					></textarea>
				</div>
			</fieldset>

			<fieldset class="space-y-4">
				<legend class="text-sm font-semibold text-muted-foreground">Execution limits</legend>
				<div class="grid gap-4 sm:grid-cols-2">
					<div class="space-y-2">
						<Label for="max-concurrent">Global concurrency cap</Label>
						<Input
							id="max-concurrent"
							type="number"
							min="1"
							bind:value={form.exec_max_concurrent}
							disabled={saving}
						/>
					</div>
					<div class="space-y-2">
						<Label for="timeout">Per-run timeout (seconds)</Label>
						<Input id="timeout" type="number" min="1" bind:value={form.exec_timeout_secs} disabled={saving} />
					</div>
					<div class="space-y-2">
						<Label for="output">Output cap (KiB)</Label>
						<Input
							id="output"
							type="number"
							min="1"
							bind:value={form.exec_max_output_kib}
							disabled={saving}
						/>
					</div>
					<div class="space-y-2">
						<Label for="rate-max">Rate limit (runs)</Label>
						<Input id="rate-max" type="number" min="1" bind:value={form.exec_rate_max} disabled={saving} />
					</div>
					<div class="space-y-2">
						<Label for="rate-window">Rate window (seconds)</Label>
						<Input
							id="rate-window"
							type="number"
							min="1"
							bind:value={form.exec_rate_window_secs}
							disabled={saving}
						/>
					</div>
				</div>
			</fieldset>

			{#if formError}
				<p class="text-sm text-destructive" role="alert">{formError}</p>
			{/if}

			<Button type="submit" disabled={saving}>
				{#if saving}
					<LoaderCircle class="size-4 animate-spin" aria-hidden="true" />
					Saving…
				{:else}
					Save settings
				{/if}
			</Button>
		</form>
	{/if}
</div>
