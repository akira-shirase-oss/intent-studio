//! `intent-server reconcile`: after a drill or a load run, checks what clients were told
//! against what the database holds.
//!
//! Input: the k6 console file, where every acknowledged submission was logged as
//! `ACK <submission_id> <lead_id> <status>`. Pass conditions (failure case 3 in `docs/END_GOAL.md`): every
//! acknowledged submission exists exactly once, under the lead id it was acknowledged with; a
//! resubmission was answered with the same lead; every lead has exactly one outbox row; with
//! `--require-terminal`, every outbox row is `delivered`, or `dead` after its last attempt.

use std::collections::{BTreeMap, HashMap};

use intent_db::PrefixReport;
use serde::Serialize;
use uuid::Uuid;

/// One acknowledged submission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ack {
    /// The client's submission id.
    pub submission_id: String,
    /// The lead id the server returned.
    pub lead_id: Uuid,
}

/// Parses `ACK <submission_id> <lead_id> <status>` out of each line (k6 wraps console output
/// as `msg="..."`); other lines are ignored.
pub fn parse_acks(text: &str) -> Result<Vec<Ack>, String> {
    let mut acks = Vec::new();
    for (number, line) in text.lines().enumerate() {
        let Some(start) = line.find("ACK ") else {
            continue;
        };
        let rest = line[start + 4..].trim_end_matches('"');
        let mut parts = rest.split_whitespace();
        let (Some(submission_id), Some(lead_id)) = (parts.next(), parts.next()) else {
            return Err(format!("line {}: malformed ACK", number + 1));
        };
        let lead_id = Uuid::parse_str(lead_id)
            .map_err(|_| format!("line {}: lead id is not a UUID", number + 1))?;
        acks.push(Ack {
            submission_id: submission_id.to_owned(),
            lead_id,
        });
    }
    Ok(acks)
}

/// The verdict.
#[derive(Debug, Default, Serialize)]
pub struct Verdict {
    /// Acknowledgments read (resubmissions included).
    pub acknowledgments: usize,
    /// Distinct acknowledged submissions.
    pub acknowledged_submissions: usize,
    /// Leads stored under the run's prefix.
    pub stored_leads: usize,
    /// Stored but never acknowledged (a commit whose answer was lost, e.g. at a deadline or a
    /// kill): allowed, never the reverse.
    pub stored_not_acknowledged: usize,
    /// Outbox rows by `state/attempts`.
    pub outbox: BTreeMap<String, i64>,
    /// Uncertain attempts (a duplicate notification is possible after each).
    pub uncertain_attempts: i64,
    /// Every broken pass condition; empty means pass.
    pub violations: Vec<String>,
}

/// Compares acknowledgments with the database.
pub fn judge(
    acks: &[Ack],
    report: &PrefixReport,
    require_terminal: bool,
    max_attempts: u8,
) -> Verdict {
    let mut verdict = Verdict {
        acknowledgments: acks.len(),
        stored_leads: report.leads.values().map(Vec::len).sum(),
        uncertain_attempts: report.outbox.uncertain_attempts,
        ..Verdict::default()
    };
    let mut acknowledged: HashMap<&str, Uuid> = HashMap::new();
    let mut violations = Vec::new();
    let note = |violations: &mut Vec<String>, text: String| {
        if violations.len() < 50 {
            violations.push(text);
        }
    };
    let mut replay_mismatches = 0_usize;
    for ack in acks {
        match acknowledged.insert(&ack.submission_id, ack.lead_id) {
            Some(previous) if previous != ack.lead_id => {
                replay_mismatches += 1;
                note(
                    &mut violations,
                    format!(
                        "{} was acknowledged as two different leads",
                        ack.submission_id
                    ),
                );
            }
            _ => {}
        }
    }
    verdict.acknowledged_submissions = acknowledged.len();
    let mut missing = 0_usize;
    for (submission, lead) in &acknowledged {
        match report.leads.get(&format!("c:{submission}")) {
            None => {
                missing += 1;
                note(
                    &mut violations,
                    format!("acknowledged {submission} is not stored"),
                );
            }
            Some(ids) if ids.len() != 1 => {
                note(
                    &mut violations,
                    format!("{submission} is stored {} times", ids.len()),
                );
            }
            Some(ids) if ids[0] != *lead => {
                note(
                    &mut violations,
                    format!("{submission} is stored under a different lead id"),
                );
            }
            Some(_) => {}
        }
    }
    if missing > 0 {
        violations.insert(0, format!("{missing} acknowledged submission(s) missing"));
    }
    if replay_mismatches > 0 {
        violations.insert(
            0,
            format!("{replay_mismatches} resubmission(s) answered with a different lead"),
        );
    }
    let duplicates = report.leads.values().filter(|ids| ids.len() > 1).count();
    if duplicates > 0 {
        violations.push(format!("{duplicates} duplicate logical lead(s)"));
    }
    if report.leads_without_outbox > 0 {
        violations.push(format!(
            "{} lead(s) without an outbox row",
            report.leads_without_outbox
        ));
    }
    if report.leads_with_several_outbox_rows > 0 {
        violations.push(format!(
            "{} lead(s) with several outbox rows",
            report.leads_with_several_outbox_rows
        ));
    }
    verdict.stored_not_acknowledged = report
        .leads
        .keys()
        .filter(|key| !acknowledged.contains_key(key.trim_start_matches("c:")))
        .count();
    check_outbox(
        report,
        require_terminal,
        max_attempts,
        &mut verdict,
        &mut violations,
    );
    verdict.violations = violations;
    verdict
}

/// Tallies the outbox and, with `require_terminal`, flags rows that are neither delivered nor
/// dead after their last attempt.
fn check_outbox(
    report: &PrefixReport,
    require_terminal: bool,
    max_attempts: u8,
    verdict: &mut Verdict,
    violations: &mut Vec<String>,
) {
    for ((state, attempts), rows) in &report.outbox.by_state_attempts {
        verdict.outbox.insert(format!("{state}/{attempts}"), *rows);
        if require_terminal {
            let terminal =
                state == "delivered" || (state == "dead" && *attempts >= i16::from(max_attempts));
            if !terminal && state != "dead" {
                violations.push(format!(
                    "{rows} outbox row(s) still {state} after {attempts} attempt(s)"
                ));
            } else if !terminal {
                violations.push(format!(
                    "{rows} outbox row(s) dead after only {attempts} attempt(s)"
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use intent_db::OutboxTally;

    use super::*;

    fn lead(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn report(entries: &[(&str, &[u128])], outbox: &[(&str, i16, i64)]) -> PrefixReport {
        PrefixReport {
            leads: entries
                .iter()
                .map(|(k, ids)| ((*k).to_owned(), ids.iter().map(|n| lead(*n)).collect()))
                .collect(),
            leads_without_outbox: 0,
            leads_with_several_outbox_rows: 0,
            outbox: OutboxTally {
                by_state_attempts: outbox
                    .iter()
                    .map(|(s, a, n)| (((*s).to_owned(), *a), *n))
                    .collect(),
                uncertain_attempts: 0,
            },
        }
    }

    #[test]
    fn parses_k6_console_lines() {
        let text = format!(
            "time=\"t\" level=info msg=\"ACK run-1 {} 201\"\nnoise\ntime=\"t\" level=info msg=\"ACK run-1 {} 200\"\n",
            lead(1),
            lead(1)
        );
        let acks = parse_acks(&text).unwrap();
        assert_eq!(acks.len(), 2);
        assert_eq!(acks[0].submission_id, "run-1");
        assert!(parse_acks("msg=\"ACK only\"").is_err());
    }

    #[test]
    fn a_clean_run_passes() {
        let acks = vec![
            Ack {
                submission_id: "r-1".into(),
                lead_id: lead(1),
            },
            Ack {
                submission_id: "r-1".into(),
                lead_id: lead(1),
            },
            Ack {
                submission_id: "r-2".into(),
                lead_id: lead(2),
            },
        ];
        let db = report(
            &[("c:r-1", &[1]), ("c:r-2", &[2]), ("c:r-3", &[3])],
            &[("delivered", 1, 3)],
        );
        let verdict = judge(&acks, &db, true, 8);
        assert!(verdict.violations.is_empty(), "{:?}", verdict.violations);
        assert_eq!(verdict.acknowledged_submissions, 2);
        assert_eq!(verdict.stored_not_acknowledged, 1);
    }

    #[test]
    fn a_lost_acknowledged_lead_fails() {
        let acks = vec![Ack {
            submission_id: "r-9".into(),
            lead_id: lead(9),
        }];
        let verdict = judge(&acks, &report(&[], &[]), false, 8);
        assert_eq!(
            verdict.violations[0],
            "1 acknowledged submission(s) missing"
        );
    }

    #[test]
    fn a_replay_with_another_lead_and_unfinished_outbox_rows_fail() {
        let acks = vec![
            Ack {
                submission_id: "r-1".into(),
                lead_id: lead(1),
            },
            Ack {
                submission_id: "r-1".into(),
                lead_id: lead(2),
            },
        ];
        let db = report(&[("c:r-1", &[1])], &[("pending", 3, 1), ("dead", 2, 1)]);
        let verdict = judge(&acks, &db, true, 8);
        let text = verdict.violations.join("\n");
        assert!(text.contains("answered with a different lead"), "{text}");
        assert!(text.contains("still pending after 3"), "{text}");
        assert!(text.contains("dead after only 2"), "{text}");
    }
}
