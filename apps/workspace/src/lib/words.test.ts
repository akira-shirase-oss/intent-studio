// The D-063 wording contract: an uncertain write says it cannot be confirmed — never that it
// saved, never that it failed — and every named code the server sends becomes plain words.

import { describe, expect, test } from 'vitest';
import { ApiError, NoAnswer, type Fact } from './api.ts';
import { day, factStatus, failureText, problemsByPath, problemText, refusalText, todayUtc } from './words.ts';

function apiError(status: number, code: string, message = 'm'): ApiError {
	return new ApiError(status, code, message);
}

describe('failureText', () => {
	test('a write that got no answer says it cannot be confirmed, not that it failed', () => {
		const text = failureText(new NoAnswer('no answer'), true);
		expect(text).toContain('cannot tell whether this was saved');
		expect(text).not.toMatch(/failed|was not saved/i);
	});

	test('a write refused 503 is just as honest', () => {
		expect(failureText(apiError(503, 'unavailable'), true)).toContain('could not confirm this was saved');
	});

	test('a read with no answer or a 503 is simply unreachable', () => {
		expect(failureText(new NoAnswer('no answer'))).toBe('The server could not be reached. Check your connection and try again.');
		expect(failureText(apiError(503, 'unavailable'))).toBe('The server is not available right now. Try again in a moment.');
	});

	test('a 401 ends the session; the named codes keep their own words', () => {
		expect(failureText(apiError(401, 'unauthorized'))).toContain('Sign in again');
		expect(failureText(apiError(429, 'rate_limited'))).toContain('Wait a minute');
		expect(failureText(apiError(503, 'store_unavailable'))).toContain('INTENT_STORE_DIR');
		expect(failureText(apiError(503, 'overloaded'))).toContain('busy');
		expect(failureText(apiError(404, 'not_found'))).toContain('another client');
	});

	test('anything else surfaces its own message or the generic words', () => {
		expect(failureText(apiError(422, 'fact_invalid', 'The fact was refused.'))).toBe('The fact was refused.');
		expect(failureText(new Error('boom'))).toBe('Something went wrong. Try again.');
	});
});

describe('problemText and problemsByPath', () => {
	test('an unbound claim quotes the flagged token', () => {
		expect(problemText({ path: 'pages[0].title', code: 'unbound_claim', detail: 'since 1985' })).toContain('“since 1985”');
	});

	test('a known code reads in plain words; an unknown one stays honest', () => {
		expect(problemText({ path: 'x', code: 'fact_stale' })).toContain('approval has run out');
		expect(problemText({ path: 'x', code: 'some_future_code' })).toBe('This was refused.');
	});

	test('problems on one path join so the field shows them all', () => {
		const map = problemsByPath([
			{ path: 'services[0].name', code: 'required' },
			{ path: 'services[0].name', code: 'too_short' },
			{ path: 'domains', code: 'too_many' }
		]);
		expect(map.get('services[0].name')).toBe('This is required. This is too short.');
		expect(map.get('domains')).toBe('There are too many of these.');
	});
});

describe('refusalText', () => {
	test('each file and fetch refusal has its own words', () => {
		expect(refusalText('address_forbidden')).toContain('private, local or reserved');
		expect(refusalText('unsupported_type')).toContain('SVG, PNG or JPEG');
	});

	test('a missing or unknown code gets the generic refusal', () => {
		expect(refusalText(null)).toBe('It was refused.');
		expect(refusalText('unknown_future_reason')).toBe('It was refused.');
	});
});

describe('factStatus, day and todayUtc', () => {
	function fact(status: Fact['status'], reviewBy: string | null): Fact {
		return {
			id: 'f',
			kind: 'price',
			statement: 's',
			source: 'src',
			link: null,
			status,
			approved_by: null,
			approved_on: null,
			review_by: reviewBy,
			review_days: 30,
			created_at: '2026-11-01T00:00:00Z',
			withdrawn_at: null
		};
	}

	test('each status reads for the operator and carries its badge variant', () => {
		expect(factStatus(fact('approved', '2026-12-01'))).toMatchObject({ variant: 'live' });
		expect(factStatus(fact('approved', '2026-12-01')).text).toContain('Approved until');
		expect(factStatus(fact('stale', '2026-10-01'))).toMatchObject({ variant: 'attention' });
		expect(factStatus(fact('withdrawn', null))).toEqual({ text: 'Withdrawn', variant: 'draft' });
		expect(factStatus(fact('proposed', null))).toEqual({ text: 'Waiting for approval', variant: 'pending' });
	});

	test('a calendar day renders for the operator; a missing day is empty', () => {
		expect(day('2026-11-20')).toContain('2026');
		expect(day(null)).toBe('');
	});

	test('todayUtc is the server day shape', () => {
		expect(todayUtc()).toMatch(/^\d{4}-\d{2}-\d{2}$/);
		expect(todayUtc()).toBe(new Date().toISOString().slice(0, 10));
	});
});
