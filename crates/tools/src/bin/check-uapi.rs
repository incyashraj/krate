use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use wit_parser::{Function, Interface, Resolve, Type, TypeDefKind, WorldItem};

const EXPECTED_IMPORTS: &[&str] = &[
    "layer36:fs/files@0.1.0",
    "layer36:fs/types@0.1.0",
    "layer36:io/args@0.1.0",
    "layer36:io/log@0.1.0",
    "layer36:io/stdio@0.1.0",
    "layer36:io/streams@0.1.0",
    "layer36:io/types@0.1.0",
    "layer36:locale/format@0.1.0",
    "layer36:locale/info@0.1.0",
    "layer36:locale/types@0.1.0",
    "layer36:net/http-client@0.1.0",
    "layer36:net/types@0.1.0",
    "layer36:time/clock@0.1.0",
    "layer36:time/sleep@0.1.0",
];

const EXPECTED_PACKAGES: &[&str] = &[
    "layer36:app@0.1.0",
    "layer36:fs@0.1.0",
    "layer36:io@0.1.0",
    "layer36:locale@0.1.0",
    "layer36:net@0.1.0",
    "layer36:time@0.1.0",
];

fn main() -> Result<()> {
    let report = check_phase2_uapi()?;

    println!("Layer36 Phase 2 UAPI check passed");
    println!("- package: {}", report.app_package);
    println!("- world: {}", report.world);
    println!("- imports: {}", report.import_count);
    println!("- packages: {}", report.package_count);

    Ok(())
}

struct CheckReport {
    app_package: String,
    world: String,
    import_count: usize,
    package_count: usize,
}

fn check_phase2_uapi() -> Result<CheckReport> {
    let root = workspace_root();
    let wit_dir = root.join("wit/layer36/phase2");

    let mut resolve = Resolve::default();
    let (app_package, _) = resolve
        .push_dir(&wit_dir)
        .with_context(|| format!("parse WIT package at {}", wit_dir.display()))?;

    let world_id = resolve
        .select_world(&[app_package], Some("cli"))
        .context("select layer36:app/cli world")?;

    let app_package_name = resolve.packages[app_package].name.to_string();
    ensure(
        app_package_name == "layer36:app@0.1.0",
        format!("expected app package `layer36:app@0.1.0`, got `{app_package_name}`"),
    )?;

    check_packages(&resolve)?;
    check_world(&resolve, world_id)?;
    check_naming(&resolve)?;
    check_permission_errors(&resolve)?;
    check_docs(&resolve, world_id)?;

    let world = &resolve.worlds[world_id];

    Ok(CheckReport {
        app_package: app_package_name,
        world: world.name.clone(),
        import_count: world.imports.len(),
        package_count: resolve.packages.len(),
    })
}

fn check_packages(resolve: &Resolve) -> Result<()> {
    let actual = resolve
        .packages
        .iter()
        .map(|(_, package)| package.name.to_string())
        .collect::<BTreeSet<_>>();
    let expected = EXPECTED_PACKAGES
        .iter()
        .map(|package| (*package).to_string())
        .collect::<BTreeSet<_>>();

    ensure(
        actual == expected,
        format!("Phase 2 package set changed\nexpected: {expected:?}\nactual:   {actual:?}"),
    )
}

fn check_world(resolve: &Resolve, world_id: wit_parser::WorldId) -> Result<()> {
    let world = &resolve.worlds[world_id];
    ensure(
        world.name == "cli",
        format!("expected `cli` world, got `{}`", world.name),
    )?;
    ensure(
        is_kebab_case(&world.name),
        format!("world name `{}` is not kebab-case", world.name),
    )?;

    let actual_imports = world
        .imports
        .values()
        .filter_map(|item| match item {
            WorldItem::Interface { id, .. } => {
                Some(interface_name(resolve, &resolve.interfaces[*id]))
            }
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let expected_imports = EXPECTED_IMPORTS
        .iter()
        .map(|name| (*name).to_string())
        .collect::<BTreeSet<_>>();

    ensure(
        actual_imports == expected_imports,
        format!(
            "Phase 2 cli imports changed\nexpected: {expected_imports:?}\nactual:   {actual_imports:?}"
        ),
    )?;

    let Some(WorldItem::Function(run)) = world
        .exports
        .iter()
        .find_map(|(key, item)| (String::from(key.clone()) == "run").then_some(item))
    else {
        bail!("Phase 2 cli world must export `run: func() -> s32`");
    };
    ensure(
        run.params.is_empty(),
        "`run` export must not take parameters".to_string(),
    )?;
    ensure(
        matches!(run.result, Some(Type::S32)),
        "`run` export must return s32".to_string(),
    )?;

    Ok(())
}

fn check_naming(resolve: &Resolve) -> Result<()> {
    for (_, interface) in resolve.interfaces.iter() {
        if let Some(name) = &interface.name {
            ensure(
                is_kebab_case(name),
                format!("interface name `{name}` is not kebab-case"),
            )?;
        }

        for type_id in interface.types.values() {
            let ty = &resolve.types[*type_id];
            if let Some(name) = &ty.name {
                ensure(
                    is_kebab_case(name),
                    format!("type name `{name}` is not kebab-case"),
                )?;
            }

            match &ty.kind {
                TypeDefKind::Record(record) => {
                    for field in &record.fields {
                        ensure(
                            is_kebab_case(&field.name),
                            format!("record field `{}` is not kebab-case", field.name),
                        )?;
                    }
                }
                TypeDefKind::Enum(enum_) => {
                    for case in &enum_.cases {
                        ensure(
                            is_kebab_case(&case.name),
                            format!("enum case `{}` is not kebab-case", case.name),
                        )?;
                    }
                }
                TypeDefKind::Variant(variant) => {
                    for case in &variant.cases {
                        ensure(
                            is_kebab_case(&case.name),
                            format!("variant case `{}` is not kebab-case", case.name),
                        )?;
                    }
                }
                _ => {}
            }
        }

        for function in interface.functions.values() {
            let name = render_function_name(resolve, function);
            ensure(
                is_kebab_case(&name),
                format!("function name `{name}` is not kebab-case"),
            )?;
        }
    }

    Ok(())
}

fn check_permission_errors(resolve: &Resolve) -> Result<()> {
    for expected_error in ["fs-error", "net-error"] {
        let Some(type_id) = find_type(resolve, expected_error) else {
            bail!("expected `{expected_error}` variant in Phase 2 WIT");
        };

        let TypeDefKind::Variant(variant) = &resolve.types[type_id].kind else {
            bail!("expected `{expected_error}` to be a variant");
        };

        ensure(
            variant
                .cases
                .iter()
                .any(|case| case.name == "permission-denied"),
            format!("`{expected_error}` must include `permission-denied`"),
        )?;
    }

    Ok(())
}

fn check_docs(resolve: &Resolve, world_id: wit_parser::WorldId) -> Result<()> {
    let world = &resolve.worlds[world_id];
    ensure_has_docs(&world.docs, format!("world `{}`", world.name))?;

    for (_, interface) in resolve.interfaces.iter() {
        let interface_label = interface_name(resolve, interface);
        ensure_has_docs(&interface.docs, format!("interface `{interface_label}`"))?;

        for type_id in interface.types.values() {
            let ty = &resolve.types[*type_id];
            if matches!(ty.kind, TypeDefKind::Type(_)) {
                continue;
            }
            let type_name = ty.name.as_deref().unwrap_or("anonymous");
            ensure_has_docs(&ty.docs, format!("type `{type_name}`"))?;

            match &ty.kind {
                TypeDefKind::Record(record) => {
                    for field in &record.fields {
                        ensure_has_docs(
                            &field.docs,
                            format!("field `{type_name}.{}`", field.name),
                        )?;
                    }
                }
                TypeDefKind::Enum(enum_) => {
                    for case in &enum_.cases {
                        ensure_has_docs(
                            &case.docs,
                            format!("enum case `{type_name}.{}`", case.name),
                        )?;
                    }
                }
                TypeDefKind::Variant(variant) => {
                    for case in &variant.cases {
                        ensure_has_docs(
                            &case.docs,
                            format!("variant case `{type_name}.{}`", case.name),
                        )?;
                    }
                }
                _ => {}
            }
        }

        for function in interface.functions.values() {
            let name = function_note_key(resolve, function);
            ensure_has_docs(
                &function.docs,
                format!("function `{interface_label}.{name}`"),
            )?;
        }
    }

    Ok(())
}

fn ensure_has_docs(docs: &wit_parser::Docs, label: String) -> Result<()> {
    ensure(
        docs.contents
            .as_deref()
            .map(|contents| !contents.trim().is_empty())
            .unwrap_or(false),
        format!("{label} must have WIT docs before UAPI freeze"),
    )
}

fn find_type(resolve: &Resolve, name: &str) -> Option<wit_parser::TypeId> {
    resolve
        .types
        .iter()
        .find_map(|(id, ty)| (ty.name.as_deref() == Some(name)).then_some(id))
}

fn render_function_name(resolve: &Resolve, function: &Function) -> String {
    if let Some(resource_id) = function.kind.resource() {
        let resource_name = resolve.types[resource_id]
            .name
            .as_deref()
            .unwrap_or("resource");
        for prefix in [
            format!("[method]{resource_name}."),
            format!("[static]{resource_name}."),
        ] {
            if let Some(name) = function.name.strip_prefix(&prefix) {
                return name.to_string();
            }
        }
        if let Some(name) = function.name.strip_prefix("[constructor]") {
            return name.to_string();
        }
    }

    function.name.clone()
}

fn function_note_key(resolve: &Resolve, function: &Function) -> String {
    let name = render_function_name(resolve, function);
    if let Some(resource_id) = function.kind.resource() {
        let resource_name = resolve.types[resource_id]
            .name
            .as_deref()
            .unwrap_or("resource");
        format!("{resource_name}.{name}")
    } else {
        name
    }
}

fn interface_name(resolve: &Resolve, interface: &Interface) -> String {
    match (interface.package, interface.name.as_deref()) {
        (Some(package_id), Some(name)) => resolve.packages[package_id].name.interface_id(name),
        (_, Some(name)) => name.to_string(),
        _ => "anonymous-interface".to_string(),
    }
}

fn is_kebab_case(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !value.starts_with('-')
        && !value.ends_with('-')
        && !value.contains("--")
}

fn ensure(condition: bool, message: String) -> Result<()> {
    if condition {
        Ok(())
    } else {
        bail!(message)
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase2_uapi_contract_check_passes() {
        check_phase2_uapi().expect("Phase 2 UAPI check");
    }

    #[test]
    fn kebab_case_rejects_ambiguous_names() {
        assert!(is_kebab_case("http-client"));
        assert!(is_kebab_case("s32"));
        assert!(!is_kebab_case("http_client"));
        assert!(!is_kebab_case("HttpClient"));
        assert!(!is_kebab_case("-http-client"));
        assert!(!is_kebab_case("http--client"));
    }
}
