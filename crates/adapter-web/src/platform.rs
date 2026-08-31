//! The eight symbols wasmtime needs from a host it does not recognise.
//!
//! wasmtime's virtual-memory layer has three backends: unix, windows, and
//! `custom`. A browser tab is none of the first two, so it lands on
//! `custom`, and `custom` means "the embedder supplies these". This file is
//! that embedder.
//!
//! # Why this is short rather than a port of mmap
//!
//! Inside a wasm module there is no address space to manage. There is one
//! flat linear memory, it only ever grows, and nothing can be unmapped or
//! protected. So each of these has a genuine, honest implementation rather
//! than a stub:
//!
//! - **Mapping** is an aligned allocation, zeroed, which is what a fresh
//!   anonymous mapping is.
//! - **Unmapping** is a free.
//! - **Protecting** is a no-op, because wasm has no page permissions. That
//!   is not a shortcut being taken: there is nothing weaker being allowed,
//!   because the guest is already confined by wasm's own memory model and
//!   cannot address anything outside it.
//! - **A memory image** is a copy of the bytes, and mapping one is a copy
//!   into place. Copy-on-write is an optimisation, and the semantics
//!   wasmtime asks for -- "looks the same as the contents it was created
//!   with, changes do not reflect back" -- are exactly what a copy gives.
//! - **TLS** is a plain static, because a wasm module has one thread.
//!
//! The sync primitives (`wasmtime_sync_*`) are NOT here: they are only
//! wanted when wasmtime is built without `std`, and this build has it.
//!
//! # The one assumption worth writing down
//!
//! Single-threaded. Every browser wasm module is, unless it opts into
//! shared memory and workers. When the event-loop work moves the guest
//! into a Web Worker, the worker still has its own module instance and its
//! own copy of this state, so the assumption holds there too. If a future
//! build ever shares one memory across workers, the TLS slots below become
//! wrong and must become real thread-locals.

use std::alloc::{alloc_zeroed, dealloc, Layout};

/// wasm's page size, and the alignment every mapping is made to.
const PAGE_SIZE: usize = 65536;

/// A mapping's size, remembered so it can be freed.
///
/// `wasmtime_munmap` is handed back only a pointer and a length, and a
/// Rust deallocation needs the original layout. The length wasmtime passes
/// is the one it asked for, and it is rounded the same way here, so the
/// layout is reconstructible rather than needing a side table.
fn layout_for(size: usize) -> Layout {
    let rounded = size.max(PAGE_SIZE).next_multiple_of(PAGE_SIZE);
    Layout::from_size_align(rounded, PAGE_SIZE).expect("a page-aligned layout")
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasmtime_page_size() -> usize {
    PAGE_SIZE
}

/// A new anonymous mapping: an aligned, zeroed allocation.
///
/// Protection flags are accepted and ignored, because wasm has no page
/// permissions to set. Returns 0 on success, as the contract asks.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasmtime_mmap_new(
    size: usize,
    _prot_flags: u32,
    ret: &mut *mut u8,
) -> i32 {
    let ptr = unsafe { alloc_zeroed(layout_for(size)) };
    if ptr.is_null() {
        // Out of memory is the one real failure here, and wasmtime wants a
        // non-zero code rather than a panic across the C boundary.
        return 1;
    }
    *ret = ptr;
    0
}

/// Replace a mapping's contents with a fresh, blank one.
///
/// The contract is "unmap any prior mappings and decommit them, then map
/// new anonymous memory here". Anonymous memory reads as zero, so zeroing
/// in place is that, and it keeps the address stable -- which the caller
/// requires, since it asked for this specific address.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasmtime_mmap_remap(addr: *mut u8, size: usize, _prot_flags: u32) -> i32 {
    unsafe { std::ptr::write_bytes(addr, 0, size) };
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasmtime_munmap(ptr: *mut u8, size: usize) -> i32 {
    unsafe { dealloc(ptr, layout_for(size)) };
    0
}

/// No page permissions exist to change inside a wasm module.
///
/// Answering success is correct rather than convenient: the protection
/// wasmtime is reaching for is already provided, and more strictly, by
/// wasm's own memory model -- a guest cannot address anything outside its
/// linear memory whatever this returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasmtime_mprotect(_ptr: *mut u8, _size: usize, _prot_flags: u32) -> i32 {
    0
}

/// A memory image: the bytes wasmtime wants to be able to stamp into place
/// repeatedly, kept as an owned copy.
struct MemoryImage {
    bytes: Vec<u8>,
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasmtime_memory_image_new(
    ptr: *const u8,
    len: usize,
    ret: &mut *mut u8,
) -> i32 {
    // The contract is explicit that ptr and len are valid only for this
    // call, so the copy is required, not a choice.
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec();
    let image = Box::new(MemoryImage { bytes });
    *ret = Box::into_raw(image) as *mut u8;
    0
}

/// Stamp an image's bytes at an address.
///
/// A real host maps this copy-on-write; here it is a copy. The observable
/// semantics wasmtime asks for are the same -- the region looks like the
/// image, and writes to it do not travel back to the image.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasmtime_memory_image_map_at(
    image: *mut u8,
    addr: *mut u8,
    len: usize,
) -> i32 {
    let image = unsafe { &*(image as *const MemoryImage) };
    let copied = image.bytes.len().min(len);
    unsafe {
        std::ptr::copy_nonoverlapping(image.bytes.as_ptr(), addr, copied);
        // Anything past the image is fresh anonymous memory, which reads
        // as zero. Without this a remapped region could show whatever the
        // previous tenant left behind.
        if len > copied {
            std::ptr::write_bytes(addr.add(copied), 0, len - copied);
        }
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasmtime_memory_image_free(image: *mut u8) {
    if !image.is_null() {
        drop(unsafe { Box::from_raw(image as *mut MemoryImage) });
    }
}

/// wasmtime's thread-local slots.
///
/// Two of them: slot 0 is the runtime pointer, slot 1 is used by the
/// component-model-async feature. A wasm module has one thread, so plain
/// statics are the honest implementation -- see the assumption noted at the
/// top of this file.
static mut TLS: [*mut u8; 2] = [std::ptr::null_mut(); 2];

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasmtime_tls_get(slot: usize) -> *mut u8 {
    unsafe { *(&raw const TLS).cast::<*mut u8>().add(slot.min(1)) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wasmtime_tls_set(slot: usize, ptr: *mut u8) {
    unsafe { *(&raw mut TLS).cast::<*mut u8>().add(slot.min(1)) = ptr };
}
