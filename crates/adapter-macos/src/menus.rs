//! Menus on a Mac (IC-1002): the app's own menus in the real menu bar, with
//! the standard App, Edit and Window menus kept; right-click menus at the
//! pointer; and the menus tray icons open (IC-1003).
//!
//! Every item Krate builds targets one `KrateMenuTarget`. Its tag looks up
//! what the item means in [`ACTIONS`]: the app's id, which goes to the
//! runtime as a signal, or a standard role, which does the Mac's own thing
//! -- an edit command goes down the responder chain when a native text field
//! is being edited and to the app as its shortcut when the app draws its own
//! editor, About opens the system panel, Hide hides, Quit ends the app.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

use krate_adapter_common::menus::{
    push_signal, ItemKind, MenuNode, MenuSignal, MenuTree, Role, Shortcut,
};
use krate_adapter_common::ui::Modifiers;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{define_class, msg_send, sel, ClassType, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSEventModifierFlags, NSMenu, NSMenuItem, NSText, NSView, NSWindow,
};
use objc2_foundation::{NSObject, NSObjectProtocol, NSPoint, NSRect, NSString};

/// What one item does when chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    /// The app's item: its id goes to the runtime.
    Chosen(u64),
    /// A standard item.
    Role(Role),
}

/// Who built an item, so a menu built again lets go of its old tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Owner {
    Bar,
    Popup,
    Context(u64),
    #[allow(dead_code)] // the tray's menus (IC-1003)
    Tray(u64),
}

thread_local! {
    static ACTIONS: RefCell<BTreeMap<isize, (Owner, Action)>> = const { RefCell::new(BTreeMap::new()) };
    static NEXT_TAG: Cell<isize> = const { Cell::new(1) };
    static TARGET: RefCell<Option<Retained<KrateMenuTarget>>> = const { RefCell::new(None) };
    /// The shortcuts of the bar the app set, so a keystroke the app's own
    /// pump would keep as a game key reaches the menu instead.
    static BAR_SHORTCUTS: RefCell<Vec<Shortcut>> = const { RefCell::new(Vec::new()) };
    /// Each window's right-click menu, by its NSWindow pointer.
    static CONTEXT: RefCell<BTreeMap<u64, Retained<NSMenu>>> = const { RefCell::new(BTreeMap::new()) };
    /// The app's name for the standard items, once the CLI knows it.
    static APP_NAME: RefCell<String> = const { RefCell::new(String::new()) };
}

define_class!(
    // SAFETY:
    // - NSObject has no subclassing requirements for a target-action receiver.
    // - AppKit sends menu actions on the main thread.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    struct KrateMenuTarget;

    // SAFETY: NSObjectProtocol has no additional requirements.
    unsafe impl NSObjectProtocol for KrateMenuTarget {}

    impl KrateMenuTarget {
        // SAFETY: `-(void)action:(id)sender`, the target-action convention;
        // only NSMenuItems Krate built carry this selector.
        #[unsafe(method(krateMenuChosen:))]
        fn chosen(&self, sender: &NSMenuItem) {
            let tag = sender.tag();
            let action = ACTIONS.with(|a| a.borrow().get(&tag).map(|(_, act)| *act));
            match action {
                Some(Action::Chosen(id)) => push_signal(MenuSignal::Chosen(id)),
                Some(Action::Role(role)) => perform_role(role, sender),
                None => {}
            }
        }
    }
);

fn target(mtm: MainThreadMarker) -> Retained<KrateMenuTarget> {
    TARGET.with(|t| {
        t.borrow_mut()
            .get_or_insert_with(|| {
                let this = KrateMenuTarget::alloc(mtm).set_ivars(());
                // SAFETY: NSObject's init on a fresh allocation.
                unsafe { msg_send![super(this), init] }
            })
            .clone()
    })
}

/// Name the app in the standard items ("About Notes", "Quit Notes").
pub(crate) fn set_app_name(name: &str) {
    APP_NAME.with(|n| *n.borrow_mut() = name.to_string());
}

fn app_name(fallback: &str) -> String {
    let name = APP_NAME.with(|n| n.borrow().clone());
    if !name.is_empty() {
        return name;
    }
    if !fallback.is_empty() {
        return fallback.to_string();
    }
    // SAFETY: NSProcessInfo's processName.
    unsafe {
        let info: Option<Retained<AnyObject>> =
            objc2::runtime::AnyClass::get(c"NSProcessInfo").and_then(|c| msg_send![c, processInfo]);
        info.and_then(|i| {
            let n: Option<Retained<NSString>> = msg_send![&*i, processName];
            n.map(|n| n.to_string())
        })
        .unwrap_or_default()
    }
}

/// Do what a standard item does on a Mac.
fn perform_role(role: Role, sender: &NSMenuItem) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    let send = |selector: Sel| {
        // SAFETY: a standard NSResponder/NSApplication action, sent down
        // the responder chain from the key window.
        unsafe {
            let _: bool = msg_send![&*app, sendAction: selector, to: Option::<&AnyObject>::None, from: sender];
        }
    };
    if role.is_edit() {
        // A native text field being edited takes the command itself; an
        // app that draws its own editor gets it as its shortcut.
        let editing = app
            .keyWindow()
            .and_then(|w| w.firstResponder())
            .is_some_and(|r| r.isKindOfClass(NSText::class()));
        if editing {
            let selector = match role {
                Role::Undo => sel!(undo:),
                Role::Redo => sel!(redo:),
                Role::Cut => sel!(cut:),
                Role::Copy => sel!(copy:),
                Role::Paste => sel!(paste:),
                _ => sel!(selectAll:),
            };
            send(selector);
        } else {
            push_signal(MenuSignal::Role(role));
        }
        return;
    }
    match role {
        Role::About => send(sel!(orderFrontStandardAboutPanel:)),
        Role::Hide => send(sel!(hide:)),
        Role::Minimize => send(sel!(performMiniaturize:)),
        Role::CloseWindow => send(sel!(performClose:)),
        // Quit ends the app at its next safepoint, even one busy in its own
        // code: the runtime closes its windows and the process exits.
        Role::Quit => {
            krate_adapter_common::ui::end_running_app();
            push_signal(MenuSignal::Role(Role::Quit));
        }
        _ => {}
    }
}

fn tag_for(owner: Owner, action: Action) -> isize {
    let tag = NEXT_TAG.with(|n| {
        let tag = n.get();
        n.set(if tag >= isize::MAX - 1 { 1 } else { tag + 1 });
        tag
    });
    ACTIONS.with(|a| a.borrow_mut().insert(tag, (owner, action)));
    tag
}

fn release_tags(owner: Owner) {
    ACTIONS.with(|a| a.borrow_mut().retain(|_, (o, _)| *o != owner));
}

fn modifier_mask(shortcut: &Shortcut) -> NSEventModifierFlags {
    let m = shortcut.resolved(true);
    let mut flags = NSEventModifierFlags::empty();
    if m.meta {
        flags |= NSEventModifierFlags::Command;
    }
    if m.control {
        flags |= NSEventModifierFlags::Control;
    }
    if m.alt {
        flags |= NSEventModifierFlags::Option;
    }
    if m.shift {
        flags |= NSEventModifierFlags::Shift;
    }
    flags
}

/// One NSMenuItem for a node, its submenu built under it.
fn build_item(
    node: &MenuNode,
    owner: Owner,
    target: &KrateMenuTarget,
    mtm: MainThreadMarker,
) -> Retained<NSMenuItem> {
    if node.item.kind == ItemKind::Separator {
        return NSMenuItem::separatorItem(mtm);
    }
    let title = NSString::from_str(&node.item.label);
    let key = node
        .shortcut
        .as_ref()
        .map(Shortcut::mac_key_equivalent)
        .unwrap_or_default();
    // SAFETY: a fresh NSMenuItem with Krate's selector; the target outlives
    // it (held in TARGET for the life of the process).
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &title,
            (node.item.kind != ItemKind::Submenu).then_some(sel!(krateMenuChosen:)),
            &NSString::from_str(&key),
        )
    };
    if let Some(shortcut) = &node.shortcut {
        item.setKeyEquivalentModifierMask(modifier_mask(shortcut));
    }
    item.setEnabled(node.item.enabled);
    if matches!(node.item.kind, ItemKind::Check | ItemKind::Radio) {
        item.setState(if node.item.checked { 1 } else { 0 });
    }
    if node.item.kind == ItemKind::Submenu {
        let menu = build_menu(&node.item.label, &node.children, owner, target, mtm);
        item.setSubmenu(Some(&menu));
    } else {
        let action = if node.item.role == Role::None {
            Action::Chosen(node.item.id)
        } else {
            Action::Role(node.item.role)
        };
        item.setTag(tag_for(owner, action));
        // SAFETY: the target is an NSObject that answers krateMenuChosen:.
        unsafe { item.setTarget(Some(target)) };
    }
    item
}

fn build_menu(
    title: &str,
    nodes: &[MenuNode],
    owner: Owner,
    target: &KrateMenuTarget,
    mtm: MainThreadMarker,
) -> Retained<NSMenu> {
    let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(title));
    // Krate says what is enabled; AppKit's own guessing would grey every
    // item whose target it cannot see through.
    menu.setAutoenablesItems(false);
    for node in nodes {
        menu.addItem(&build_item(node, owner, target, mtm));
    }
    menu
}

/// A standard item that does the Mac's own thing.
fn standard(
    title: &str,
    role: Option<Role>,
    selector: Sel,
    key: &str,
    mask: NSEventModifierFlags,
    target: &KrateMenuTarget,
    mtm: MainThreadMarker,
) -> Retained<NSMenuItem> {
    // SAFETY: a fresh NSMenuItem; nil target sends the selector down the
    // responder chain, Krate's target takes the role items.
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            Some(if role.is_some() {
                sel!(krateMenuChosen:)
            } else {
                selector
            }),
            &NSString::from_str(key),
        )
    };
    if !key.is_empty() {
        item.setKeyEquivalentModifierMask(mask);
    }
    if let Some(role) = role {
        item.setTag(tag_for(Owner::Bar, Action::Role(role)));
        // SAFETY: as in build_item.
        unsafe { item.setTarget(Some(target)) };
    }
    item
}

fn submenu_item(menu: &NSMenu, mtm: MainThreadMarker) -> Retained<NSMenuItem> {
    let item = NSMenuItem::new(mtm);
    item.setTitle(&menu.title());
    item.setSubmenu(Some(menu));
    item
}

/// The app's menu bar on a Mac: the App menu first, then the app's own
/// menus with the standard Edit menu after the first unless the app gives
/// its own, then the Window menu. `bar` None is the bar of an app that set
/// none.
pub(crate) fn install_bar(bar: Option<&MenuTree>, fallback_name: &str) -> Result<(), String> {
    let mtm = MainThreadMarker::new().ok_or("menus are built on the main thread")?;
    let app = NSApplication::sharedApplication(mtm);
    let target = target(mtm);
    let name = app_name(fallback_name);
    release_tags(Owner::Bar);
    let cmd = NSEventModifierFlags::Command;
    let main = NSMenu::new(mtm);

    // The App menu: AppKit shows the app's name in bold whatever its title.
    let app_menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(&name));
    app_menu.setAutoenablesItems(false);
    let about = format!("About {name}");
    app_menu.addItem(&standard(
        &about,
        Some(Role::About),
        sel!(krateMenuChosen:),
        "",
        cmd,
        &target,
        mtm,
    ));
    app_menu.addItem(&NSMenuItem::separatorItem(mtm));
    let hide = format!("Hide {name}");
    app_menu.addItem(&standard(
        &hide,
        Some(Role::Hide),
        sel!(hide:),
        "h",
        cmd,
        &target,
        mtm,
    ));
    let others = standard(
        "Hide Others",
        None,
        sel!(hideOtherApplications:),
        "h",
        cmd | NSEventModifierFlags::Option,
        &target,
        mtm,
    );
    app_menu.addItem(&others);
    app_menu.addItem(&standard(
        "Show All",
        None,
        sel!(unhideAllApplications:),
        "",
        cmd,
        &target,
        mtm,
    ));
    app_menu.addItem(&NSMenuItem::separatorItem(mtm));
    let quit = format!("Quit {name}");
    // The standard Quit is the system's terminate: -- it works even when the
    // app has stopped answering. An app's own Quit item (a role) ends it
    // through the runtime.
    app_menu.addItem(&standard(
        &quit,
        None,
        sel!(terminate:),
        "q",
        cmd,
        &target,
        mtm,
    ));
    main.addItem(&submenu_item(&app_menu, mtm));

    // The Edit menu, unless the app gives one of its own.
    let own: Vec<&MenuNode> = bar
        .map(|b| {
            b.roots
                .iter()
                .filter(|n| n.item.kind == ItemKind::Submenu)
                .collect()
        })
        .unwrap_or_default();
    let has = |label: &str| own.iter().any(|n| n.item.label.eq_ignore_ascii_case(label));
    let edit = (!has("Edit")).then(|| standard_edit_menu(&target, mtm));
    if own.is_empty() {
        if let Some(edit) = &edit {
            main.addItem(&submenu_item(edit, mtm));
        }
    }
    // The Window menu, unless the app gives its own; AppKit lists the
    // windows in it.
    let mut window_menu = (!has("Window")).then(|| {
        let window = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str("Window"));
        window.setAutoenablesItems(false);
        window.addItem(&standard(
            "Minimize",
            Some(Role::Minimize),
            sel!(performMiniaturize:),
            "m",
            cmd,
            &target,
            mtm,
        ));
        window.addItem(&standard(
            "Zoom",
            None,
            sel!(performZoom:),
            "",
            cmd,
            &target,
            mtm,
        ));
        window.addItem(&standard(
            "Close Window",
            Some(Role::CloseWindow),
            sel!(performClose:),
            "w",
            cmd,
            &target,
            mtm,
        ));
        window.addItem(&NSMenuItem::separatorItem(mtm));
        window.addItem(&standard(
            "Bring All to Front",
            None,
            sel!(arrangeInFront:),
            "",
            cmd,
            &target,
            mtm,
        ));
        app.setWindowsMenu(Some(&window));
        window
    });
    for (at, node) in own.iter().enumerate() {
        let children = mac_children(&node.children);
        if children.iter().all(|n| n.item.kind == ItemKind::Separator) {
            continue;
        }
        // Window comes before Help, as on every Mac.
        if node.item.label.eq_ignore_ascii_case("Help") {
            if let Some(window) = window_menu.take() {
                main.addItem(&submenu_item(&window, mtm));
            }
        }
        let menu = build_menu(&node.item.label, &children, Owner::Bar, &target, mtm);
        main.addItem(&submenu_item(&menu, mtm));
        if at == 0 {
            if let Some(edit) = &edit {
                main.addItem(&submenu_item(edit, mtm));
            }
        }
    }

    if let Some(window) = window_menu.take() {
        main.addItem(&submenu_item(&window, mtm));
    }
    app.setMainMenu(Some(&main));

    let shortcuts = bar
        .map(|b| {
            b.walk()
                .into_iter()
                .filter_map(|n| n.shortcut.clone())
                .collect()
        })
        .unwrap_or_default();
    BAR_SHORTCUTS.with(|s| *s.borrow_mut() = shortcuts);
    Ok(())
}

/// A Mac keeps About, Hide and Quit in the App menu, so the app's own copies
/// of them leave its other menus; separators left at an edge or doubled go
/// with them.
fn mac_children(nodes: &[MenuNode]) -> Vec<MenuNode> {
    let mut out: Vec<MenuNode> = Vec::new();
    for n in nodes {
        if matches!(n.item.role, Role::About | Role::Hide | Role::Quit) {
            continue;
        }
        let sep = n.item.kind == ItemKind::Separator;
        if sep
            && out
                .last()
                .is_none_or(|l| l.item.kind == ItemKind::Separator)
        {
            continue;
        }
        let mut n = n.clone();
        n.children = mac_children(&n.children);
        out.push(n);
    }
    while out
        .last()
        .is_some_and(|l| l.item.kind == ItemKind::Separator)
    {
        out.pop();
    }
    out
}

fn standard_edit_menu(target: &KrateMenuTarget, mtm: MainThreadMarker) -> Retained<NSMenu> {
    let cmd = NSEventModifierFlags::Command;
    let edit = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str("Edit"));
    edit.setAutoenablesItems(false);
    edit.addItem(&standard(
        "Undo",
        Some(Role::Undo),
        sel!(undo:),
        "z",
        cmd,
        target,
        mtm,
    ));
    edit.addItem(&standard(
        "Redo",
        Some(Role::Redo),
        sel!(redo:),
        "z",
        cmd | NSEventModifierFlags::Shift,
        target,
        mtm,
    ));
    edit.addItem(&NSMenuItem::separatorItem(mtm));
    // Where the organisation blocks the clipboard for this app, a native
    // text field may not reach it on its own either (G17).
    let (read_blocked, write_blocked) = krate_adapter_common::ui::clipboard_kept();
    if !write_blocked {
        edit.addItem(&standard(
            "Cut",
            Some(Role::Cut),
            sel!(cut:),
            "x",
            cmd,
            target,
            mtm,
        ));
        edit.addItem(&standard(
            "Copy",
            Some(Role::Copy),
            sel!(copy:),
            "c",
            cmd,
            target,
            mtm,
        ));
    }
    if !read_blocked {
        edit.addItem(&standard(
            "Paste",
            Some(Role::Paste),
            sel!(paste:),
            "v",
            cmd,
            target,
            mtm,
        ));
    }
    edit.addItem(&standard(
        "Select All",
        Some(Role::SelectAll),
        sel!(selectAll:),
        "a",
        cmd,
        target,
        mtm,
    ));
    edit
}

/// Whether a key press is a shortcut of the app's bar: the pump then lets
/// the menu take it instead of keeping it as a key for the app.
pub(crate) fn bar_has_shortcut(key: &str, modifiers: Modifiers) -> bool {
    BAR_SHORTCUTS.with(|s| s.borrow().iter().any(|s| s.matches(key, modifiers, true)))
}

/// Hand a key press to the menu bar; true when an item took it.
pub(crate) fn perform_key_equivalent(event: &objc2_app_kit::NSEvent) -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    NSApplication::sharedApplication(mtm)
        .mainMenu()
        .is_some_and(|menu| menu.performKeyEquivalent(event))
}

/// A menu for a tray icon (IC-1003).
#[allow(dead_code)]
pub(crate) fn tray_menu(tray: u64, tree: &MenuTree) -> Option<Retained<NSMenu>> {
    let mtm = MainThreadMarker::new()?;
    release_tags(Owner::Tray(tray));
    let target = target(mtm);
    Some(build_menu("", &tree.roots, Owner::Tray(tray), &target, mtm))
}

/// Let go of a tray icon's menu items.
#[allow(dead_code)]
pub(crate) fn forget_tray(tray: u64) {
    release_tags(Owner::Tray(tray));
}

/// The menu a right-click (or a Control-click) opens anywhere in the
/// window whose NSWindow is `ns_window`: AppKit's own contextual menu,
/// set on the content view and every view inside it, so it opens over the
/// app's drawing as well. `window` keys the tags; None takes it away.
pub(crate) fn set_context(
    ns_window: u64,
    window: u64,
    tree: Option<&MenuTree>,
) -> Result<(), String> {
    let mtm = MainThreadMarker::new().ok_or("menus are built on the main thread")?;
    // SAFETY: the NSWindow the adapter made for this app, retained for the
    // call.
    let ns: Retained<NSWindow> = unsafe { Retained::retain(ns_window as usize as *mut NSWindow) }
        .ok_or("the app's window is gone")?;
    let view: Retained<NSView> = ns.contentView().ok_or("the window has no content")?;
    release_tags(Owner::Context(window));
    let menu = tree.map(|t| {
        let target = target(mtm);
        build_menu("", &t.roots, Owner::Context(window), &target, mtm)
    });
    CONTEXT.with(|c| {
        let mut c = c.borrow_mut();
        match &menu {
            Some(m) => c.insert(ns_window, m.clone()),
            None => c.remove(&ns_window),
        }
    });
    apply_context(&view, menu.as_deref());
    Ok(())
}

fn apply_context(view: &NSView, menu: Option<&NSMenu>) {
    // SAFETY: an NSView's contextual menu, on the main thread; the menu is
    // retained by the view and by CONTEXT.
    unsafe { view.setMenu(menu) };
    for sub in view.subviews().iter() {
        apply_context(&sub, menu);
    }
}

/// Views added after the menu was set -- the canvas a window draws on comes
/// with its first frame -- take the window's menu too. Cheap: called by the
/// pump on a right-click, before AppKit asks the view for its menu.
pub(crate) fn refresh_context(ns_window: &NSWindow) {
    let key = ns_window as *const NSWindow as usize as u64;
    let Some(menu) = CONTEXT.with(|c| c.borrow().get(&key).cloned()) else {
        return;
    };
    if let Some(view) = ns_window.contentView() {
        apply_context(&view, Some(&menu));
    }
}

/// Open a menu at `(x, y)` points from the top-left of the window whose
/// NSWindow is `ns_window`; returns when the person has chosen or dismissed
/// it.
pub(crate) fn popup(ns_window: u64, x: f32, y: f32, tree: &MenuTree) -> Result<(), String> {
    let mtm = MainThreadMarker::new().ok_or("menus open on the main thread")?;
    // SAFETY: the NSWindow the adapter made for this app (its pointer is the
    // native handle), retained for the call.
    let window: Retained<NSWindow> =
        unsafe { Retained::retain(ns_window as usize as *mut NSWindow) }
            .ok_or("the app's window is gone")?;
    let view: Retained<NSView> = window.contentView().ok_or("the window has no content")?;
    let bounds: NSRect = view.bounds();
    let point = if view.isFlipped() {
        NSPoint::new(x as f64, y as f64)
    } else {
        NSPoint::new(x as f64, bounds.size.height - y as f64)
    };
    release_tags(Owner::Popup);
    let target = target(mtm);
    let menu = build_menu("", &tree.roots, Owner::Popup, &target, mtm);
    menu.popUpMenuPositioningItem_atLocation_inView(None, point, Some(&view));
    Ok(())
}
