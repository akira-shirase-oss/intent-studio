import { defineConfig, mergeConfig } from 'vitest/config';
import viteConfig from './vite.config.ts';

// Component tests run under jsdom; `browser` makes bare `svelte` imports resolve the
// client build (mount), not the server build.
export default mergeConfig(
	viteConfig,
	defineConfig({
		resolve: { conditions: ['browser'] },
		test: { environment: 'jsdom' }
	})
);
