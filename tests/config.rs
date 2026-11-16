mod common;

use common::{MINUTE, TestApp};
use scraper::{Html, Selector};

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
            "line 1, column 10: `repos` entry 1 is not a valid URL",
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
        (
            "intervall = \"24h\"",
            "line 1, column 1: unknown field `…`, expected one of `interval`",
        ),
        (
            "[hosts.\"github.com\"]\ntoken = \"\"",
            "`hosts.\"github.com\".token` must not be empty",
        ),
        (
            "[hosts.\"github.com\"]\ntoken = \"ghp_x\\nurl=https://evil\"",
            "`hosts.\"github.com\".token` must not be empty or contain spaces",
        ),
        (
            "[hosts.\"github.com\"]\ntokn = \"x\"",
            "line 2 (`hosts.\"github.com\".…`), column 1: unknown field `…`, expected `token`",
        ),
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

const GOOD_REPO: &str = "https://example.com/good/repo";

/// Text from the configs in [`every_config_error_shows_where_and_what_but_nothing_the_user_wrote`]
/// that no error, page or log may show: the passwords and tokens, which hold
/// `secretpw`, `se://cretpw` or `ghp_`, and the user names before them.
const MARKERS: [&str; 5] = ["secret", "cretpw", "se://", "ghp_", "user:"];

/// Whether `shown` ends with the error `expected`, which starts with the
/// place it names, such as ``line 3 (`hosts.…`), column 1``.
fn ends_with_error(shown: &str, expected: &str) -> bool {
    shown.ends_with(&format!(": {expected}"))
}

/// What the app shows once a config is written over a good one by hand.
struct HandEditError {
    /// The error in the config warning on the list page.
    reason: String,
    /// The whole list page.
    page: String,
    logged: String,
}

/// What the app shows once `config` is written over the config of an app
/// tracking [`GOOD_REPO`]. Checks that the app keeps the last good config.
async fn hand_edit_error(config: &str) -> HandEditError {
    let logs = common::logs::capture();
    let app = TestApp::tracking(&[GOOD_REPO]).await;

    app.write_config(config);
    app.advance_and_settle(MINUTE).await;

    let page = app.get("/").await.text();
    let html = Html::parse_document(&page);
    let reason = html
        .select(&Selector::parse("main .banner pre.reason").unwrap())
        .next()
        .unwrap_or_else(|| panic!("{config:?}: no config warning: {page}"))
        .text()
        .collect();
    let repos: Vec<String> = html
        .select(&Selector::parse("main article .repo-url").unwrap())
        .map(|repo| repo.text().collect::<String>().trim().to_owned())
        .collect();
    assert_eq!(repos, [GOOD_REPO], "{config:?}: keeps the last good config");
    HandEditError {
        reason,
        page,
        logged: logs.lines().join("\n"),
    }
}

#[tokio::test]
async fn every_config_error_shows_where_and_what_but_nothing_the_user_wrote() {
    let host = r#"must be a host such as "github.com", with a port if the repo URLs name one"#;
    let top_level = "expected one of `interval`, `listen`, `repos`, `hosts`";
    let escapes =
        r#"missing escaped value, expected `b`, `e`, `f`, `n`, `r`, `\`, `"`, `x`, `u`, `U`"#;
    let cases: Vec<(&str, String)> = vec![
        // A rejected `repos` entry is named from its parsed URL, or else by
        // its position.
        (
            r#"repos = ["https://user:secretpw@example.com/foo/bar"]"#,
            r#"line 1, column 10: `repos` entry "https://…@example.com/foo/bar" must not contain credentials; set a token under [hosts."<host>"] instead"#.into(),
        ),
        (
            r#"repos = ["https://user:secretpw@example.com:99999/foo/bar"]"#,
            "line 1, column 10: `repos` entry 1 is not a valid URL".into(),
        ),
        (
            r#"repos = ["https://user:12345#secretpw@example.com/foo/bar"]"#,
            "line 1, column 10: `repos` entry 1 has no repository path".into(),
        ),
        (
            r#"repos = ["https://example.com/secretpw/b", "ftp://user:secretpw@example.com/foo/bar"]"#,
            r#"line 1, column 44: `repos` entry "…://…@example.com/foo/bar" must be an http:// or https:// URL"#.into(),
        ),
        (
            r#"repos = ["user:secretpw@example.com/foo/bar"]"#,
            "line 1, column 10: `repos` entry 1 must be an http:// or https:// URL".into(),
        ),
        (
            r#"repos = ["https://example.com/secretpw/b", "user:se://cretpw@example.com/foo/bar"]"#,
            "line 1, column 44: `repos` entry 2 must be an http:// or https:// URL".into(),
        ),
        (
            r#"repos = ["user://secretpw@github.com/foo/bar"]"#,
            r#"line 1, column 10: `repos` entry "…://…@github.com/foo/bar" must be an http:// or https:// URL"#.into(),
        ),
        // A rejected `hosts` key likewise.
        (
            "[hosts.\"https://user:secretpw@github.com\"]\ntoken = \"ghp_secretpw\"",
            format!(r#"line 1, column 8: `hosts` key "https://…@github.com" {host}"#),
        ),
        (
            "[hosts]\n\"https://user:secretpw@github.com\" = { token = \"ghp_secretpw\" }",
            format!(r#"line 2, column 1: `hosts` key "https://…@github.com" {host}"#),
        ),
        (
            "[hosts.\"user://secretpw@github.com\"]\ntoken = \"ghp_secretpw\"",
            format!(r#"line 1, column 8: `hosts` key "…://…@github.com" {host}"#),
        ),
        (
            "[hosts.\"user:se://cretpw@github.com\"]\ntoken = \"ghp_secretpw\"",
            format!("line 1, column 8: `hosts` key {host}"),
        ),
        // A `hosts` key shaped like a host but holding credentials.
        (
            "[hosts.\"user:secretpw\"]\ntoken = \"ghp_secretpw\"",
            format!("line 1, column 8: `hosts` key {host}"),
        ),
        (
            "[hosts.\"user:secretpw\"]\ntokn = \"ghp_secretpw\"",
            "line 2 (`hosts.….…`), column 1: unknown field `…`, expected `token`".into(),
        ),
        (
            "[hosts.\"[user:secretpw]:1\"]\ntoken = \"ghp_secretpw\"",
            format!("line 1, column 8: `hosts` key {host}"),
        ),
        (
            "[hosts.\"[::1]:8080\"]\ntoken = \"ghp_secretpw x\"",
            r#"line 2, column 9: `hosts."[::1]:8080".token` must not be empty or contain spaces, line breaks or control characters"#.into(),
        ),
        // A token that isn't a string, or a table without one.
        (
            "[hosts]\n\"https://user:secretpw@github.com\" = \"ghp_secretpw\"",
            r#"line 2 (`hosts.…`), column 38: invalid type: string "…", expected struct HostConfig"#.into(),
        ),
        (
            "hosts.\"https://user:secretpw@github.com\".token = 5",
            "line 1 (`hosts.….token`), column 50: invalid type: integer `…`, expected a string".into(),
        ),
        (
            "[hosts]\n\"https://user:secret=secretpw@github.com\" = 5",
            "line 2 (`hosts.…`), column 45: invalid type: integer `…`, expected struct HostConfig".into(),
        ),
        (
            "[hosts]\n\"https://user:se\\\"cret=secretpw@github.com\" = 5",
            "line 2 (`hosts.…`), column 47: invalid type: integer `…`, expected struct HostConfig".into(),
        ),
        (
            "[hosts.\"https://user:secret=secretpw@github.com\"]",
            "line 1 (`hosts.…`), column 1: missing field `token`".into(),
        ),
        (
            "[hosts]\n\"github.com\" = \"ghp_secretpw\"\n",
            r#"line 2 (`hosts."github.com"`), column 16: invalid type: string "…", expected struct HostConfig"#.into(),
        ),
        (
            "[hosts]\n\"github.com\" = \"ghp_\\\"secretpw`ab`cd\"\n",
            r#"line 2 (`hosts."github.com"`), column 16: invalid type: string "…", expected struct HostConfig"#.into(),
        ),
        (
            "repos = \"https://user:ghp_\\\"secretpw`x`pw@github.com\"\n",
            r#"line 1 (`repos`), column 9: invalid type: string "…", expected a sequence"#.into(),
        ),
        // Keys whose escapes spell out credentials.
        (
            r#"[hosts]
"https://user:secretpw\u0040github.com" = "ghp_secretpw""#,
            r#"line 2 (`hosts.…`), column 43: invalid type: string "…", expected struct HostConfig"#.into(),
        ),
        (
            r#"[hosts]
"https://user:secretpw\U00000040github.com" = "ghp_secretpw""#,
            r#"line 2 (`hosts.…`), column 47: invalid type: string "…", expected struct HostConfig"#.into(),
        ),
        (
            r#"[hosts]
"https://user:secret\u0070w@github.com" = "ghp_secretpw""#,
            r#"line 2 (`hosts.…`), column 43: invalid type: string "…", expected struct HostConfig"#.into(),
        ),
        (
            r#"[hosts]
"https://user\u003asecretpw@github.com" = "ghp_secretpw""#,
            r#"line 2 (`hosts.…`), column 43: invalid type: string "…", expected struct HostConfig"#.into(),
        ),
        (
            r#"[hosts]
"https\u003a//user:secretpw\u0040github.com" = "ghp_secretpw""#,
            r#"line 2 (`hosts.…`), column 48: invalid type: string "…", expected struct HostConfig"#.into(),
        ),
        (
            r#"hosts."https://user:secretpw\u0040github.com".token = 5"#,
            "line 1 (`hosts.….token`), column 55: invalid type: integer `…`, expected a string".into(),
        ),
        (
            r#"[hosts]
"https://user:secretpw\q\u0040github.com" = "ghp_secretpw""#,
            format!("line 2, column 24: {escapes}"),
        ),
        // An unknown field, however it's quoted or escaped.
        (
            r#"[hosts."github.com"]
token = "ghp_secretpw"
"https://user:secretpw@github.com" = "ghp_secretpw""#,
            r#"line 3 (`hosts."github.com".…`), column 1: unknown field `…`, expected `token`"#.into(),
        ),
        (
            r#"[hosts."github.com"]
token = "ghp_secretpw"
"https://user:secretpw\u0040github.com" = "ghp_secretpw""#,
            r#"line 3 (`hosts."github.com".…`), column 1: unknown field `…`, expected `token`"#.into(),
        ),
        (
            r#"[hosts."github.com"]
token = "ghp_secretpw"
"https://user:secret\"pw@github.com" = "ghp_secretpw""#,
            r#"line 3 (`hosts."github.com".…`), column 1: unknown field `…`, expected `token`"#.into(),
        ),
        (
            r#"[hosts."github.com"]
token = "ghp_secretpw"
'https://user:secret"pw@github.com' = "ghp_secretpw""#,
            r#"line 3 (`hosts."github.com".…`), column 1: unknown field `…`, expected `token`"#.into(),
        ),
        (
            r#"[hosts."github.com"]
token = "ghp_secretpw"
'https://user:secretpw"@"github.com' = "ghp_secretpw""#,
            r#"line 3 (`hosts."github.com".…`), column 1: unknown field `…`, expected `token`"#.into(),
        ),
        (
            r#"[hosts."github.com"]
token = "ghp_secretpw"
"https://user:sec`ret, expected `token` pw@github.com" = "ghp_secretpw""#,
            r#"line 3 (`hosts."github.com".…`), column 1: unknown field `…`, expected `token`"#.into(),
        ),
        (
            "[\"https://user:secretpw@github.com\"]\ntoken = \"ghp_secretpw\"",
            format!("line 1, column 2: unknown field `…`, {top_level}"),
        ),
        (
            "[\"https://user:sec`retpw@github.com\"]\ntoken = \"ghp_secretpw\"",
            format!("line 1, column 2: unknown field `…`, {top_level}"),
        ),
        (
            "[\"user:secretpw@github.com\"]\ntoken = \"ghp_secretpw\"",
            format!("line 1, column 2: unknown field `…`, {top_level}"),
        ),
        (
            "[\"user:secretpw@github.com/?x=://\"]\ntoken = \"ghp_secretpw\"",
            format!("line 1, column 2: unknown field `…`, {top_level}"),
        ),
        (
            "['https://user:secret\"pw@github.com']\ntoken = \"ghp_secretpw\"",
            format!("line 1, column 2: unknown field `…`, {top_level}"),
        ),
        (
            "[\"user:se://cretpw@github.com\"]\ntoken = \"ghp_secretpw\"",
            format!("line 1, column 2: unknown field `…`, {top_level}"),
        ),
        // An unknown field named like a setting is shown, being no secret.
        (
            "[hosts.\"github.com\"]\ntoken = \"ghp_secretpw\"\ninterval = 5",
            r#"line 3 (`hosts."github.com".interval`), column 1: unknown field `interval`, expected `token`"#.into(),
        ),
        // A token written as a key, or as a `hosts` key, which an error
        // names only if it looks like a domain name.
        (
            "[hosts]\nghp_secretpw = \"ghp_secretpw\"",
            r#"line 2 (`hosts.…`), column 16: invalid type: string "…", expected struct HostConfig"#.into(),
        ),
        (
            "[hosts.ghp_secretpw]",
            "line 1 (`hosts.…`), column 1: missing field `token`".into(),
        ),
        (
            "[hosts.ghp_secretpw]\ntokn = \"ghp_secretpw\"",
            "line 2 (`hosts.….…`), column 1: unknown field `…`, expected `token`".into(),
        ),
        (
            "[hosts.ghp_secretpw]\ntoken = \"\"",
            "line 2, column 9: `hosts.….token` must not be empty or contain spaces, line breaks or control characters".into(),
        ),
        (
            "[hosts.ghp_secretpw]\ntoken = \"ghp_secretpw\"\n[hosts.GHP_secretpw]\ntoken = \"ghp_secretpw\"",
            "line 3, column 8: `hosts` lists a host more than once (host names ignore case)".into(),
        ),
        (
            "[hosts.\"github.com\"]\ntoken = \"ghp_secretpw\"\nghp_secretpw = \"ghp_secretpw\"",
            r#"line 3 (`hosts."github.com".…`), column 1: unknown field `…`, expected `token`"#.into(),
        ),
        (
            "ghp_secretpw = \"ghp_secretpw\"",
            format!("line 1, column 1: unknown field `…`, {top_level}"),
        ),
        // A syntax error names no key, even on a line that looks like it
        // sets one.
        (
            "repos = []\n[hosts.\"github.com\"]\ntoken = \"ghp_secretpw\n",
            "line 3, column 22: invalid basic string, expected `\"`".into(),
        ),
        (
            "[hosts.\"github.com\"]\ntoken = \"\"\"\nghp_secretpw = \\q\"\"\"",
            format!("line 3, column 17: {escapes}"),
        ),
        (
            "[hosts.\"github.com\"]\ntoken = \"\"\"\nghp_secret=pw\"\"\" secretpw",
            "line 3, column 18: unexpected key or value, expected newline, `#`".into(),
        ),
        (
            "[hosts.\"github.com\"]\ntoken = \"\"\"\nghp_secretpw = \u{7}\"\"\"",
            "line 3, column 16: invalid multi-line basic string, expected `\\`, characters".into(),
        ),
        (
            "[hosts.\"github.com\"]\ntoken = '''\nghp_secretpw = \u{7}'''",
            "line 3, column 16: invalid multi-line literal string, expected non-single-quote characters".into(),
        ),
        (
            "[hosts.\"github.com\"]\ntoken = '''\nghp_secret=pw''' secretpw",
            "line 3, column 18: unexpected key or value, expected newline, `#`".into(),
        ),
        // toml's own words, which it quotes as what it expected.
        (
            "interval = tru",
            "line 1, column 12: invalid boolean, expected `true`".into(),
        ),
        (
            "interval = fals",
            "line 1, column 12: invalid boolean, expected `false`".into(),
        ),
        (
            "interval = +inx",
            "line 1, column 13: invalid float, expected `inf`".into(),
        ),
        (
            "interval = nax",
            "line 1, column 12: invalid float, expected `nan`".into(),
        ),
        // Values the app checks itself.
        (
            "interval = \"secretpw\"",
            r#"line 1, column 12: `interval` must be a duration such as "24h" or "90m""#.into(),
        ),
        (
            "listen = \"secretpw\"",
            r#"line 1, column 10: `listen` must be an IP address and port such as "0.0.0.0:8080""#.into(),
        ),
        (
            "[hosts.\"github.com\"]\ntoken = \"ghp_secretpw x\"",
            r#"line 2, column 9: `hosts."github.com".token` must not be empty or contain spaces, line breaks or control characters"#.into(),
        ),
        (
            "[hosts.\"github.com\"]\ntoken = \"ghp_secretpw\"\n[hosts.\"GitHub.com\"]\ntoken = \"ghp_secretpw\"",
            r#"line 3, column 8: `hosts` lists "GitHub.com" more than once (host names ignore case)"#.into(),
        ),
    ];
    for (config, expected) in &cases {
        let at_start = TestApp::start_error(config).await;
        assert!(
            ends_with_error(&at_start, expected),
            "{config:?} at startup:\n  expected …: {expected}\n  got        {at_start}"
        );

        let HandEditError {
            reason,
            page,
            logged,
        } = hand_edit_error(config).await;
        assert!(
            ends_with_error(&reason, expected),
            "{config:?} in the warning:\n  expected …: {expected}\n  got        {reason}"
        );
        // The log quotes the error, escaping its quotes and backslashes.
        let expected_logged = format!("{}\"", format!(": {expected}").escape_debug());
        assert!(
            logged.contains(&expected_logged),
            "{config:?} in the log: expected {expected_logged}, got: {logged}"
        );
        for marker in MARKERS {
            for (shown, what) in [(&at_start, "error"), (&page, "page"), (&logged, "log")] {
                assert!(
                    !shown.contains(marker),
                    "{config:?}: the {what} shows {marker:?}: {shown}"
                );
            }
        }
    }
}

/// Checks that `config` fails startup with the error `expected`.
async fn assert_start_error(config: &str, expected: &str) {
    let error = TestApp::start_error(config).await;
    assert!(
        ends_with_error(&error, expected),
        "{config:?}:\n  expected …: {expected}\n  got        {error}"
    );
}

#[tokio::test]
async fn an_error_names_a_hosts_key_only_if_it_looks_like_a_domain_name() {
    // Each host, as the key in an error shows it, and as the duplicate-host
    // error names it when the host is listed again in upper case. A host
    // without letters can't be.
    let cases = [
        ("github.com", r#""github.com""#, Some(r#""GITHUB.COM""#)),
        ("GitHub.com", r#""GitHub.com""#, Some(r#""GITHUB.COM""#)),
        ("localhost", "localhost", Some(r#""LOCALHOST""#)),
        (
            "localhost:3000",
            r#""localhost:3000""#,
            Some(r#""LOCALHOST:3000""#),
        ),
        (
            "git.example.com:8443",
            r#""git.example.com:8443""#,
            Some(r#""GIT.EXAMPLE.COM:8443""#),
        ),
        ("[::1]", r#""[::1]""#, None),
        ("[::1]:8080", r#""[::1]:8080""#, None),
        ("ghp_secretpw", "…", Some("a host")),
        ("glpat-secretpw", "…", Some("a host")),
        ("gitea", "…", Some("a host")),
        ("secretpw:443", "…", Some("a host")),
        // A host named like one of the app's settings is still a host.
        ("token", "…", Some("a host")),
    ];
    for (host, shown, named_twice) in cases {
        assert_start_error(
            &format!("[hosts.\"{host}\"]"),
            &format!("line 1 (`hosts.{shown}`), column 1: missing field `token`"),
        )
        .await;
        assert_start_error(
            &format!("[hosts.\"{host}\"]\ntoken = \"ghp_secretpw x\""),
            &format!(
                "line 2, column 9: `hosts.{shown}.token` must not be empty or contain spaces, \
                 line breaks or control characters"
            ),
        )
        .await;
        if let Some(named) = named_twice {
            let upper = host.to_ascii_uppercase();
            assert_start_error(
                &format!(
                    "[hosts.\"{host}\"]\ntoken = \"ghp_secretpw\"\n\
                     [hosts.\"{upper}\"]\ntoken = \"ghp_secretpw\""
                ),
                &format!(
                    "line 3, column 8: `hosts` lists {named} more than once (host names ignore case)"
                ),
            )
            .await;
        }
    }
}

#[tokio::test]
async fn a_hosts_key_without_a_dot_is_still_accepted() {
    for host in [
        "gitea",
        "gitea:3000",
        "localhost",
        "[::1]:8080",
        "git_server-2",
    ] {
        TestApp::with_config(&format!("[hosts.\"{host}\"]\ntoken = \"t0ken\"\n")).await;
    }
}
