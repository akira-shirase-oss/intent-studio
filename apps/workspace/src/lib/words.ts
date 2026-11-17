// Plain words for everything the server names with a code. The codes stay available behind a
// "details" toggle; the operator reads these.

import { ApiError, NoAnswer, type Fact, type FactKind, type Permission, type Problem } from './api.ts';

export const kindLabels: Record<FactKind, string> = {
	testimonial: 'Customer review',
	metric: 'Number about the business',
	price: 'Price',
	guarantee: 'Guarantee',
	licence: 'Licence or registration',
	certification: 'Certification',
	insurance: 'Insurance',
	service_area: 'Where they work',
	opening_hours: 'Opening hours',
	contact_details: 'Contact details',
	legal_promise: 'Legal promise'
};

export const kindHints: Record<FactKind, string> = {
	testimonial: 'The customer’s words and who said them, e.g. “Fixed our boiler the same day.” — Sam, Leeds',
	metric: 'e.g. “Over 500 boilers fitted since 2015”',
	price: 'e.g. “Boiler service from £80”. Prices need re-approving every 30 days.',
	guarantee: 'e.g. “12-month guarantee on parts and labour”',
	licence: 'e.g. “Gas Safe registered, number 123456”',
	certification: 'e.g. “Which? Trusted Trader”',
	insurance: 'e.g. “£5 million public liability insurance”',
	service_area: 'e.g. “Leeds and 10 miles around”',
	opening_hours: 'e.g. “Mon–Fri 8:00–18:00”. Hours need re-approving every 30 days.',
	contact_details: 'e.g. “Call 0113 496 0000”. Add the number, e-mail or web address below so pages can link to it.',
	legal_promise: 'e.g. “You can cancel within 14 days”'
};

export const permissionLabels: Record<Permission, string> = {
	inspiration_only: 'Look only: layout and tone, nothing copied',
	own_content: 'The client’s own site: its text and images may be reused',
	licensed_assets: 'Images may be reused under a licence (say which in the notes)'
};

const dateFormat = new Intl.DateTimeFormat(undefined, { dateStyle: 'medium', timeZone: 'UTC' });
const dateTimeFormat = new Intl.DateTimeFormat(undefined, { dateStyle: 'medium', timeStyle: 'short' });

/** `YYYY-MM-DD` as a calendar day, in UTC (the server's day). */
export function day(text: string | null): string {
	return text ? dateFormat.format(new Date(`${text}T00:00:00Z`)) : '';
}

export function when(text: string | null): string {
	return text ? dateTimeFormat.format(new Date(text)) : '';
}

/** Today in UTC as `YYYY-MM-DD`, the format the server takes. */
export function todayUtc(offsetDays = 0): string {
	const now = new Date(Date.now() + offsetDays * 86_400_000);
	return now.toISOString().slice(0, 10);
}

export function factStatus(fact: Fact): { text: string; variant: 'live' | 'pending' | 'attention' | 'draft' } {
	switch (fact.status) {
		case 'approved':
			return { text: `Approved until ${day(fact.review_by)}`, variant: 'live' };
		case 'stale':
			return { text: `Approval ran out after ${day(fact.review_by)}`, variant: 'attention' };
		case 'withdrawn':
			return { text: 'Withdrawn', variant: 'draft' };
		default:
			return { text: 'Waiting for approval', variant: 'pending' };
	}
}

const problemWords: Record<string, string> = {
	malformed: 'This is not in the expected form.',
	required: 'This is required.',
	too_long: 'This is too long.',
	too_short: 'This is too short.',
	too_many: 'There are too many of these.',
	too_few: 'Add at least one.',
	invalid_characters: 'This contains hidden or control characters. Retype it rather than pasting.',
	invalid_format: 'This is not in the right format.',
	unknown_value: 'This is not one of the choices.',
	duplicate: 'This appears twice.',
	in_future: 'This date is in the future.',
	review_before_approval: 'The review date must be after the approval date.',
	review_too_late: 'The review date is later than this kind of fact allows.',
	fact_unknown: 'This fact does not exist for this client.',
	fact_unapproved: 'This fact has not been approved yet.',
	fact_stale: 'This fact’s approval has run out. Record a new approval first.',
	fact_withdrawn: 'This fact was withdrawn and can no longer be used.',
	fact_kind_mismatch: 'This kind of fact cannot go here.',
	fact_not_in_brief: 'The brief does not list this fact among its approved claims.',
	fact_has_no_link: 'This contact fact has no phone number, e-mail or web address to link to.',
	link_not_allowed: 'Pages may only link to the client’s own web addresses and contact details.',
	anchor_unknown: 'There is no section with that id on the page.',
	page_unknown: 'There is no page at that path.',
	service_unknown: 'The brief does not offer this service.',
	missing_home: 'The site needs a home page.',
	missing_quote_form: 'The home page needs a quote form.',
	unknown_component_set: 'This page tree uses a component set this server does not have.',
	asset_unknown: 'This file does not exist for this client.',
	too_many_problems: 'There are more problems than shown here.'
};

/** One problem in plain words. */
export function problemText(problem: Problem): string {
	if (problem.code === 'unbound_claim') {
		return `“${problem.detail ?? ''}” reads as a claim (a number, a price or a promise) that no approved fact supports. Remove it or bind a fact that says it.`;
	}
	return problemWords[problem.code] ?? 'This was refused.';
}

/** The words a refused file or fetch gets (the `reason` the server sends as a problem code). */
const refusalWords: Record<string, string> = {
	unsupported_type: 'This type of file is not accepted. Upload an SVG, PNG or JPEG image, or a TTF or OTF font.',
	type_mismatch: 'The file’s contents are not the type its name says.',
	malformed: 'The file is damaged or is not a valid file of its type.',
	too_large: 'The file is too large (at most 10 MiB, and 8,192 pixels a side).',
	worker_timeout: 'Checking the file took too long, so it was stopped.',
	worker_crashed: 'The file made the checker fail, so it was refused. Nothing else was affected.',
	worker_unavailable: 'The file checker could not be started on this server.',
	invalid_url: 'The address is not a valid web address.',
	scheme_not_allowed: 'Only http and https addresses can be fetched.',
	credentials_in_url: 'Addresses with a user name or password are not fetched.',
	port_not_allowed: 'Only the standard web ports are fetched.',
	address_forbidden: 'The address points to a private, local or reserved network, which is never fetched.',
	dns_failed: 'The site’s name could not be looked up.',
	connect_failed: 'The site could not be reached.',
	tls_failed: 'The site’s secure connection could not be verified.',
	too_many_redirects: 'The site redirected more than three times.',
	redirect_downgrade: 'The site redirected from https to plain http.',
	redirect_invalid: 'The site redirected to an invalid address.',
	http_status: 'The site answered with an error instead of the page.',
	timeout: 'The site did not answer within 10 seconds.',
	protocol_error: 'The site did not answer like a web server.'
};

export function refusalText(code: string | null | undefined): string {
	return (code && refusalWords[code]) || 'It was refused.';
}

/**
 * What to tell the operator about a failed request. A write that got no answer, or a 503 from a
 * write, may or may not have happened: say so instead of claiming either.
 */
export function failureText(error: unknown, write = false): string {
	if (error instanceof NoAnswer) {
		return write
			? 'The server did not answer, so we cannot tell whether this was saved. Reload and check before trying again.'
			: 'The server could not be reached. Check your connection and try again.';
	}
	if (error instanceof ApiError) {
		if (error.status === 401) return 'Your session has ended. Sign in again.';
		if (error.code === 'rate_limited') return 'Too many attempts. Wait a minute and try again.';
		if (error.code === 'store_unavailable') return 'This server has no file store configured, so files cannot be kept. Ask whoever runs it to set INTENT_STORE_DIR.';
		if (error.code === 'overloaded') return 'The server is busy. Try again in a moment.';
		if (error.status === 503 || error.code === 'timeout') {
			return write
				? 'The server could not confirm this was saved. Reload and check before trying again.'
				: 'The server is not available right now. Try again in a moment.';
		}
		if (error.status === 404) return 'This no longer exists, or belongs to another client.';
		return error.message;
	}
	return 'Something went wrong. Try again.';
}

/** Problems keyed by path, for showing each next to its field. */
export function problemsByPath(problems: Problem[]): Map<string, string> {
	const map = new Map<string, string>();
	for (const problem of problems) {
		const existing = map.get(problem.path);
		map.set(problem.path, existing ? `${existing} ${problemText(problem)}` : problemText(problem));
	}
	return map;
}
