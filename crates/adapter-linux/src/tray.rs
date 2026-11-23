//! A tray icon on Linux (IC-1003): a StatusNotifierItem over D-Bus, the
//! freedesktop tray KDE, XFCE, Cinnamon, Budgie and GNOME (with its
//! AppIndicator extension) show. ksni speaks the protocol in pure Rust on a
//! thread of its own, so nothing is linked from the system; a desktop with
//! no StatusNotifier host answers `show` with words, not a crash.
//!
//! The icon's menu is the same tree a menu bar uses (krate_adapter_common::
//! menus); a choice and a click go to the runtime as signals from ksni's
//! thread.

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use krate_adapter_common::menus::{
    push_signal, ItemKind, MenuNode, MenuSignal, MenuTree, Role, TrayIcon,
};
use ksni::blocking::{Handle, TrayMethods};
use ksni::menu::{CheckmarkItem, MenuItem, RadioGroup, RadioItem, StandardItem, SubMenu};

/// One icon as ksni asks about it.
pub(crate) struct KrateTray {
    id: u64,
    app: String,
    title: String,
    tooltip: String,
    icon: Vec<ksni::Icon>,
    menu: Option<MenuTree>,
}

/// The app's pixels as StatusNotifier wants them: ARGB32, network order.
fn argb(icon: &TrayIcon) -> ksni::Icon {
    let mut data = Vec::with_capacity(icon.rgba.len());
    for px in icon.rgba.chunks_exact(4) {
        data.extend_from_slice(&[px[3], px[0], px[1], px[2]]);
    }
    ksni::Icon {
        width: icon.width as i32,
        height: icon.height as i32,
        data,
    }
}

/// What choosing an item does: the app's id, or a standard role.
fn signal_for(node: &MenuNode) -> Option<MenuSignal> {
    match node.item.role {
        Role::None => Some(MenuSignal::Chosen(node.item.id)),
        // No system About panel: the app shows its own.
        Role::About => (node.item.id != 0).then_some(MenuSignal::Chosen(node.item.id)),
        role => Some(MenuSignal::Role(role)),
    }
}

/// A label as a StatusNotifier menu shows it: an underscore is a
/// mnemonic there, so a literal one is doubled.
fn label(text: &str) -> String {
    text.replace('_', "__")
}

fn items(nodes: &[MenuNode]) -> Vec<MenuItem<KrateTray>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < nodes.len() {
        let node = &nodes[i];
        match node.item.kind {
            ItemKind::Separator => out.push(MenuItem::Separator),
            ItemKind::Submenu => out.push(MenuItem::SubMenu(SubMenu {
                label: label(&node.item.label),
                enabled: node.item.enabled,
                submenu: items(&node.children),
                ..Default::default()
            })),
            ItemKind::Check => {
                let signal = signal_for(node);
                out.push(MenuItem::Checkmark(CheckmarkItem {
                    label: label(&node.item.label),
                    enabled: node.item.enabled,
                    checked: node.item.checked,
                    activate: Box::new(move |_| {
                        if let Some(s) = signal {
                            push_signal(s);
                        }
                    }),
                    ..Default::default()
                }));
            }
            ItemKind::Radio => {
                // Next-door radio items are one group.
                let start = i;
                while i + 1 < nodes.len() && nodes[i + 1].item.kind == ItemKind::Radio {
                    i += 1;
                }
                let group = &nodes[start..=i];
                let ids: Vec<u64> = group.iter().map(|n| n.item.id).collect();
                out.push(MenuItem::RadioGroup(RadioGroup {
                    selected: group.iter().position(|n| n.item.checked).unwrap_or(0),
                    select: Box::new(move |_, index| {
                        if let Some(id) = ids.get(index) {
                            push_signal(MenuSignal::Chosen(*id));
                        }
                    }),
                    options: group
                        .iter()
                        .map(|n| RadioItem {
                            label: label(&n.item.label),
                            enabled: n.item.enabled,
                            ..Default::default()
                        })
                        .collect(),
                }));
            }
            ItemKind::Normal => {
                let signal = signal_for(node);
                out.push(MenuItem::Standard(StandardItem {
                    label: label(&node.item.label),
                    enabled: node.item.enabled,
                    activate: Box::new(move |_| {
                        if let Some(s) = signal {
                            push_signal(s);
                        }
                    }),
                    ..Default::default()
                }));
            }
        }
        i += 1;
    }
    out
}

impl ksni::Tray for KrateTray {
    fn id(&self) -> String {
        format!("krate-{}-{}", std::process::id(), self.id)
    }

    fn title(&self) -> String {
        if self.title.is_empty() {
            self.app.clone()
        } else {
            format!("{} {}", self.app, self.title)
        }
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.icon.clone()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: self.tooltip.clone(),
            ..Default::default()
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        push_signal(MenuSignal::TrayClick {
            tray: self.id,
            right: false,
        });
    }

    fn secondary_activate(&mut self, _x: i32, _y: i32) {
        push_signal(MenuSignal::TrayClick {
            tray: self.id,
            right: true,
        });
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        self.menu
            .as_ref()
            .map(|t| items(&t.roots))
            .unwrap_or_default()
    }
}

fn handles() -> &'static Mutex<BTreeMap<u64, Handle<KrateTray>>> {
    static HANDLES: OnceLock<Mutex<BTreeMap<u64, Handle<KrateTray>>>> = OnceLock::new();
    HANDLES.get_or_init(Default::default)
}

static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Show an icon; its id, or why the desktop cannot show one.
pub(crate) fn show(icon: &TrayIcon, tooltip: &str, app: &str) -> Result<u64, String> {
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tray = KrateTray {
        id,
        app: app.to_string(),
        title: String::new(),
        tooltip: tooltip.to_string(),
        icon: vec![argb(icon)],
        menu: None,
    };
    let handle = tray.spawn().map_err(|e| {
        format!(
            "this desktop shows no tray icons (no StatusNotifier host on the session bus: {e}); \
             on GNOME, the AppIndicator extension adds one"
        )
    })?;
    handles()
        .lock()
        .map_err(|_| "the tray is unavailable".to_string())?
        .insert(id, handle);
    Ok(id)
}

fn update(tray: u64, f: impl FnOnce(&mut KrateTray)) -> Result<(), String> {
    let handles = handles()
        .lock()
        .map_err(|_| "the tray is unavailable".to_string())?;
    let handle = handles.get(&tray).ok_or("no such tray icon")?;
    handle
        .update(f)
        .ok_or_else(|| "the tray icon has gone".to_string())
}

pub(crate) fn set_icon(tray: u64, icon: &TrayIcon) -> Result<(), String> {
    let icon = argb(icon);
    update(tray, |t| t.icon = vec![icon])
}

pub(crate) fn set_tooltip(tray: u64, tooltip: &str) -> Result<(), String> {
    let tooltip = tooltip.to_string();
    update(tray, |t| t.tooltip = tooltip)
}

pub(crate) fn set_title(tray: u64, title: &str) -> Result<(), String> {
    let title = title.to_string();
    update(tray, |t| t.title = title)
}

pub(crate) fn set_menu(tray: u64, menu: Option<&MenuTree>) -> Result<(), String> {
    let menu = menu.cloned();
    update(tray, |t| t.menu = menu)
}

pub(crate) fn remove(tray: u64) {
    let handle = handles().lock().ok().and_then(|mut h| h.remove(&tray));
    if let Some(handle) = handle {
        // Takes the icon off the bus; the awaiter is not needed.
        drop(handle.shutdown());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use krate_adapter_common::menus::MenuItem as Item;

    #[test]
    fn pixels_become_argb_in_network_order() {
        let icon = TrayIcon {
            width: 1,
            height: 1,
            rgba: vec![10, 20, 30, 40],
            template: false,
        };
        assert_eq!(argb(&icon).data, vec![40, 10, 20, 30]);
    }

    #[test]
    fn next_door_radio_items_become_one_group() {
        let radio = |id, checked| Item {
            kind: ItemKind::Radio,
            checked,
            ..Item::command(id, 0, "r")
        };
        let tree = MenuTree::build(
            &[
                radio(1, false),
                radio(2, true),
                Item {
                    kind: ItemKind::Separator,
                    ..Item::command(0, 0, "")
                },
                Item::command(3, 0, "do_it"),
            ],
            "App",
        )
        .unwrap();
        let built = items(&tree.roots);
        assert_eq!(built.len(), 3);
        match &built[0] {
            MenuItem::RadioGroup(g) => {
                assert_eq!(g.selected, 1);
                assert_eq!(g.options.len(), 2);
            }
            other => panic!("{other:?}"),
        }
        match &built[2] {
            MenuItem::Standard(s) => assert_eq!(s.label, "do__it"),
            other => panic!("{other:?}"),
        }
    }
}
