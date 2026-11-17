//! A real headless Chromium, driven over the DevTools protocol, for tests of
//! what a person sees and does in the page.
//!
//! The browser is the first of `GIT_ARCHIVER_TEST_BROWSER` (a path or a
//! command name) or a Chrome/Chromium found on `PATH`. With none, tests fail
//! rather than skip, so a run can't pass without exercising the page.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chromiumoxide::browser::{Browser as Chromium, BrowserConfig};
use chromiumoxide::cdp::browser_protocol::emulation::{
    EventVirtualTimeBudgetExpired, SetScriptExecutionDisabledParams, SetVirtualTimePolicyParams,
    VirtualTimePolicy,
};
use chromiumoxide::cdp::browser_protocol::network::TimeSinceEpoch;
use chromiumoxide::element::Element;
use chromiumoxide::handler::viewport::Viewport as EmulatedViewport;
use chromiumoxide::page::Page;
use futures::StreamExt;
use tempfile::TempDir;
use tokio::task::JoinHandle;

/// The environment variable naming the browser binary.
pub const BROWSER_ENV: &str = "GIT_ARCHIVER_TEST_BROWSER";

const CANDIDATES: &[&str] = &[
    "chromium",
    "chromium-browser",
    "google-chrome",
    "google-chrome-stable",
    "chrome",
];

/// How long a test waits for the page to show what it expects.
const PATIENCE: Duration = Duration::from_secs(10);

/// How far the clock of a tab opened with [`Browser::open_at`] moves at a
/// time while its page loads.
const LOAD_STEP: Duration = Duration::from_millis(10);

/// The screen sizes the page is tested at.
#[derive(Debug, Clone, Copy)]
pub enum Viewport {
    Desktop,
    Phone,
}

impl Viewport {
    fn width(self) -> u32 {
        self.emulation().width
    }

    fn emulation(self) -> EmulatedViewport {
        let (width, height, mobile) = match self {
            Viewport::Desktop => (1280, 800, false),
            Viewport::Phone => (390, 844, true),
        };
        EmulatedViewport {
            width,
            height,
            device_scale_factor: None,
            emulating_mobile: mobile,
            is_landscape: false,
            has_touch: mobile,
        }
    }
}

/// The browser binary to launch, or an explanation of how to provide one.
fn find_browser() -> Result<PathBuf, String> {
    find_browser_in(std::env::var_os(BROWSER_ENV), std::env::var_os("PATH"))
}

/// [`find_browser`] given the value of `GIT_ARCHIVER_TEST_BROWSER` and `PATH`.
pub fn find_browser_in(named: Option<OsString>, path: Option<OsString>) -> Result<PathBuf, String> {
    if let Some(named) = named.filter(|v| !v.is_empty()) {
        let named = PathBuf::from(named);
        return which(&named, &path).ok_or_else(|| {
            format!(
                "{BROWSER_ENV} is set to {named:?}, which is neither a file nor a command on PATH"
            )
        });
    }
    CANDIDATES
        .iter()
        .find_map(|name| which(name.as_ref(), &path))
        .ok_or_else(|| {
            format!(
                "no Chrome or Chromium found on PATH (tried {}); install one, or set {BROWSER_ENV} \
                 to the browser's binary",
                CANDIDATES.join(", ")
            )
        })
}

fn which(name: &Path, path: &Option<OsString>) -> Option<PathBuf> {
    if name.components().count() > 1 {
        return name.is_file().then(|| name.to_owned());
    }
    std::env::split_paths(path.as_ref()?)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// A running browser. It quits when dropped.
pub struct Browser {
    browser: Chromium,
    events: JoinHandle<()>,
    viewport: Viewport,
    _profile: TempDir,
}

impl Browser {
    /// Launches a headless browser whose pages have this viewport.
    pub async fn launch(viewport: Viewport) -> Self {
        let binary =
            find_browser().unwrap_or_else(|why| panic!("browser tests need a browser: {why}"));
        let profile = TempDir::new().unwrap();
        let config = BrowserConfig::builder()
            .user_data_dir(profile.path())
            .chrome_executable(binary)
            .viewport(viewport.emulation())
            // CI runners forbid the sandbox's user namespaces.
            .no_sandbox()
            .build()
            .expect("browser config");
        let (browser, mut handler) = Chromium::launch(config)
            .await
            .expect("the browser launches");
        let events = tokio::spawn(async move { while handler.next().await.is_some() {} });
        Self {
            browser,
            events,
            viewport,
            _profile: profile,
        }
    }

    /// Opens `url` in a new tab and waits for it to load.
    pub async fn open(&self, url: &str) -> Tab {
        let page = self.browser.new_page(url).await.expect("the page opens");
        self.loaded(page).await
    }

    /// Opens `url` in a new tab whose clock reads `now` when the page starts
    /// loading. The clock moves only as far as loading needs, a fraction of a
    /// second, then stands still until the test lets time pass with
    /// [`Tab::let_time_pass`].
    pub async fn open_at(&self, url: &str, now: SystemTime) -> Tab {
        let page = self
            .browser
            .new_page("about:blank")
            .await
            .expect("a blank page opens");
        let since_epoch = now.duration_since(UNIX_EPOCH).unwrap().as_secs_f64();
        page.execute(
            SetVirtualTimePolicyParams::builder()
                .policy(VirtualTimePolicy::Pause)
                .initial_virtual_time(TimeSinceEpoch::new(since_epoch))
                .build()
                .unwrap(),
        )
        .await
        .expect("the page's clock is set");
        // Navigating only finishes once the clock lets the page load, below.
        let navigation = tokio::spawn({
            let page = page.clone();
            let url = url.to_owned();
            async move { page.goto(url).await.map(|_| ()) }
        });
        let tab = Tab { page };
        // The page only loads while its clock runs, so run it in small steps
        // until it has.
        tab.wait_until("the page to load", || async {
            tab.run_clock(VirtualTimePolicy::PauseIfNetworkFetchesPending, LOAD_STEP)
                .await;
            let loaded = "location.href !== 'about:blank' && document.readyState === 'complete'";
            let loaded = tab.page.evaluate(loaded).await.ok()?;
            loaded.into_value::<bool>().ok()?.then_some(())
        })
        .await;
        navigation.await.unwrap().expect("the page loads");
        self.loaded(tab.page).await
    }

    async fn loaded(&self, page: Page) -> Tab {
        page.wait_for_navigation().await.expect("the page loads");
        self.tab(page).await
    }

    /// Opens `url` in a new tab that runs no JavaScript, as for a person who
    /// turned it off, and waits for it to load.
    pub async fn open_without_javascript(&self, url: &str) -> Tab {
        let page = self
            .browser
            .new_page("about:blank")
            .await
            .expect("the page opens");
        page.execute(SetScriptExecutionDisabledParams::new(true))
            .await
            .expect("JavaScript turns off");
        // Check it took: a page whose only script marks the document.
        page.goto("data:text/html,<script>document.documentElement.dataset.ran = 1</script>")
            .await
            .expect("the check page opens");
        let ran: bool = page
            .evaluate("'ran' in document.documentElement.dataset")
            .await
            .expect("script check")
            .into_value()
            .expect("script check is a bool");
        assert!(!ran, "a script ran with JavaScript off");
        page.goto(url).await.expect("the page opens");
        page.wait_for_navigation().await.expect("the page loads");
        self.tab(page).await
    }

    async fn tab(&self, page: Page) -> Tab {
        let width: u32 = page
            .evaluate("window.innerWidth")
            .await
            .expect("page width")
            .into_value()
            .expect("page width is a number");
        assert_eq!(
            width,
            self.viewport.width(),
            "the page isn't shown at the {:?} viewport",
            self.viewport
        );
        Tab { page }
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        self.events.abort();
        // Dropping the handle kills the child process.
    }
}

/// A page in the browser, seen and used the way a person would.
pub struct Tab {
    page: Page,
}

impl Tab {
    /// The text a person can read on the page now: hidden things (a closed
    /// dialog, say) are left out. Matching on it ignores case, as a reader
    /// does, so CSS that upper-cases a label doesn't change what's seen.
    pub async fn visible_text(&self) -> String {
        self.try_visible_text()
            .await
            .expect("page text, with no navigation under way")
    }

    /// The visible text, or `None` while a navigation replaces the page.
    async fn try_visible_text(&self) -> Option<String> {
        self.page
            .evaluate("document.body.innerText")
            .await
            .ok()?
            .into_value()
            .ok()
    }

    async fn shows(&self, text: &str) -> bool {
        self.try_visible_text()
            .await
            .is_some_and(|shown| shown.to_lowercase().contains(&text.to_lowercase()))
    }

    /// Waits until `text` can be read on the page.
    pub async fn sees(&self, text: &str) {
        self.wait_until(&format!("{text:?} to be visible"), || async {
            self.shows(text).await.then_some(())
        })
        .await;
    }

    /// Waits until `text` can no longer be read on the page.
    pub async fn stops_seeing(&self, text: &str) {
        self.wait_until(&format!("{text:?} to disappear"), || async {
            (!self.shows(text).await).then_some(())
        })
        .await;
    }

    /// Asserts that `text` can't be read on the page right now.
    pub async fn doesnt_see(&self, text: &str) {
        assert!(
            !self.shows(text).await,
            "{text:?} is visible; the page shows:\n{}",
            self.visible_text().await
        );
    }

    /// Clicks the visible button labelled `label`, with a real mouse click.
    /// While a modal dialog is open, only its buttons count: the rest of the
    /// page can't be clicked.
    pub async fn click_button(&self, label: &str) {
        self.click("button", label).await;
    }

    /// Clicks the visible button labelled `label` in the open dialog, modal
    /// or not.
    pub async fn click_dialog_button(&self, label: &str) {
        self.click("dialog[open] button", label).await;
    }

    async fn click(&self, selector: &str, label: &str) {
        let button = self
            .wait_until(
                &format!("a visible {selector:?} labelled {label:?}"),
                || self.visible_button(selector, label),
            )
            .await;
        self.click_on(&button).await;
    }

    /// Clicks the middle of `element` with the mouse, scrolling it into view
    /// first. Unlike `Element::click`, this works with JavaScript off: that
    /// waits on an `IntersectionObserver`, whose callback never runs then.
    async fn click_on(&self, element: &Element) {
        element
            .call_js_fn(
                "function() {
                    this.scrollIntoView({ block: 'center', inline: 'center', behavior: 'instant' });
                }",
                false,
            )
            .await
            .expect("scroll into view");
        let point = element.clickable_point().await.expect("a point to click");
        self.page.click(point).await.expect("click");
    }

    async fn visible_button(&self, selector: &str, label: &str) -> Option<Element> {
        for button in self.page.find_elements(selector).await.unwrap_or_default() {
            let shown = button
                .call_js_fn(
                    "function() {
                        const modal = document.querySelector('dialog:modal');
                        return this.checkVisibility() && (!modal || modal.contains(this));
                    }",
                    false,
                )
                .await
                .ok()
                .and_then(|r| r.result.value)
                == Some(true.into());
            let text = button.inner_text().await.ok().flatten().unwrap_or_default();
            if shown && text.trim() == label {
                return Some(button);
            }
        }
        None
    }

    /// The text of every `<time>` on the page, without repeats, so the same
    /// time shown in two places is read once. A copy that went stale still
    /// shows up, as a second text.
    pub async fn times(&self) -> BTreeSet<String> {
        let times: Vec<String> = self
            .page
            .evaluate("[...document.querySelectorAll('time')].map((t) => t.innerText.trim())")
            .await
            .expect("times")
            .into_value()
            .expect("times are strings");
        times.into_iter().collect()
    }

    /// Moves the clock of a tab opened with [`Browser::open_at`] forward by
    /// `by`, running every timer that comes due on the way, and stops it again.
    pub async fn let_time_pass(&self, by: Duration) {
        self.run_clock(VirtualTimePolicy::Advance, by).await;
    }

    /// Runs the page's clock for `by` under `policy`, then stops it.
    async fn run_clock(&self, policy: VirtualTimePolicy, by: Duration) {
        let mut stopped = self
            .page
            .event_listener::<EventVirtualTimeBudgetExpired>()
            .await
            .expect("listen for the clock to stop");
        self.page
            .execute(
                SetVirtualTimePolicyParams::builder()
                    .policy(policy)
                    .budget(by.as_millis() as f64)
                    .build()
                    .unwrap(),
            )
            .await
            .expect("the page's clock moves");
        tokio::time::timeout(PATIENCE, stopped.next())
            .await
            .unwrap_or_else(|_| panic!("the page's clock didn't move {by:?} in time"))
            .expect("the browser is still there");
    }

    /// Waits until a modal dialog is open, so the rest of the page can't be
    /// used until it closes.
    pub async fn sees_a_modal_dialog(&self) {
        self.wait_until("a modal dialog", || async {
            let open: bool = self
                .page
                .evaluate("document.querySelector('dialog:modal') !== null")
                .await
                .ok()?
                .into_value()
                .ok()?;
            open.then_some(())
        })
        .await;
    }

    /// Replaces what's in the visible field labelled `label` with `text`,
    /// typed key by key.
    pub async fn fill(&self, label: &str, text: &str) {
        let field = self.field(label).await;
        self.click_on(&field).await;
        field
            .call_js_fn("function() { this.select() }", false)
            .await
            .expect("select the field's text");
        field.type_str(text).await.expect("type");
    }

    /// What the visible field labelled `label` holds.
    pub async fn value_of(&self, label: &str) -> String {
        self.field(label)
            .await
            .property("value")
            .await
            .expect("the field's value")
            .and_then(|value| value.as_str().map(str::to_owned))
            .expect("the field's value is a string")
    }

    /// The visible field whose label reads `label`.
    async fn field(&self, label: &str) -> Element {
        let has_label = format!(
            "function() {{
                return this.checkVisibility()
                    && [...this.labels].some(l => l.innerText.trim() === {});
            }}",
            serde_json::to_string(label).unwrap()
        );
        self.wait_until(&format!("a visible field labelled {label:?}"), || async {
            for field in self.page.find_elements("input").await.unwrap_or_default() {
                let labelled = field
                    .call_js_fn(has_label.clone(), false)
                    .await
                    .ok()
                    .and_then(|r| r.result.value)
                    == Some(true.into());
                if labelled {
                    return Some(field);
                }
            }
            None
        })
        .await
    }

    /// The path of the page shown now, such as `/`.
    pub async fn path(&self) -> String {
        let url = self.page.url().await.expect("page url").expect("a url");
        url::Url::parse(&url)
            .expect("a valid url")
            .path()
            .to_owned()
    }

    /// Reloads the page.
    pub async fn reload(&self) {
        self.page.reload().await.expect("reload");
        self.page.wait_for_navigation().await.expect("reload loads");
    }

    /// Polls `found` until it gives something, failing the test after
    /// [`PATIENCE`] with what the page shows.
    async fn wait_until<T, F, Fut>(&self, what: &str, mut found: F) -> T
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Option<T>>,
    {
        let deadline = Instant::now() + PATIENCE;
        loop {
            if let Some(thing) = found().await {
                return thing;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}; the page shows:\n{}",
                self.visible_text().await
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}
