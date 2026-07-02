use crate::shell::Shell;
use anyhow::Context as _;
use heck::KebabCase as _;
use std::path::Path;

/// Register a cross-check binary in Cargo.toml if not already present.
/// Returns the registered bin name.
pub(crate) fn ensure_cross_bin_registered(
    manifest_path: &Path,
    cross_src: &Path,
    contest: &str,
    problem_url: &str,
    shell: &mut Shell,
) -> anyhow::Result<String> {
    let stem = cross_src
        .file_stem()
        .and_then(|s| s.to_str())
        .with_context(|| format!("invalid cross file: {}", cross_src.display()))?;
    let stem_kebab = stem.to_kebab_case();
    let bin_name = format!("{}-{}", contest, stem_kebab);
    let alias = stem_kebab.clone();

    let content = std::fs::read_to_string(manifest_path)
        .with_context(|| format!("failed to read {}", manifest_path.display()))?;
    let mut doc: toml_edit::Document = content
        .parse()
        .with_context(|| "failed to parse Cargo.toml")?;

    let manifest_dir = manifest_path.parent().unwrap_or(Path::new("."));
    let rel = cross_src
        .strip_prefix(manifest_dir)
        .unwrap_or(cross_src)
        .to_string_lossy()
        .replace('\\', "/");
    let already = doc["package"]["metadata"]["cargo-compete"]["bin"]
        .as_table()
        .map(|t| t.contains_key(&bin_name))
        .unwrap_or(false);

    if already {
        let mut updated = false;
        if let Some(arr) = doc["bin"].as_array_of_tables_mut() {
            if let Some(tbl) = arr
                .iter_mut()
                .find(|tbl| tbl.get("name").and_then(|item| item.as_str()) == Some(&bin_name))
            {
                if tbl.get("path").and_then(|item| item.as_str()) != Some(rel.as_str()) {
                    tbl["path"] = toml_edit::value(rel);
                    updated = true;
                }
            }
        }
        if updated {
            shell.status("Updating", format!("cross-check binary `{}`", bin_name))?;
            std::fs::write(manifest_path, doc.to_string())
                .with_context(|| "failed to write Cargo.toml")?;
        }
        return Ok(bin_name);
    }

    shell.status("Registering", format!("cross-check binary `{}`", bin_name))?;

    let bin_item = doc
        .entry("bin")
        .or_insert_with(|| toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new()));
    if let toml_edit::Item::ArrayOfTables(arr) = bin_item {
        let mut tbl = toml_edit::Table::new();
        tbl["name"] = toml_edit::value(bin_name.clone());
        tbl["path"] = toml_edit::value(rel);
        arr.push(tbl);
    }

    let meta = &mut doc["package"]["metadata"]["cargo-compete"]["bin"];
    meta[&bin_name]["alias"] = toml_edit::value(alias);
    meta[&bin_name]["problem"] = toml_edit::value(problem_url);

    std::fs::write(manifest_path, doc.to_string()).with_context(|| "failed to write Cargo.toml")?;

    Ok(bin_name)
}
