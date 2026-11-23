//! A tray icon on Windows (IC-1003): Shell_NotifyIconW in the notification
//! area, owned by a hidden message-only window of Krate's so its clicks
//! arrive even when the app has no window open. A left click sends the
//! app's left action; a right click opens the icon's menu (built by
//! menus.rs, the same items as a menu bar) or sends the right action.

use std::cell::RefCell;
use std::collections::BTreeMap;

use krate_adapter_common::menus::{push_signal, MenuSignal, MenuTree, TrayIcon};
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateBitmap, CreateDIBSection, DeleteObject, GetDC, ReleaseDC, BITMAPINFO, BITMAPINFOHEADER,
    BI_RGB, DIB_RGB_COLORS, HGDIOBJ,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NIM_SETVERSION, NOTIFYICONDATAW, NOTIFYICON_VERSION_4,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconIndirect, CreateWindowExW, DefWindowProcW, DestroyIcon, DispatchMessageW,
    PeekMessageW, RegisterClassW, TranslateMessage, HICON, HWND_MESSAGE, ICONINFO, MSG, PM_REMOVE,
    WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_CONTEXTMENU, WM_LBUTTONUP, WM_RBUTTONUP, WNDCLASSW,
};

/// The message Windows sends the owner window about the icon.
const TRAY_MESSAGE: u32 = WM_APP + 0x4B;

struct Icon {
    handle: HICON,
    menu: Option<MenuTree>,
}

thread_local! {
    static OWNER: RefCell<Option<isize>> = const { RefCell::new(None) };
    static ICONS: RefCell<BTreeMap<u64, Icon>> = const { RefCell::new(BTreeMap::new()) };
    static NEXT: RefCell<u64> = const { RefCell::new(1) };
}

unsafe extern "system" fn owner_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == TRAY_MESSAGE {
        // NOTIFYICON_VERSION_4: the event in the low word of lparam, the
        // icon's id in its high word.
        let event = (lparam.0 & 0xFFFF) as u32;
        let tray = ((lparam.0 >> 16) & 0xFFFF) as u64;
        match event {
            WM_LBUTTONUP => push_signal(MenuSignal::TrayClick { tray, right: false }),
            WM_RBUTTONUP | WM_CONTEXTMENU => {
                let menu = ICONS.with(|i| i.borrow().get(&tray).and_then(|i| i.menu.clone()));
                match menu {
                    Some(tree) => crate::menus::tray_popup(hwnd.0 as isize, tray, &tree),
                    None => push_signal(MenuSignal::TrayClick { tray, right: true }),
                }
            }
            _ => {}
        }
        return LRESULT(0);
    }
    // SAFETY: everything else gets the default handling.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Krate's hidden owner window, made once.
fn owner() -> Result<HWND, String> {
    if let Some(h) = OWNER.with(|o| *o.borrow()) {
        return Ok(HWND(h as *mut _));
    }
    // SAFETY: a window class and a message-only window of Krate's own.
    unsafe {
        let instance = GetModuleHandleW(None).map_err(|e| e.to_string())?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(owner_proc),
            hInstance: instance.into(),
            lpszClassName: w!("KrateTrayOwner"),
            ..Default::default()
        };
        RegisterClassW(&class);
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("KrateTrayOwner"),
            w!("Krate"),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(instance.into()),
            None,
        )
        .map_err(|e| format!("Windows made no tray window: {e}"))?;
        OWNER.with(|o| *o.borrow_mut() = Some(hwnd.0 as isize));
        Ok(hwnd)
    }
}

/// An HICON of the app's pixels: a 32-bit top-down DIB with its alpha.
fn hicon(icon: &TrayIcon) -> Result<HICON, String> {
    let (w, h) = (icon.width as i32, icon.height as i32);
    // SAFETY: a DIB section Krate fills with exactly w*h*4 bytes, a mask
    // bitmap, both handed to CreateIconIndirect (which copies them) and
    // deleted after.
    unsafe {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let dc = GetDC(None);
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let color = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0);
        ReleaseDC(None, dc);
        let color = color.map_err(|e| format!("the icon could not be drawn: {e}"))?;
        // BGRA, premultiplied is not needed for icons: straight alpha.
        let out = std::slice::from_raw_parts_mut(bits as *mut u8, icon.rgba.len());
        for (o, px) in out.chunks_exact_mut(4).zip(icon.rgba.chunks_exact(4)) {
            o.copy_from_slice(&[px[2], px[1], px[0], px[3]]);
        }
        let mask = CreateBitmap(w, h, 1, 1, None);
        let made = CreateIconIndirect(&ICONINFO {
            fIcon: true.into(),
            hbmMask: mask,
            hbmColor: color,
            ..Default::default()
        });
        let _ = DeleteObject(HGDIOBJ(color.0));
        let _ = DeleteObject(HGDIOBJ(mask.0));
        made.map_err(|e| format!("the icon could not be drawn: {e}"))
    }
}

fn data(owner: HWND, tray: u64) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: owner,
        uID: tray as u32,
        uCallbackMessage: TRAY_MESSAGE,
        ..Default::default()
    }
}

fn copy_tip(nid: &mut NOTIFYICONDATAW, tooltip: &str) {
    let wide: Vec<u16> = tooltip.encode_utf16().take(nid.szTip.len() - 1).collect();
    nid.szTip[..wide.len()].copy_from_slice(&wide);
    nid.szTip[wide.len()] = 0;
}

/// Show an icon; its id.
pub(crate) fn show(icon: &TrayIcon, tooltip: &str) -> Result<u64, String> {
    let owner = owner()?;
    let tray = NEXT.with(|n| {
        let id = *n.borrow();
        *n.borrow_mut() += 1;
        id
    });
    if tray > 0xFFFF {
        return Err("too many tray icons this run".to_string());
    }
    let handle = hicon(icon)?;
    let mut nid = data(owner, tray);
    nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    nid.hIcon = handle;
    copy_tip(&mut nid, tooltip);
    // SAFETY: a NOTIFYICONDATAW Krate filled, for Krate's owner window.
    unsafe {
        if !Shell_NotifyIconW(NIM_ADD, &nid).as_bool() {
            let _ = DestroyIcon(handle);
            return Err("Windows did not add the icon to the notification area".to_string());
        }
        nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        let _ = Shell_NotifyIconW(NIM_SETVERSION, &nid);
    }
    ICONS.with(|i| i.borrow_mut().insert(tray, Icon { handle, menu: None }));
    Ok(tray)
}

pub(crate) fn set_icon(tray: u64, icon: &TrayIcon) -> Result<(), String> {
    let owner = owner()?;
    let handle = hicon(icon)?;
    let mut nid = data(owner, tray);
    nid.uFlags = NIF_ICON;
    nid.hIcon = handle;
    // SAFETY: as in show.
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
    let old = ICONS.with(|i| {
        i.borrow_mut()
            .get_mut(&tray)
            .map(|i| std::mem::replace(&mut i.handle, handle))
    });
    if let Some(old) = old {
        // SAFETY: the icon Krate made before, no longer shown.
        unsafe {
            let _ = DestroyIcon(old);
        }
    }
    Ok(())
}

pub(crate) fn set_tooltip(tray: u64, tooltip: &str) -> Result<(), String> {
    let mut nid = data(owner()?, tray);
    nid.uFlags = NIF_TIP;
    copy_tip(&mut nid, tooltip);
    // SAFETY: as in show.
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
    Ok(())
}

pub(crate) fn set_menu(tray: u64, menu: Option<&MenuTree>) -> Result<(), String> {
    ICONS.with(|i| {
        i.borrow_mut()
            .get_mut(&tray)
            .map(|i| i.menu = menu.cloned())
            .ok_or_else(|| "no such tray icon".to_string())
    })
}

pub(crate) fn remove(tray: u64) {
    let Some(icon) = ICONS.with(|i| i.borrow_mut().remove(&tray)) else {
        return;
    };
    if let Ok(owner) = owner() {
        // SAFETY: the icon Krate added, taken back, its HICON destroyed.
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &data(owner, tray));
            let _ = DestroyIcon(icon.handle);
        }
    }
}

/// Dispatch this thread's messages with no window open, so the icon's
/// clicks reach the owner window.
pub(crate) fn pump() {
    let mut msg = MSG::default();
    // SAFETY: this thread's own message queue.
    unsafe {
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
