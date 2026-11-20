// The studio API under load (I1): the operator's reads and the page-tree check, against
// intent-server. Not a lead path — its load is one operator — but every route that serves
// traffic gets a scenario, a threshold and, on a quiet machine, a full run (workspace rule).
//
// The unit is one studio request. The mix: 50% the fact list, 25% one brief, 25% a spec check
// of a valid page tree (the heaviest: it loads the brief and every referenced fact, validates,
// lints and audits). setup() signs in once (sign-in is limited to 5 a minute per address) and
// builds one client with approved facts and a brief; every request then reuses the session.
//
//   BASE_URL     intent-server            (http://127.0.0.1:59406)
//   PASSWORD     the operator password    (required)
//   RATE         requests per second      (200)
//   TOTAL        requests in the run      (RATE * 60; the full run is 1,000,000)
//   P99_READ_MS  p99 budget for reads     (50)
//   P99_SPEC_MS  p99 budget for the spec check (100)
//   SUMMARY      where handleSummary writes the k6 summary JSON
//
// Thresholds (proposed, D-063: to be set from the first quiet baseline, as D-054 did for
// intake): reads p99 < 50 ms, spec check p99 < 100 ms, zero failed requests, zero dropped
// iterations above 0.01%. scripts/studio-load.sh runs this and records the result under
// perf/results/.

import http from 'k6/http';
import exec from 'k6/execution';
import { check, fail } from 'k6';

const BASE_URL = __ENV.BASE_URL || 'http://127.0.0.1:59406';
const PASSWORD = __ENV.PASSWORD;
const RATE = Number(__ENV.RATE || 200);
const TOTAL = Number(__ENV.TOTAL || RATE * 60);
const P99_READ_MS = Number(__ENV.P99_READ_MS || 50);
const P99_SPEC_MS = Number(__ENV.P99_SPEC_MS || 100);
const DURATION_S = Math.ceil(TOTAL / RATE);
const MAX_DROPPED = Math.max(1, Math.floor(TOTAL * 0.0001));

if (!PASSWORD) throw new Error('PASSWORD is required');

export const options = {
	scenarios: {
		studio: {
			executor: 'constant-arrival-rate',
			rate: RATE,
			timeUnit: '1s',
			duration: `${DURATION_S}s`,
			preAllocatedVUs: Math.min(500, Math.max(20, RATE)),
			maxVUs: Math.min(2000, Math.max(100, RATE * 4)),
			gracefulStop: '30s'
		}
	},
	thresholds: {
		'http_req_duration{kind:read}': [`p(99)<${P99_READ_MS}`],
		'http_req_duration{kind:spec}': [`p(99)<${P99_SPEC_MS}`],
		http_req_failed: ['rate==0'],
		dropped_iterations: [`count<${MAX_DROPPED}`]
	},
	summaryTrendStats: ['min', 'med', 'avg', 'p(90)', 'p(95)', 'p(99)', 'p(99.9)', 'max']
};

function json(response, what) {
	if (response.status < 200 || response.status >= 300) fail(`${what}: ${response.status} ${response.body}`);
	return response.json();
}

export function setup() {
	const signIn = http.post(`${BASE_URL}/v1/operator/session`, JSON.stringify({ password: PASSWORD }), {
		headers: { 'content-type': 'application/json' }
	});
	if (signIn.status !== 200) fail(`sign-in: ${signIn.status}`);
	const cookie = signIn.headers['Set-Cookie'].split(';')[0];
	const headers = { 'content-type': 'application/json', cookie };
	const post = (path, body, what) => json(http.post(`${BASE_URL}${path}`, JSON.stringify(body), { headers }), what);

	const tenant = post('/v1/operator/tenants', { name: `Load client ${Date.now()}` }, 'tenant').id;
	const base = `/v1/operator/tenants/${tenant}`;
	const today = new Date().toISOString().slice(0, 10);
	const facts = [];
	for (let i = 0; i < 20; i += 1) {
		const kind = i === 0 ? 'price' : 'testimonial';
		const statement = i === 0 ? 'Boiler service from £80' : `"Good work, number ${i}." — Customer ${i}`;
		const fact = post(`${base}/facts`, { kind, statement, source: 'Load setup' }, 'fact');
		post(`${base}/facts/${fact.id}/approval`, { approved_by: 'Load owner', approved_on: today }, 'approval');
		facts.push(fact.id);
	}
	const brief = post(
		`${base}/briefs`,
		{
			business_name: 'Load Heating',
			goal: 'quote_requests',
			intent: 'More quote requests',
			audience: 'Home owners',
			services: [{ name: 'Boiler service' }],
			domains: ['load.example'],
			claims: facts
		},
		'brief'
	).id;
	const tree = {
		component_set: 'quote-request@0',
		pages: [
			{
				path: '/',
				title: 'Load Heating',
				description: 'Local heating engineers',
				sections: [
					{ component: 'hero', heading: 'Warm homes', action: { label: 'Get a quote', target: { anchor: 'quote' } } },
					{ component: 'services', items: [{ name: 'Boiler service', price: { fact: facts[0] } }] },
					{ component: 'testimonials', items: facts.slice(1, 7).map((fact) => ({ fact })) },
					{ component: 'quote_form', id: 'quote', services: ['Boiler service'], consent_notice: 'privacy-v1' },
					{ component: 'footer', links: [{ label: 'Blog', target: { url: 'https://load.example/blog' } }] }
				]
			}
		]
	};
	return { cookie, base, brief, tree: JSON.stringify(tree) };
}

export default function (data) {
	const n = exec.scenario.iterationInTest % 4;
	const headers = { cookie: data.cookie };
	let response;
	if (n < 2) {
		response = http.get(`${BASE_URL}${data.base}/facts`, { headers, tags: { kind: 'read', name: 'GET facts' } });
	} else if (n === 2) {
		response = http.get(`${BASE_URL}${data.base}/briefs/${data.brief}`, { headers, tags: { kind: 'read', name: 'GET brief' } });
	} else {
		response = http.post(`${BASE_URL}${data.base}/briefs/${data.brief}/spec-check`, data.tree, {
			headers: { ...headers, 'content-type': 'application/json' },
			tags: { kind: 'spec', name: 'POST spec-check' }
		});
	}
	check(response, { 'answered 200': (r) => r.status === 200 });
}

export function handleSummary(data) {
	const out = {};
	if (__ENV.SUMMARY) out[__ENV.SUMMARY] = JSON.stringify(data, null, 2);
	return out;
}
