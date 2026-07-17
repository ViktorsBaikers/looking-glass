<script lang="ts" generics="T extends { id: string }">
	import type { JsonResult } from '$lib/api.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import Dialog from '$lib/components/ui/dialog.svelte';
	import Pencil from '@lucide/svelte/icons/pencil';
	import Trash2 from '@lucide/svelte/icons/trash-2';
	import Plus from '@lucide/svelte/icons/plus';

	interface FieldDef {
		key: string;
		label: string;
		type?: 'text' | 'number' | 'select';
		options?: { value: string; label: string }[];
		placeholder?: string;
		optional?: boolean;
	}

	let {
		title,
		addLabel,
		items,
		fields,
		summarize,
		create,
		update,
		remove,
		onchanged
	}: {
		title: string;
		addLabel: string;
		items: T[];
		fields: FieldDef[];
		summarize: (item: T) => string;
		create: (draft: Record<string, unknown>) => Promise<JsonResult<unknown>>;
		update: (id: string, draft: Record<string, unknown>) => Promise<JsonResult<unknown>>;
		remove: (id: string) => Promise<JsonResult<unknown>>;
		onchanged: () => void;
	} = $props();

	let editing = $state<T | null>(null);
	let showForm = $state(false);
	let draft = $state<Record<string, string>>({});
	let submitting = $state(false);
	let formError = $state('');
	let pendingDelete = $state<T | null>(null);
	let showDelete = $state(false);
	let deleting = $state(false);
	let deleteError = $state('');

	function askDelete(item: T) {
		pendingDelete = item;
		deleteError = '';
		showDelete = true;
	}

	function startAdd() {
		editing = null;
		draft = Object.fromEntries(fields.map((f) => [f.key, defaultFor(f)]));
		formError = '';
		showForm = true;
	}

	function startEdit(item: T) {
		editing = item;
		const record = item as Record<string, unknown>;
		draft = Object.fromEntries(fields.map((f) => [f.key, String(record[f.key] ?? '')]));
		formError = '';
		showForm = true;
	}

	function defaultFor(field: FieldDef): string {
		if (field.type === 'select' && field.options?.length) return field.options[0].value;
		return '';
	}

	function buildBody(): Record<string, unknown> {
		const body: Record<string, unknown> = {};
		for (const field of fields) {
			const raw = draft[field.key] ?? '';
			if (field.type === 'number') {
				body[field.key] = Number(raw);
			} else if (field.optional) {
				body[field.key] = raw.trim() === '' ? null : raw;
			} else {
				body[field.key] = raw;
			}
		}
		return body;
	}

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		if (submitting) return;
		submitting = true;
		formError = '';
		const body = buildBody();
		const result = editing ? await update(editing.id, body) : await create(body);
		submitting = false;
		if (result.ok) {
			showForm = false;
			onchanged();
		} else {
			formError = result.message;
		}
	}

	async function confirmDelete() {
		if (!pendingDelete || deleting) return;
		deleting = true;
		deleteError = '';
		const result = await remove(pendingDelete.id);
		deleting = false;
		if (result.ok) {
			showDelete = false;
			pendingDelete = null;
			onchanged();
		} else {
			deleteError = result.message;
		}
	}
</script>

<section class="space-y-4">
	<div class="flex items-center justify-between">
		<h3 class="text-sm font-semibold text-muted-foreground">{title}</h3>
		<Button size="sm" variant="outline" onclick={startAdd}>
			<Plus class="size-4" aria-hidden="true" />
			{addLabel}
		</Button>
	</div>

	{#if items.length === 0}
		<p class="rounded-md border border-dashed border-border px-4 py-6 text-center text-sm text-muted-foreground">
			Nothing here yet.
		</p>
	{:else}
		<ul class="divide-y divide-border rounded-md border border-border">
			{#each items as item (item.id)}
				<li class="flex items-center justify-between gap-3 px-4 py-3">
					<span class="min-w-0 break-words font-mono text-sm">{summarize(item)}</span>
					<div class="flex shrink-0 gap-1">
						<Button size="icon" variant="ghost" onclick={() => startEdit(item)} aria-label="Edit {title}">
							<Pencil class="size-4" aria-hidden="true" />
						</Button>
						<Button
							size="icon"
							variant="ghost"
							onclick={() => askDelete(item)}
							aria-label="Delete {title}"
						>
							<Trash2 class="size-4 text-destructive" aria-hidden="true" />
						</Button>
					</div>
				</li>
			{/each}
		</ul>
	{/if}
</section>

<Dialog bind:open={showForm} title={editing ? `Edit ${addLabel}` : addLabel}>
	<form class="space-y-4" onsubmit={submit} novalidate>
		{#each fields as field (field.key)}
			<div class="space-y-2">
				<Label for="field-{field.key}">{field.label}</Label>
				{#if field.type === 'select'}
					<select
						id="field-{field.key}"
						bind:value={draft[field.key]}
						disabled={submitting}
						class="h-11 w-full rounded-md border border-input bg-background px-3 text-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
					>
						{#each field.options ?? [] as option (option.value)}
							<option value={option.value}>{option.label}</option>
						{/each}
					</select>
				{:else}
					<Input
						id="field-{field.key}"
						type={field.type === 'number' ? 'number' : 'text'}
						placeholder={field.placeholder}
						bind:value={draft[field.key]}
						disabled={submitting}
					/>
				{/if}
			</div>
		{/each}

		{#if formError}
			<p class="text-sm text-destructive" role="alert">{formError}</p>
		{/if}

		<div class="flex justify-end gap-2">
			<Button type="button" variant="ghost" onclick={() => (showForm = false)} disabled={submitting}>
				Cancel
			</Button>
			<Button type="submit" disabled={submitting}>
				{submitting ? 'Saving…' : 'Save'}
			</Button>
		</div>
	</form>
</Dialog>

<Dialog bind:open={showDelete} title="Delete this entry?" description="This cannot be undone.">
	<div class="space-y-4">
		{#if deleteError}
			<p class="text-sm text-destructive" role="alert">{deleteError}</p>
		{/if}
		<div class="flex justify-end gap-2">
			<Button type="button" variant="ghost" onclick={() => (showDelete = false)} disabled={deleting}>
				Cancel
			</Button>
			<Button type="button" variant="destructive" onclick={confirmDelete} disabled={deleting}>
				{deleting ? 'Deleting…' : 'Delete'}
			</Button>
		</div>
	</div>
</Dialog>
