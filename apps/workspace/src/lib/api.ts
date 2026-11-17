// The workspace's only door to the server: same-origin JSON over the operator session cookie.
// The server is the authority; this file holds no state beyond one request.

export type LeadState = 'stored' | 'notification_pending' | 'delivered' | 'notification_dead';

export interface InboxLead {
	id: string;
	site_id: string;
	site_name: string;
	received_at: string;
	retain_until: string;
	name: string;
	email: string | null;
	phone: string | null;
	service: string | null;
	area: string | null;
	message: string;
	is_test: boolean;
	state: LeadState;
	notification: {
		attempts: number;
		uncertain_attempts: number;
		last_error: string | null;
		delivered_at: string | null;
		next_attempt_at: string | null;
	};
}

export interface InboxPage {
	leads: InboxLead[];
	next_before: string | null;
}

/** A refusal from the server, with its stable code. */
export class ApiError extends Error {
	constructor(
		readonly status: number,
		readonly code: string,
		message: string
	) {
		super(message);
	}
}

async function call<T>(path: string, init: RequestInit = {}): Promise<T> {
	const response = await fetch(path, {
		credentials: 'same-origin',
		...init,
		headers: { accept: 'application/json', ...(init.body ? { 'content-type': 'application/json' } : {}), ...init.headers }
	});
	if (response.status === 204) return undefined as T;
	const body: unknown = await response.json().catch(() => ({}));
	if (!response.ok) {
		const error = (body as { error?: { code?: string; message?: string } }).error;
		throw new ApiError(response.status, error?.code ?? 'unknown', error?.message ?? `request failed (${response.status})`);
	}
	return body as T;
}

export const api = {
	session: () => call<{ signed_in: boolean }>('/v1/operator/session'),
	signIn: (password: string) =>
		call<{ signed_in: boolean }>('/v1/operator/session', { method: 'POST', body: JSON.stringify({ password }) }),
	signOut: () => call<void>('/v1/operator/session', { method: 'DELETE' }),
	leads: (before?: string) => {
		const query = new URLSearchParams({ limit: '50' });
		if (before) query.set('before', before);
		return call<InboxPage>(`/v1/operator/leads?${query.toString()}`);
	}
};

/** How a lead's notification reads in the inbox. */
export function notificationLabel(lead: InboxLead): { text: string; variant: 'live' | 'pending' | 'attention' | 'draft' } {
	const { attempts, last_error } = lead.notification;
	switch (lead.state) {
		case 'delivered':
			return { text: 'Notified', variant: 'live' };
		case 'notification_dead':
			return { text: `Not notified (${attempts} attempts${last_error ? `, ${last_error.replaceAll('_', ' ')}` : ''})`, variant: 'attention' };
		case 'notification_pending':
			return { text: `Retrying (attempt ${attempts} failed)`, variant: 'pending' };
		default:
			return { text: 'Saved, notifying', variant: 'draft' };
	}
}
