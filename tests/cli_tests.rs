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
    let out = localshare()
        .args(["3000", "--json", "-r", "ws://127.0.0.1:1"])
        .timeout(std::time::Duration::from_secs(15))
        .assert()
        .code(0)
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
