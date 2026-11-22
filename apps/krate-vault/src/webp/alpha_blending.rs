// Ported from image-webp 0.2.4 (MIT OR Apache-2.0, see LICENSE-image-webp.txt
// in this folder) to no_std + alloc: std::io is replaced by `super::io`.
#![allow(warnings, clippy::all, clippy::pedantic, clippy::nursery, unused)]
#[allow(unused_imports)]
use alloc::{boxed::Box, format, string::String, vec, vec::Vec};
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;
// Optimized alpha blending routines based on libwebp
//
// <https://github.com/webmproject/libwebp/blob/e4f7a9f0c7c9fbfae1568bc7fa5c94b989b50872/src/demux/anim_decode.c#L215-L267>

const fn channel_shift(i: u32) -> u32 {
    i * 8
}

/// Blend a single channel of `src` over `dst`, given their alpha channel values.
/// `src` and `dst` are assumed to be NOT pre-multiplied by alpha.
fn blend_channel_nonpremult(
    src: u32,
    src_a: u8,
    dst: u32,
    dst_a: u8,
    scale: u32,
    shift: u32,
) -> u8 {
    let src_channel = ((src >> shift) & 0xff) as u8;
    let dst_channel = ((dst >> shift) & 0xff) as u8;
    let blend_unscaled =
        (u32::from(src_channel) * u32::from(src_a)) + (u32::from(dst_channel) * u32::from(dst_a));
    debug_assert!(u64::from(blend_unscaled) < (1u64 << 32) / u64::from(scale));
    ((blend_unscaled * scale) >> channel_shift(3)) as u8
}

/// Blend `src` over `dst` assuming they are NOT pre-multiplied by alpha.
fn blend_pixel_nonpremult(src: u32, dst: u32) -> u32 {
    let src_a = ((src >> channel_shift(3)) & 0xff) as u8;

    if src_a == 0 {
        dst
    } else {
        let dst_a = ((dst >> channel_shift(3)) & 0xff) as u8;
        // Approximate integer arithmetic for: dst_factor_a = (dst_a * (255 - src_a)) / 255
        // libwebp used the following formula here:
        //let dst_factor_a = (dst_a as u32 * (256 - src_a as u32)) >> 8;
        // however, we've found that we can use a more precise approximation without losing performance:
        let dst_factor_a = div_by_255(u32::from(dst_a) * (255 - u32::from(src_a)));
        let blend_a = u32::from(src_a) + dst_factor_a;
        let scale = (1u32 << 24) / blend_a;

        let blend_r =
            blend_channel_nonpremult(src, src_a, dst, dst_factor_a as u8, scale, channel_shift(0));
        let blend_g =
            blend_channel_nonpremult(src, src_a, dst, dst_factor_a as u8, scale, channel_shift(1));
        let blend_b =
            blend_channel_nonpremult(src, src_a, dst, dst_factor_a as u8, scale, channel_shift(2));
        debug_assert!(u32::from(src_a) + dst_factor_a < 256);

        (u32::from(blend_r) << channel_shift(0))
            | (u32::from(blend_g) << channel_shift(1))
            | (u32::from(blend_b) << channel_shift(2))
            | (blend_a << channel_shift(3))
    }
}

pub(crate) fn do_alpha_blending(buffer: [u8; 4], canvas: [u8; 4]) -> [u8; 4] {
    // The original C code contained different shift functions for different endianness,
    // but they didn't work when ported to Rust directly (and probably didn't work in C either).
    // So instead we reverse the order of bytes on big-endian here, at the interface.
    // `from_le_bytes` is a no-op on little endian (most systems) and a cheap shuffle on big endian.
    blend_pixel_nonpremult(u32::from_le_bytes(buffer), u32::from_le_bytes(canvas)).to_le_bytes()
}

/// Divides by 255, rounding to nearest (as opposed to down, like regular integer division does).
/// TODO: cannot output 256, so the output is effecitively u8. Plumb that through the code.
//
// Sources:
// https://arxiv.org/pdf/2202.02864
// https://github.com/image-rs/image-webp/issues/119#issuecomment-2544007820
#[inline]
const fn div_by_255(v: u32) -> u32 {
    (((v + 0x80) >> 8) + v + 0x80) >> 8
}
