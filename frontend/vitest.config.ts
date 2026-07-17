import { svelteTesting } from '@testing-library/svelte/vite';
import { mergeConfig } from 'vite';
import { defineConfig } from 'vitest/config';
import viteConfig from './vite.config.js';

export default mergeConfig(
	viteConfig,
	defineConfig({
		plugins: [svelteTesting()],
		test: {
			environment: 'jsdom',
			include: ['src/**/*.test.ts']
		}
	})
);
