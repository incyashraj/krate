//! A web app inside a Krate app (IC-1004): the runtime's side.
//!
//! The app names a folder of its assets; the runtime checks it is inside
//! the bundle and holds the start page, hands the platform the folder and
//! the hosts the app was granted (so the page's own reach is the app's),
//! and turns the page's messages into `action(view)` events the app answers
//! with `receive`. The bridge carries text only: whatever the page needs
//! from the computer it asks the app's own code for, which calls Krate
//! behind the same permission wall as any app.

use std::collections::{BTreeMap, BTreeSet};

use krate_adapter_common::appview;

use crate::phase3_gui_bindings::krate::ui as ui3;
use crate::phase3_gui_host::Phase3GuiHost;
use crate::phase4_gui_bindings::krate::ui as ui4;
use crate::phase4_gui_host::error_to_phase4;

/// The web views one run opened.
#[derive(Debug, Default)]
pub(crate) struct Views {
    /// Each view's window.
    pub(crate) windows: BTreeMap<u64, u64>,
    /// Views already told (by an `action` event) that messages wait.
    told: BTreeSet<u64>,
    /// What the app posted, kept headless for checks.
    pub(crate) posted: Vec<(u64, String)>,
    next: u64,
}

fn platform(why: String) -> ui3::types::UiError {
    ui3::types::UiError::Platform(why)
}

impl Phase3GuiHost {
    /// Fill a window with the app's web files.
    pub(crate) fn app_view_open(
        &self,
        window: u64,
        folder: &str,
        start: &str,
        preload: &str,
    ) -> Result<u64, ui3::types::UiError> {
        let id = self.window_id(window)?;
        if self.views.borrow().windows.len() >= appview::MAX_VIEWS {
            return Err(platform(format!(
                "an app shows at most {} web views",
                appview::MAX_VIEWS
            )));
        }
        appview::check_message(preload).map_err(platform)?;
        let assets = self
            .asset_root
            .clone()
            .ok_or_else(|| platform("this app carries no assets, so no web files".to_string()))?;
        let folder_rel =
            appview::resolve_path(folder).map_err(|r| platform(format!("{folder}: {r:?}")))?;
        // resolve_path names index.html for an empty folder; a folder is a
        // folder.
        let folder_rel = folder_rel
            .strip_suffix("index.html")
            .unwrap_or(&folder_rel)
            .trim_end_matches('/')
            .to_string();
        let dir = if folder_rel.is_empty() {
            assets
        } else {
            assets.join(&folder_rel)
        };
        let start_rel =
            appview::resolve_path(start).map_err(|r| platform(format!("{start}: {r:?}")))?;
        appview::read_file(&dir, &start_rel)
            .map_err(|why| platform(format!("the start page: {why}")))?;
        let view = {
            let mut views = self.views.borrow_mut();
            let view = appview::FIRST_VIEW_ID + views.next;
            views.next += 1;
            view
        };
        if !self.headless_run() {
            self.dispatcher()
                .bluetooth()
                .open_app_view(id, view, dir, &start_rel, preload, &self.web_page_hosts())
                .map_err(ui3::types::UiError::Unsupported)?;
        }
        self.views.borrow_mut().windows.insert(view, window);
        Ok(view)
    }

    fn owns_view(&self, view: u64) -> bool {
        self.views.borrow().windows.contains_key(&view)
    }

    /// Hand the page a message.
    pub(crate) fn app_view_post(
        &self,
        view: u64,
        message: &str,
    ) -> Result<(), ui3::types::UiError> {
        if !self.owns_view(view) {
            return Err(ui3::types::UiError::InvalidWidget);
        }
        appview::check_message(message).map_err(platform)?;
        if self.headless_run() {
            self.views
                .borrow_mut()
                .posted
                .push((view, message.to_string()));
            return Ok(());
        }
        self.dispatcher()
            .bluetooth()
            .app_view_post(view, message)
            .map_err(platform)
    }

    /// What the page sent since the app last asked.
    pub(crate) fn app_view_receive(&self, view: u64) -> Vec<String> {
        if !self.owns_view(view) {
            return Vec::new();
        }
        self.views.borrow_mut().told.remove(&view);
        appview::take_messages(view)
    }

    /// Take a view away.
    pub(crate) fn app_view_close(&self, view: u64) {
        let owned = self.views.borrow_mut().windows.remove(&view).is_some();
        if owned {
            let _ = appview::take_messages(view);
            if !self.headless_run() {
                self.dispatcher().bluetooth().close_app_view(view);
            }
        }
    }

    /// One `action(view)` for each view whose page has sent something the
    /// app has not read yet.
    pub(crate) fn tell_waiting_views(&self) {
        let waiting = appview::views_waiting();
        if waiting.is_empty() {
            return;
        }
        let mut views = self.views.borrow_mut();
        for view in waiting {
            if views.windows.contains_key(&view) && views.told.insert(view) {
                self.pending_events
                    .borrow_mut()
                    .push_back(ui3::types::Event::Action(view));
            }
        }
    }
}

/// A web app inside the app (IC-1004). Its reach is the app's: the hosts
/// granted `net.connect`, and everything else through the app's own code.
impl ui4::app_view::Host for Phase3GuiHost {
    fn open(
        &mut self,
        window: u64,
        folder: String,
        start: String,
        preload: String,
    ) -> wasmtime::Result<Result<u64, ui4::types::UiError>> {
        Ok(self
            .app_view_open(window, &folder, &start, &preload)
            .map_err(error_to_phase4))
    }

    fn post(
        &mut self,
        view: u64,
        message: String,
    ) -> wasmtime::Result<Result<(), ui4::types::UiError>> {
        Ok(self.app_view_post(view, &message).map_err(error_to_phase4))
    }

    fn receive(&mut self, view: u64) -> wasmtime::Result<Vec<String>> {
        Ok(self.app_view_receive(view))
    }

    fn close(&mut self, view: u64) -> wasmtime::Result<()> {
        self.app_view_close(view);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phase3_ui::Phase3HostUiMode;
    use crate::uapi::UapiGuard;

    fn host(assets: &std::path::Path) -> (Phase3GuiHost, u64) {
        let policy =
            krate_policy::SessionPolicy::from_cli_grants(&["ui.window:create".to_string()])
                .expect("policy");
        let mut host = Phase3GuiHost::new(UapiGuard::new(policy), Phase3HostUiMode::HeadlessDraft)
            .expect("headless host")
            .with_asset_root(Some(assets.to_path_buf()));
        let w = ui3::window::Host::create(
            &mut host,
            "Web".into(),
            ui3::types::WindowSize {
                width: 400,
                height: 300,
            },
        )
        .unwrap()
        .unwrap();
        (host, w)
    }

    fn assets(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("krate-appview-host-{name}-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("web")).unwrap();
        std::fs::write(dir.join("web/index.html"), "<p>hi</p>").unwrap();
        std::fs::write(dir.join("secret.txt"), "no").unwrap();
        dir
    }

    /// A page's message is an `action(view)` event, once, until read.
    #[test]
    fn a_page_message_wakes_the_app_once_until_read() {
        let dir = assets("wake");
        let (mut host, w) = host(&dir);
        let view = host.app_view_open(w, "web", "index.html", "").unwrap();
        assert!(view >= appview::FIRST_VIEW_ID);
        appview::push_message(view, "{\"hello\":1}".into());
        appview::push_message(view, "two".into());
        host.tell_waiting_views();
        host.tell_waiting_views();
        let events: Vec<_> = host.pending_events.borrow_mut().drain(..).collect();
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(
            ui4::app_view::Host::receive(&mut host, view).unwrap(),
            vec!["{\"hello\":1}".to_string(), "two".to_string()]
        );
        ui4::app_view::Host::post(&mut host, view, "back".into())
            .unwrap()
            .unwrap();
        assert_eq!(host.views.borrow().posted, vec![(view, "back".to_string())]);
        let too_big = "x".repeat(appview::MAX_MESSAGE + 1);
        assert!(ui4::app_view::Host::post(&mut host, view, too_big)
            .unwrap()
            .is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The folder and the start page stay inside the app's assets.
    #[test]
    fn the_folder_and_start_page_stay_inside_the_assets() {
        let dir = assets("inside");
        let (host, w) = host(&dir);
        assert!(host.app_view_open(w, "../", "secret.txt", "").is_err());
        assert!(host.app_view_open(w, "web", "../secret.txt", "").is_err());
        assert!(host.app_view_open(w, "web", "missing.html", "").is_err());
        assert!(host.app_view_open(w, "", "web/index.html", "").is_ok());
        assert!(matches!(
            host.app_view_open(99, "web", "index.html", ""),
            Err(ui3::types::UiError::InvalidWindow)
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
