// Ambient types for build-time Material Symbols imports via unplugin-icons,
// e.g. `import Icon from '~icons/material-symbols/content-copy'`. Each icon is
// compiled to an inline-SVG Svelte component that accepts SVG element props
// (class, width, height, aria-hidden, …). Self-contained so it does not depend
// on the unplugin-icons package exposing a `./types` export.
declare module '~icons/*' {
	import type { Component } from 'svelte';
	import type { SvelteHTMLElements } from 'svelte/elements';
	const component: Component<SvelteHTMLElements['svg']>;
	export default component;
}
