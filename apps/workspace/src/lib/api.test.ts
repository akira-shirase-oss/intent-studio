import { describe, expect, test } from 'vitest';
import { notificationLabel, type InboxLead } from './api.ts';

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
