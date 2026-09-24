<script lang="ts" generics="T extends { label: string; value: string }">
	import { Select, Portal, createListCollection } from '@ark-ui/svelte';
	import { cx } from 'styled-system/css';
	import { select } from 'styled-system/recipes';
	import { selectInvalid as invalidStyle } from '$lib/styles.js';
	import ExpandMore from '~icons/material-symbols/expand-more';
	import Check from '~icons/material-symbols/check';

	/**
	 * Styled single-select on the Ark Select machine (typeahead, arrow keys,
	 * ARIA listbox). `items` should be a stable array; `value` is the selected
	 * item's `value` (bindable). Pair with an external <Label for={id}>.
	 */
	let {
		items,
		value = $bindable(''),
		placeholder = '',
		disabled = false,
		invalid = false,
		name,
		id,
		class: className,
		'aria-label': ariaLabel
	}: {
		items: T[];
		value?: string;
		placeholder?: string;
		disabled?: boolean;
		invalid?: boolean;
		name?: string;
		id?: string;
		class?: string;
		'aria-label'?: string;
	} = $props();

	const s = select();
	const collection = $derived(
		createListCollection<T>({
			items,
			itemToString: (item) => item.label,
			itemToValue: (item) => item.value
		})
	);
</script>

<Select.Root
	{collection}
	{disabled}
	{name}
	value={value === '' ? [] : [value]}
	onValueChange={(e) => (value = e.value[0] ?? '')}
	class={cx(s.root, className)}
>
	<Select.Control>
		<Select.Trigger
			class={cx(s.trigger, invalid ? invalidStyle : '')}
			{id}
			aria-label={ariaLabel}
			aria-invalid={invalid || undefined}
		>
			<Select.ValueText class={s.valueText} {placeholder} />
			<Select.Indicator class={s.indicator}>
				<ExpandMore />
			</Select.Indicator>
		</Select.Trigger>
	</Select.Control>
	<Portal>
		<Select.Positioner class={s.positioner}>
			<Select.Content class={s.content}>
				{#each items as item (item.value)}
					<Select.Item class={s.item} {item}>
						<Select.ItemText class={s.itemText}>{item.label}</Select.ItemText>
						<Select.ItemIndicator class={s.itemIndicator}>
							<Check />
						</Select.ItemIndicator>
					</Select.Item>
				{/each}
			</Select.Content>
		</Select.Positioner>
	</Portal>
	<Select.HiddenSelect />
</Select.Root>
