use assert_cmd::Command;
use predicates::prelude::*;

fn localshare() -> Command {
    Command::cargo_bin("localshare").unwrap()
}

// ── Invalid targets ───────────────────────────────────────────────────────────

#[test]
fn invalid_target_port_zero_fails() {
    localshare()
        .arg("0")
        .assert()
        .failure()
        .stderr(predicate::str::contains("port must be between 1 and 65535"));
}

#[test]
fn invalid_target_not_a_number_fails() {
    localshare()
        .arg("abc")
        .assert()
        .failure()
        .stderr(predicate::str::contains("expected a port number"));
}

#[test]
fn invalid_target_missing_port_fails() {
    localshare()
        .arg("127.0.0.1:")
        .assert()
        .failure()
        .stderr(predicate::str::contains("missing port after colon"));
}

// ── Flag conflicts ────────────────────────────────────────────────────────────

// --json and --quiet are two different output modes; there is no hard Clap
// conflict, so a bare conflict test would be meaningless. Instead we assert:
//   - an unknown flag is rejected by Clap (using error exit code 2)
//   - `--json --quiet` is accepted by the parser (both run, quiet wins)
#[test]
fn unknown_flag_is_rejected() {
    localshare()
        .args(["3000", "--definitely-not-a-flag"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unexpected argument"));
}

#[test]
fn json_and_quiet_flags_are_accepted_together() {
    // Neither flag conflicts at the Clap level; this simply asserts the
    // binary does not fail at argument-parsing time (a parse error returns
    // exit code 2). Outcome beyond parsing depends on the unreachable relay.
    localshare()
        .args(["3000", "--json", "--quiet", "-r", "ws://127.0.0.1:1"])
        .timeout(std::time::Duration::from_secs(10))
        .assert()
        .code(predicate::ne(2));
}

// ── Help & version ────────────────────────────────────────────────────────────

#[test]
fn help_contains_examples_section() {
    localshare()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("EXAMPLES:"))
        .stdout(predicate::str::contains("localshare 3000"))
        .stdout(predicate::str::contains("localshare 127.0.0.1:8080"));
}

#[test]
fn version_flag_succeeds() {
    localshare()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("localshare"));
}

// ── JSON mode ─────────────────────────────────────────────────────────────────

#[test]
fn json_mode_with_unreachable_relay_emits_valid_json() {
    // Point at an unreachable relay to force a quick, deterministic exit:
    // DNS fails instantly, so the reconnect loop emits a fatal Disconnected.
    // A fatal tunnel error must exit non-zero (1) so scripts can detect it.
    let out = localshare()
        .args(["3000", "--json", "-r", "ws://127.0.0.1:1"])
        .timeout(std::time::Duration::from_secs(15))
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();

    let text = String::from_utf8(out).expect("stdout should be valid UTF-8");
    assert!(
        text.trim().starts_with('{'),
        "expected JSON output, got: {text}"
    );
    // First emitted line should be a valid JSON object.
    let first_line = text.lines().next().expect("expected at least one line");
    let parsed: serde_json::Value =
        serde_json::from_str(first_line).expect("first line should parse as JSON");
    assert_eq!(parsed["event"], "connecting");
}

// ── Piped output (non-TTY) ─────────────────────────────────────────────────────

// assert_cmd runs the child with pipes, so stdout is never a TTY here. The tool
// must not emit ANSI escape sequences when it cannot detect a terminal; doing so
// would corrupt scripts and CI logs.

#[test]
fn no_ansi_escapes_when_stdout_is_piped() {
    let out = localshare()
        .args(["3000", "-r", "ws://127.0.0.1:1", "--quiet"])
        .timeout(std::time::Duration::from_secs(15))
        .assert()
        .code(1) // fatal disconnect while nothing listens on 127.0.0.1:1
        .get_output()
        .stdout
        .clone();

    assert!(
        !out.contains(&0x1b),
        "unexpected ANSI escape in piped stdout: {:?}",
        String::from_utf8_lossy(&out)
    );
    // The trailing reason line goes to stderr and should also be plain.
    let err = localshare()
        .args(["3000", "-r", "ws://127.0.0.1:1", "--quiet"])
        .timeout(std::time::Duration::from_secs(15))
        .assert()
        .code(1)
        .get_output()
        .stderr
        .clone();
    assert!(
        !err.contains(&0x1b),
        "unexpected ANSI escape in piped stderr: {:?}",
        String::from_utf8_lossy(&err)
    );
}

#[test]
fn fatal_disconnect_reports_actionable_hint_on_stderr() {
    localshare()
        .args(["3000", "-r", "ws://127.0.0.1:1"])
        .timeout(std::time::Duration::from_secs(15))
        .assert()
        .code(1)
        .stderr(predicate::str::contains("Could not reach relay"))
        .stderr(predicate::str::contains("→ Check the relay address"));
}

// ── Subdomain validation ───────────────────────────────────────────────────────

#[test]
fn invalid_subdomain_is_rejected_by_parser() {
    // All of these must fail during argument parsing (exit code 2) and never
    // reach the network layer. Note: `-leading` is additionally intercepted by
    // Clap itself as an unknown flag, hence the "unexpected argument" branch.
    for bad in [
        "test_underscore".to_string(),
        "-leading".to_string(),
        "trailing-".to_string(),
        "has space".to_string(),
        String::new(),
        "a".repeat(64),
    ] {
        localshare()
            .args(["3000", "-s", &bad])
            .assert()
            .code(2)
            .stderr(
                predicate::str::contains("subdomain")
                    .or(predicate::str::contains("unexpected argument")),
            );
    }
}

#[test]
fn unreachable_relay_stderr_has_no_ansi_in_json_mode() {
    let err = localshare()
        .args(["3000", "--json", "-r", "ws://127.0.0.1:1"])
        .timeout(std::time::Duration::from_secs(15))
        .assert()
        .code(1)
        .get_output()
        .stderr
        .clone();
    assert!(
        !err.contains(&0x1b),
        "unexpected ANSI escape in piped stderr for JSON mode: {:?}",
        String::from_utf8_lossy(&err)
    );
}
