use std::error::Error;

#[cfg(target_os = "macos")]
fn main() -> Result<(), Box<dyn Error>> {
    use layer36_adapter_common::ui::{
        UiEvent, WidgetId, WidgetKind, WidgetNode, WidgetStyle, WindowBackendKind, WindowOptions,
        WindowSize,
    };
    use layer36_adapter_macos::{AppKitWidgetPlacement, MacosAppKitPrototypeUiAdapter};
    use layer36_layout::{absolute_rect, LayoutViewport};
    use layer36_policy::SessionPolicy;
    use layer36_runtime::{phase3_ui::Phase3UiDispatcher, uapi::UapiGuard};

    let guard = UapiGuard::new(SessionPolicy::default());
    let adapter = MacosAppKitPrototypeUiAdapter::new();
    let dispatcher = Phase3UiDispatcher::new(&guard, &adapter);
    let info = dispatcher.adapter_info();
    let window = dispatcher.create_window(WindowOptions::new(
        "Layer36 runtime AppKit smoke",
        WindowSize::new(640, 480)?,
    )?)?;

    dispatcher.show_window(window)?;
    let tick = dispatcher
        .pump_event_loop_once(window)?
        .ok_or("AppKit prototype returned no native tick")?;
    let record = dispatcher
        .window(window)?
        .ok_or("AppKit prototype window record missing")?;

    assert_eq!(info.backend, "macos-appkit-prototype");
    assert_eq!(info.window_backend, WindowBackendKind::AppKit);
    assert!(info.native_windows);
    assert!(info.native_event_loop);
    assert_eq!(tick.window, window);
    assert!(tick.snapshot_refreshed);
    assert_eq!(record.title, "Layer36 runtime AppKit smoke");
    assert!(record.visible);

    // P3-VS-01 sub-slice 1: lower a small widget tree to real AppKit controls,
    // drive the native button, and watch the click come back as a routed
    // Layer36 event.
    let root = WidgetId::new(1)?;
    let button = WidgetId::new(2)?;
    let field = WidgetId::new(3)?;

    dispatcher.set_root(
        window,
        WidgetNode {
            id: root,
            parent: None,
            kind: WidgetKind::Stack,
            label: None,
            role: None,
            style: WidgetStyle {
                width: Some(640.0),
                height: Some(480.0),
                grow: 0.0,
                padding: 16.0,
            },
        },
    )?;
    dispatcher.upsert_node(
        window,
        WidgetNode {
            id: button,
            parent: Some(root),
            kind: WidgetKind::Button,
            label: Some("Click me".to_string()),
            role: Some("button".to_string()),
            style: WidgetStyle {
                width: Some(160.0),
                height: Some(32.0),
                ..WidgetStyle::default()
            },
        },
    )?;
    dispatcher.upsert_node(
        window,
        WidgetNode {
            id: field,
            parent: Some(root),
            kind: WidgetKind::TextField,
            label: Some("waiting for click".to_string()),
            role: Some("textfield".to_string()),
            style: WidgetStyle {
                width: Some(320.0),
                height: Some(28.0),
                ..WidgetStyle::default()
            },
        },
    )?;

    let layout = dispatcher.compute_layout(window, LayoutViewport::new(640.0, 480.0)?)?;
    let tree = dispatcher
        .widget_tree(window)?
        .ok_or("widget tree missing")?;

    let mut placements = Vec::new();
    for id in [button, field] {
        let node = tree.node(id).ok_or("widget node missing")?;
        let rect = absolute_rect(&tree, &layout, id).ok_or("widget rect missing")?;
        placements.push(AppKitWidgetPlacement::new(
            id,
            node.kind,
            node.label.clone(),
            rect.x,
            rect.y,
            rect.width,
            rect.height,
        )?);
    }

    let lowered = adapter
        .lower_widget_placements(window, &placements)?
        .ok_or("no tracked native session for widget lowering")?;
    assert_eq!(lowered.lowered, vec![button, field]);
    assert_eq!(
        adapter.widget_text(window, field)?.as_deref(),
        Some("waiting for click")
    );

    // Drive the real NSButton through AppKit's own target-action path.
    assert!(adapter.perform_widget_click(window, button)?);
    let click_tick = dispatcher
        .pump_event_loop_once(window)?
        .ok_or("AppKit prototype returned no native tick after click")?;
    assert!(click_tick.callbacks_handled >= 1);

    let events = dispatcher.drain_events()?;
    let clicked = events
        .iter()
        .any(|event| matches!(event, UiEvent::Pointer(pointer) if pointer.widget == Some(button)));
    assert!(
        clicked,
        "expected a routed Layer36 pointer event for the native NSButton click"
    );

    // React the way an app would: update the native text field.
    assert!(adapter.set_widget_text(window, field, "clicked!")?);
    assert_eq!(
        adapter.widget_text(window, field)?.as_deref(),
        Some("clicked!")
    );

    dispatcher.close_window(window)?;

    println!("Layer36 Phase 3 AppKit runtime smoke passed");
    println!("- window: {}", window.get());
    println!("- backend: {}", info.backend);
    println!("- widgets lowered natively: {}", lowered.lowered.len());
    println!(
        "- native click callbacks handled: {}",
        click_tick.callbacks_handled
    );
    println!("- routed click event observed: {clicked}");
    println!("- text field after click: clicked!");

    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn main() -> Result<(), Box<dyn Error>> {
    println!("Layer36 Phase 3 AppKit runtime smoke skipped: host is not macOS");
    Ok(())
}
