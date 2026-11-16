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
        (
            "repos = [\"https://example.com/foo//bar\"]",
            "unsupported repository path",
        ),
        (
            "repos = [\"https://example.com/foo.git/bar\"]",
            "unsupported repository path",
        ),
        ("intervall = \"24h\"", "intervall"),
        (
            "[hosts.\"github.com\"]\ntoken = \"\"",
            "`hosts.\"github.com\".token` must not be empty",
        ),
        (
            "[hosts.\"github.com\"]\ntoken = \"ghp_x\\nurl=https://evil\"",
            "`hosts.\"github.com\".token` must not be empty or contain spaces",
        ),
        ("[hosts.\"github.com\"]\ntokn = \"x\"", "tokn"),
        (
            "[hosts.\"https://github.com\"]\ntoken = \"x\"",
            "`hosts` key \"https://github.com\" must be a host",
        ),
        (
            "[hosts.\"github.com\"]\ntoken = \"x\"\n[hosts.\"GitHub.com\"]\ntoken = \"y\"",
            "more than once",
        ),
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

#[tokio::test]
async fn a_rejected_repo_url_is_named_without_its_credentials() {
    let cases = [
        (
            "https://user:secretpw@example.com/foo/bar",
            "\"https://…@example.com/foo/bar\" must not contain credentials",
        ),
        (
            "https://user:secretpw@example.com:99999/foo/bar",
            "\"https://…@example.com:99999/foo/bar\" is not a valid URL",
        ),
        (
            "https://user:12345#secretpw@example.com/foo/bar",
            "\"https://…@example.com/foo/bar\" has no repository path",
        ),
        (
            "ftp://user:secretpw@example.com/foo/bar",
            "\"ftp://…@example.com/foo/bar\" must be an http:// or https:// URL",
        ),
        (
            "user:secretpw@example.com/foo/bar",
            "\"…@example.com/foo/bar\" must be an http:// or https:// URL",
        ),
        (
            "user:se://cretpw@example.com/foo/bar",
            "\"…@example.com/foo/bar\" must be an http:// or https:// URL",
        ),
    ];
    for (url, expected) in cases {
        let error = TestApp::start_error(&format!("repos = [\"{url}\"]")).await;
        assert!(error.contains(expected), "{url:?}: {error}");
        assert!(!error.contains("secretpw"), "{url:?}: {error}");
        assert!(!error.contains("se://"), "{url:?}: {error}");
        assert!(!error.contains("user:"), "{url:?}: {error}");
    }
}

#[tokio::test]
async fn a_rejected_hosts_key_is_named_without_its_credentials() {
    let cases = [
        (
            "[hosts.\"https://user:secretpw@github.com\"]\ntoken = \"x\"",
            "`hosts` key \"https://…@github.com\" must be a host",
        ),
        (
            "[hosts]\n\"https://user:secretpw@github.com\" = { token = \"x\" }",
            "`hosts` key \"https://…@github.com\" must be a host",
        ),
        (
            "[hosts]\n\"https://user:secretpw@github.com\" = \"x\"",
            "line 2 (`\"https://…@github.com\"`)",
        ),
        (
            "hosts.\"https://user:secretpw@github.com\".token = 5",
            "line 1 (`hosts.\"https://…@github.com\".token`)",
        ),
        (
            "[hosts]\n\"https://user:secret=secretpw@github.com\" = 5",
            "line 2 (`\"https://…@github.com\"`)",
        ),
        (
            "[hosts]\n\"https://user:se\\\"cret=secretpw@github.com\" = 5",
            "line 2 (`\"https://…@github.com\"`)",
        ),
        (
            "[hosts.\"https://user:secret=secretpw@github.com\"]",
            "line 1, column 1: missing field `token`",
        ),
        (
            "[hosts.\"user:se://cretpw@github.com\"]\ntoken = \"x\"",
            "`hosts` key \"…@github.com\" must be a host",
        ),
    ];
    for (config, expected) in cases {
        let error = TestApp::start_error(config).await;
        assert!(error.contains(expected), "{config:?}: {error}");
        assert!(!error.contains("secretpw"), "{config:?}: {error}");
        assert!(!error.contains("se://"), "{config:?}: {error}");
        assert!(!error.contains("user:"), "{config:?}: {error}");
    }
}

#[tokio::test]
async fn a_key_is_named_without_its_credentials_whatever_escapes_it_uses() {
    let cases = [
        (
            "[hosts]\n\"https://user:secretpw\\u0040github.com\" = \"ghp_x\"",
            "line 2 (`\"https://…@github.com\"`), column 43: invalid type",
        ),
        (
            "[hosts]\n\"https://user:secretpw\\U00000040github.com\" = \"ghp_x\"",
            "line 2 (`\"https://…@github.com\"`), column 47: invalid type",
        ),
        (
            "[hosts]\n\"https://user:secret\\u0070w@github.com\" = \"ghp_x\"",
            "line 2 (`\"https://…@github.com\"`)",
        ),
        (
            "[hosts]\n\"https://user\\u003asecretpw@github.com\" = \"ghp_x\"",
            "line 2 (`\"https://…@github.com\"`)",
        ),
        (
            "[hosts]\n\"https\\u003a//user:secretpw\\u0040github.com\" = \"ghp_x\"",
            "line 2 (`\"https://…@github.com\"`)",
        ),
        (
            "hosts.\"https://user:secretpw\\u0040github.com\".token = 5",
            "line 1 (`hosts.\"https://…@github.com\".token`)",
        ),
        (
            "[hosts.\"github.com\"]\ntoken = \"a\"\n\"https://user:secretpw\\u0040github.com\" = \"x\"",
            "line 3 (`\"https://…@github.com\"`), column 1: \
             unknown field `https://…@github.com`, expected `token`",
        ),
        // A key toml can't read is named by its place alone.
        (
            "[hosts]\n\"https://user:secretpw\\q\\u0040github.com\" = \"x\"",
            "line 2, column ",
        ),
    ];
    for (config, expected) in cases {
        let error = TestApp::start_error(config).await;
        assert!(error.contains(expected), "{config:?}: {error}");
        assert!(!error.contains("secretpw"), "{config:?}: {error}");
        assert!(!error.contains("user:"), "{config:?}: {error}");
    }
}

#[tokio::test]
async fn an_unknown_field_is_named_without_its_credentials() {
    let cases = [
        (
            "[hosts.\"github.com\"]\ntoken = \"a\"\n\"https://user:secretpw@github.com\" = \"x\"",
            "line 3 (`\"https://…@github.com\"`), column 1: \
             unknown field `https://…@github.com`, expected `token`",
        ),
        (
            "[\"https://user:secretpw@github.com\"]\ntoken = \"x\"",
            "line 1, column 2: unknown field `https://…@github.com`, \
             expected one of `interval`, `listen`, `repos`, `hosts`",
        ),
        (
            "[\"https://user:sec`retpw@github.com\"]\ntoken = \"x\"",
            "unknown field `https://…@github.com`, expected one of `interval`",
        ),
        (
            "[\"user:secretpw@github.com\"]\ntoken = \"x\"",
            "unknown field `…@github.com`, expected one of `interval`",
        ),
        (
            "[\"user:secretpw@github.com/?x=://\"]\ntoken = \"x\"",
            "unknown field `…@github.com/?x=://`, expected one of `interval`",
        ),
        (
            "[hosts.\"github.com\"]\ntoken = \"a\"\n\"https://user:secret\\\"pw@github.com\" = \"x\"",
            "line 3 (`\"https://…@github.com\"`), column 1: \
             unknown field `https://…@github.com`, expected `token`",
        ),
        (
            "[hosts.\"github.com\"]\ntoken = \"a\"\n'https://user:secret\"pw@github.com' = \"x\"",
            "line 3 (`\"https://…@github.com\"`), column 1: \
             unknown field `https://…@github.com`, expected `token`",
        ),
        (
            "['https://user:secret\"pw@github.com']\ntoken = \"x\"",
            "line 1, column 2: unknown field `https://…@github.com`, \
             expected one of `interval`, `listen`, `repos`, `hosts`",
        ),
        (
            "[hosts.\"github.com\"]\ntoken = \"a\"\n'https://user:secretpw\"@\"github.com' = \"x\"",
            "line 3 (`\"https://…@\\\"github.com\"`), column 1: \
             unknown field `https://…@\"github.com`, expected `token`",
        ),
        (
            "[\"user:se://cretpw@github.com\"]\ntoken = \"x\"",
            "line 1, column 2: unknown field `…@github.com`, \
             expected one of `interval`, `listen`, `repos`, `hosts`",
        ),
    ];
    for (config, expected) in cases {
        let error = TestApp::start_error(config).await;
        assert!(error.contains(expected), "{config:?}: {error}");
        for leaked in ["secret", "se://", "user:"] {
            assert!(
                !error.contains(leaked),
                "{config:?} shows {leaked:?}: {error}"
            );
        }
    }
}

#[tokio::test]
async fn an_invalid_config_names_where_without_ever_quoting_a_token() {
    let cases = [
        (
            "repos = []\n[hosts.\"github.com\"]\ntoken = \"ghp_secret\n",
            &["line 3"][..],
        ),
        (
            "[hosts]\n\"github.com\" = \"ghp_secret\"\n",
            &["line 2", "invalid type"][..],
        ),
        // serde writes the value with `\"` escaped, which mustn't end it early.
        (
            "[hosts]\n\"github.com\" = \"ghp_\\\"secret`ab`cd\"\n",
            &["line 2", "invalid type: string \"…\", expected"][..],
        ),
        (
            "repos = \"https://user:ghp_\\\"secret`x`pw@github.com\"\n",
            &["line 1", "invalid type: string \"…\", expected"][..],
        ),
    ];
    for (config, expected) in cases {
        let error = TestApp::start_error(config).await;
        for leaked in ["secret", "`ab", "pw@", "user:"] {
            assert!(
                !error.contains(leaked),
                "{config:?} shows {leaked:?}: {error}"
            );
        }
        for expected in expected {
            assert!(
                error.contains(expected),
                "config {config:?}: expected error to mention {expected:?}, got: {error}"
            );
        }
    }
}
