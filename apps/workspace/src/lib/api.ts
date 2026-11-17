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
	consent_notice: string;
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

/** One refused part of a record: where, why, and for an unbound claim the words that made it. */
export interface Problem {
	path: string;
	code: string;
	detail?: string | null;
	latest_revision?: number;
}

/** A refusal from the server, with its stable code and any problems by path. */
export class ApiError extends Error {
	constructor(
		readonly status: number,
		readonly code: string,
		message: string,
		readonly problems: Problem[] = []
	) {
		super(message);
	}
}

/** The request never got an answer (offline, the server down, a proxy timeout). */
export class NoAnswer extends Error {}

async function call<T>(path: string, init: RequestInit = {}): Promise<T> {
	let response: Response;
	try {
		const json = typeof init.body === 'string';
		response = await fetch(path, {
			credentials: 'same-origin',
			...init,
			headers: { accept: 'application/json', ...(json ? { 'content-type': 'application/json' } : {}), ...init.headers }
		});
	} catch {
		throw new NoAnswer('no answer');
	}
	if (response.status === 204) return undefined as T;
	const body: unknown = await response.json().catch(() => ({}));
	if (!response.ok) {
		const error = (body as { error?: { code?: string; message?: string; problems?: Problem[] } }).error;
		throw new ApiError(response.status, error?.code ?? 'unknown', error?.message ?? `request failed (${response.status})`, error?.problems ?? []);
	}
	return body as T;
}

// ---- The studio's records (I1) ----------------------------------------------------------

export interface Tenant {
	id: string;
	name: string;
	created_at: string;
}

export type FactKind =
	| 'testimonial'
	| 'metric'
	| 'price'
	| 'guarantee'
	| 'licence'
	| 'certification'
	| 'insurance'
	| 'service_area'
	| 'opening_hours'
	| 'contact_details'
	| 'legal_promise';

export type FactStatus = 'proposed' | 'approved' | 'stale' | 'withdrawn';

export interface Fact {
	id: string;
	kind: FactKind;
	statement: string;
	source: string;
	link: { kind: 'tel' | 'mailto' | 'url'; value: string; href: string } | null;
	status: FactStatus;
	approved_by: string | null;
	approved_on: string | null;
	review_by: string | null;
	review_days: number;
	created_at: string;
	withdrawn_at: string | null;
}

export interface FactList {
	today: string;
	facts: Fact[];
	truncated: boolean;
}

export type Permission = 'inspiration_only' | 'own_content' | 'licensed_assets';

export interface Brief {
	business_name: string;
	goal: 'quote_requests';
	intent: string;
	audience: string;
	services: { name: string; description: string | null }[];
	brand: { tone: string | null; colors: string[]; logo: string | null };
	domains: string[];
	references: { url: string; permission: Permission; notes: string | null }[];
	claims: string[];
}

export interface BriefRecord {
	id: string;
	tenant_id: string;
	revision: number;
	latest_revision: number;
	created_at: string;
	revised_at: string;
	brief: Brief;
}

export interface BriefSummary {
	id: string;
	business_name: string;
	latest_revision: number;
	created_at: string;
	updated_at: string;
}

export interface Asset {
	id: string;
	media_type: string;
	bytes: number;
	width: number | null;
	height: number | null;
	removed_parts: number;
	sha256: string;
	original: { name: string | null; media_type: string; bytes: number; sha256: string };
	licence: string;
	source: string;
	created_at: string;
}

export interface Snapshot {
	id: string;
	url: string;
	outcome: 'stored' | 'refused';
	refusal: string | null;
	final_url: string | null;
	sha256: string | null;
	bytes: number | null;
	media_type: string | null;
	fetched_at: string;
}

const tenantPath = (tenant: string) => `/v1/operator/tenants/${encodeURIComponent(tenant)}`;

export const api = {
	session: () => call<{ signed_in: boolean }>('/v1/operator/session'),
	signIn: (password: string) =>
		call<{ signed_in: boolean }>('/v1/operator/session', { method: 'POST', body: JSON.stringify({ password }) }),
	signOut: () => call<void>('/v1/operator/session', { method: 'DELETE' }),
	leads: (before?: string) => {
		const query = new URLSearchParams({ limit: '50' });
		if (before) query.set('before', before);
		return call<InboxPage>(`/v1/operator/leads?${query.toString()}`);
	},

	tenants: () => call<{ tenants: Tenant[] }>('/v1/operator/tenants'),
	createTenant: (name: string) => call<Tenant>('/v1/operator/tenants', { method: 'POST', body: JSON.stringify({ name }) }),

	facts: (tenant: string) => call<FactList>(`${tenantPath(tenant)}/facts`),
	createFact: (tenant: string, fact: { kind: string; statement: string; source: string; link?: Record<string, string> }) =>
		call<Fact>(`${tenantPath(tenant)}/facts`, { method: 'POST', body: JSON.stringify(fact) }),
	approveFact: (tenant: string, fact: string, approval: { approved_by: string; approved_on: string; review_by?: string }) =>
		call<Fact>(`${tenantPath(tenant)}/facts/${encodeURIComponent(fact)}/approval`, { method: 'POST', body: JSON.stringify(approval) }),
	withdrawFact: (tenant: string, fact: string) =>
		call<Fact>(`${tenantPath(tenant)}/facts/${encodeURIComponent(fact)}/withdrawal`, { method: 'POST', body: '{}' }),

	briefs: (tenant: string) => call<{ briefs: BriefSummary[] }>(`${tenantPath(tenant)}/briefs`),
	brief: (tenant: string, id: string, revision?: number) =>
		call<BriefRecord>(`${tenantPath(tenant)}/briefs/${encodeURIComponent(id)}${revision ? `?revision=${revision}` : ''}`),
	createBrief: (tenant: string, brief: unknown) =>
		call<{ id: string; revision: number }>(`${tenantPath(tenant)}/briefs`, { method: 'POST', body: JSON.stringify(brief) }),
	reviseBrief: (tenant: string, id: string, expected_revision: number, brief: unknown) =>
		call<{ id: string; revision: number }>(`${tenantPath(tenant)}/briefs/${encodeURIComponent(id)}`, {
			method: 'PUT',
			body: JSON.stringify({ expected_revision, brief })
		}),
	snapshots: (tenant: string, brief: string) =>
		call<{ store: boolean; snapshots: Snapshot[] }>(`${tenantPath(tenant)}/briefs/${encodeURIComponent(brief)}/snapshots`),
	fetchSnapshot: (tenant: string, brief: string, reference: number) =>
		call<Snapshot>(`${tenantPath(tenant)}/briefs/${encodeURIComponent(brief)}/snapshots`, {
			method: 'POST',
			body: JSON.stringify({ reference })
		}),

	assets: (tenant: string) => call<{ store: boolean; assets: Asset[] }>(`${tenantPath(tenant)}/assets`),
	assetUrl: (tenant: string, asset: string) => `${tenantPath(tenant)}/assets/${encodeURIComponent(asset)}/content`,
	uploadAsset: (tenant: string, file: File, licence: string, source: string) => {
		const form = new FormData();
		form.set('licence', licence);
		form.set('source', source);
		form.set('file', file, file.name);
		// The header a cross-site form cannot send (the server requires it).
		return call<Asset>(`${tenantPath(tenant)}/assets`, { method: 'POST', body: form, headers: { 'x-intent-upload': '1' } });
	}
};

/**
 * How a lead's notification reads in the inbox. Delivery is at least once: after an uncertain
 * attempt (a timeout, or a lease that expired mid-attempt) the provider may have sent it already,
 * so the inbox says so instead of implying exactly once.
 */
export function notificationLabel(lead: InboxLead): { text: string; variant: 'live' | 'pending' | 'attention' | 'draft' } {
	const { attempts, last_error, uncertain_attempts } = lead.notification;
	switch (lead.state) {
		case 'delivered':
			return { text: uncertain_attempts > 0 ? 'Notified (may have been sent twice)' : 'Notified', variant: 'live' };
		case 'notification_dead':
			return { text: `Not notified (${attempts} attempts${last_error ? `, ${last_error.replaceAll('_', ' ')}` : ''})`, variant: 'attention' };
		case 'notification_pending':
			return {
				text:
					uncertain_attempts > 0
						? `Retrying (attempt ${attempts} failed; an earlier one may have reached the provider)`
						: `Retrying (attempt ${attempts} failed)`,
				variant: 'pending'
			};
		default:
			return { text: 'Saved, notifying', variant: 'draft' };
	}
}
