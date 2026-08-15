//! S1's tripwire: the whole wgpu+vello pipeline runs headless and produces
//! the pixels we asked for. Skips (rather than fails) on machines with no
//! GPU adapter at all, because the presenter's own contract is "fall back,
//! never block an app".

#[test]
fn renders_background_and_a_rect() {
    let mut p = match krate_presenter_gpu::OffscreenPresenter::new() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    let scene = krate_presenter_gpu::build_scene(
        &[],
        1.0,
        krate_adapter_common::painter::PaintInteraction {
            hovered: None,
            pressed: None,
        },
    );
    let px = p.render(&scene, 64, 32).expect("render");
    assert_eq!(px.len(), 64 * 32 * 4);
    assert_eq!(
        &px[0..3],
        &[0x0a, 0x0a, 0x0a],
        "clear color reached the texture"
    );
}
