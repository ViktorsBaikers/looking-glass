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
		traceAddr,
		routeRow,
		stationCell,
		originRoundel,
		originName,
		originNote,
		stationSilent,
		station,
		stationEnd,
		stationLossy,
		stationSilentMark,
		stationLive
	} from '$lib/public/styles.js';
	import { srOnly } from '$lib/styles.js';
	import { lineCode } from '$lib/lines.js';
	import type { TraceRow } from './trace.js';

	/** `live`: the run is still streaming, so the newest hop is "you are here". */
	/** `origin`: the run's Location name, drawn as the line's first stop. */
	let { rows, live, origin }: { rows: TraceRow[]; live: boolean; origin?: string } = $props();

	const probes = $derived(Math.max(...rows.map((row) => row.rtts.length)));
	const silent = (row: TraceRow) => row.rtts.every((rtt) => rtt === '*');
	const lossy = (row: TraceRow) => !silent(row) && row.rtts.includes('*');
</script>

<table class={mtrTable}>
	<thead>
		<tr>
			<th class={mtrTh} scope="col"><span class={srOnly}>Route</span></th>
			<th class={cx(mtrTh, mtrThNum)} scope="col">#</th>
			<th class={mtrTh} scope="col">Host</th>
			{#each { length: probes }, probe (probe)}
				<th class={cx(mtrTh, mtrThNum)} scope="col">Probe {probe + 1}</th>
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
				<td class={cx(mtrTd, mtrTdHost)} colspan={probes + 1}>
					<span class={originName}>{origin}</span><span class={originNote}>origin</span>
				</td>
			</tr>
		{/if}
		{#each rows as row, index (row.hop)}
			{@const last = index === rows.length - 1}
			<tr class={routeRow}>
				<td class={cx(stationCell, silent(row) ? stationSilent : '')} aria-hidden="true">
					<span
						class={cx(
							station,
							silent(row) ? stationSilentMark : '',
							last && !live && !silent(row) ? stationEnd : '',
							lossy(row) ? stationLossy : '',
							last && live ? stationLive : ''
						)}
					></span>
				</td>
				<td class={cx(mtrTd, mtrTdHop)}>{row.hop}</td>
				<td class={cx(mtrTd, mtrTdHost)}>
					<span class={hostLeader}
						><span>{silent(row) ? 'No reply' : row.host}{#if row.address}<span class={traceAddr}
									>{row.address}</span
								>{/if}</span
						></span
					>
				</td>
				{#each { length: probes }, probe (probe)}
					<td class={cx(mtrTd, mtrTdNum, probe === 0 ? mtrTdStrong : '')}>
						{row.rtts[probe] === undefined ? '' : row.rtts[probe] === '*' ? '*' : `${row.rtts[probe]} ms`}
					</td>
				{/each}
			</tr>
		{/each}
	</tbody>
</table>
