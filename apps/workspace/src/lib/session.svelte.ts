// Whether the operator is signed in. The server decides; this only remembers its last answer.

import { ApiError, api } from './api.ts';

export type SessionStatus = 'checking' | 'signed-out' | 'signed-in' | 'unreachable';

export const session = $state<{ status: SessionStatus }>({ status: 'checking' });

export async function checkSession(): Promise<void> {
	try {
		await api.session();
		session.status = 'signed-in';
	} catch (error) {
		session.status = error instanceof ApiError && error.status === 401 ? 'signed-out' : 'unreachable';
	}
}

/** A request answered 401 mid-session: show the sign-in form again. */
export function sessionEnded(error: unknown): boolean {
	if (error instanceof ApiError && error.status === 401) {
		session.status = 'signed-out';
		return true;
	}
	return false;
}
