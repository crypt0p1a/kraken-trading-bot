//! The executable itself: what it tells before it takes the screen

use std::process::{Command, Output};

fn dashboard(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_kraken-dashboard"))
        .args(arguments)
        .env_remove("KRAKEN_BOT_DATABASE")
        .current_dir(std::env::temp_dir())
        .output()
        .unwrap()
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn tells_its_usage() {
    for flag in ["--help", "-h"] {
        let output = dashboard(&[flag]);
        assert_eq!(output.status.code(), Some(0), "{}", text(&output));
        assert!(
            text(&output).contains("kraken-dashboard [DATABASE]"),
            "{}",
            text(&output)
        );
    }
}

#[test]
fn refuses_wrong_arguments_with_its_usage() {
    for (arguments, error) in [
        (
            &["--refresh", "soon"][..],
            "--refresh expects a number of seconds",
        ),
        (
            &["--refresh", "0"][..],
            "--refresh expects a number of seconds",
        ),
        (&["--refresh"][..], "--refresh expects a number of seconds"),
        (&["--color"][..], "unknown option --color"),
        (
            &["one.sqlite", "two.sqlite"][..],
            "unexpected argument two.sqlite",
        ),
    ] {
        let output = dashboard(arguments);
        assert_eq!(
            output.status.code(),
            Some(1),
            "{arguments:?}: {}",
            text(&output)
        );
        assert!(
            text(&output).contains(error),
            "{arguments:?}: {}",
            text(&output)
        );
        assert!(text(&output).contains("kraken-dashboard [DATABASE]"));
    }
}

#[test]
fn tells_when_the_database_is_missing_before_taking_the_screen() {
    let output = dashboard(&["kraken-dashboard-no-such-database.sqlite"]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(text(&output).contains("no database"), "{}", text(&output));
}
