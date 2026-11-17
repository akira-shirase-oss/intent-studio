// The clients (tenants) the operator serves, and which one the studio screens are showing. The
// choice is remembered per browser for convenience; the server checks every request anyway.

import { api, type Tenant } from './api.ts';
import { failureText } from './words.ts';
import { sessionEnded } from './session.svelte.ts';

const KEY = 'intent-studio-client';

export const clients = $state<{
	status: 'idle' | 'loading' | 'ready' | 'error';
	list: Tenant[];
	selected: string;
	error: string;
}>({ status: 'idle', list: [], selected: '', error: '' });

function remembered(): string {
	try {
		return localStorage.getItem(KEY) ?? '';
	} catch {
		return '';
	}
}

export function selectClient(id: string): void {
	clients.selected = id;
	try {
		localStorage.setItem(KEY, id);
	} catch {
		// A convenience only.
	}
}

export async function loadClients(force = false): Promise<void> {
	if (clients.status === 'ready' && !force) return;
	clients.status = 'loading';
	try {
		const { tenants } = await api.tenants();
		clients.list = tenants;
		const wanted = clients.selected || remembered();
		const first = tenants[0];
		selectClient(tenants.some((t) => t.id === wanted) ? wanted : first ? first.id : '');
		clients.status = 'ready';
	} catch (error) {
		if (sessionEnded(error)) return;
		clients.error = failureText(error);
		clients.status = 'error';
	}
}

export async function addClient(name: string): Promise<Tenant> {
	const tenant = await api.createTenant(name);
	clients.list = [tenant, ...clients.list];
	selectClient(tenant.id);
	return tenant;
}
