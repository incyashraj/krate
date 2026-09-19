// Default ANSI palette and gradient calculations adapted from hexyl (MIT).
pub const COLOR_NULL: &str = "\x1b[90m";
pub const COLOR_OFFSET: &str = "\x1b[90m";
pub const COLOR_ASCII_PRINTABLE: &str = "\x1b[36m";
pub const COLOR_ASCII_WHITESPACE: &str = "\x1b[32m";
pub const COLOR_ASCII_OTHER: &str = "\x1b[32m";
pub const COLOR_NONASCII: &str = "\x1b[33m";
pub const COLOR_RESET: &str = "\x1b[39m";
pub const COLOR_NULL_RGB: &[u8] = &rgb_bytes(100, 100, 100);

pub const COLOR_DEL: &[u8] = &rgb_bytes(64, 128, 0);

pub const COLOR_GRADIENT_NONASCII: [[u8; 19]; 128] =
    generate_color_gradient(&[(255, 0, 0, 0.0), (255, 255, 0, 0.66), (255, 255, 255, 1.0)]);

pub const COLOR_GRADIENT_ASCII_NONPRINTABLE: [[u8; 19]; 31] =
    generate_color_gradient(&[(255, 0, 255, 0.0), (128, 0, 255, 1.0)]);

pub const COLOR_GRADIENT_ASCII_PRINTABLE: [[u8; 19]; 95] =
    generate_color_gradient(&[(0, 128, 255, 0.0), (0, 255, 128, 1.0)]);

const fn as_dec(byte: u8) -> [u8; 3] {
    [
        b'0' + (byte / 100),
        b'0' + ((byte % 100) / 10),
        b'0' + (byte % 10),
    ]
}

const fn rgb_bytes(r: u8, g: u8, b: u8) -> [u8; 19] {
    let mut buf = *b"\x1b[38;2;rrr;ggg;bbbm";

    // r 7
    buf[7] = as_dec(r)[0];
    buf[8] = as_dec(r)[1];
    buf[9] = as_dec(r)[2];

    // g 11
    buf[11] = as_dec(g)[0];
    buf[12] = as_dec(g)[1];
    buf[13] = as_dec(g)[2];

    // b 15
    buf[15] = as_dec(b)[0];
    buf[16] = as_dec(b)[1];
    buf[17] = as_dec(b)[2];

    buf
}

const fn generate_color_gradient<const N: usize>(stops: &[(u8, u8, u8, f64)]) -> [[u8; 19]; N] {
    let mut out = [rgb_bytes(0, 0, 0); N];

    assert!(stops.len() >= 2, "need at least two stops for the gradient");

    let mut byte = 0;
    while byte < N {
        let relative_byte = byte as f64 / N as f64;

        let mut i = 1;
        while i < stops.len() && stops[i].3 < relative_byte {
            i += 1;
        }
        if i >= stops.len() {
            i = stops.len() - 1;
        }
        let prev_stop = stops[i - 1];
        let stop = stops[i];
        let diff = stop.3 - prev_stop.3;
        let t = (relative_byte - prev_stop.3) / diff;

        let r = (prev_stop.0 as f64 + (t * (stop.0 as f64 - prev_stop.0 as f64))) as u8;
        let g = (prev_stop.1 as f64 + (t * (stop.1 as f64 - prev_stop.1 as f64))) as u8;
        let b = (prev_stop.2 as f64 + (t * (stop.2 as f64 - prev_stop.2 as f64))) as u8;

        out[byte] = rgb_bytes(r, g, b);

        byte += 1;
    }

    out
}
