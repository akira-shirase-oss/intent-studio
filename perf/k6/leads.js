// Lead submissions against intent-server (D-025: the load unit is one lead submission).
//
// Open model (constant arrival rate): submissions start on schedule whether or not earlier ones
// have finished, so a slow server shows up as latency and dropped iterations, not as a quietly
// lower rate. Every 20th iteration resubmits an earlier lead with the same id and content (5%
// deliberate resubmissions, END_GOAL failure case 3). Every acknowledged submission is logged as
// `ACK <submission_id> <lead_id> <status>` so `intent-server reconcile` can check each one
// against the database afterwards. Run through scripts/load.sh, which sets everything below.
//
//   BASE_URL   intent-server            (http://127.0.0.1:59403)
//   RUN        submission-id prefix     (required, unique per run)
//   SITES      demo sites seeded        (10000)
//   RATE       submissions per second   (50)
//   TOTAL      submissions in the run   (RATE * 60)
//   P99_MS     http_req_duration p99 threshold in ms   (100)
//   SUMMARY    where handleSummary writes the k6 summary JSON

import http from 'k6/http';
import exec from 'k6/execution';
import { Counter, Rate } from 'k6/metrics';

const BASE_URL = __ENV.BASE_URL || 'http://127.0.0.1:59403';
const RUN = __ENV.RUN;
const SITES = Number(__ENV.SITES || 10000);
const RATE = Number(__ENV.RATE || 50);
const TOTAL = Number(__ENV.TOTAL || RATE * 60);
const P99_MS = Number(__ENV.P99_MS || 100);
const DURATION_S = Math.ceil(TOTAL / RATE);
const MAX_DROPPED = Math.max(1, Math.floor(TOTAL * 0.0001));

if (!RUN) throw new Error('RUN is required');

const acknowledged = new Rate('lead_acknowledged');
const refused = new Counter('lead_refused');
const unanswered = new Counter('lead_unanswered');
const replays = new Counter('lead_replays');

export const options = {
	discardResponseBodies: false,
	scenarios: {
		leads: {
			executor: 'constant-arrival-rate',
			rate: RATE,
			timeUnit: '1s',
			duration: `${DURATION_S}s`,
			preAllocatedVUs: Math.min(2000, Math.max(50, RATE)),
			maxVUs: Math.min(5000, Math.max(200, RATE * 4)),
			gracefulStop: '30s'
		}
	},
	thresholds: {
		http_req_duration: [`p(99)<${P99_MS}`],
		lead_acknowledged: ['rate>0.999'],
		dropped_iterations: [`count<${MAX_DROPPED}`]
	},
	summaryTrendStats: ['min', 'med', 'avg', 'p(90)', 'p(95)', 'p(99)', 'p(99.9)', 'max']
};

function siteId(index) {
	return `00000000-0000-4000-9000-${index.toString(16).padStart(12, '0')}`;
}

function lead(n) {
	return {
		submission_id: `${RUN}-${n}`,
		name: `Load Lead ${n}`,
		email: `load.${n}@example.test`,
		phone: `+1 555 01${String(n % 100).padStart(2, '0')} ${String(n % 10000).padStart(4, '0')}`,
		service: 'boiler service',
		area: 'N1',
		message: `Quote request ${n}: the boiler makes a noise when the heating starts.`,
		consent: true
	};
}

export default function () {
	const n = exec.scenario.iterationInTest;
	const replay = n % 20 === 19 && n >= 10;
	const target = replay ? n - 10 : n;
	const body = lead(target);
	const response = http.post(`${BASE_URL}/v1/sites/${siteId(target % SITES)}/leads`, JSON.stringify(body), {
		headers: {
			'content-type': 'application/json',
			// One address per submission (the server trusts 127.0.0.1 as a proxy in load runs), so
			// the per-address limit is exercised through its bounded table, not tripped.
			'x-forwarded-for': `10.${(n >> 16) & 255}.${(n >> 8) & 255}.${n & 255}`
		},
		timeout: '10s',
		tags: { name: 'POST /v1/sites/{site}/leads' }
	});
	const ok = response.status === 201 || response.status === 200;
	acknowledged.add(ok);
	if (ok) {
		const leadId = response.json('lead_id');
		if (response.status === 200) replays.add(1);
		console.log(`ACK ${body.submission_id} ${leadId} ${response.status}`);
	} else if (response.status === 0) {
		unanswered.add(1);
	} else {
		refused.add(1, { status: String(response.status) });
	}
}

export function handleSummary(data) {
	const out = {};
	if (__ENV.SUMMARY) out[__ENV.SUMMARY] = JSON.stringify(data, null, 2);
	return out;
}
