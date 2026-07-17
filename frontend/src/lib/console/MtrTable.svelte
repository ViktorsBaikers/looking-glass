<script lang="ts">
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

<table class="w-full min-w-[36rem] border-collapse text-left text-xs">
	<thead>
		<tr class="text-muted-foreground">
			<th class="py-1 pr-3 font-medium">#</th>
			<th class="py-1 pr-3 font-medium">Host</th>
			{#each columns as column (column.key)}
				<th class="py-1 pr-3 text-right font-medium tabular-nums">{column.label}</th>
			{/each}
		</tr>
	</thead>
	<tbody>
		{#each rows as row (row.hop)}
			<tr class="border-t border-border/60">
				<td class="py-1 pr-3 text-muted-foreground tabular-nums">{row.hop}</td>
				<td class="py-1 pr-3">{row.host}</td>
				{#each columns as column (column.key)}
					<td class="py-1 pr-3 text-right tabular-nums">{row[column.key]}</td>
				{/each}
			</tr>
		{/each}
	</tbody>
</table>
