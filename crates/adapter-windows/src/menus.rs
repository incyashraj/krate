//! Menus on Windows (IC-1002): a real menu bar in the app's window (an
//! HMENU set on its HWND), right-click menus at the pointer
//! (WM_CONTEXTMENU), menus opened by the app (TrackPopupMenuEx), and the
//! tray icon's menu (IC-1003).
//!
//! A menu command reaches the window as WM_COMMAND, which winit does not
//! pass on, so Krate subclasses the window once and takes WM_COMMAND and
//! WM_CONTEXTMENU there. Command ids are 16 bits on Windows; each item gets
//! one from a table that says what it means -- the app's own id, or a
//! standard role. Shortcuts are written beside the label (`Save\tCtrl+S`)
//! and matched by the runtime against key presses, so they work whether or
//! not the bar is showing.

use std::cell::RefCell;
use std::collections::BTreeMap;

use krate_adapter_common::menus::{push_signal, ItemKind, MenuNode, MenuSignal, MenuTree, Role};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateMenu, CreatePopupMenu, DestroyMenu, DrawMenuBar, GetCursorPos, GetMenu,
    PostMessageW, SetForegroundWindow, SetMenu, SetMenuItemInfoW, ShowWindow, TrackPopupMenuEx,
    HMENU, MENUITEMINFOW, MENU_ITEM_FLAGS, MFS_CHECKED, MFS_DISABLED, MFT_RADIOCHECK, MFT_STRING,
    MF_CHECKED, MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING, MIIM_FTYPE, MIIM_STATE, SW_MINIMIZE,
    TPM_LEFTALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON, TPM_TOPALIGN, WM_CLOSE, WM_COMMAND,
    WM_CONTEXTMENU,
};

/// What one command id means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Chosen(u64),
    Role(Role, u64),
}

/// Who built a command, so a menu built again lets go of its old ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Owner {
    Bar(isize),
    Context(isize),
    Popup,
    Tray(u64),
}

/// The first command id Krate hands out; the system's own sit above 0xF000.
const FIRST_COMMAND: u16 = 0x1000;
const LAST_COMMAND: u16 = 0xEFFF;
const SUBCLASS_ID: usize = 0x4B52_4D4E;

thread_local! {
    static COMMANDS: RefCell<BTreeMap<u16, (Owner, Action)>> = const { RefCell::new(BTreeMap::new()) };
    static NEXT: RefCell<u16> = const { RefCell::new(FIRST_COMMAND) };
    /// Each window's right-click menu, by HWND.
    static CONTEXT: RefCell<BTreeMap<isize, MenuTree>> = const { RefCell::new(BTreeMap::new()) };
    /// Windows already subclassed.
    static HOOKED: RefCell<Vec<isize>> = const { RefCell::new(Vec::new()) };
}

fn command_for(owner: Owner, action: Action) -> u16 {
    NEXT.with(|n| {
        COMMANDS.with(|c| {
            let mut c = c.borrow_mut();
            let mut next = *n.borrow();
            // Skip ids still held by a live menu.
            for _ in 0..=(LAST_COMMAND - FIRST_COMMAND) {
                if !c.contains_key(&next) {
                    break;
                }
                next = if next >= LAST_COMMAND {
                    FIRST_COMMAND
                } else {
                    next + 1
                };
            }
            c.insert(next, (owner, action));
            *n.borrow_mut() = if next >= LAST_COMMAND {
                FIRST_COMMAND
            } else {
                next + 1
            };
            next
        })
    })
}

fn release(owner: Owner) {
    COMMANDS.with(|c| c.borrow_mut().retain(|_, (o, _)| *o != owner));
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The label as a Win32 menu shows it: `&` doubled so it is not read as a
/// mnemonic, and the shortcut after a tab.
fn label(node: &MenuNode) -> String {
    let mut text = node.item.label.replace('&', "&&");
    if let Some(shortcut) = &node.shortcut {
        // A Mac-only standard shortcut (Cmd+H, Cmd+M) means nothing here.
        if !shortcut.command || shortcut.primary {
            text.push('\t');
            text.push_str(&shortcut.display());
        }
    }
    text
}

fn fill(menu: HMENU, nodes: &[MenuNode], owner: Owner) {
    for node in nodes {
        // A Mac's Hide has no Windows meaning beyond Minimize; keep it.
        let text = wide(&label(node));
        // SAFETY: a menu Krate created, a NUL-terminated wide string that
        // outlives the call (Windows copies it).
        unsafe {
            match node.item.kind {
                ItemKind::Separator => {
                    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
                }
                ItemKind::Submenu => {
                    let Ok(sub) = CreatePopupMenu() else {
                        continue;
                    };
                    fill(sub, &node.children, owner);
                    let mut flags = MF_POPUP | MF_STRING;
                    if !node.item.enabled {
                        flags |= MF_GRAYED;
                    }
                    let _ = AppendMenuW(menu, flags, sub.0 as usize, PCWSTR(text.as_ptr()));
                }
                _ => {
                    let action = if node.item.role == Role::None {
                        Action::Chosen(node.item.id)
                    } else {
                        Action::Role(node.item.role, node.item.id)
                    };
                    let id = command_for(owner, action);
                    let mut flags: MENU_ITEM_FLAGS = MF_STRING;
                    if !node.item.enabled {
                        flags |= MF_GRAYED;
                    }
                    if node.item.checked && node.item.kind == ItemKind::Check {
                        flags |= MF_CHECKED;
                    }
                    let _ = AppendMenuW(menu, flags, id as usize, PCWSTR(text.as_ptr()));
                    if node.item.kind == ItemKind::Radio {
                        // A radio item shows a dot, not a tick.
                        let mut state = if node.item.checked {
                            MFS_CHECKED
                        } else {
                            Default::default()
                        };
                        if !node.item.enabled {
                            state |= MFS_DISABLED;
                        }
                        let info = MENUITEMINFOW {
                            cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                            fMask: MIIM_FTYPE | MIIM_STATE,
                            fType: MFT_STRING | MFT_RADIOCHECK,
                            fState: state,
                            ..Default::default()
                        };
                        let _ = SetMenuItemInfoW(menu, id as u32, false, &info);
                    }
                }
            }
        }
    }
}

/// Carry out a command the person chose in a window's menu.
fn perform(hwnd: HWND, command: u16) -> bool {
    let Some(action) = COMMANDS.with(|c| c.borrow().get(&command).map(|(_, a)| *a)) else {
        return false;
    };
    match action {
        Action::Chosen(id) => push_signal(MenuSignal::Chosen(id)),
        Action::Role(role, id) => match role {
            // SAFETY: the window this menu belongs to.
            Role::Minimize | Role::Hide => unsafe {
                let _ = ShowWindow(hwnd, SW_MINIMIZE);
            },
            // The ordinary close: winit turns it into a close request.
            Role::CloseWindow => unsafe {
                let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            },
            // Windows has no About panel of its own: the app shows its own.
            Role::About => {
                if id != 0 {
                    push_signal(MenuSignal::Chosen(id));
                }
            }
            role => push_signal(MenuSignal::Role(role)),
        },
    }
    true
}

unsafe extern "system" fn subclass(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    match msg {
        // From a menu (high word 0), not an accelerator or a control.
        WM_COMMAND
            if (wparam.0 >> 16) & 0xFFFF == 0 && perform(hwnd, (wparam.0 & 0xFFFF) as u16) =>
        {
            return LRESULT(0);
        }
        WM_CONTEXTMENU => {
            let tree = CONTEXT.with(|c| c.borrow().get(&(hwnd.0 as isize)).cloned());
            if let Some(tree) = tree {
                let (x, y) = (
                    (lparam.0 & 0xFFFF) as i16 as i32,
                    ((lparam.0 >> 16) & 0xFFFF) as i16 as i32,
                );
                let mut at = POINT { x, y };
                // From the keyboard (Shift+F10, the menu key): -1, -1.
                if x == -1 && y == -1 {
                    // SAFETY: a POINT on the stack.
                    let _ = unsafe { GetCursorPos(&mut at) };
                }
                track(hwnd, &tree, Owner::Context(hwnd.0 as isize), at);
                return LRESULT(0);
            }
        }
        _ => {}
    }
    // SAFETY: every other message goes on to winit's own procedure.
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

fn hook(hwnd: HWND) {
    let key = hwnd.0 as isize;
    if HOOKED.with(|h| h.borrow().contains(&key)) {
        return;
    }
    // SAFETY: a window this thread created (winit's), subclassed once.
    if unsafe { SetWindowSubclass(hwnd, Some(subclass), SUBCLASS_ID, 0) }.as_bool() {
        HOOKED.with(|h| h.borrow_mut().push(key));
    }
}

/// Open a menu at a point on the screen and carry out the choice.
fn track(hwnd: HWND, tree: &MenuTree, owner: Owner, at: POINT) {
    release(owner);
    // SAFETY: a popup menu built and destroyed here; the HWND is the app's.
    unsafe {
        let Ok(menu) = CreatePopupMenu() else {
            return;
        };
        fill(menu, &tree.roots, owner);
        // Without this the menu does not close when the person clicks
        // elsewhere (a documented TrackPopupMenu quirk).
        let _ = SetForegroundWindow(hwnd);
        let chosen = TrackPopupMenuEx(
            menu,
            (TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_LEFTALIGN | TPM_TOPALIGN).0,
            at.x,
            at.y,
            hwnd,
            None,
        );
        if chosen.0 != 0 {
            perform(hwnd, chosen.0 as u16);
        }
        let _ = DestroyMenu(menu);
    }
}

/// Set the window's menu bar.
pub(crate) fn set_bar(hwnd: isize, tree: &MenuTree) -> Result<(), String> {
    let hwnd = HWND(hwnd as *mut _);
    hook(hwnd);
    release(Owner::Bar(hwnd.0 as isize));
    // SAFETY: a menu bar built here, swapped in, the old one destroyed.
    unsafe {
        let bar = CreateMenu().map_err(|e| format!("Windows made no menu: {e}"))?;
        fill(bar, &tree.roots, Owner::Bar(hwnd.0 as isize));
        let old = GetMenu(hwnd);
        SetMenu(hwnd, Some(bar)).map_err(|e| format!("Windows took no menu bar: {e}"))?;
        if !old.is_invalid() {
            let _ = DestroyMenu(old);
        }
        let _ = DrawMenuBar(hwnd);
    }
    Ok(())
}

/// Set (or take away) the window's right-click menu.
pub(crate) fn set_context(hwnd: isize, tree: Option<&MenuTree>) {
    hook(HWND(hwnd as *mut _));
    CONTEXT.with(|c| {
        let mut c = c.borrow_mut();
        match tree {
            Some(t) => c.insert(hwnd, t.clone()),
            None => c.remove(&hwnd),
        };
    });
}

/// Open a menu at `(x, y)` points from the top-left of the window's client
/// area; `scale` is its pixels per point.
pub(crate) fn popup(hwnd: isize, x: f32, y: f32, scale: f32, tree: &MenuTree) {
    let hwnd = HWND(hwnd as *mut _);
    let mut at = POINT {
        x: (x * scale).round() as i32,
        y: (y * scale).round() as i32,
    };
    // SAFETY: the app's window and a POINT on the stack.
    let _ = unsafe { ClientToScreen(hwnd, &mut at) };
    track(hwnd, tree, Owner::Popup, at);
}

#[allow(dead_code)] // the tray (IC-1003)
/// Open a tray icon's menu at the cursor, owned by `hwnd` (the tray's
/// hidden window).
pub(crate) fn tray_popup(hwnd: isize, tray: u64, tree: &MenuTree) {
    let mut at = POINT::default();
    // SAFETY: a POINT on the stack.
    let _ = unsafe { GetCursorPos(&mut at) };
    track(HWND(hwnd as *mut _), tree, Owner::Tray(tray), at);
}

#[allow(dead_code)] // the tray (IC-1003)
/// Carry out a command chosen in a menu owned by a window Krate did not
/// subclass (the tray's own window).
pub(crate) fn perform_command(hwnd: isize, command: u16) -> bool {
    perform(HWND(hwnd as *mut _), command)
}
