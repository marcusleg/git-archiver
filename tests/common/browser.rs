//! A real headless Chromium, driven over the DevTools protocol, for tests of
//! what a person sees and does in the page.
//!
//! The browser is the first of `GIT_ARCHIVER_TEST_BROWSER` (a path or a
//! command name) or a Chrome/Chromium found on `PATH`. With none, tests fail
//! rather than skip, so a run can't pass without exercising the page.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chromiumoxide::browser::{Browser as Chromium, BrowserConfig};
use chromiumoxide::cdp::browser_protocol::emulation::{
    EventVirtualTimeBudgetExpired, SetScriptExecutionDisabledParams, SetVirtualTimePolicyParams,
    VirtualTimePolicy,
};
use chromiumoxide::cdp::browser_protocol::network::TimeSinceEpoch;
use chromiumoxide::cdp::browser_protocol::page::{
    CaptureScreenshotFormat, CaptureScreenshotParams, GetNavigationHistoryParams,
    NavigateToHistoryEntryParams, Viewport as Clip,
};
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
        Self::launch_with(viewport, viewport.emulation(), &[]).await
    }

    /// Launches a headless browser for screenshots: its pages have this
    /// viewport at twice the pixel density, and it reaches every URL, even
    /// `localhost`, through the HTTP proxy at `proxy`. With the app as the
    /// proxy, a page of the app opens under any URL, such as
    /// `http://localhost:8080/`, and the app sees that URL's host in `Host`.
    pub async fn launch_for_screenshots(viewport: Viewport, proxy: SocketAddr) -> Self {
        let emulation = EmulatedViewport {
            device_scale_factor: Some(2.0),
            ..viewport.emulation()
        };
        let proxy = format!("http://{proxy}");
        let args = [
            ("proxy-server", proxy.as_str()),
            // Without this, the browser reaches `localhost` directly.
            ("proxy-bypass-list", "<-loopback>"),
        ];
        Self::launch_with(viewport, emulation, &args).await
    }

    async fn launch_with(
        viewport: Viewport,
        emulation: EmulatedViewport,
        args: &[(&str, &str)],
    ) -> Self {
        let binary =
            find_browser().unwrap_or_else(|why| panic!("browser tests need a browser: {why}"));
        let profile = TempDir::new().unwrap();
        let config = BrowserConfig::builder()
            .user_data_dir(profile.path())
            .chrome_executable(binary)
            .viewport(emulation)
            .args(args.iter().copied())
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
        // until it has. The clock runs even while requests are pending: the
        // page's event stream stays pending for as long as it's open.
        tab.wait_until("the page to load", || async {
            tab.run_clock(VirtualTimePolicy::Advance, LOAD_STEP).await;
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
        for button in self.visible(selector).await {
            let in_modal = button
                .call_js_fn(
                    "function() {
                        const modal = document.querySelector('dialog:modal');
                        return !modal || modal.contains(this);
                    }",
                    false,
                )
                .await
                .ok()
                .and_then(|r| r.result.value)
                == Some(true.into());
            let text = button.inner_text().await.ok().flatten().unwrap_or_default();
            if in_modal && text.trim() == label {
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

    /// Clicks the visible table row that mentions `name`, on its status
    /// cell rather than the repo name, with a real mouse click.
    pub async fn click_row(&self, name: &str) {
        let cell = self.row_cell(name).await;
        self.click_on(&cell).await;
    }

    /// Scrolls the visible table row that mentions `name` to the middle of
    /// the screen, as [`Tab::click_row`] does before clicking, and returns
    /// how far the page is scrolled down then. Waits for the fonts first,
    /// as [`Tab::scroll_to`] does.
    pub async fn scroll_row_into_view(&self, name: &str) -> f64 {
        let cell = self.row_cell(name).await;
        cell.call_js_fn(
            "function() {
                return document.fonts.ready.then(() => {
                    this.scrollIntoView({ block: 'center', inline: 'center', behavior: 'instant' });
                    return window.scrollY;
                });
            }",
            true,
        )
        .await
        .expect("scroll the row into view")
        .result
        .value
        .and_then(|y| y.as_f64())
        .expect("the scroll position is a number")
    }

    /// The status cell of the visible table row that mentions `name`.
    async fn row_cell(&self, name: &str) -> Element {
        self.wait_until(&format!("a visible row {name:?}"), || async {
            for row in self.visible("tbody tr").await {
                if inner_text(&row).await.contains(name) {
                    return row.find_element("td:nth-child(2)").await.ok();
                }
            }
            None
        })
        .await
    }

    /// Waits until the row of the repo `identity` is the one marked
    /// selected and the pane shows the card of the repo configured as
    /// `url`.
    pub async fn shows_selected(&self, identity: &str, url: &str) {
        self.wait_until(
            &format!("{identity:?} to be selected, with {url:?} in the pane"),
            || async {
                let row = self.shown(".repo-table [aria-current]").await;
                let card = self.shown(".pane .repo-url").await;
                (row == [identity] && card == [url]).then_some(())
            },
        )
        .await;
    }

    /// Clicks the visible link whose text contains `label`, with a real
    /// mouse click.
    pub async fn click_link(&self, label: &str) {
        let link = self
            .wait_until(&format!("a visible link {label:?}"), || async {
                for link in self.visible("a").await {
                    if inner_text(&link).await.contains(label) {
                        return Some(link);
                    }
                }
                None
            })
            .await;
        self.click_on(&link).await;
    }

    /// The text of each element matching `selector` that a person can see.
    pub async fn shown(&self, selector: &str) -> Vec<String> {
        let mut texts = Vec::new();
        for element in self.visible(selector).await {
            texts.push(inner_text(&element).await);
        }
        texts
    }

    /// The elements matching `selector` that a person can see: rendered, and
    /// larger than the one-pixel box that hides text for all but screen
    /// readers.
    async fn visible(&self, selector: &str) -> Vec<Element> {
        let js = "function() {
            const box = this.getBoundingClientRect();
            return this.checkVisibility() && box.width > 1 && box.height > 1;
        }";
        let mut shown = Vec::new();
        for element in self.page.find_elements(selector).await.unwrap_or_default() {
            let visible = element
                .call_js_fn(js, false)
                .await
                .ok()
                .and_then(|r| r.result.value)
                == Some(true.into());
            if visible {
                shown.push(element);
            }
        }
        shown
    }

    /// Waits until the page's URL has the path `path` and the page there
    /// has loaded.
    pub async fn arrives_at(&self, path: &str) {
        self.wait_until(&format!("the URL path to be {path:?}"), || async {
            let url = self.page.url().await.ok()??;
            (url::Url::parse(&url).ok()?.path() == path).then_some(())
        })
        .await;
        self.wait_until("the page to load", || async {
            // Fails while the old page's context is torn down.
            let state = self.page.evaluate("document.readyState").await.ok()?;
            (state.into_value::<String>().ok()? == "complete").then_some(())
        })
        .await;
    }

    /// Goes back one page in the tab's history, as the back button does.
    pub async fn go_back(&self) {
        self.go_through_history(-1).await;
    }

    /// Goes forward one page in the tab's history, as the forward button
    /// does.
    pub async fn go_forward(&self) {
        self.go_through_history(1).await;
    }

    /// Goes `by` pages back (negative) or forward in the tab's history.
    async fn go_through_history(&self, by: i64) {
        let history = self
            .page
            .execute(GetNavigationHistoryParams::default())
            .await
            .expect("history")
            .result;
        let entry = usize::try_from(history.current_index + by)
            .ok()
            .and_then(|index| history.entries.get(index))
            .unwrap_or_else(|| panic!("there is no page {by} away in the history"));
        self.page
            .execute(NavigateToHistoryEntryParams::new(entry.id))
            .await
            .expect("moving through the history");
        self.arrives_at(url::Url::parse(&entry.url).expect("a history URL").path())
            .await;
    }

    /// The path of the page shown now, such as `/`.
    pub async fn path(&self) -> String {
        let url = self.page.url().await.expect("page url").expect("a url");
        url::Url::parse(&url)
            .expect("a valid url")
            .path()
            .to_owned()
    }

    /// Marks the document shown now, for [`Tab::still_shows_the_marked_document`].
    pub async fn mark_document(&self) {
        self.page
            .evaluate("window.testMark = true")
            .await
            .expect("mark the document");
    }

    /// Asserts the tab still shows the document [`Tab::mark_document`]
    /// marked: it neither reloaded nor navigated away since.
    pub async fn still_shows_the_marked_document(&self) {
        let marked: bool = self
            .page
            .evaluate("window.testMark === true")
            .await
            .expect("read the mark")
            .into_value()
            .expect("the mark is a bool");
        assert!(marked, "the page was reloaded or replaced");
    }

    /// Asserts the tab shows a document loaded since [`Tab::mark_document`]
    /// marked the last one: the browser navigated.
    pub async fn shows_a_new_document(&self) {
        let marked: bool = self
            .page
            .evaluate("window.testMark === true")
            .await
            .expect("read the mark")
            .into_value()
            .expect("the mark is a bool");
        assert!(!marked, "the page didn't navigate");
    }

    /// Clicks the visible `<summary>` reading `label`, with a real mouse
    /// click, opening or closing its `<details>`.
    pub async fn click_summary(&self, label: &str) {
        self.click("summary", label).await;
    }

    /// Moves keyboard focus to the visible button labelled `label` for
    /// screen readers.
    pub async fn focus_button(&self, label: &str) {
        let button = self
            .wait_until(&format!("a visible button labelled {label:?}"), || async {
                for button in self.visible("button[aria-label]").await {
                    if button.attribute("aria-label").await.ok()?.as_deref() == Some(label) {
                        return Some(button);
                    }
                }
                None
            })
            .await;
        button.focus().await.expect("focus the button");
    }

    /// The label of the element with keyboard focus: its `aria-label`, or
    /// else its text.
    pub async fn focused(&self) -> String {
        self.page
            .evaluate(
                "document.activeElement.getAttribute('aria-label') \
                 ?? document.activeElement.innerText.trim()",
            )
            .await
            .expect("read the focus")
            .into_value()
            .expect("the label is a string")
    }

    /// Scrolls the page down to `y` pixels from the top, or as far as it
    /// goes, and returns where it ended up. Waits for the fonts first: as
    /// they load, the text changes size and the browser scrolls to keep
    /// what's in view.
    pub async fn scroll_to(&self, y: u32) -> f64 {
        self.page
            .evaluate(format!(
                "document.fonts.ready.then(() => {{
                    window.scrollTo({{ top: {y}, behavior: 'instant' }});
                    return window.scrollY;
                }})"
            ))
            .await
            .expect("scroll")
            .into_value()
            .expect("the scroll position is a number")
    }

    /// How far the page is scrolled down, in pixels.
    pub async fn scroll_y(&self) -> f64 {
        self.page
            .evaluate("window.scrollY")
            .await
            .expect("scroll position")
            .into_value()
            .expect("the scroll position is a number")
    }

    /// Runs `script` in the page, for staging a screenshot.
    pub async fn run(&self, script: &str) {
        self.page.evaluate(script).await.expect("the script runs");
    }

    /// Saves the page as a PNG once its fonts have loaded: the whole page,
    /// from top to bottom, if `whole_page`, else what the viewport shows. In
    /// a tab opened with [`Browser::open_at`], the clock runs in small steps
    /// only until the fonts have loaded. (Steps that waited for pending
    /// requests would wait forever on the page's open `/events` stream.)
    pub async fn save_screenshot(&self, path: &Path, whole_page: bool) {
        self.wait_until("the fonts to load", || async {
            self.run_clock(VirtualTimePolicy::Advance, LOAD_STEP).await;
            let status = self.page.evaluate("document.fonts.status").await.ok()?;
            (status.into_value::<String>().ok()? == "loaded").then_some(())
        })
        .await;
        let (width, height): (f64, f64) = self
            .page
            .evaluate(match whole_page {
                true => "[window.innerWidth, document.documentElement.scrollHeight]",
                false => "[window.innerWidth, window.innerHeight]",
            })
            .await
            .expect("page size")
            .into_value()
            .expect("page size is two numbers");
        let capture = self.page.execute(
            CaptureScreenshotParams::builder()
                .format(CaptureScreenshotFormat::Png)
                .capture_beyond_viewport(true)
                .clip(Clip {
                    x: 0.0,
                    y: 0.0,
                    width,
                    height,
                    scale: 1.0,
                })
                .build(),
        );
        // A page draws nothing while its clock stands still, so run the clock
        // in small steps until the screenshot is taken. The steps add up to
        // far less than the minute it takes a relative time to change.
        tokio::pin!(capture);
        let screenshot = loop {
            tokio::select! {
                taken = &mut capture => break taken.expect("the browser takes a screenshot"),
                () = self.run_clock(VirtualTimePolicy::Advance, LOAD_STEP) => {}
            }
        };
        let png = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            &screenshot.result.data,
        )
        .expect("the screenshot is base64");
        std::fs::write(path, png)
            .unwrap_or_else(|e| panic!("couldn't write {}: {e}", path.display()));
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

/// The text `element` shows, or nothing if it can't be read.
async fn inner_text(element: &Element) -> String {
    element
        .inner_text()
        .await
        .ok()
        .flatten()
        .unwrap_or_default()
}
