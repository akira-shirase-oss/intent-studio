import { request, type FullConfig } from '@playwright/test';

/** Where the operator's session is kept for the journeys that start signed in. */
export const operatorState = new URL('../test-results/.auth/operator.json', import.meta.url).pathname;

// One sign-in for every journey that starts signed in: sign-in is limited to five attempts a
// minute per address (D-036), and the keyboard sign-in test makes its own.
export default async function globalSetup(config: FullConfig) {
	const baseURL = config.projects[0]?.use.baseURL ?? 'http://127.0.0.1:59402';
	const context = await request.newContext({ baseURL });
	const response = await context.post('/v1/operator/session', { data: { password: process.env['WORKSPACE_PASSWORD'] ?? '' } });
	if (response.status() !== 200) throw new Error(`global sign-in failed: ${response.status()}`);
	await context.storageState({ path: operatorState });
	await context.dispose();
}
