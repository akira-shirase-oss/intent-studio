//! In-process counters, served at `/metrics` in the Prometheus text format. Counts only: no
//! labels carry ids or content, so the series are bounded.

use std::{
    fmt::Write as _,
    sync::atomic::{AtomicU64, Ordering},
};

use intent_domain::outbox::AttemptOutcome;

macro_rules! counters {
    ($($name:ident: $help:literal),* $(,)?) => {
        /// The counters.
        #[derive(Debug, Default)]
        pub struct Metrics {
            $(#[doc = $help] pub $name: AtomicU64,)*
        }

        impl Metrics {
            /// Renders every counter (plus the extra counters and gauges given) as Prometheus
            /// text.
            pub fn render(&self, counters: &[(&str, &str, u64)], gauges: &[(&str, &str, u64)]) -> String {
                let mut out = String::new();
                for (name, help, value) in counters {
                    let _ = writeln!(out, "# HELP intent_{name} {help}");
                    let _ = writeln!(out, "# TYPE intent_{name} counter");
                    let _ = writeln!(out, "intent_{name} {value}");
                }
                $(
                    let _ = writeln!(out, "# HELP intent_{} {}", stringify!($name), $help);
                    let _ = writeln!(out, "# TYPE intent_{} counter", stringify!($name));
                    let _ = writeln!(out, "intent_{} {}", stringify!($name), self.$name.load(Ordering::Relaxed));
                )*
                for (name, help, value) in gauges {
                    let _ = writeln!(out, "# HELP intent_{name} {help}");
                    let _ = writeln!(out, "# TYPE intent_{name} gauge");
                    let _ = writeln!(out, "intent_{name} {value}");
                }
                out
            }
        }
    };
}

counters! {
    leads_created_total: "Leads committed with their outbox row and acknowledged with 201.",
    leads_replayed_total: "Resubmissions answered with the existing lead (200).",
    intake_conflicts_total: "Submission ids reused for different content (409).",
    intake_invalid_total: "Submissions refused for malformed bodies or invalid fields.",
    intake_honeypot_total: "Submissions refused because the honeypot was filled.",
    intake_rate_limited_address_total: "Submissions refused by the per-address limit.",
    intake_rate_limited_site_total: "Submissions refused by the per-site limit.",
    intake_unavailable_total: "Submissions refused because the database could not take them (503).",
    intake_timeouts_total: "Submissions whose store did not finish before the intake deadline (503).",
    intake_shed_total: "Requests shed because too many were in flight (503).",
    notify_attempts_total: "Notification attempts made.",
    notify_delivered_total: "Notifications the provider accepted.",
    notify_unavailable_total: "Attempts the provider refused for now.",
    notify_timeouts_total: "Attempts that timed out (uncertain: a duplicate is possible).",
    notify_rejected_total: "Attempts the provider refused permanently.",
    notify_dead_total: "Outbox rows that stopped (attempts exhausted or rejected).",
    notify_lease_lost_total: "Results discarded because the lease had expired.",
    notify_swept_total: "Rows marked dead after dying holding their last attempt.",
    signout_tombstoned_answers_total: "Sign-outs answered 204 from memory for a delete already confirmed.",
}

impl Metrics {
    /// Counts one attempt's outcome.
    pub fn attempt(&self, outcome: AttemptOutcome) {
        self.notify_attempts_total.fetch_add(1, Ordering::Relaxed);
        let counter = match outcome {
            AttemptOutcome::Accepted => &self.notify_delivered_total,
            AttemptOutcome::Unavailable => &self.notify_unavailable_total,
            AttemptOutcome::TimedOut => &self.notify_timeouts_total,
            AttemptOutcome::Rejected => &self.notify_rejected_total,
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    /// Adds one to a counter.
    pub fn inc(counter: &AtomicU64) {
        counter.fetch_add(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_every_counter_and_gauge() {
        let metrics = Metrics::default();
        Metrics::inc(&metrics.leads_created_total);
        metrics.attempt(AttemptOutcome::TimedOut);
        let text = metrics.render(
            &[("extra_total", "An extra counter.", 7)],
            &[("in_flight", "Requests in flight.", 3)],
        );
        assert!(text.contains("# TYPE intent_extra_total counter\nintent_extra_total 7\n"));
        assert!(text.contains("intent_leads_created_total 1\n"));
        assert!(text.contains("intent_notify_timeouts_total 1\n"));
        assert!(text.contains("intent_notify_attempts_total 1\n"));
        assert!(text.contains("# TYPE intent_in_flight gauge\nintent_in_flight 3\n"));
    }
}
