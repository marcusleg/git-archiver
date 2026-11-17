mod common;

use common::TestApp;
use scraper::{Html, Selector};

/// Every resource the list page references: link hrefs and src attributes.
fn page_resources(html: &str) -> Vec<String> {
    let html = Html::parse_document(html);
    let mut resources = Vec::new();
    for (selector, attr) in [("link[href]", "href"), ("[src]", "src")] {
        for element in html.select(&Selector::parse(selector).unwrap()) {
            resources.push(element.value().attr(attr).unwrap().to_owned());
        }
    }
    resources
}

/// Every `url(...)` in a stylesheet.
fn css_urls(css: &str) -> Vec<String> {
    css.split("url(")
        .skip(1)
        .map(|rest| {
            rest[..rest.find(')').unwrap()]
                .trim_matches(['"', '\''])
                .to_owned()
        })
        .collect()
}

/// What in a script would repeat the stylesheet's layout: each
/// `matchMedia`, and each CSS length, a number directly followed by `rem`,
/// `em` or `px`.
fn layout_values(script: &str) -> Vec<String> {
    let is_word_char = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '$';
    let mut found: Vec<String> = script
        .match_indices("matchMedia")
        .map(|(_, name)| name.to_owned())
        .collect();
    let mut rest = script;
    while let Some(start) = rest.find(|c: char| c.is_ascii_digit()) {
        let in_word = rest[..start].ends_with(is_word_char);
        let tail = &rest[start..];
        let end = tail
            .find(|c: char| !c.is_ascii_digit() && c != '.')
            .unwrap_or(tail.len());
        let after = &tail[end..];
        let unit = ["rem", "em", "px"]
            .into_iter()
            .find(|unit| after.starts_with(unit) && !after[unit.len()..].starts_with(is_word_char));
        if !in_word && let Some(unit) = unit {
            found.push(tail[..end + unit.len()].to_owned());
        }
        rest = after;
    }
    found
}

fn is_same_origin(url: &str) -> bool {
    url.starts_with("data:") || (!url.contains("://") && !url.starts_with("//"))
}

#[tokio::test]
async fn the_page_loads_no_resources_from_other_hosts() {
    let app = TestApp::with_config(r#"repos = ["https://github.com/foo/bar"]"#).await;
    let page = app.get("/").await.text();

    let resources = page_resources(&page);
    assert!(!resources.is_empty());
    for resource in &resources {
        assert!(is_same_origin(resource), "page loads {resource}");
    }

    let css = app.get("/assets/style.css").await;
    assert_eq!(css.status, 200);
    assert!(
        css.content_type.starts_with("text/css"),
        "{}",
        css.content_type
    );
    let css = css.text();
    for url in css_urls(&css) {
        assert!(is_same_origin(&url), "stylesheet loads {url}");
    }
    assert!(!css.contains("@import"));
}

#[tokio::test]
async fn the_script_is_served_by_the_app() {
    let app = TestApp::start().await;
    let page = app.get("/").await.text();
    assert!(page_resources(&page).contains(&"/assets/script.js".to_owned()));

    let script = app.get("/assets/script.js").await;
    assert_eq!(script.status, 200);
    assert_eq!(script.content_type, "text/javascript; charset=utf-8");
    let script = script.text();
    assert!(
        script.contains("new EventSource(\"/events\")"),
        "the page script"
    );
}

#[tokio::test]
async fn the_stylesheet_alone_holds_the_layout_breakpoint() {
    let app = TestApp::start().await;
    let script = app.get("/assets/script.js").await.text();
    assert_eq!(layout_values(&script), Vec::<String>::new());
}

#[test]
fn layout_values_finds_media_queries_and_css_lengths() {
    assert_eq!(
        layout_values(r#"matchMedia("(max-width: 64rem)").matches"#),
        ["matchMedia", "64rem"]
    );
    assert_eq!(
        layout_values("if (innerWidth < 960) return"),
        Vec::<String>::new()
    );
    assert_eq!(layout_values("el.style.width = \"960px\";"), ["960px"]);
    assert_eq!(layout_values("const gap = 1.5em;"), ["1.5em"]);
    assert_eq!(
        layout_values("setTimeout(f, 1500); h1em; 2emit; x2px"),
        Vec::<String>::new()
    );
}

#[tokio::test]
async fn fonts_are_served_by_the_app() {
    let app = TestApp::start().await;
    let page = app.get("/").await.text();
    assert!(page_resources(&page).contains(&"/assets/style.css".to_owned()));
    let css = app.get("/assets/style.css").await.text();

    let fonts = css_urls(&css);
    assert_eq!(fonts.len(), 3, "Inter 400 and 700, JetBrains Mono 400");
    for font in fonts {
        let font = app.get(&format!("/assets/{font}")).await;
        assert_eq!(font.status, 200);
        assert_eq!(font.content_type, "font/woff2");
        assert_eq!(&font.body[..4], b"wOF2");
    }
    assert!(css.contains("font-family: Inter;"));
    assert!(css.contains("font-family: \"JetBrains Mono\";"));
}

#[tokio::test]
async fn unknown_assets_are_not_found() {
    let app = TestApp::start().await;
    assert_eq!(app.get("/assets/fonts/nope.woff2").await.status, 404);
    assert_eq!(
        app.get("/assets/fonts/..%2F..%2FCargo.toml").await.status,
        404
    );
}

#[tokio::test]
async fn the_stylesheet_uses_the_design_system_dark_tokens() {
    let app = TestApp::start().await;
    let css = app.get("/assets/style.css").await.text();

    // Values from the "Marcus Legendre" design system, dark theme.
    for token in [
        "--page: #0c0d0a;",
        "--bg: #121310;",
        "--surface: #1a1b15;",
        "--surface-hover: #20211a;",
        "--fg: #f8f8f2;",
        "--muted: #8c8875;",
        "--border: #3a3a30;",
        "--monokai-orange: #fd971f;",
        "--monokai-pink: #f92672;",
        "--monokai-purple: #ae81ff;",
        "--rule-height: 3px;",
    ] {
        assert!(css.contains(token), "missing {token}");
    }
    assert!(css.contains("color-scheme: dark;"));
    assert!(!css.contains("prefers-color-scheme"), "dark only");
    assert!(!css.contains("data-theme"), "no theme toggle");
    assert!(css.contains("background: var(--page);"), "page ground");
    assert!(
        css.contains("background: var(--surface);"),
        "cards on surface"
    );
    assert!(
        css.contains("border: 1px solid var(--border);"),
        "cards bordered"
    );
    assert!(css.contains(
        "linear-gradient(90deg, var(--rule-start) 0%, var(--rule-mid) 50%, var(--rule-end) 100%)"
    ));
    assert!(!css.contains("box-shadow"), "flat");
}
