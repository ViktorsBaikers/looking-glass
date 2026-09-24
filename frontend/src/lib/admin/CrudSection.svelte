<script lang="ts" module>
	export interface CrudColumn<Item> {
		label: string;
		value: (item: Item) => string;
		mono?: boolean;
		muted?: boolean;
	}

	export interface CrudField {
		key: string;
		label: string;
		type?: 'text' | 'number' | 'select';
		options?: { value: string; label: string }[];
		placeholder?: string;
		optional?: boolean;
		mono?: boolean;
	}
</script>

<script lang="ts" generics="T extends { id: string }">
	import type { JsonResult } from '$lib/api.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import Dialog from '$lib/components/ui/dialog.svelte';
	import ConfirmDialog from '$lib/components/ui/confirm-dialog.svelte';
	import Field from '$lib/components/ui/field.svelte';
	import Select from '$lib/components/ui/select.svelte';
	import { toast } from '$lib/toast.svelte.js';
	import { cx } from 'styled-system/css';
	import {
		panelHead,
		panelTitle,
		panelDesc,
		tableScroller,
		table,
		th,
		thActions,
		td,
		tdMono,
		tdMuted,
		trHover,
		tdActions,
		rowAction,
		rowActionDanger,
		emptyWell,
		formStack,
		errorText
	} from './editor-styles.js';
	import Edit from '~icons/material-symbols/edit';
	import Delete from '~icons/material-symbols/delete';
	import Add from '~icons/material-symbols/add';

	let {
		title,
		description,
		addLabel,
		itemLabel,
		items,
		columns,
		fields,
		rowName,
		create,
		update,
		remove,
		onchanged,
		savedMessage,
		deletedMessage
	}: {
		title: string;
		description: string;
		addLabel: string;
		/** Short noun for dialog titles and row action labels ("IP", "endpoint"). */
		itemLabel: string;
		items: T[];
		columns: CrudColumn<T>[];
		fields: CrudField[];
		rowName: (item: T) => string;
		create: (draft: Record<string, unknown>) => Promise<JsonResult<unknown>>;
		update: (id: string, draft: Record<string, unknown>) => Promise<JsonResult<unknown>>;
		remove: (id: string) => Promise<JsonResult<unknown>>;
		onchanged: () => void;
		savedMessage: string;
		deletedMessage: string;
	} = $props();

	let editing = $state<T | null>(null);
	let showForm = $state(false);
	let draft = $state<Record<string, string>>(blankDraft());
	let submitting = $state(false);
	let formError = $state('');
	let pendingDelete = $state<T | null>(null);
	let showDelete = $state(false);
	let deleting = $state(false);

	const selectItems = $derived(
		Object.fromEntries(fields.filter((field) => field.options).map((field) => [field.key, field.options ?? []]))
	);
	function blankDraft(): Record<string, string> {
		return Object.fromEntries(
			fields.map((field) => [
				field.key,
				field.type === 'select' ? (field.options?.[0]?.value ?? '') : ''
			])
		);
	}

	function startAdd() {
		editing = null;
		draft = blankDraft();
		formError = '';
		showForm = true;
	}

	function startEdit(item: T) {
		editing = item;
		const record = item as Record<string, unknown>;
		draft = Object.fromEntries(fields.map((field) => [field.key, String(record[field.key] ?? '')]));
		formError = '';
		showForm = true;
	}

	function buildBody(): Record<string, unknown> {
		const body: Record<string, unknown> = {};
		for (const field of fields) {
			const raw = draft[field.key] ?? '';
			if (field.type === 'number') body[field.key] = Number(raw);
			else if (field.optional) body[field.key] = raw.trim() === '' ? null : raw;
			else body[field.key] = raw;
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
			toast.success(savedMessage);
			onchanged();
		} else {
			formError = result.message;
			toast.error(result.message);
		}
	}

	function askDelete(item: T) {
		pendingDelete = item;
		showDelete = true;
	}

	async function confirmDelete() {
		if (!pendingDelete || deleting) return;
		deleting = true;
		const result = await remove(pendingDelete.id);
		deleting = false;
		if (result.ok) {
			showDelete = false;
			pendingDelete = null;
			toast.success(deletedMessage);
			onchanged();
		} else {
			toast.error(result.message);
		}
	}
</script>

<section>
	<div class={panelHead}>
		<div>
			<h3 class={panelTitle}>{title}</h3>
			<p class={panelDesc}>{description}</p>
		</div>
		<Button variant="secondary" size="sm" onclick={startAdd}>
			<Add aria-hidden="true" />
			{addLabel}
		</Button>
	</div>

	{#if items.length === 0}
		<p class={emptyWell}>Nothing here yet.</p>
	{:else}
		<div class={tableScroller}>
			<table class={table}>
				<thead>
					<tr>
						{#each columns as column (column.label)}
							<th class={th} scope="col">{column.label}</th>
						{/each}
						<th class={cx(th, thActions)} scope="col">Actions</th>
					</tr>
				</thead>
				<tbody>
					{#each items as item (item.id)}
						<tr class={trHover}>
							{#each columns as column (column.label)}
								<td class={cx(td, column.mono ? tdMono : '', column.muted ? tdMuted : '')}>
									{column.value(item)}
								</td>
							{/each}
							<td class={cx(td, tdActions)}>
								<button
									type="button"
									class={rowAction}
									aria-label={`Edit ${rowName(item)}`}
									onclick={() => startEdit(item)}
								>
									<Edit aria-hidden="true" />
								</button>
								<button
									type="button"
									class={cx(rowAction, rowActionDanger)}
									aria-label={`Delete ${rowName(item)}`}
									onclick={() => askDelete(item)}
								>
									<Delete aria-hidden="true" />
								</button>
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	{/if}
</section>

<Dialog bind:open={showForm} title={editing ? `Edit ${itemLabel}` : addLabel}>
	<form class={formStack} onsubmit={submit} novalidate>
		{#each fields as field (field.key)}
			<Field label={field.label} for={`crud-${field.key}`}>
				{#if field.type === 'select'}
					<Select
						id={`crud-${field.key}`}
						items={selectItems[field.key]}
						bind:value={draft[field.key]}
						portaled={false}
					/>
				{:else}
					<Input
						id={`crud-${field.key}`}
						type={field.type === 'number' ? 'number' : 'text'}
						mono={field.mono}
						placeholder={field.placeholder}
						bind:value={draft[field.key]}
					/>
				{/if}
			</Field>
		{/each}

		{#if formError}
			<p class={errorText} role="alert">{formError}</p>
		{/if}

		<Button type="submit" loading={submitting}>Save</Button>
	</form>
</Dialog>

<ConfirmDialog
	bind:open={showDelete}
	title={`Delete ${itemLabel}?`}
	message={pendingDelete ? `“${rowName(pendingDelete)}” will be removed. This cannot be undone.` : ''}
	confirmLabel="Delete"
	danger
	busy={deleting}
	onconfirm={confirmDelete}
/>
