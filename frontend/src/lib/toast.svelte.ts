// Toast queue backed by the Ark UI toaster machine. `toast.success/error(msg)`
// is the only API pages need; the <Toaster> in the root layout renders whatever
// is pushed. Kept as `.svelte.ts` so the existing `$lib/toast.svelte.js` import
// path is unchanged.
import { createToaster } from '@ark-ui/svelte';

/** The Ark toaster instance. Consumed by <Toaster>; not part of the page API. */
export const arkToaster = createToaster({
	placement: 'bottom',
	pauseOnPageIdle: true,
	removeDelay: 200
});

export const toast = {
	success: (message: string) => arkToaster.success({ title: message }),
	error: (message: string) => arkToaster.error({ title: message })
};
