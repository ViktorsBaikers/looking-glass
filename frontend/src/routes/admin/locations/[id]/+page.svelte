<script lang="ts">
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import LocationEditor from '$lib/admin/LocationEditor.svelte';

	const TAB_IDS = ['settings', 'methods', 'test-ips', 'iperf', 'speedtest', 'enrollment'];

	const locationId = $derived(page.params.id ?? '');
	const requested = $derived(page.url.searchParams.get('tab') ?? '');
	const tab = $derived(TAB_IDS.includes(requested) ? requested : 'settings');

	// The tab is part of the URL so editors are deep-linkable and survive reload;
	// replaceState keeps clicks out of history, keepFocus/noScroll stay put.
	function ontab(id: string) {
		void goto(`/admin/locations/${locationId}?tab=${id}`, {
			replaceState: true,
			keepFocus: true,
			noScroll: true
		});
	}
</script>

<LocationEditor locationId={locationId} {tab} {ontab} />
