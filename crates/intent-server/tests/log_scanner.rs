//! The log-capture check (D-029) proves itself on every needle it is given (F11): a needle the
//! scanner could not see in a log line must fail the check, never pass it silently.

mod support;

#[test]
#[should_panic(expected = "cannot catch")]
fn a_needle_the_scanner_cannot_see_fails_the_check() {
    // "two\nlines" can never appear inside one log line, so no leak of it would ever be found;
    // only planting each needle shows that.
    let log = "{\"message\":\"listening\"}\n";
    support::assert_no_leaks(log, &["ok@example.test", "two\nlines"]);
}

#[test]
fn every_needle_is_caught_when_planted_and_a_clean_log_passes() {
    let log =
        "{\"message\":\"listening\"}\n{\"message\":\"lead stored\",\"lead_id\":\"0192f4a0\"}\n";
    support::assert_no_leaks(
        log,
        &["ada@example.test", "+1 555 0100 0001", "Ada Lovelace"],
    );
}
