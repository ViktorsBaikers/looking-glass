<script lang="ts">
	import { cx } from 'styled-system/css';
	import { mtrTable, mtrTh, mtrThNum, mtrTd, mtrTdHop, mtrTdHost } from '$lib/public/styles.js';
	import type { MtrRow } from './mtr.js';

	let { rows }: { rows: MtrRow[] } = $props();

	const columns = [
		{ key: 'lossPct', label: 'Loss%' },
		{ key: 'sent', label: 'Snt' },
		{ key: 'last', label: 'Last' },
		{ key: 'avg', label: 'Avg' },
		{ key: 'best', label: 'Best' },
		{ key: 'worst', label: 'Wrst' },
		{ key: 'stdev', label: 'StDev' }
	] as const;
</script>

<table class={mtrTable}>
	<thead>
		<tr>
			<th class={mtrTh} scope="col">#</th>
			<th class={mtrTh} scope="col">Host</th>
			{#each columns as column (column.key)}
				<th class={cx(mtrTh, mtrThNum)} scope="col">{column.label}</th>
			{/each}
		</tr>
	</thead>
	<tbody>
		{#each rows as row (row.hop)}
			<tr>
				<td class={cx(mtrTd, mtrTdHop)}>{row.hop}</td>
				<td class={cx(mtrTd, mtrTdHost)}>{row.host}</td>
				{#each columns as column (column.key)}
					<td class={mtrTd}>{row[column.key]}</td>
				{/each}
			</tr>
		{/each}
	</tbody>
</table>
