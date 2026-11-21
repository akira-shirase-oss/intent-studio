//! NEW-4: the dropped-parts report is bounded so the worker's framed answer always
//! fits the protocol's 1024-byte header line. Proved on the real binary — the
//! process answers, `decode_answer` (the driver's only read path) parses it, and
//! the report at the bound decodes while one past it is cut to fit. An unbounded
//! report overflows the line and `decode_answer` returns `worker_crashed`, so
//! removing the bound fails this test.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test: panics are failures"
)]

use std::io::Write as _;
use std::process::{Command, Stdio};

/// The sanitizer's report bound: at most this many refused names, each cut to this
/// many bytes (`svg.rs` keeps the product under the 1024-byte header line).
const REPORT_ENTRIES: usize = 24;
const REPORT_ENTRY_BYTES: usize = 24;

/// `names` disallowed elements inside an otherwise valid document.
fn svg_with_dropped(names: &[String]) -> String {
    let mut svg = String::from(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"8\" height=\"8\">\
         <rect width=\"8\" height=\"8\"/>",
    );
    for name in names {
        svg.push('<');
        svg.push_str(name);
        svg.push_str("/>");
    }
    svg.push_str("</svg>");
    svg
}

/// Runs the real binary on `input`; the answer is all of stdout.
fn run_worker(input: &[u8]) -> Vec<u8> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_intent-worker"))
        .arg("svg")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let bytes = input.to_vec();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&bytes);
    });
    let out = child.wait_with_output().unwrap();
    writer.join().unwrap();
    assert!(out.status.success(), "worker exited {}", out.status);
    out.stdout
}

#[test]
fn a_report_at_the_bound_decodes_and_one_past_it_is_cut_to_fit() {
    // At the bound: the largest report the protocol carries. It must decode.
    let at_bound: Vec<String> = (0..REPORT_ENTRIES).map(|n| format!("x{n:023}")).collect();
    assert_eq!(at_bound[0].len(), REPORT_ENTRY_BYTES);
    let answer = run_worker(svg_with_dropped(&at_bound).as_bytes());
    let processed = intent_render::decode_answer(&answer)
        .expect("a report at the bound must fit the header line");
    assert_eq!(processed.media_type, "image/svg+xml");
    assert_eq!(processed.dropped_parts.len(), REPORT_ENTRIES);
    assert_eq!(
        processed.removed_parts,
        u32::try_from(REPORT_ENTRIES).unwrap()
    );
    assert!(
        processed
            .dropped_parts
            .iter()
            .all(|p| p.len() <= REPORT_ENTRY_BYTES)
    );

    // Past the bound: more refusals, longer names. The report is cut — never the
    // answer: removed keeps the true count while reported stays inside the bound.
    let past_bound: Vec<String> = (0..REPORT_ENTRIES + 6)
        .map(|n| format!("y{n:039}"))
        .collect();
    let answer = run_worker(svg_with_dropped(&past_bound).as_bytes());
    let processed = intent_render::decode_answer(&answer)
        .expect("a report past the bound must be cut to fit, not crash the worker's answer");
    assert!(processed.dropped_parts.len() <= REPORT_ENTRIES);
    assert!(
        processed
            .dropped_parts
            .iter()
            .all(|p| p.len() <= REPORT_ENTRY_BYTES)
    );
    assert!(
        processed.removed_parts > u32::try_from(processed.dropped_parts.len()).unwrap(),
        "the cut report kept no evidence the extra refusals happened"
    );
}
