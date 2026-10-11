import { mergeConfig } from 'vite';
import viteConfig from '../vite.config.js';

export default mergeConfig(viteConfig, {
	server: {
		proxy: { '/api': `http://127.0.0.1:${process.env.E2E_FIXTURE_PORT ?? 4173}` },
		// E2E pages load fresh per test; HMR full-reloads triggered by concurrent
		// file writes elsewhere in the repo detach pages mid-test.
		hmr: false
	}
});
