//! Menus for the running app (IC-1002): its menu bar, right-click menus,
//! and what the person chooses in them.
//!
//! The runtime keeps each window's bar as a checked [`MenuTree`] and is the
//! one place a choice is decided: a check item flips and a radio group moves
//! its mark here, the native menu is set again to show it, and the app gets
//! an ordinary `action(id)` event. A standard item the system cannot carry
//! out alone arrives as a role -- Quit ends the app; Copy, Paste and the
//! other edit items reach an app that draws its own editor as the shortcut
//! it already handles.
//!
//! Headless runs keep the menus and show nothing, so a check can read what
//! an app put in its bar.

use std::collections::BTreeMap;

use krate_adapter_common::menus::{ItemKind, MenuItem, MenuSignal, MenuTree, Role, TrayIcon};
use krate_adapter_common::ui::{KeyEvent, Modifiers, WindowId};

use crate::phase3_gui_bindings::krate::ui as ui3;
use crate::phase3_gui_host::Phase3GuiHost;
use crate::phase4_gui_bindings::krate::ui as ui4;
use crate::phase4_gui_host::error_to_phase4;

/// The menus one run has set.
#[derive(Debug, Default)]
pub(crate) struct Shell {
    /// Each window's bar, by the app's window id.
    pub(crate) bars: BTreeMap<u64, MenuTree>,
    /// Each window's right-click menu.
    pub(crate) contexts: BTreeMap<u64, MenuTree>,
    /// The tray icons shown (IC-1003), by id.
    pub(crate) trays: BTreeMap<u64, TrayState>,
    /// The next id a headless run hands out.
    next_tray: u64,
    /// The last menu opened at the pointer (kept for headless checks).
    pub(crate) last_popup: Option<(u64, f32, f32, MenuTree)>,
}

/// The person chose Quit (IC-1002): the error a look for input ends the
/// app with, which the run reads as a close, not a crash.
#[derive(Debug)]
pub(crate) struct PersonQuit;

impl std::fmt::Display for PersonQuit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the person quit the app")
    }
}

impl std::error::Error for PersonQuit {}

/// One tray icon as the run keeps it.
#[derive(Debug, Default, Clone)]
pub(crate) struct TrayState {
    /// Its menu, if it has one.
    pub(crate) menu: Option<MenuTree>,
    /// The action ids its left and right clicks send.
    pub(crate) left: u64,
    pub(crate) right: u64,
    /// What it says (kept for headless checks).
    pub(crate) tooltip: String,
    pub(crate) title: String,
}

/// A WIT item as the shared module's.
fn item_from_wit(item: ui4::menus::Item) -> MenuItem {
    use ui4::menus::{ItemKind as K, Role as R};
    MenuItem {
        id: item.id,
        parent: item.parent,
        kind: match item.kind {
            K::Normal => ItemKind::Normal,
            K::Check => ItemKind::Check,
            K::Radio => ItemKind::Radio,
            K::Separator => ItemKind::Separator,
            K::Submenu => ItemKind::Submenu,
        },
        label: item.label,
        enabled: item.enabled,
        checked: item.checked,
        shortcut: item.shortcut,
        role: match item.role {
            R::None => Role::None,
            R::Undo => Role::Undo,
            R::Redo => Role::Redo,
            R::Cut => Role::Cut,
            R::Copy => Role::Copy,
            R::Paste => Role::Paste,
            R::SelectAll => Role::SelectAll,
            R::Quit => Role::Quit,
            R::About => Role::About,
            R::Hide => Role::Hide,
            R::Minimize => Role::Minimize,
            R::CloseWindow => Role::CloseWindow,
        },
    }
}

/// Phase 3's flat `menu.set-items`: one menu of commands, called Actions.
pub(crate) fn items_from_phase3(items: Vec<ui3::types::MenuItem>) -> Vec<MenuItem> {
    // An id the app cannot have used for one of its items: the flat list's
    // ids are the app's, and the menu holding them needs one of its own.
    let holder = items
        .iter()
        .map(|i| i.id)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    let mut out = vec![MenuItem {
        kind: ItemKind::Submenu,
        ..MenuItem::command(holder, 0, "Actions")
    }];
    out.extend(items.into_iter().map(|i| MenuItem {
        enabled: i.enabled,
        ..MenuItem::command(i.id, holder, &i.label)
    }));
    out
}

fn platform(why: String) -> ui3::types::UiError {
    ui3::types::UiError::Platform(why)
}

impl Phase3GuiHost {
    /// Set a window's menu bar from a checked list.
    pub(crate) fn set_menu_bar(
        &self,
        window: u64,
        items: Vec<MenuItem>,
    ) -> Result<(), ui3::types::UiError> {
        let id = self.window_id(window)?;
        let tree = MenuTree::build(&items, &self.app_display_name()).map_err(platform)?;
        let shown = if self.headless_run() {
            Ok(())
        } else {
            self.dispatcher()
                .bluetooth()
                .set_menu_bar(id, &tree, &self.app_display_name())
        };
        // Kept even where no bar can be shown: its shortcuts still choose
        // its items, and the app is told the bar is not on screen.
        self.shell.borrow_mut().bars.insert(window, tree);
        shown.map_err(ui3::types::UiError::Unsupported)
    }

    /// The menu a right-click opens in a window; an empty list for none.
    pub(crate) fn set_context_menu(
        &self,
        window: u64,
        items: Vec<MenuItem>,
    ) -> Result<(), ui3::types::UiError> {
        let id = self.window_id(window)?;
        let tree = if items.is_empty() {
            None
        } else {
            Some(MenuTree::build(&items, &self.app_display_name()).map_err(platform)?)
        };
        if !self.headless_run() {
            self.dispatcher()
                .bluetooth()
                .set_context_menu(id, tree.as_ref(), &self.app_display_name())
                .map_err(ui3::types::UiError::Unsupported)?;
        }
        let mut shell = self.shell.borrow_mut();
        match tree {
            Some(tree) => shell.contexts.insert(window, tree),
            None => shell.contexts.remove(&window),
        };
        Ok(())
    }

    /// Open a menu at a point of a window; returns once it has closed.
    pub(crate) fn popup_menu(
        &self,
        window: u64,
        x: f32,
        y: f32,
        items: Vec<MenuItem>,
    ) -> Result<(), ui3::types::UiError> {
        let id = self.window_id(window)?;
        if !(x.is_finite() && y.is_finite()) {
            return Err(platform("the point is not a number".to_string()));
        }
        let tree = MenuTree::build(&items, &self.app_display_name()).map_err(platform)?;
        if !self.headless_run() {
            self.dispatcher()
                .bluetooth()
                .popup_menu(id, x, y, &tree, &self.app_display_name())
                .map_err(ui3::types::UiError::Unsupported)?;
        }
        self.shell.borrow_mut().last_popup = Some((window, x, y, tree));
        Ok(())
    }

    /// Show a bar again after its checks changed.
    fn refresh_bar(&self, window: u64) {
        if self.headless_run() {
            return;
        }
        let Ok(id) = self.window_id(window) else {
            return;
        };
        let tree = self.shell.borrow().bars.get(&window).cloned();
        if let Some(tree) = tree {
            let _ = self
                .dispatcher()
                .bluetooth()
                .set_menu_bar(id, &tree, &self.app_display_name());
        }
    }

    /// Turn an item of a bar on or off.
    pub(crate) fn menu_set_enabled(&self, id: u64, enabled: bool) {
        let windows: Vec<u64> = {
            let mut shell = self.shell.borrow_mut();
            shell
                .bars
                .iter_mut()
                .filter_map(|(w, tree)| tree.set_enabled(id, enabled).then_some(*w))
                .collect()
        };
        for w in windows {
            self.refresh_bar(w);
        }
        self.tray_items_changed(|tree| tree.set_enabled(id, enabled));
    }

    /// Mark or unmark an item of a bar.
    pub(crate) fn menu_set_checked(&self, id: u64, checked: bool) {
        let windows: Vec<u64> = {
            let mut shell = self.shell.borrow_mut();
            shell
                .bars
                .iter_mut()
                .filter_map(|(w, tree)| tree.set_checked(id, checked).then_some(*w))
                .collect()
        };
        for w in windows {
            self.refresh_bar(w);
        }
        self.tray_items_changed(|tree| tree.set_checked(id, checked));
    }

    /// Whether an item of a bar is marked now.
    pub(crate) fn menu_checked(&self, id: u64) -> bool {
        let shell = self.shell.borrow();
        shell
            .bars
            .values()
            .find_map(|tree| tree.find(id).map(|n| n.item.checked))
            .or_else(|| {
                shell
                    .trays
                    .values()
                    .filter_map(|t| t.menu.as_ref())
                    .find_map(|tree| tree.find(id).map(|n| n.item.checked))
            })
            .unwrap_or(false)
    }

    /// A key press that is a shortcut of the bar, on hosts whose menus do
    /// not take their own: the item's id, the choice made as a click makes
    /// it. None when the press is an ordinary key.
    pub(crate) fn menu_shortcut(&self, window: WindowId, key: &str, modifiers: Modifiers) -> bool {
        if self.headless_run() || self.dispatcher().bluetooth().menu_shortcuts_native() {
            return false;
        }
        let mac = cfg!(target_os = "macos");
        let hit = {
            let shell = self.shell.borrow();
            shell.bars.get(&window.get()).and_then(|tree| {
                tree.match_key(key, modifiers, mac)
                    .map(|n| (n.item.id, n.item.role))
            })
        };
        match hit {
            Some((_, role)) if role != Role::None && role != Role::About => {
                krate_adapter_common::menus::push_signal(MenuSignal::Role(role));
                true
            }
            Some((id, _)) if id != 0 => {
                krate_adapter_common::menus::push_signal(MenuSignal::Chosen(id));
                true
            }
            _ => false,
        }
    }

    /// Turn what the person chose in a menu into events for the app.
    pub(crate) fn drain_menu_signals(&self) {
        // A web app's page has written to the app (IC-1004).
        self.tell_waiting_views();
        for signal in krate_adapter_common::menus::take_signals() {
            match signal {
                MenuSignal::Chosen(id) => self.menu_chosen(id),
                MenuSignal::Role(role) => self.menu_role(role),
                MenuSignal::TrayClick { tray, right } => self.tray_clicked(tray, right),
            }
        }
    }

    fn menu_chosen(&self, id: u64) {
        // A bar's check or radio item moves its mark, and the bar shows it.
        let changed: Vec<u64> = {
            let mut shell = self.shell.borrow_mut();
            shell
                .bars
                .iter_mut()
                .filter_map(|(w, tree)| {
                    let kind = tree.find(id).map(|n| n.item.kind)?;
                    tree.choose(id)?;
                    matches!(kind, ItemKind::Check | ItemKind::Radio).then_some(*w)
                })
                .collect()
        };
        for w in changed {
            self.refresh_bar(w);
        }
        self.tray_items_changed(|tree| {
            let kind = tree.find(id).map(|n| n.item.kind);
            tree.choose(id).is_some() && matches!(kind, Some(ItemKind::Check | ItemKind::Radio))
        });
        self.pending_events
            .borrow_mut()
            .push_back(ui3::types::Event::Action(id));
    }

    fn menu_role(&self, role: Role) {
        match role {
            Role::Quit => self.quit_asked.set(true),
            Role::CloseWindow => {
                if let Some(window) = self.windows.first() {
                    self.pending_events
                        .borrow_mut()
                        .push_back(ui3::types::Event::CloseRequested(window.get()));
                }
            }
            role => {
                // An edit item reaches an app that draws its own editor as
                // the shortcut it already answers: Copy is Cmd-C on a Mac and
                // Ctrl-C elsewhere, and Paste brings the clipboard's text as
                // a person's own paste does (K-1109).
                let Some((key, shift)) = role.edit_key() else {
                    return;
                };
                let Some(window) = self.windows.first().copied() else {
                    return;
                };
                let mac = cfg!(target_os = "macos");
                let modifiers = Modifiers {
                    shift,
                    control: !mac,
                    alt: false,
                    meta: mac,
                };
                let dispatcher = self.dispatcher();
                let focused = dispatcher.focused_widget(window).ok().flatten();
                for pressed in [true, false] {
                    if let Ok(event) = KeyEvent::new(window, focused, key, pressed, modifiers) {
                        let _ = dispatcher.queue_key_event(event);
                    }
                    if pressed {
                        self.persons_keystroke(&dispatcher, window, focused, key, modifiers);
                    }
                }
            }
        }
    }
}

/// Tray icons (IC-1003).
impl Phase3GuiHost {
    /// Change each tray menu with `f`; a menu it changed is shown again.
    fn tray_items_changed(&self, mut f: impl FnMut(&mut MenuTree) -> bool) {
        let changed: Vec<(u64, MenuTree)> = {
            let mut shell = self.shell.borrow_mut();
            shell
                .trays
                .iter_mut()
                .filter_map(|(id, t)| {
                    let tree = t.menu.as_mut()?;
                    f(tree).then(|| (*id, tree.clone()))
                })
                .collect()
        };
        if self.headless_run() {
            return;
        }
        for (id, tree) in changed {
            let _ = self.dispatcher().bluetooth().tray_set_menu(
                id,
                Some(&tree),
                &self.app_display_name(),
            );
        }
    }

    fn tray_clicked(&self, tray: u64, right: bool) {
        let id = self
            .shell
            .borrow()
            .trays
            .get(&tray)
            .map(|t| if right { t.right } else { t.left })
            .unwrap_or(0);
        if id != 0 {
            self.pending_events
                .borrow_mut()
                .push_back(ui3::types::Event::Action(id));
        }
    }

    fn owns_tray(&self, tray: u64) -> Result<(), ui3::types::UiError> {
        if self.shell.borrow().trays.contains_key(&tray) {
            Ok(())
        } else {
            Err(ui3::types::UiError::InvalidWidget)
        }
    }

    /// Show a tray icon, behind `ui.tray`.
    pub(crate) fn tray_show(
        &self,
        icon: TrayIcon,
        tooltip: &str,
    ) -> Result<u64, ui3::types::UiError> {
        if !self.tray_allowed() {
            return Err(ui3::types::UiError::PermissionDenied);
        }
        icon.validate().map_err(platform)?;
        if self.shell.borrow().trays.len() >= krate_adapter_common::menus::MAX_TRAY_ICONS {
            return Err(platform(format!(
                "an app shows at most {} tray icons",
                krate_adapter_common::menus::MAX_TRAY_ICONS
            )));
        }
        let tooltip =
            krate_adapter_common::menus::clip(tooltip, krate_adapter_common::menus::MAX_TOOLTIP);
        let id = if self.headless_run() {
            let mut shell = self.shell.borrow_mut();
            shell.next_tray += 1;
            shell.next_tray
        } else {
            self.dispatcher()
                .bluetooth()
                .tray_show(&icon, &tooltip)
                .map_err(ui3::types::UiError::Unsupported)?
        };
        self.shell.borrow_mut().trays.insert(
            id,
            TrayState {
                tooltip,
                ..Default::default()
            },
        );
        Ok(id)
    }

    pub(crate) fn tray_set_icon(
        &self,
        tray: u64,
        icon: TrayIcon,
    ) -> Result<(), ui3::types::UiError> {
        self.owns_tray(tray)?;
        icon.validate().map_err(platform)?;
        if self.headless_run() {
            return Ok(());
        }
        self.dispatcher()
            .bluetooth()
            .tray_set_icon(tray, &icon)
            .map_err(platform)
    }

    pub(crate) fn tray_set_text(
        &self,
        tray: u64,
        text: &str,
        title: bool,
    ) -> Result<(), ui3::types::UiError> {
        self.owns_tray(tray)?;
        let max = if title {
            krate_adapter_common::menus::MAX_TRAY_TITLE
        } else {
            krate_adapter_common::menus::MAX_TOOLTIP
        };
        let text = krate_adapter_common::menus::clip(text, max);
        if let Some(t) = self.shell.borrow_mut().trays.get_mut(&tray) {
            if title {
                t.title = text.clone();
            } else {
                t.tooltip = text.clone();
            }
        }
        if self.headless_run() {
            return Ok(());
        }
        let adapter = self.dispatcher();
        let adapter = adapter.bluetooth();
        if title {
            adapter.tray_set_title(tray, &text)
        } else {
            adapter.tray_set_tooltip(tray, &text)
        }
        .map_err(platform)
    }

    pub(crate) fn tray_set_menu(
        &self,
        tray: u64,
        items: Vec<MenuItem>,
    ) -> Result<(), ui3::types::UiError> {
        self.owns_tray(tray)?;
        let tree = if items.is_empty() {
            None
        } else {
            Some(MenuTree::build(&items, &self.app_display_name()).map_err(platform)?)
        };
        if !self.headless_run() {
            self.dispatcher()
                .bluetooth()
                .tray_set_menu(tray, tree.as_ref(), &self.app_display_name())
                .map_err(platform)?;
        }
        if let Some(t) = self.shell.borrow_mut().trays.get_mut(&tray) {
            t.menu = tree;
        }
        Ok(())
    }

    pub(crate) fn tray_on_click(
        &self,
        tray: u64,
        left: u64,
        right: u64,
    ) -> Result<(), ui3::types::UiError> {
        self.owns_tray(tray)?;
        if let Some(t) = self.shell.borrow_mut().trays.get_mut(&tray) {
            t.left = left;
            t.right = right;
        }
        Ok(())
    }

    /// Take a tray icon away. With no window open and no other icon, the
    /// app ends: nothing would be left for the person to reach it by.
    pub(crate) fn tray_remove(&self, tray: u64) {
        if self.shell.borrow_mut().trays.remove(&tray).is_none() {
            return;
        }
        if !self.headless_run() {
            self.dispatcher().bluetooth().tray_remove(tray);
        }
        if self.windows.is_empty() && self.shell.borrow().trays.is_empty() {
            self.quit_asked.set(true);
        }
    }

    /// Whether this device has a tray: the three desktops.
    pub(crate) fn tray_available(&self) -> bool {
        cfg!(any(
            target_os = "macos",
            target_os = "windows",
            target_os = "linux"
        ))
    }

    /// Whether any tray icon is shown (a tray app may run with no window).
    pub(crate) fn has_tray(&self) -> bool {
        !self.shell.borrow().trays.is_empty()
    }
}

fn icon_from_wit(icon: ui4::tray::Icon) -> TrayIcon {
    TrayIcon {
        width: icon.width,
        height: icon.height,
        rgba: icon.rgba,
        template: icon.template,
    }
}

/// Tray icons (IC-1003), behind `ui.tray`.
impl ui4::tray::Host for Phase3GuiHost {
    fn available(&mut self) -> wasmtime::Result<bool> {
        Ok(self.tray_available())
    }

    fn show(
        &mut self,
        icon: ui4::tray::Icon,
        tooltip: String,
    ) -> wasmtime::Result<Result<u64, ui4::types::UiError>> {
        Ok(self
            .tray_show(icon_from_wit(icon), &tooltip)
            .map_err(error_to_phase4))
    }

    fn set_icon(
        &mut self,
        tray: u64,
        icon: ui4::tray::Icon,
    ) -> wasmtime::Result<Result<(), ui4::types::UiError>> {
        Ok(self
            .tray_set_icon(tray, icon_from_wit(icon))
            .map_err(error_to_phase4))
    }

    fn set_tooltip(
        &mut self,
        tray: u64,
        tooltip: String,
    ) -> wasmtime::Result<Result<(), ui4::types::UiError>> {
        Ok(self
            .tray_set_text(tray, &tooltip, false)
            .map_err(error_to_phase4))
    }

    fn set_title(
        &mut self,
        tray: u64,
        title: String,
    ) -> wasmtime::Result<Result<(), ui4::types::UiError>> {
        Ok(self
            .tray_set_text(tray, &title, true)
            .map_err(error_to_phase4))
    }

    fn set_menu(
        &mut self,
        tray: u64,
        items: Vec<ui4::menus::Item>,
    ) -> wasmtime::Result<Result<(), ui4::types::UiError>> {
        let items = items.into_iter().map(item_from_wit).collect();
        Ok(self.tray_set_menu(tray, items).map_err(error_to_phase4))
    }

    fn on_click(
        &mut self,
        tray: u64,
        left: u64,
        right: u64,
    ) -> wasmtime::Result<Result<(), ui4::types::UiError>> {
        Ok(self
            .tray_on_click(tray, left, right)
            .map_err(error_to_phase4))
    }

    fn remove(&mut self, tray: u64) -> wasmtime::Result<()> {
        self.tray_remove(tray);
        Ok(())
    }
}

/// Real menus (IC-1002). No permission: a menu reaches nothing but the
/// app's own commands.
impl ui4::menus::Host for Phase3GuiHost {
    fn set_bar(
        &mut self,
        window: u64,
        items: Vec<ui4::menus::Item>,
    ) -> wasmtime::Result<Result<(), ui4::types::UiError>> {
        let items = items.into_iter().map(item_from_wit).collect();
        Ok(self.set_menu_bar(window, items).map_err(error_to_phase4))
    }

    fn set_context(
        &mut self,
        window: u64,
        items: Vec<ui4::menus::Item>,
    ) -> wasmtime::Result<Result<(), ui4::types::UiError>> {
        let items = items.into_iter().map(item_from_wit).collect();
        Ok(self
            .set_context_menu(window, items)
            .map_err(error_to_phase4))
    }

    fn popup(
        &mut self,
        window: u64,
        x: f32,
        y: f32,
        items: Vec<ui4::menus::Item>,
    ) -> wasmtime::Result<Result<(), ui4::types::UiError>> {
        let items = items.into_iter().map(item_from_wit).collect();
        Ok(self
            .popup_menu(window, x, y, items)
            .map_err(error_to_phase4))
    }

    fn set_enabled(&mut self, id: u64, enabled: bool) -> wasmtime::Result<()> {
        self.menu_set_enabled(id, enabled);
        Ok(())
    }

    fn set_checked(&mut self, id: u64, checked: bool) -> wasmtime::Result<()> {
        self.menu_set_checked(id, checked);
        Ok(())
    }

    fn checked(&mut self, id: u64) -> wasmtime::Result<bool> {
        Ok(self.menu_checked(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phase3_ui::Phase3HostUiMode;
    use crate::uapi::UapiGuard;

    fn host() -> Phase3GuiHost {
        host_with(&["ui.window:create"])
    }

    fn host_with(grants: &[&str]) -> Phase3GuiHost {
        let grants: Vec<String> = grants.iter().map(|g| g.to_string()).collect();
        let policy = krate_policy::SessionPolicy::from_cli_grants(&grants).expect("policy");
        Phase3GuiHost::new(UapiGuard::new(policy), Phase3HostUiMode::HeadlessDraft)
            .expect("headless host")
    }

    fn window(host: &mut Phase3GuiHost) -> u64 {
        ui3::window::Host::create(
            host,
            "Menus".into(),
            ui3::types::WindowSize {
                width: 400,
                height: 300,
            },
        )
        .unwrap()
        .unwrap()
    }

    fn wit(id: u64, parent: u64, kind: ui4::menus::ItemKind, label: &str) -> ui4::menus::Item {
        ui4::menus::Item {
            id,
            parent,
            kind,
            label: label.into(),
            enabled: true,
            checked: false,
            shortcut: String::new(),
            role: ui4::menus::Role::None,
        }
    }

    fn next(host: &mut Phase3GuiHost) -> Option<ui4::types::Event> {
        ui4::events::Host::poll(host).unwrap()
    }

    /// A choice arrives as `action(id)`; a check item flips in the bar the
    /// runtime keeps, and `checked` says so.
    #[test]
    fn a_choice_is_an_action_and_a_check_flips() {
        use ui4::menus::ItemKind as K;
        let mut host = host();
        let w = window(&mut host);
        let items = vec![
            wit(1, 0, K::Submenu, "View"),
            wit(2, 1, K::Check, "Sidebar"),
            wit(3, 1, K::Normal, "Zoom In"),
        ];
        ui4::menus::Host::set_bar(&mut host, w, items)
            .unwrap()
            .unwrap();
        // As the drain does with a signal (the queue is the process's, and a
        // test pushing to it would race the other tests that look for input).
        host.menu_chosen(2);
        assert!(matches!(
            next(&mut host),
            Some(ui4::types::Event::Action(2))
        ));
        assert!(ui4::menus::Host::checked(&mut host, 2).unwrap());
        ui4::menus::Host::set_checked(&mut host, 2, false).unwrap();
        assert!(!ui4::menus::Host::checked(&mut host, 2).unwrap());
    }

    /// A broken list is refused with the reason, not half shown.
    #[test]
    fn a_broken_bar_is_refused_with_the_reason() {
        use ui4::menus::ItemKind as K;
        let mut host = host();
        let w = window(&mut host);
        let mut bad = wit(2, 1, K::Normal, "Save");
        bad.shortcut = "S".into();
        let answer =
            ui4::menus::Host::set_bar(&mut host, w, vec![wit(1, 0, K::Submenu, "File"), bad])
                .unwrap();
        match answer {
            Err(ui4::types::UiError::Platform(why)) => assert!(why.contains("types"), "{why}"),
            other => panic!("expected a refusal, got {other:?}"),
        }
        let answer = ui4::menus::Host::set_bar(&mut host, 999, vec![]).unwrap();
        assert!(matches!(answer, Err(ui4::types::UiError::InvalidWindow)));
    }

    /// Quit from a menu ends the app: the next look for input traps, and
    /// the run reads it as the person's own close.
    #[test]
    fn quit_ends_the_app_at_its_next_look_for_input() {
        let mut host = host();
        let _ = window(&mut host);
        host.menu_role(Role::Quit);
        let ended = ui4::events::Host::poll(&mut host).unwrap_err();
        assert!(ended.downcast_ref::<PersonQuit>().is_some(), "{ended}");
    }

    fn icon() -> ui4::tray::Icon {
        ui4::tray::Icon {
            width: 2,
            height: 2,
            rgba: vec![255; 16],
            template: true,
        }
    }

    /// A tray icon needs `ui.tray`; a bad icon is refused with its sizes.
    #[test]
    fn a_tray_icon_needs_its_grant_and_a_whole_picture() {
        let mut host = host();
        let denied = ui4::tray::Host::show(&mut host, icon(), "Timer".into()).unwrap();
        assert!(matches!(denied, Err(ui4::types::UiError::PermissionDenied)));
        let mut host = host_with(&["ui.window:create", "ui.tray"]);
        let mut bad = icon();
        bad.rgba.pop();
        let refused = ui4::tray::Host::show(&mut host, bad, "Timer".into()).unwrap();
        match refused {
            Err(ui4::types::UiError::Platform(why)) => assert!(why.contains("16 bytes"), "{why}"),
            other => panic!("{other:?}"),
        }
        assert!(ui4::tray::Host::show(&mut host, icon(), "Timer".into())
            .unwrap()
            .is_ok());
    }

    /// A click sends the id the app asked for, and a tray menu's check item
    /// flips like the bar's.
    #[test]
    fn a_tray_click_and_a_tray_menu_choice_are_actions() {
        use ui4::menus::ItemKind as K;
        let mut host = host_with(&["ui.window:create", "ui.tray"]);
        let tray = ui4::tray::Host::show(&mut host, icon(), "Timer".into())
            .unwrap()
            .unwrap();
        ui4::tray::Host::on_click(&mut host, tray, 7, 8)
            .unwrap()
            .unwrap();
        host.tray_clicked(tray, true);
        assert!(matches!(
            next(&mut host),
            Some(ui4::types::Event::Action(8))
        ));
        let items = vec![wit(5, 0, K::Check, "Paused"), wit(0, 0, K::Separator, "")];
        ui4::tray::Host::set_menu(&mut host, tray, items)
            .unwrap()
            .unwrap();
        host.menu_chosen(5);
        assert!(matches!(
            next(&mut host),
            Some(ui4::types::Event::Action(5))
        ));
        assert!(ui4::menus::Host::checked(&mut host, 5).unwrap());
        let gone = ui4::tray::Host::set_title(&mut host, tray + 99, "x".into()).unwrap();
        assert!(matches!(gone, Err(ui4::types::UiError::InvalidWidget)));
    }

    /// A headless run shows nobody an icon: closing its last window ends it.
    #[test]
    fn a_headless_tray_app_ends_with_its_last_window() {
        let mut host = host_with(&["ui.window:create", "ui.tray"]);
        let w = window(&mut host);
        let _tray = ui4::tray::Host::show(&mut host, icon(), "Timer".into())
            .unwrap()
            .unwrap();
        ui3::window::Host::close(&mut host, w).unwrap().unwrap();
        assert!(ui4::events::Host::poll(&mut host).is_err());
    }

    /// The founder's rule: a tray app may run with no window only while its
    /// icon shows. Taking the last icon away with no window open ends it; with
    /// a window open it does not.
    #[test]
    fn removing_the_last_icon_with_no_window_ends_the_app() {
        let mut host = host_with(&["ui.window:create", "ui.tray"]);
        let _w = window(&mut host);
        let tray = ui4::tray::Host::show(&mut host, icon(), "Timer".into())
            .unwrap()
            .unwrap();
        // On screen, the window goes and the icon still shows: the app runs
        // on (a headless run shows no icon, so there the close would end it).
        host.windows.clear();
        assert!(ui4::events::Host::poll(&mut host).is_ok());
        ui4::tray::Host::remove(&mut host, tray).unwrap();
        let ended = ui4::events::Host::poll(&mut host).unwrap_err();
        assert!(ended.downcast_ref::<PersonQuit>().is_some(), "{ended}");

        let mut host = host_with(&["ui.window:create", "ui.tray"]);
        let _w = window(&mut host);
        let tray = ui4::tray::Host::show(&mut host, icon(), "Timer".into())
            .unwrap()
            .unwrap();
        ui4::tray::Host::remove(&mut host, tray).unwrap();
        assert!(
            ui4::events::Host::poll(&mut host).is_ok(),
            "a window is still open"
        );
    }

    /// Phase 3's flat list becomes one menu of commands with the same ids.
    #[test]
    fn the_old_flat_menu_becomes_one_menu() {
        let items = items_from_phase3(vec![
            ui3::types::MenuItem {
                id: 4,
                label: "Refresh".into(),
                enabled: true,
            },
            ui3::types::MenuItem {
                id: 9,
                label: "Reset".into(),
                enabled: false,
            },
        ]);
        let tree = MenuTree::build(&items, "App").unwrap();
        assert_eq!(tree.roots.len(), 1);
        assert_eq!(tree.roots[0].item.label, "Actions");
        assert_eq!(tree.roots[0].item.id, 10);
        assert_eq!(tree.roots[0].children[1].item.id, 9);
        assert!(!tree.roots[0].children[1].item.enabled);
    }
}
