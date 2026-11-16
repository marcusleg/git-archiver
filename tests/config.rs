mod common;

use common::TestApp;

#[tokio::test]
async fn first_start_writes_a_commented_default_config_listing_every_setting() {
    let app = TestApp::start().await;

    let config = app.config();
    assert!(config.contains("\ninterval = \"24h\""), "{config}");
    assert!(config.contains("\nlisten = \"0.0.0.0:8080\""), "{config}");
    assert!(config.contains("\nrepos = ["), "{config}");
    assert!(config.contains("# [hosts.\"github.com\"]"), "{config}");
    assert!(config.contains("# token = "), "{config}");
    let comment_lines = config.lines().filter(|l| l.starts_with('#')).count();
    assert!(comment_lines >= 5, "{config}");
}

#[tokio::test]
async fn an_existing_config_is_never_overwritten() {
    let hand_written =
        "# my own notes\ninterval = \"12h\"\nrepos = [\"https://example.com/foo/bar\"]\n";
    let mut app = TestApp::with_config(hand_written).await;
    assert_eq!(app.config(), hand_written);

    app.restart().await;
    assert_eq!(app.config(), hand_written);
}

#[tokio::test]
async fn the_default_config_survives_a_restart_unchanged() {
    let mut app = TestApp::start().await;
    let written = app.config();

    app.restart().await;
    assert_eq!(app.config(), written);
}

#[tokio::test]
async fn an_invalid_config_fails_startup_naming_the_problem() {
    let cases = [
        ("interval = \"24h\"\nrepos = [", "config.toml"),
        ("interval = \"often\"", "`interval` must be a duration"),
        ("interval = \"0s\"", "`interval` must be longer than zero"),
        (
            "listen = \"localhost\"",
            "`listen` must be an IP address and port",
        ),
        (
            "repos = [\"not a url\"]",
            "\"not a url\" is not a valid URL",
        ),
        (
            "repos = [\"ftp://example.com/foo\"]",
            "must be an http:// or https:// URL",
        ),
        ("repos = \"https://example.com/foo\"", "repos"),
        (
            "repos = [\"https://user:secret@example.com/foo/bar\"]",
            "must not contain credentials",
        ),
        ("intervall = \"24h\"", "intervall"),
        (
            "[hosts.\"github.com\"]\ntoken = \"\"",
            "`hosts.\"github.com\".token` must not be empty",
        ),
        ("[hosts.\"github.com\"]\ntokn = \"x\"", "tokn"),
    ];
    for (config, expected) in cases {
        let error = TestApp::start_error(config).await;
        assert!(
            error.contains(expected),
            "config {config:?}: expected error to mention {expected:?}, got: {error}"
        );
        assert!(error.contains("invalid config"), "{error}");
    }
}
