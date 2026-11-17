//! Reading the list page the way a person sees it: the tables of tracked
//! repos and untracked archives, and the selected repo's card in the pane.

use scraper::{ElementRef, Html, Selector};

pub fn select<'a>(html: &'a Html, selector: &str) -> Vec<ElementRef<'a>> {
    html.select(&Selector::parse(selector).unwrap()).collect()
}

pub fn select_in<'a>(element: &ElementRef<'a>, selector: &str) -> Vec<ElementRef<'a>> {
    element
        .select(&Selector::parse(selector).unwrap())
        .collect()
}

/// An element's text, with runs of whitespace collapsed to one space.
pub fn text(element: &ElementRef) -> String {
    element
        .text()
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// One row of a list page table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The repo cell's text: the identity, `host/path`.
    pub repo: String,
    /// The muted `host/` part of the repo cell.
    pub host: String,
    /// The bold `path` part of the repo cell.
    pub path: String,
    /// The repo name's hover title: the configured URL, or the identity of
    /// an untracked archive.
    pub title: String,
    /// The status cell's text.
    pub status: String,
    /// The `class` of the status cell's icon.
    pub icon: String,
    pub last_success: String,
    /// The `datetime` of the last success cell's `<time>`, if any.
    pub last_success_time: Option<String>,
    pub size: String,
    /// Where the repo name links to: the repo's page.
    pub href: String,
    /// Whether the row is marked as the repo the pane shows.
    pub selected: bool,
}

pub struct ListPage {
    pub html: Html,
}

impl ListPage {
    pub fn parse(text: &str) -> Self {
        Self {
            html: Html::parse_document(text),
        }
    }

    /// The section headed `heading`, such as "Tracked repos", if any.
    fn section(&self, heading: &str) -> Option<ElementRef<'_>> {
        select(&self.html, "main section")
            .into_iter()
            .find(|section| {
                select_in(section, "h2")
                    .first()
                    .is_some_and(|h2| text(h2).starts_with(heading))
            })
    }

    /// The column headings of the table in the section headed `heading`.
    pub fn columns(&self, heading: &str) -> Vec<String> {
        let section = self
            .section(heading)
            .unwrap_or_else(|| panic!("no {heading:?} section"));
        select_in(&section, "table thead th")
            .iter()
            .map(text)
            .collect()
    }

    /// The rows of the table in the section headed `heading`, in page order.
    /// Empty if there is no such section.
    pub fn rows(&self, heading: &str) -> Vec<Row> {
        let Some(section) = self.section(heading) else {
            return Vec::new();
        };
        select_in(&section, "table tbody tr")
            .iter()
            .map(|tr| {
                let cells = select_in(tr, "td");
                assert_eq!(cells.len(), 4, "a row has four cells: {}", tr.html());
                let time = select_in(&cells[2], "time").into_iter().next();
                Row {
                    repo: text(&cells[0]),
                    host: select_in(&cells[0], ".host")
                        .first()
                        .map(text)
                        .unwrap_or_default(),
                    path: select_in(&cells[0], "strong")
                        .first()
                        .map(text)
                        .unwrap_or_default(),
                    title: cells[0].value().attr("title").unwrap().to_owned(),
                    status: text(&cells[1]),
                    icon: select_in(&cells[1], "svg")
                        .first()
                        .and_then(|svg| svg.value().attr("class"))
                        .unwrap_or_default()
                        .to_owned(),
                    last_success: text(&cells[2]),
                    last_success_time: time
                        .and_then(|t| t.value().attr("datetime").map(str::to_owned)),
                    size: text(&cells[3]),
                    href: select_in(&cells[0], "a[href]")
                        .first()
                        .and_then(|a| a.value().attr("href"))
                        .unwrap_or_else(|| panic!("the repo name is a link: {}", tr.html()))
                        .to_owned(),
                    selected: tr.value().attr("aria-current") == Some("true"),
                }
            })
            .collect()
    }

    pub fn tracked(&self) -> Vec<Row> {
        self.rows("Tracked repos")
    }

    pub fn untracked(&self) -> Vec<Row> {
        self.rows("Untracked archives")
    }

    /// The page's `<title>`.
    pub fn title(&self) -> String {
        text(&select(&self.html, "head title")[0])
    }

    /// The card in the detail pane, if the page has a pane.
    pub fn pane_card(&self) -> Option<ElementRef<'_>> {
        let cards = select(&self.html, "main aside article");
        assert!(cards.len() <= 1, "the pane shows one card");
        cards.into_iter().next()
    }

    /// The repo the pane's card is about, as its heading names it.
    pub fn pane_title(&self) -> Option<String> {
        self.pane_card()
            .map(|card| text(&select_in(&card, ".repo-url")[0]))
    }
}
