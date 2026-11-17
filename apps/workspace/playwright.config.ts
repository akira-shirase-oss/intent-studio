import { defineConfig, devices } from '@playwright/test';

// Run by scripts/workspace-e2e.sh, which starts a real intent-server (release build) on
// 127.0.0.1:59402 with this app built and a few leads stored.
export default defineConfig({
	testDir: 'tests',
	fullyParallel: false,
	retries: 0,
	reporter: [['list']],
	use: {
		baseURL: process.env['WORKSPACE_BASE_URL'] ?? 'http://127.0.0.1:59402',
		trace: 'retain-on-failure'
	},
	projects: [
		{ name: 'desktop', use: { ...devices['Desktop Chrome'], viewport: { width: 1280, height: 800 } } },
		{ name: 'mobile', use: { ...devices['Pixel 7'], viewport: { width: 360, height: 800 } } }
	]
});
