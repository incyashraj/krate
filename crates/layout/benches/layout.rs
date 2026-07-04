use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion};
use krate_adapter_common::ui::{WidgetId, WidgetKind, WidgetNode, WidgetStyle, WidgetTree};
use krate_layout::{compute_layout_cold, LayoutViewport, PreparedLayoutTree};

fn phase3_layout_benches(c: &mut Criterion) {
    let viewport = LayoutViewport::new(1440.0, 900.0).expect("viewport");
    let stack_1k = generated_stack_tree(1_000);
    let stack_10k = generated_stack_tree(10_000);

    let mut group = c.benchmark_group("phase3_layout");
    group.sample_size(10);
    group.bench_function("cold_stack_1k_nodes", |b| {
        b.iter(|| compute_layout_cold(black_box(&stack_1k), black_box(viewport)).expect("layout"))
    });
    group.bench_function("cold_stack_10k_nodes", |b| {
        b.iter(|| compute_layout_cold(black_box(&stack_10k), black_box(viewport)).expect("layout"))
    });

    let mut prepared_1k = PreparedLayoutTree::new(&stack_1k).expect("prepared 1k");
    let mut prepared_10k = PreparedLayoutTree::new(&stack_10k).expect("prepared 10k");
    group.bench_function("prepared_stack_1k_nodes", |b| {
        b.iter(|| prepared_1k.compute(black_box(viewport)).expect("layout"))
    });
    group.bench_function("prepared_stack_10k_nodes", |b| {
        b.iter(|| prepared_10k.compute(black_box(viewport)).expect("layout"))
    });
    group.finish();
}

fn generated_stack_tree(node_count: u64) -> WidgetTree {
    let root = WidgetNode::new(WidgetId::new(1).expect("root"), WidgetKind::Stack);
    let mut tree = WidgetTree::new(root).expect("tree");
    for id in 2..=node_count {
        let parent = if id <= 64 { 1 } else { (id / 8).max(1) };
        let kind = match id % 8 {
            0 => WidgetKind::Stack,
            1 => WidgetKind::Scroll,
            2 => WidgetKind::ListView,
            3 => WidgetKind::Button,
            4 => WidgetKind::TextField,
            5 => WidgetKind::TextArea,
            6 => WidgetKind::Canvas,
            _ => WidgetKind::Text,
        };
        let style = WidgetStyle {
            width: (id % 5 == 0).then_some(80.0),
            height: (id % 3 == 0).then_some(24.0),
            grow: if id % 7 == 0 { 1.0 } else { 0.0 },
            padding: if id % 11 == 0 { 2.0 } else { 0.0 },
        };
        let node = WidgetNode::new(WidgetId::new(id).expect("id"), kind)
            .with_parent(WidgetId::new(parent).expect("parent"))
            .with_style(style)
            .expect("style");
        tree.upsert(node).expect("node");
    }

    tree
}

criterion_group!(benches, phase3_layout_benches);
criterion_main!(benches);
