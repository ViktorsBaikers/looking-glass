<script lang="ts">
	import { cx } from 'styled-system/css';
	import {
		mtrTable,
		mtrTh,
		mtrThNum,
		mtrTd,
		mtrTdNum,
		mtrTdHop,
		mtrTdHost,
		hostLeader,
		mtrTdStrong,
		mtrTdLoss,
		routeRow,
		stationCell,
		originRoundel,
		originName,
		originNote,
		station,
		stationEnd,
		stationLossy
	} from '$lib/public/styles.js';
	import { srOnly } from '$lib/styles.js';
	import { lineCode } from '$lib/lines.js';
	import type { MtrRow } from './mtr.js';

	/** `origin`: the run's Location name, drawn as the line's first stop. */
	let { rows, origin }: { rows: MtrRow[]; origin?: string } = $props();

	const columns = [
		{ key: 'lossPct', label: 'Loss%' },
		{ key: 'sent', label: 'Snt' },
		{ key: 'last', label: 'Last' },
		{ key: 'avg', label: 'Avg' },
		{ key: 'best', label: 'Best' },
		{ key: 'worst', label: 'Wrst' },
		{ key: 'stdev', label: 'StDev' }
	] as const;

	const lossy = (row: MtrRow) => parseFloat(row.lossPct) > 0;
</script>

<table class={mtrTable}>
	<thead>
		<tr>
			<th class={mtrTh} scope="col"><span class={srOnly}>Route</span></th>
			<th class={cx(mtrTh, mtrThNum)} scope="col">#</th>
			<th class={mtrTh} scope="col">Host</th>
			{#each columns as column (column.key)}
				<th class={cx(mtrTh, mtrThNum)} scope="col">{column.label}</th>
			{/each}
		</tr>
	</thead>
	<tbody>
		{#if origin}
			<tr class={routeRow}>
				<td class={stationCell} aria-hidden="true">
					<span class={originRoundel}>{lineCode(origin)}</span>
				</td>
				<td class={cx(mtrTd, mtrTdHop)}></td>
				<td class={cx(mtrTd, mtrTdHost)} colspan={columns.length}>
					<span class={originName}>{origin}</span><span class={originNote}>origin</span>
				</td>
			</tr>
		{/if}
		{#each rows as row, index (row.hop)}
			<tr class={routeRow} style="animation-delay: {index * 60}ms">
				<td class={stationCell} aria-hidden="true">
					<span
						class={cx(
							station,
							index === rows.length - 1 ? stationEnd : '',
							lossy(row) ? stationLossy : ''
						)}
					></span>
				</td>
				<td class={cx(mtrTd, mtrTdHop)}>{row.hop}</td>
				<td class={cx(mtrTd, mtrTdHost)}><span class={hostLeader}>{row.host}</span></td>
				{#each columns as column (column.key)}
					<td
						class={cx(
							mtrTd,
							mtrTdNum,
							column.key === 'avg' ? mtrTdStrong : '',
							column.key === 'lossPct' && lossy(row) ? mtrTdLoss : ''
						)}>{row[column.key]}</td
					>
				{/each}
			</tr>
		{/each}
	</tbody>
</table>
