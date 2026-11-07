// Time the canvas effects at a phone's full resolution (1080x2316).
fn main() {
    let mut surface =
        krate_lowering::canvas_raster::CanvasSurface::new(1080, 2316).expect("surface");
    surface.clear(0xFF_E6_E7_EB);
    let started = std::time::Instant::now();
    for _ in 0..10 {
        surface.radial_gradient(540.0, 579.0, 2316.0 * 0.8, 0x4C_3A_4D_F2, 0x00_3A_4D_F2);
    }
    println!(
        "radial: {:.1} ms each",
        started.elapsed().as_secs_f64() * 100.0
    );
    let started = std::time::Instant::now();
    for _ in 0..10 {
        surface.drop_shadow_round_rect(
            48.0,
            1100.0,
            984.0,
            1100.0,
            (90.0, 90.0, 90.0, 90.0),
            108.0,
            0x2E_00_00_00,
        );
    }
    println!(
        "shadow: {:.1} ms each",
        started.elapsed().as_secs_f64() * 100.0
    );
}
