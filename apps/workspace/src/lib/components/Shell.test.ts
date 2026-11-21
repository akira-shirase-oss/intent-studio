// @vitest-environment jsdom

// NEW-6: the sign-out button's three states, against the real component. A 204 or a
// 401 ends the session (the cookie is gone server-side either way — no retry); a 503
// means the delete was never confirmed, the cookie is still held, and the button must
// offer "Sign-out failed — retry" while the session stays signed in. A label that
// invites a retry after a 401, or a status drop on an unconfirmed delete, fails here.

import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/svelte';
import { createRawSnippet, tick } from 'svelte';
import Shell from './Shell.svelte';
import { ApiError, api } from '#lib/api.ts';
import { session } from '#lib/session.svelte.ts';

vi.mock('$app/state', () => ({ page: { url: { pathname: '/workspace/' } } }));
vi.mock('$app/paths', () => ({ resolve: (path: string) => path }));
vi.mock('#lib/api.ts', async (importOriginal) => {
	const actual = await importOriginal<typeof import('#lib/api.ts')>();
	return { ...actual, api: { ...actual.api, signOut: vi.fn() } };
});

const children = createRawSnippet(() => ({ render: () => '<p>inbox</p>' }));
const signOutMock = () => vi.mocked(api.signOut);

async function clickSignOut(): Promise<void> {
	render(Shell, { children });
	await fireEvent.click(screen.getByRole('button', { name: 'Sign out' }));
	await tick();
}

describe('the sign-out button', () => {
	beforeEach(() => {
		session.status = 'signed-in';
		signOutMock().mockReset();
	});
	afterEach(cleanup);

	test('a confirmed sign-out ends the session', async () => {
		signOutMock().mockResolvedValue(undefined);
		await clickSignOut();
		expect(session.status).toBe('signed-out');
	});

	test('a 401 ends the session — the button never offers a retry', async () => {
		signOutMock().mockRejectedValue(new ApiError(401, 'unauthorized', 'no session'));
		await clickSignOut();
		expect(session.status).toBe('signed-out');
		expect(screen.queryByRole('button', { name: /retry/i })).toBeNull();
	});

	test('a 503 offers a retry and keeps the session', async () => {
		signOutMock().mockRejectedValue(new ApiError(503, 'unavailable', 'delete unconfirmed'));
		await clickSignOut();
		// The cookie is still held: the session must not read signed-out.
		expect(session.status).toBe('signed-in');
		screen.getByRole('button', { name: 'Sign-out failed — retry' });
	});
});
