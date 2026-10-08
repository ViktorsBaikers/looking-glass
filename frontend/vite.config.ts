import { sveltekit } from '@sveltejs/kit/vite';
import Icons from 'unplugin-icons/vite';
import { defineConfig } from 'vite';

// Tailwind is gone: styling is Panda CSS (see postcss.config.js + panda.config.ts).
// Material Symbols are compiled to inline SVG Svelte components at build time.
export default defineConfig({
	plugins: [Icons({ compiler: 'svelte' }), sveltekit()]
});
