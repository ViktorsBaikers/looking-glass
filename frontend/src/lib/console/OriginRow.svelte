<script lang="ts">
	import { cx } from 'styled-system/css';
	import {
		mtrTd,
		mtrTdHop,
		mtrTdHost,
		routeRow,
		stationCell,
		originRoundel,
		originName,
		originNote
	} from '$lib/public/styles.js';
	import { lineCode } from '$lib/lines.js';

	/** The run's Location drawn as the route's first stop; `span` covers the host + metric columns. */
	let { origin, span }: { origin: string; span: number } = $props();
</script>

<tr class={routeRow}>
	<td class={stationCell} aria-hidden="true">
		<span class={originRoundel}>{lineCode(origin)}</span>
	</td>
	<td class={cx(mtrTd, mtrTdHop)}></td>
	<td class={cx(mtrTd, mtrTdHost)} colspan={span}>
		<span class={originName}>{origin}</span><span class={originNote}>origin</span>
	</td>
</tr>
