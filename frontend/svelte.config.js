import adapter from '@sveltejs/adapter-static';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';

/** @type {import('@sveltejs/kit').Config} */
const config = {
	preprocess: vitePreprocess(),
	kit: {
		adapter: adapter({
			pages: 'build',
			assets: 'build',
			fallback: 'index.html',
			precompress: false
		}),
		alias: {
			$lib: 'src/lib',
			// Panda CSS generated runtime; kit.alias wires both Vite resolve and
			// the generated tsconfig paths so `styled-system/css` imports typecheck.
			'styled-system': './styled-system'
		}
	}
};

export default config;
