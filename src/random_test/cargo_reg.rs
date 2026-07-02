use crate::shell::Shell;
use anyhow::Context as _;
use heck::KebabCase as _;
use std::path::Path;

/// Register a cross-check binary in Cargo.toml if not already present.
/// Returns the registered bin name.
///
/// The `[[bin]]` entry and the `[package.metadata.cargo-compete.bin]` entry are
/// checked independently so a manifest where only one side exists (hand-edited
/// or partially written) is repaired instead of duplicated or left broken:
/// `cargo build --bin {name}` needs the `[[bin]]` entry, alias-based commands
/// need the metadata entry. A drifted `[[bin]].path` is also updated.
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

    let mut added: Vec<&str> = Vec::new();
    let mut path_updated = false;

    let bin_item = doc
        .entry("bin")
        .or_insert_with(|| toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new()));
    if let toml_edit::Item::ArrayOfTables(arr) = bin_item {
        let existing = arr
            .iter()
            .position(|tbl| tbl.get("name").and_then(|item| item.as_str()) == Some(&bin_name));
        match existing {
            Some(i) => {
                let tbl = arr.get_mut(i).expect("index from position");
                if tbl.get("path").and_then(|item| item.as_str()) != Some(rel.as_str()) {
                    tbl["path"] = toml_edit::value(rel.clone());
                    path_updated = true;
                }
            }
            None => {
                let mut tbl = toml_edit::Table::new();
                tbl["name"] = toml_edit::value(bin_name.clone());
                tbl["path"] = toml_edit::value(rel.clone());
                arr.push(tbl);
                added.push("[[bin]]");
            }
        }
    } else {
        anyhow::bail!(
            "`bin` in {} is not an array of tables",
            manifest_path.display()
        );
    }

    let meta_present = doc["package"]["metadata"]["cargo-compete"]["bin"]
        .as_table()
        .map(|t| t.contains_key(&bin_name))
        .unwrap_or(false);
    if !meta_present {
        let meta = &mut doc["package"]["metadata"]["cargo-compete"]["bin"];
        meta[&bin_name]["alias"] = toml_edit::value(alias);
        meta[&bin_name]["problem"] = toml_edit::value(problem_url);
        added.push("package metadata");
    }

    if !added.is_empty() {
        shell.status(
            "Registering",
            format!("cross-check binary `{}` ({})", bin_name, added.join(" + ")),
        )?;
    } else if path_updated {
        shell.status("Updating", format!("cross-check binary `{}`", bin_name))?;
    }
    if !added.is_empty() || path_updated {
        std::fs::write(manifest_path, doc.to_string())
            .with_context(|| "failed to write Cargo.toml")?;
    }

    Ok(bin_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const BASE_MANIFEST: &str = r#"[package]
name = "abc999"
version = "0.1.0"
edition = "2024"

[[bin]]
name = "abc999-a"
path = "src/bin/a.rs"

[package.metadata.cargo-compete.bin]
abc999-a = { alias = "a", problem = "https://example.com/a" }
"#;

    struct Fixture {
        _dir: tempfile::TempDir,
        manifest: PathBuf,
        cross_src: PathBuf,
    }

    fn fixture(manifest_content: &str) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join("Cargo.toml");
        std::fs::write(&manifest, manifest_content).unwrap();
        let cross_src = dir.path().join("src/bin/a_cross.rs");
        Fixture {
            _dir: dir,
            manifest,
            cross_src,
        }
    }

    fn register(f: &Fixture) -> String {
        let mut shell = Shell::new();
        ensure_cross_bin_registered(
            &f.manifest,
            &f.cross_src,
            "abc999",
            "https://example.com/a",
            &mut shell,
        )
        .unwrap()
    }

    fn manifest_text(f: &Fixture) -> String {
        std::fs::read_to_string(&f.manifest).unwrap()
    }

    #[test]
    fn registers_bin_and_metadata_when_both_absent() {
        let f = fixture(BASE_MANIFEST);
        let bin_name = register(&f);
        assert_eq!(bin_name, "abc999-a-cross");
        let doc: toml_edit::Document = manifest_text(&f).parse().unwrap();
        let arr = doc["bin"].as_array_of_tables().unwrap();
        let entry = arr
            .iter()
            .find(|t| t["name"].as_str() == Some("abc999-a-cross"))
            .expect("[[bin]] entry added");
        assert_eq!(entry["path"].as_str(), Some("src/bin/a_cross.rs"));
        let meta = doc["package"]["metadata"]["cargo-compete"]["bin"]["abc999-a-cross"]
            .as_table_like()
            .expect("metadata entry added");
        assert_eq!(meta.get("alias").and_then(|i| i.as_str()), Some("a-cross"));
        assert_eq!(
            meta.get("problem").and_then(|i| i.as_str()),
            Some("https://example.com/a")
        );
    }

    #[test]
    fn second_call_is_idempotent() {
        let f = fixture(BASE_MANIFEST);
        register(&f);
        let after_first = manifest_text(&f);
        register(&f);
        assert_eq!(manifest_text(&f), after_first);
    }

    #[test]
    fn adds_missing_bin_when_only_metadata_present() {
        // Previously this desync returned early: the metadata check reported
        // "already registered" and the [[bin]] entry was never added, so
        // `cargo build --bin abc999-a-cross` failed.
        let manifest = format!(
            "{}abc999-a-cross = {{ alias = \"a-cross\", problem = \"https://example.com/a\" }}\n",
            BASE_MANIFEST
        );
        let f = fixture(&manifest);
        register(&f);
        let doc: toml_edit::Document = manifest_text(&f).parse().unwrap();
        let arr = doc["bin"].as_array_of_tables().unwrap();
        assert!(
            arr.iter()
                .any(|t| t["name"].as_str() == Some("abc999-a-cross")),
            "[[bin]] entry must be added even when metadata already exists"
        );
    }

    #[test]
    fn does_not_duplicate_bin_when_only_bin_present() {
        // Previously this desync pushed a second identical [[bin]] entry.
        let manifest = format!(
            "{}
[[bin]]
name = \"abc999-a-cross\"
path = \"src/bin/a_cross.rs\"
",
            BASE_MANIFEST
        );
        let f = fixture(&manifest);
        register(&f);
        let doc: toml_edit::Document = manifest_text(&f).parse().unwrap();
        let arr = doc["bin"].as_array_of_tables().unwrap();
        let count = arr
            .iter()
            .filter(|t| t["name"].as_str() == Some("abc999-a-cross"))
            .count();
        assert_eq!(count, 1, "[[bin]] entry must not be duplicated");
        assert!(
            doc["package"]["metadata"]["cargo-compete"]["bin"]
                .as_table()
                .map(|t| t.contains_key("abc999-a-cross"))
                .unwrap_or(false),
            "metadata entry must be added"
        );
    }

    #[test]
    fn updates_drifted_bin_path() {
        let manifest = format!(
            "{}
[[bin]]
name = \"abc999-a-cross\"
path = \"src/bin/old_location.rs\"
",
            BASE_MANIFEST
        );
        let f = fixture(&manifest);
        register(&f);
        let doc: toml_edit::Document = manifest_text(&f).parse().unwrap();
        let arr = doc["bin"].as_array_of_tables().unwrap();
        let entry = arr
            .iter()
            .find(|t| t["name"].as_str() == Some("abc999-a-cross"))
            .unwrap();
        assert_eq!(entry["path"].as_str(), Some("src/bin/a_cross.rs"));
    }
}
