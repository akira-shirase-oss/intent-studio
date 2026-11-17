import { defineConfig, devices } from '@playwright/test';

// Run by scripts/workspace-e2e.sh, which starts a real intent-server (release build) on
// 127.0.0.1:59402 with this app built, a content store, the asset worker and a few leads stored.
// Two widths: a 1440 px desktop and a 360 px phone. The journeys are serial within a project
// (each builds on the client the one before set up).
export default defineConfig({
	testDir: 'tests',
	globalSetup: './tests/global-setup.ts',
	fullyParallel: false,
	retries: 0,
	reporter: [['list']],
	use: {
		baseURL: process.env['WORKSPACE_BASE_URL'] ?? 'http://127.0.0.1:59402',
		trace: 'retain-on-failure'
	},
	projects: [
		{ name: 'desktop', use: { ...devices['Desktop Chrome'], viewport: { width: 1440, height: 900 } } },
		{ name: 'mobile', use: { ...devices['Pixel 7'], viewport: { width: 360, height: 800 } } }
	]
});
