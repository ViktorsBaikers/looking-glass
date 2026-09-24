<script lang="ts">
	import { onMount } from 'svelte';
	import { cx } from 'styled-system/css';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import Textarea from '$lib/components/ui/textarea.svelte';
	import Field from '$lib/components/ui/field.svelte';
	import Select from '$lib/components/ui/select.svelte';
	import { Card, CardContent, CardHeader, CardTitle } from '$lib/components/ui/card/index.js';
	import { toast } from '$lib/toast.svelte.js';
	import { spin, adminPage, adminPageHead, adminTitle, adminLede } from '$lib/styles.js';
	import { lineStyle } from '$lib/lines.js';
	import { getSettings, saveSettings } from '$lib/admin/api.js';
	import { isDirty, toPayload, type SettingsDraft } from '$lib/admin/settings.js';
	import * as s from '$lib/admin/settings-styles.js';
	import Spinner from '~icons/material-symbols/progress-activity';
	import type { GlobalSettings } from '$lib/admin/types.js';

	const THEME_ITEMS = [
		{ label: 'Follow system', value: 'system' },
		{ label: 'Light', value: 'light' },
		{ label: 'Dark', value: 'dark' }
	];

	let phase = $state<'loading' | 'ready' | 'error'>('loading');
	let saved = $state<GlobalSettings | null>(null);
	let form = $state<SettingsDraft | null>(null);
	let saving = $state(false);

	onMount(load);

	async function load() {
		phase = 'loading';
		const result = await getSettings();
		if (result.ok) {
			saved = result.data;
			form = { ...result.data };
			phase = 'ready';
		} else {
			phase = 'error';
		}
	}

	const dirty = $derived(saved !== null && form !== null && isDirty(saved, form));
	const hasLogo = $derived(!!form?.logo_url && form.logo_url.trim() !== '');
	const hasTerms = $derived(!!form?.terms_url && form.terms_url.trim() !== '');
	const message = $derived(
		form && form.custom_block && form.custom_block.trim() !== ''
			? form.custom_block
			: 'No custom message configured.'
	);

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		if (!form || saving) return;
		saving = true;
		const result = await saveSettings(toPayload(form));
		saving = false;
		if (result.ok) {
			saved = result.data;
			form = { ...result.data };
			toast.success('Settings saved.');
		} else {
			toast.error(result.message);
		}
	}
</script>

{#snippet previewCard(dark: boolean)}
	{#if form}
		<div class={cx(s.previewCard, dark ? 'dark' : 'light')}>
			<div class={s.previewBar}>
				{#if hasLogo}
					<img src={form.logo_url ?? ''} alt="" class={s.previewLogo} />
				{:else}
					<span class={s.previewMark} aria-hidden="true"></span>
				{/if}
				<span class={s.previewTitle}>{form.site_title}</span>
				<span class={s.previewNav} aria-hidden="true">Diagnostics</span>
			</div>
			<div class={s.previewBody}>
				<div class={s.previewLine} data-line style={lineStyle('preview')} aria-hidden="true">
					<span class={s.previewRoundel}>LG</span>
				</div>
				<p class={s.previewMessage}>{message}</p>
				<p class={s.previewMeta}>
					<span class={s.previewScheme}>{dark ? 'Dark theme' : 'Light theme'}</span> ·
					<span>{hasLogo ? 'Custom logo' : 'Default mark'}</span> ·
					<span>{hasTerms ? 'Terms link' : 'No terms link'}</span> ·
					<span>Default: {form.default_theme}</span>
				</p>
			</div>
		</div>
	{/if}
{/snippet}

{#if phase === 'loading'}
	<div class={s.loadingRow}>
		<Spinner class={spin} aria-hidden="true" />
		Loading settings…
	</div>
{:else if phase === 'error'}
	<Card>
		<CardContent>
			<p class={s.errorText} role="alert">Settings could not be loaded.</p>
			<Button variant="secondary" size="sm" onclick={load}>Try again</Button>
		</CardContent>
	</Card>
{:else if form}
	<div class={adminPage}>
	<header class={adminPageHead}>
		<div>
			<h1 class={adminTitle}>Settings</h1>
			<p class={adminLede}>
				Preview branding and appearance, then publish them with the limits every run must follow.
			</p>
		</div>
	</header>

	<div class={s.pageGrid}>
		<form class={s.formCol} onsubmit={submit} novalidate>
			<Card>
				<CardHeader><CardTitle class={s.sectionTitle}>Branding</CardTitle></CardHeader>
				<CardContent>
					<div class={s.fieldStack}>
						<Field label="Site title" for="site-title">
							<Input id="site-title" bind:value={form.site_title} required />
						</Field>
						<Field label="Logo URL (optional)" for="logo-url">
							<Input
								id="logo-url"
								bind:value={form.logo_url}
								placeholder="https://example.test/logo.svg"
							/>
						</Field>
						<Field label="Terms-of-service URL (optional)" for="terms-url">
							<Input
								id="terms-url"
								bind:value={form.terms_url}
								placeholder="https://example.test/terms"
							/>
						</Field>
						<Field label="Custom content block (optional)" for="custom-block">
							<Textarea id="custom-block" bind:value={form.custom_block} rows={4} />
						</Field>
					</div>
				</CardContent>
			</Card>

			<Card>
				<CardHeader><CardTitle class={s.sectionTitle}>Appearance</CardTitle></CardHeader>
				<CardContent>
					<Field
						label="Default theme"
						for="default-theme"
						hint="This changes the default only. It does not replace a visitor's saved preference."
					>
						<Select id="default-theme" items={THEME_ITEMS} bind:value={form.default_theme} />
					</Field>
				</CardContent>
			</Card>

			<Card>
				<CardHeader><CardTitle class={s.sectionTitle}>Execution limits</CardTitle></CardHeader>
				<CardContent>
					<div class={s.limitsGrid}>
						<Field label="Global concurrency cap" for="max-concurrent">
							<Input id="max-concurrent" type="number" min={1} bind:value={form.exec_max_concurrent} />
						</Field>
						<Field label="Per-run timeout (seconds)" for="timeout">
							<Input id="timeout" type="number" min={1} bind:value={form.exec_timeout_secs} />
						</Field>
						<Field label="Output cap (KiB)" for="output">
							<Input id="output" type="number" min={1} bind:value={form.exec_max_output_kib} />
						</Field>
						<Field label="Rate limit (runs)" for="rate-max">
							<Input id="rate-max" type="number" min={1} bind:value={form.exec_rate_max} />
						</Field>
						<Field label="Rate window (seconds)" for="rate-window">
							<Input id="rate-window" type="number" min={1} bind:value={form.exec_rate_window_secs} />
						</Field>
					</div>
				</CardContent>
			</Card>

			<div class={s.actionsRow}>
				<Button type="submit" loading={saving}>Save settings</Button>
				<span class={s.indicator}>{dirty ? 'Unsaved changes.' : 'No unsaved changes.'}</span>
			</div>
		</form>

		<aside class={s.previewCol} aria-label="Preview">
			<h2 class={s.previewHeading}>Preview</h2>
			<p class={s.previewLabel}>{dirty ? 'Unsaved changes preview.' : 'Saved settings preview.'}</p>
			{@render previewCard(false)}
			{@render previewCard(true)}
		</aside>
	</div>
	</div>
{/if}
