// The three small state modules the screens share: the server decides the session, the client
// choice and the theme are per-browser conveniences that must never break the page when storage
// is unavailable.

import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest';

const fetchMock = vi.fn();
vi.stubGlobal('fetch', fetchMock);

function reply(status: number, body: unknown = null): Response {
	return new Response(body === null ? null : JSON.stringify(body), {
		status,
		headers: { 'content-type': 'application/json' }
	});
}

beforeEach(() => {
	vi.resetModules();
	fetchMock.mockReset();
});
afterEach(() => {
	vi.unstubAllGlobals();
	vi.stubGlobal('fetch', fetchMock);
});

describe('session', () => {
	test('the server\'s answer is the session', async () => {
		const { checkSession, session } = await import('./session.svelte.ts');
		fetchMock.mockResolvedValue(reply(200, { signed_in: true }));
		await checkSession();
		expect(session.status).toBe('signed-in');

		fetchMock.mockResolvedValue(reply(401, { error: { code: 'unauthorized', message: 'no' } }));
		await checkSession();
		expect(session.status).toBe('signed-out');

		fetchMock.mockRejectedValue(new TypeError('fetch failed'));
		await checkSession();
		expect(session.status).toBe('unreachable');
	});

	test('sessionEnded flips to signed-out on a mid-session 401, nothing else', async () => {
		const { ApiError, NoAnswer } = await import('./api.ts');
		const { session, sessionEnded } = await import('./session.svelte.ts');
		session.status = 'signed-in';
		expect(sessionEnded(new ApiError(401, 'unauthorized', 'no'))).toBe(true);
		expect(session.status).toBe('signed-out');
		expect(sessionEnded(new ApiError(503, 'unavailable', 'busy'))).toBe(false);
		expect(sessionEnded(new NoAnswer('no answer'))).toBe(false);
		expect(session.status).toBe('signed-out');
	});
});

describe('clients', () => {
	const tenants = [
		{ id: 't1', name: 'One', created_at: '2026-11-01T00:00:00Z' },
		{ id: 't2', name: 'Two', created_at: '2026-11-02T00:00:00Z' }
	];

	test('the first client is selected when nothing is remembered', async () => {
		const { clients, loadClients } = await import('./clients.svelte.ts');
		fetchMock.mockResolvedValue(reply(200, { tenants }));
		await loadClients();
		expect(clients.status).toBe('ready');
		expect(clients.selected).toBe('t1');
	});

	test('a remembered client wins; a stale memory falls back to the first', async () => {
		vi.stubGlobal('localStorage', { getItem: () => 't2', setItem: vi.fn() });
		const { clients, loadClients } = await import('./clients.svelte.ts');
		fetchMock.mockResolvedValue(reply(200, { tenants }));
		await loadClients(true);
		expect(clients.selected).toBe('t2');
	});

	test('an error becomes plain words, and a 401 ends the session instead', async () => {
		const { ApiError } = await import('./api.ts');
		const { session } = await import('./session.svelte.ts');
		const { clients, loadClients } = await import('./clients.svelte.ts');
		session.status = 'signed-in';
		fetchMock.mockResolvedValue(reply(503, { error: { code: 'unavailable', message: 'down' } }));
		await loadClients(true);
		expect(clients.status).toBe('error');
		expect(clients.error).toContain('not available');

		fetchMock.mockResolvedValue(reply(401, { error: { code: 'unauthorized', message: 'no' } }));
		await loadClients(true);
		expect(session.status).toBe('signed-out');
	});

	test('a new client is added, first in the list and selected', async () => {
		const { clients, addClient } = await import('./clients.svelte.ts');
		clients.list = [...tenants];
		fetchMock.mockResolvedValue(reply(200, { id: 't3', name: 'Three', created_at: '2026-11-03T00:00:00Z' }));
		const tenant = await addClient('Three');
		expect(tenant.id).toBe('t3');
		expect(clients.list[0]?.id).toBe('t3');
		expect(clients.selected).toBe('t3');
	});
});

describe('theme', () => {
	function stubDom(): { dark: string[]; scheme: { colorScheme?: string } } {
		const dark: string[] = [];
		const scheme: { colorScheme?: string } = {};
		vi.stubGlobal('document', {
			documentElement: {
				classList: { toggle: (_c: string, on: boolean) => (on ? dark.push('dark') : dark.splice(0)) },
				style: scheme
			}
		});
		return { dark, scheme };
	}

	test('dark and light apply the class and the scheme', async () => {
		const { dark, scheme } = stubDom();
		const { setTheme } = await import('./theme.svelte.ts');
		setTheme('dark');
		expect(dark).toContain('dark');
		expect(scheme.colorScheme).toBe('dark');
		setTheme('light');
		expect(dark).not.toContain('dark');
		expect(scheme.colorScheme).toBe('light');
	});

	test('system follows the media query', async () => {
		const { dark } = stubDom();
		vi.stubGlobal('matchMedia', () => ({ matches: true, addEventListener: vi.fn() }));
		const { setTheme } = await import('./theme.svelte.ts');
		setTheme('system');
		expect(dark).toContain('dark');
	});

	test('blocked storage changes nothing but the applied theme', async () => {
		const { dark } = stubDom();
		vi.stubGlobal('localStorage', {
			getItem: () => {
				throw new Error('denied');
			},
			setItem: () => {
				throw new Error('denied');
			}
		});
		const { loadTheme, setTheme, theme } = await import('./theme.svelte.ts');
		expect(() => loadTheme()).not.toThrow();
		expect(() => setTheme('dark')).not.toThrow();
		expect(theme.value).toBe('dark');
		expect(dark).toContain('dark');
	});
});
