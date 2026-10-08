<script lang="ts" module>
	import type { JsonResult } from '$lib/api.js';

	export interface CrudColumn<Item> {
		label: string;
		value: (item: Item) => string;
		mono?: boolean;
		muted?: boolean;
	}

	export interface CrudField<Input> {
		key: keyof Input & string;
		label: string;
		type?: 'text' | 'number' | 'select';
		options?: { value: string; label: string }[];
		placeholder?: string;
		optional?: boolean;
		mono?: boolean;
	}

	// Saves still in flight, by row id (or the Add form's key for a create).
	// Reopening such a row shows the values being saved with Save busy and adopts
	// that save, so a second Save cannot send the stale row back over it, or a
	// duplicate create (F-195). Kept outside the component: a tab switch
	// unmounts the section while its save is still running (F-248).
	const inFlight = new Map<string, { draft: Record<string, string>; result: Promise<JsonResult<unknown>> }>();
</script>

<script lang="ts" generics="T extends { id: string }, I">
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
		scope = '',
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
		/** Owner of the rows (a location id): a pending create is adopted only within it. */
		scope?: string;
		items: T[];
		columns: CrudColumn<T>[];
		fields: CrudField<I>[];
		rowName: (item: T) => string;
		create: (draft: I) => Promise<JsonResult<unknown>>;
		update: (id: string, draft: I) => Promise<JsonResult<unknown>>;
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
	// Bumped on every open: a save that answers after its dialog was closed and
	// another opened still toasts and refreshes, but leaves the new form alone.
	let generation = 0;

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

	// Keyed by owner too: another location's Add form must not adopt it (F-320).
	const addKey = $derived(`add:${scope}:${addLabel}`);

	function startAdd() {
		open(null, addKey, blankDraft());
	}

	function startEdit(item: T) {
		const record = item as Record<string, unknown>;
		open(item, item.id, Object.fromEntries(fields.map((field) => [field.key, String(record[field.key] ?? '')])));
	}

	function open(item: T | null, key: string, values: Record<string, string>) {
		const opened = ++generation;
		const pending = inFlight.get(key);
		submitting = pending !== undefined;
		editing = item;
		draft = pending ? { ...pending.draft } : values;
		formError = '';
		showForm = true;
		// Adopt the save: when it answers, an edit stays open on the saved values;
		// a finished create closes, as its own form would have.
		pending?.result.then((result) => {
			if (opened !== generation) return;
			submitting = false;
			if (!result.ok) formError = result.message;
			else if (!item) showForm = false;
		});
	}

	function buildBody(): I {
		const body: Record<string, unknown> = {};
		for (const field of fields) {
			const raw = draft[field.key] ?? '';
			if (field.type === 'number') body[field.key] = Number(raw);
			else if (field.optional) body[field.key] = raw.trim() === '' ? null : raw;
			else body[field.key] = raw;
		}
		return body as I;
	}

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		if (submitting) return;
		const started = generation;
		const id = editing?.id;
		const key = id ?? addKey;
		submitting = true;
		formError = '';
		const body = buildBody();
		// request() settles every response as a result, an unreadable one (a 2xx
		// proxy page, a cut body) as a failure, so this form and any adopting one
		// leave busy and the row leaves inFlight (F-284).
		const request = id !== undefined ? update(id, body) : create(body);
		inFlight.set(key, { draft: { ...draft }, result: request });
		const result = await request;
		inFlight.delete(key);
		// A dialog reopened mid-save (a later generation) settles through its own adoption.
		const current = started === generation;
		if (current) submitting = false;
		if (result.ok) {
			if (current) showForm = false;
			toast.success(savedMessage);
			onchanged();
		} else {
			if (current) formError = result.message;
			toast.error(result.message);
		}
	}

	// While saving, the Select refuses changes without `disabled`, which would
	// drop its focus to <body>. Escape still reaches the dialog.
	function holdWhileSaving(event: Event) {
		if (!submitting || (event as KeyboardEvent).key === 'Escape') return;
		event.stopPropagation();
		if (event.type === 'click') event.preventDefault();
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
		<p class={emptyWell}>No {itemLabel}s yet.</p>
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
	<form class={formStack} onsubmit={submit} novalidate aria-busy={submitting || undefined}>
		{#each fields as field (field.key)}
			<Field label={field.label} for={`crud-${field.key}`}>
				{#if field.type === 'select'}
					<!-- Select takes no aria-disabled; the group exposes it to the trigger. -->
					<div
						class="locked"
						role="group"
						aria-disabled={submitting || undefined}
						onclickcapture={holdWhileSaving}
						onkeydowncapture={holdWhileSaving}
					>
						<Select
							id={`crud-${field.key}`}
							items={selectItems[field.key]}
							bind:value={draft[field.key]}
							portaled={false}
						/>
					</div>
				{:else}
					<Input
						id={`crud-${field.key}`}
						type={field.type === 'number' ? 'number' : 'text'}
						mono={field.mono}
						placeholder={field.placeholder}
						bind:value={draft[field.key]}
						readonly={submitting}
						aria-disabled={submitting || undefined}
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

<style>
	/* While saving, the group's aria-disabled gives the locked Select trigger the
	   recipe's disabled look. Panda does not extract css() from .svelte files. */
	.locked[aria-disabled='true'] :global([data-part='trigger']) {
		opacity: 0.55;
		cursor: not-allowed;
		background: var(--colors-sunk);
	}
</style>
