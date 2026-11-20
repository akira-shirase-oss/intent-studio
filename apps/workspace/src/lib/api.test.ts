import { afterEach, describe, expect, test, vi } from 'vitest';
import { ApiError, api, NoAnswer, notificationLabel, type InboxLead } from './api.ts';

function lead(state: InboxLead['state'], attempts: number, uncertain: number, lastError: string | null = null): InboxLead {
	return {
		id: 'l',
		site_id: 's',
		site_name: 'Site',
		received_at: '2026-11-17T00:00:00Z',
		retain_until: '2027-11-17T00:00:00Z',
		name: 'Ada',
		email: 'ada@example.test',
		phone: null,
		service: null,
		area: null,
		message: '',
		is_test: false,
		consent_notice: 'privacy-v1',
		state,
		notification: { attempts, uncertain_attempts: uncertain, last_error: lastError, delivered_at: null, next_attempt_at: null }
	};
}

describe('notificationLabel', () => {
	test('a delivery after an uncertain attempt says it may have been sent twice', () => {
		expect(notificationLabel(lead('delivered', 3, 1)).text).toBe('Notified (may have been sent twice)');
		expect(notificationLabel(lead('delivered', 1, 0)).text).toBe('Notified');
	});

	test('a retry after an uncertain attempt says so', () => {
		expect(notificationLabel(lead('notification_pending', 2, 1, 'provider_timeout')).text).toBe(
			'Retrying (attempt 2 failed; an earlier one may have reached the provider)'
		);
		expect(notificationLabel(lead('notification_pending', 2, 0, 'provider_unavailable')).text).toBe('Retrying (attempt 2 failed)');
	});

	test('a stopped notification names its attempts and reason', () => {
		const label = notificationLabel(lead('notification_dead', 8, 0, 'provider_unavailable'));
		expect(label).toEqual({ text: 'Not notified (8 attempts, provider unavailable)', variant: 'attention' });
	});
});

// `call()` is the only door to the server: which credentials go out, how a refusal becomes an
// ApiError with its code and problems, how a dropped request becomes NoAnswer. Proven against a
// stubbed fetch; the response shapes are the server's (crates/intent-server's error body).
describe('call', () => {
	const fetchMock = vi.fn();
	afterEach(() => fetchMock.mockReset());
	vi.stubGlobal('fetch', fetchMock);

	function reply(status: number, body: unknown = null, headers: Record<string, string> = {}): Response {
		return new Response(body === null ? null : JSON.stringify(body), {
			status,
			headers: { 'content-type': 'application/json', ...headers }
		});
	}

	test('every request goes same-origin with the session cookie and asks for JSON', async () => {
		fetchMock.mockResolvedValue(reply(200, { tenants: [] }));
		await api.tenants();
		const [path, init] = fetchMock.mock.calls[0] as [string, RequestInit];
		expect(path).toBe('/v1/operator/tenants');
		expect(init.credentials).toBe('same-origin');
		expect(init.headers).toMatchObject({ accept: 'application/json' });
	});

	test('a JSON string body sets the content type; no body does not', async () => {
		fetchMock.mockResolvedValue(reply(200, { id: 'f' }));
		await api.createFact('t', { kind: 'price', statement: 'x', source: 'y' });
		const create = fetchMock.mock.calls[0]?.[1] as RequestInit;
		expect(create.headers).toMatchObject({ 'content-type': 'application/json' });
		fetchMock.mockResolvedValue(reply(204));
		await api.signOut();
		const signOut = fetchMock.mock.calls[1]?.[1] as RequestInit;
		expect(signOut.headers).not.toMatchObject({ 'content-type': 'application/json' });
	});

	test('an upload sends the multipart form and the cross-site marker, and no JSON content type', async () => {
		fetchMock.mockResolvedValue(reply(200, { id: 'a' }));
		await api.uploadAsset('t', new File(['<svg/>'], 'logo.svg', { type: 'image/svg+xml' }), 'licence', 'source');
		const [, init] = fetchMock.mock.calls[0] as [string, RequestInit];
		expect(init.headers).toMatchObject({ 'x-intent-upload': '1' });
		expect(init.headers).not.toMatchObject({ 'content-type': 'application/json' });
		expect(init.body).toBeInstanceOf(FormData);
	});

	test('a refusal body becomes an ApiError with its status, code, message and problems', async () => {
		const problems = [{ path: 'claims[0]', code: 'fact_stale', detail: 'f1' }];
		fetchMock.mockResolvedValue(reply(422, { error: { code: 'spec_invalid', message: 'named problems', problems } }));
		const error = await api.briefs('t').then(() => null, (e: unknown) => e);
		expect(error).toBeInstanceOf(ApiError);
		expect(error).toMatchObject({ status: 422, code: 'spec_invalid', message: 'named problems', problems });
	});

	test('a refusal without the error shape still becomes an ApiError, not a parse crash', async () => {
		fetchMock.mockResolvedValue(new Response('busy', { status: 503, headers: { 'content-type': 'text/plain' } }));
		const error = await api.tenants().then(() => null, (e: unknown) => e);
		expect(error).toBeInstanceOf(ApiError);
		expect(error).toMatchObject({ status: 503, code: 'unknown', problems: [] });
	});

	test('a request that never got an answer is a NoAnswer, never an ApiError', async () => {
		fetchMock.mockRejectedValue(new TypeError('fetch failed'));
		const error = await api.tenants().then(() => null, (e: unknown) => e);
		expect(error).toBeInstanceOf(NoAnswer);
	});

	test('a 204 answers undefined', async () => {
		fetchMock.mockResolvedValue(new Response(null, { status: 204 }));
		await expect(api.signOut()).resolves.toBeUndefined();
	});
});
