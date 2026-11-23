//! Web viewer (a core plugin): web pages in a tab. The address field takes
//! an address or words to search for, as the notes app's does: an address
//! with its scheme as typed, something that looks like a host given
//! `https://`, anything else searched on DuckDuckGo. The page itself is the
//! system's web view placed over the tab (krate:ui web-page), which goes
//! only to https hosts the app was granted.

use crate::app::{App, Input, Mods, Tab, View};
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;

pub const BLANK: &str = "about:blank";

/// A web page's tab.
pub struct WebTab {
    /// The address it shows, as workspace.json keeps it.
    pub url: String,
    /// The page's title, or its address until it has one.
    pub title: String,
    /// The address field while it is being typed in.
    pub field: Option<Input>,
    /// The system's web view for it: the window and the page, once open.
    pub page: Option<(u64, u64)>,
    /// The frame it was last drawn in, to take it away when it is not.
    pub seen: u64,
    /// Loading, and why the last address was not shown.
    pub loading: bool,
    pub problem: String,
    /// Asked to go again to `url` (the reload button, a typed address).
    pub go: bool,
    /// Asked to go back a page.
    pub back: bool,
    /// The addresses it has shown, for the back button.
    pub hist: alloc::vec::Vec<String>,
}

impl WebTab {
    pub fn new(url: &str) -> WebTab {
        WebTab {
            url: String::from(url),
            title: title_for(url, ""),
            field: None,
            page: None,
            seen: 0,
            loading: false,
            problem: String::new(),
            go: true,
            back: false,
            hist: alloc::vec::Vec::new(),
        }
    }

    /// The text the address field shows.
    pub fn shown(&self) -> &str {
        self.field
            .as_ref()
            .map_or(self.url.as_str(), |f| f.text.as_str())
    }
}

/// A tab's title: the page's, else its address; a blank one is "New web
/// viewer".
pub fn title_for(url: &str, title: &str) -> String {
    if !title.is_empty() {
        String::from(title)
    } else if url == BLANK || url.is_empty() {
        String::from("New web viewer")
    } else {
        String::from(url)
    }
}

/// Whether typed text looks like a host and path (no spaces, a dot in the
/// host, or localhost), so it is opened rather than searched for.
fn looks_like_host(t: &str) -> bool {
    if t.contains(char::is_whitespace) || t.is_empty() {
        return false;
    }
    let host = t.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit_once(':').map_or(host, |(h, port)| {
        if port.chars().all(|c| c.is_ascii_digit()) {
            h
        } else {
            host
        }
    });
    if host == "localhost" {
        return true;
    }
    let mut parts = host.split('.');
    let n = host.split('.').count();
    n >= 2
        && parts.all(|p| !p.is_empty() && p.chars().all(|c| c.is_alphanumeric() || c == '-'))
        && host.rsplit('.').next().is_some_and(|tld| {
            tld.chars().all(char::is_alphabetic) || tld.chars().all(|c| c.is_ascii_digit())
        })
}

/// What the address field opens for `typed`.
pub fn address_for(typed: &str) -> String {
    let t = typed.trim();
    let low = t.to_ascii_lowercase();
    if t.is_empty() || low == BLANK {
        return String::from(BLANK);
    }
    if low.starts_with("http://") || low.starts_with("https://") {
        return String::from(t);
    }
    if looks_like_host(t) {
        return format!("https://{t}");
    }
    search_for(t)
}

/// The search page for `q` (the notes app's default engine).
pub fn search_for(q: &str) -> String {
    format!("https://duckduckgo.com/?q={}", encode_component(q))
}

/// `encodeURIComponent`.
fn encode_component(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

impl App {
    /// The view a group shows.
    pub fn pane_view(&self, pane: usize) -> Option<&View> {
        let (tabs, act) = self.pane_tabs(pane);
        tabs.get(act).and_then(|t| t.view.as_ref())
    }

    /// Every tab of every group, to change.
    pub fn all_tabs_mut(&mut self) -> impl Iterator<Item = &mut Tab> {
        self.tabs
            .iter_mut()
            .chain(self.panes.iter_mut().flat_map(|p| p.tabs.iter_mut()))
    }

    /// A press in a web viewer's header.
    pub fn web_hit(&mut self, h: &crate::app::Hit) {
        use crate::app::Hit;
        match h {
            Hit::WebBack => {
                if let Some(w) = self.web_mut() {
                    w.back = true;
                }
            }
            Hit::WebReload => {
                if let Some(w) = self.web_mut() {
                    w.go = true;
                    w.problem.clear();
                }
            }
            Hit::WebAddress if self.web().is_some_and(|w| w.field.is_none()) => {
                self.focus_address();
            }
            Hit::WebReader => self.say(
                "Reader view needs the page's text, which the system's web view does not give apps",
            ),
            _ => {}
        }
    }

    pub fn web(&self) -> Option<&WebTab> {
        match self.view() {
            Some(View::Web(w)) => Some(w),
            _ => None,
        }
    }

    pub fn web_mut(&mut self) -> Option<&mut WebTab> {
        match self.view_mut() {
            Some(View::Web(w)) => Some(w),
            _ => None,
        }
    }

    /// Open `url` in a web viewer: in a new tab, or the empty one in front.
    pub fn open_web(&mut self, url: &str) {
        let reuse = self
            .tab()
            .is_some_and(|t| t.doc.is_none() && t.view.is_none());
        if !reuse {
            self.new_tab();
        }
        if let Some(t) = self.tab_mut() {
            *t = Tab::empty();
            t.view = Some(View::Web(Box::new(WebTab::new(url))));
        }
        self.save_workspace();
    }

    /// "Web viewer: Open web viewer": a blank page with its address field
    /// ready to type in.
    pub fn new_web_viewer(&mut self) {
        self.open_web(BLANK);
        self.focus_address();
    }

    /// "Web viewer: Focus address bar".
    pub fn focus_address(&mut self) {
        if let Some(w) = self.web_mut() {
            let mut f = Input::with(&w.url);
            f.select_all();
            w.field = Some(f);
        }
    }

    /// Go to what is typed in the address field.
    pub fn web_go(&mut self, typed: &str) {
        let url = address_for(typed);
        if let Some(w) = self.web_mut() {
            w.title = title_for(&url, "");
            w.url = url;
            w.field = None;
            w.problem.clear();
            w.go = true;
        }
        self.save_workspace();
    }

    /// A link to the web: in a web viewer when the plugin is on and opens
    /// links (the notes app's "Open external links" option). False when it
    /// is the browser's to open.
    pub fn open_link_in_web(&mut self, url: &str) -> bool {
        let low = url.to_ascii_lowercase();
        if !self.settings.plugin_on("webviewer")
            || !(low.starts_with("http://") || low.starts_with("https://"))
            || !self.web_opens_links()
        {
            return false;
        }
        self.open_web(url);
        true
    }

    /// The plugin's own option, from `.obsidian/webviewer.json`
    /// (`openExternalURLs`, on unless turned off).
    fn web_opens_links(&mut self) -> bool {
        self.vault
            .as_mut()
            .and_then(|v| v.fs.read(".obsidian/webviewer.json").ok())
            .and_then(|t| crate::json::parse(&t))
            .and_then(|d| d.get_bool("openExternalURLs"))
            .unwrap_or(true)
    }

    /// Typing while the address field is open. True when it took it.
    pub fn web_text(&mut self, s: &str) -> bool {
        match self.web_mut().and_then(|w| w.field.as_mut()) {
            Some(f) => {
                f.insert(s);
                true
            }
            None => false,
        }
    }

    /// A key while the address field is open. True when it took it.
    pub fn web_key(&mut self, key: &str, m: Mods) -> bool {
        let Some(f) = self.web_mut().and_then(|w| w.field.as_mut()) else {
            return false;
        };
        match key {
            "Enter" => {
                let typed = f.text.clone();
                self.web_go(&typed);
            }
            "Escape" => {
                if let Some(w) = self.web_mut() {
                    w.field = None;
                }
            }
            _ => {
                f.key(key, m);
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_address_field_opens_or_searches_as_the_notes_app_does() {
        // Each answer read from the notes app 1.15.1's own address field
        // (its suggestion list, through its developer tools).
        assert_eq!(address_for("example.com"), "https://example.com");
        assert_eq!(
            address_for("hello world"),
            "https://duckduckgo.com/?q=hello%20world"
        );
        assert_eq!(
            address_for("https://example.com/a"),
            "https://example.com/a"
        );
        assert_eq!(address_for("about:blank"), BLANK);
        assert_eq!(address_for("krate.tech/docs"), "https://krate.tech/docs");
        assert_eq!(address_for("localhost:3000"), "https://localhost:3000");
        assert_eq!(address_for("rust"), "https://duckduckgo.com/?q=rust");
        assert_eq!(title_for(BLANK, ""), "New web viewer");
        assert_eq!(
            title_for("https://example.com/", "Example Domain"),
            "Example Domain"
        );
    }
}
