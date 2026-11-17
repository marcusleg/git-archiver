// The page script. Opens the dialog a `commandfor` button names where the
// browser lacks invoker commands, shows a dialog the page opened on load as
// a modal, copies a `data-copy` button's text, once a minute rewords every
// `data-ago` time the way `time_ago` in `src/web.rs` does, by the browser's
// clock, keeps a page marked `data-live` up to date (see `events` in
// `src/web.rs`), and on a desktop selects a row without leaving the page, so
// the list keeps its scroll position.
//
// The clipboard API needs a secure context, which a plain-HTTP server on
// another host isn't, so copying falls back to selecting the text and
// copying the selection, as it does when the browser denies clipboard
// access.

for (const dialog of document.querySelectorAll("dialog[open]")) {
  dialog.close();
  dialog.showModal();
}
document.addEventListener("click", (event) => {
  const invoker = event.target.closest("[commandfor]");
  if (invoker && !("commandForElement" in invoker)) {
    const dialog = document.getElementById(invoker.getAttribute("commandfor"));
    if (!dialog.open) dialog.showModal();
  }
  const button = event.target.closest("[data-copy]");
  if (!button) return;
  const copied = () => {
    button.classList.add("copied");
    setTimeout(() => button.classList.remove("copied"), 1500);
  };
  const selectAndCopy = () => {
    const range = document.createRange();
    range.selectNodeContents(button.previousElementSibling);
    getSelection().removeAllRanges();
    getSelection().addRange(range);
    if (document.execCommand("copy")) copied();
  };
  if (navigator.clipboard && window.isSecureContext) {
    navigator.clipboard.writeText(button.dataset.copy).then(copied, selectAndCopy);
  } else {
    selectAndCopy();
  }
});

const timeAgo = (then) => {
  const secs = Math.floor((Date.now() - then) / 1000);
  if (secs < 60) return "just now";
  const [count, unit] =
    secs < 3600 ? [Math.floor(secs / 60), "minute"]
    : secs < 86400 ? [Math.floor(secs / 3600), "hour"]
    : [Math.floor(secs / 86400), "day"];
  return `${count} ${unit}${count === 1 ? "" : "s"} ago`;
};
setInterval(() => {
  for (const time of document.querySelectorAll("time[data-ago]")) {
    time.textContent = timeAgo(Date.parse(time.dateTime));
  }
}, 60000);

// A page marked `data-live` shows what its path renders. On each `changed`
// event, fetch that path again and swap in its content. Events that come
// while fetching fold into one more fetch. While a dialog is open the swap
// waits, and happens once it closes. The swap keeps the scroll position,
// the focus, open `<details>` and the banner of a failed action. A repo
// page whose repo is gone becomes the list.
//
// On a desktop, selecting a row swaps in the repo's page the same way and
// pushes its URL, so the list stays where it was scrolled; back and forward
// swap in the page of their entry, and the browser restores the scroll
// position it had there. While a selection loads, updates wait for it. A
// narrow screen shows the list and a repo on pages of their own, so there a
// row navigates as a plain link.
const main = document.querySelector("main[data-live]");
if (main) {
  // The path whose page to show. It isn't always the URL's: a failed form
  // post shows the page of the path it came from.
  let wanted = main.dataset.live;
  history.replaceState({ live: wanted }, "");
  // Each fetch is numbered, and only the latest one's page is shown.
  let latest = 0;
  let stale = false;
  let busy = false;
  let selecting = 0;
  const dialogOpen = () => document.querySelector("dialog[open]") !== null;
  // Fetches the `wanted` page and swaps it in, unless a later fetch started
  // meanwhile. A `live` update waits for an open dialog and keeps a failed
  // action's banner; a selection does neither. Throws if the page can't be
  // had and no later fetch started.
  const show = async (live) => {
    const fetching = ++latest;
    let path = wanted;
    let page;
    try {
      let response = await fetch(path);
      if (response.status === 404 && path !== "/") {
        path = "/";
        response = await fetch(path);
      }
      if (!response.ok) throw new Error(`${path}: ${response.status}`);
      page = new DOMParser().parseFromString(await response.text(), "text/html");
    } catch (error) {
      if (fetching === latest) throw error;
      return;
    }
    if (fetching !== latest) return;
    const fresh = page.querySelector("main[data-live]");
    if (!fresh) throw new Error(`${path}: not a live page`);
    if (live && dialogOpen()) {
      stale = true;
      return;
    }
    if (path !== wanted) {
      wanted = path;
      history.replaceState({ live: path }, "", path);
    }
    const [x, y] = [scrollX, scrollY];
    const banner = main.querySelector(":scope > [role=alert]");
    if (live && banner) fresh.prepend(banner.cloneNode(true));
    const repoUrlOf = (element) => element.closest("article")?.querySelector(".repo-url")?.textContent;
    const opened = [...main.querySelectorAll("details[open]")].map(repoUrlOf);
    for (const details of fresh.querySelectorAll("details")) {
      if (opened.includes(repoUrlOf(details))) details.open = true;
    }
    const focused = main.contains(document.activeElement) ? describe(document.activeElement) : null;
    main.replaceChildren(...fresh.childNodes);
    main.dataset.live = fresh.dataset.live;
    document.title = page.title;
    scrollTo(x, y);
    if (focused) {
      const again = [...main.querySelectorAll(focused.tag)].find((e) => describe(e).key === focused.key);
      again?.focus({ preventScroll: true });
    }
  };
  // What tells a focused element apart from others of its kind.
  const describe = (element) => ({
    tag: element.tagName,
    key: element.getAttribute("href") ?? element.getAttribute("aria-label") ?? element.textContent.trim(),
  });

  // Brings the page up to date if a change was announced, once nothing
  // holds it back, and again for changes announced meanwhile.
  const update = async () => {
    if (busy || selecting || !stale || dialogOpen()) return;
    busy = true;
    stale = false;
    try {
      await show(true);
    } catch {
      // The app can't be reached; the next event tries again.
    } finally {
      busy = false;
    }
    update();
  };
  // `close` doesn't bubble, so listen as it comes down from the document.
  document.addEventListener("close", update, true);
  if ("EventSource" in window) {
    const events = new EventSource("/events");
    events.addEventListener("changed", () => {
      stale = true;
      update();
    });
    // Changes may have gone unannounced while the stream was down.
    let lost = false;
    const note = document.querySelector(".live-note");
    events.addEventListener("error", () => {
      lost = true;
      note.hidden = false;
    });
    events.addEventListener("open", () => {
      note.hidden = true;
      if (!lost) return;
      lost = false;
      stale = true;
      update();
    });
  }

  // Shows the page of `path`, whose URL the history already holds. Where
  // the app can't give it, the browser loads it instead, and says why.
  const select = async (path) => {
    wanted = path;
    selecting++;
    try {
      await show(false);
    } catch {
      location.replace(path);
    } finally {
      selecting--;
    }
    update();
  };
  // Whether the stylesheet gives the list and each repo a page of their own,
  // which it says by hiding the list page's pane. The stylesheet alone knows
  // the width where that starts. Rows can only be clicked on the list page
  // then, since a repo's page hides the list. A list page without a pane
  // (no tracked repo) can't tell, so its rows navigate, which always works.
  const narrow = (row) => {
    const pane = row.closest(".columns").querySelector(".pane");
    return !pane || getComputedStyle(pane).display === "none";
  };
  document.addEventListener("click", (event) => {
    const link = event.target.closest(".row-link");
    if (!link || narrow(link) || event.defaultPrevented || event.button !== 0
        || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    event.preventDefault();
    const path = new URL(link.href).pathname;
    if (path !== location.pathname) history.pushState({ live: path }, "", path);
    select(path);
  });
  addEventListener("popstate", (event) => select(event.state?.live ?? location.pathname));
}
