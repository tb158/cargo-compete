use crate::shell::Shell;
use anyhow::{bail, Context as _};
use camino::{Utf8Path, Utf8PathBuf};
use cargo_metadata as cm;
use easy_ext::ext;
use indexmap::{indexset, IndexMap};
use itertools::Itertools as _;
use serde::{
    de::{Deserializer, Error as _, IntoDeserializer},
    Deserialize,
};
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    str,
};
use url::Url;

#[derive(Deserialize, Debug, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct PackageMetadataCargoCompete {
    pub(crate) config: Option<Utf8PathBuf>,
    #[serde(default, deserialize_with = "deserialize_bin_example")]
    pub(crate) bin: IndexMap<String, PackageMetadataCargoCompeteBinExample>,
    #[serde(default, deserialize_with = "deserialize_bin_example")]
    pub(crate) example: IndexMap<String, PackageMetadataCargoCompeteBinExample>,
}

fn deserialize_bin_example<'de, D>(
    deserializer: D,
) -> Result<IndexMap<String, PackageMetadataCargoCompeteBinExample>, D::Error>
where
    D: Deserializer<'de>,
{
    let map = IndexMap::<String, Repr>::deserialize(deserializer)?;
    return Ok(map
        .into_iter()
        .map(
            |(
                key,
                Repr {
                    name,
                    alias,
                    problem,
                },
            )| {
                let (name, alias) = if let Some(alias) = alias {
                    (key, alias)
                } else if let Some(name) = name {
                    (name, key)
                } else {
                    (key.clone(), key)
                };
                (
                    name,
                    PackageMetadataCargoCompeteBinExample { alias, problem },
                )
            },
        )
        .collect());

    #[derive(Deserialize)]
    #[serde(rename_all = "kebab-case")]
    struct Repr {
        name: Option<String>,
        alias: Option<String>,
        #[serde(deserialize_with = "deserialize_bin_problem")]
        problem: Url,
    }

    fn deserialize_bin_problem<'de, D>(deserializer: D) -> Result<Url, D::Error>
    where
        D: Deserializer<'de>,
    {
        return match Repr::deserialize(deserializer) {
            Ok(Repr::V1 { url }) | Ok(Repr::V2(url)) => Ok(url),
            Err(_) => Err(D::Error::custom(r#"expected `"<url>" | { url: "<url>" }`"#)),
        };

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            V1 { url: Url },
            V2(Url),
        }
    }
}

impl PackageMetadataCargoCompete {
    pub(crate) fn bin_like_by_name_or_alias(
        &self,
        name_or_alias: impl AsRef<str>,
    ) -> anyhow::Result<(&str, &PackageMetadataCargoCompeteBinExample)> {
        let bin_name_or_alias = name_or_alias.as_ref();

        match *itertools::chain(&self.bin, &self.example)
            .filter(
                |(name, PackageMetadataCargoCompeteBinExample { alias, .. })| {
                    [&**name, &**alias].contains(&bin_name_or_alias)
                },
            )
            .collect::<Vec<_>>()
        {
            [(k, v)] => Ok((k, v)),
            [] => bail!("no `problem` for: {}", bin_name_or_alias),
            [..] => bail!("multiple `problem`s for {}", bin_name_or_alias),
        }
    }

    /// Resolve a name or alias, also accepting a cross-check companion target.
    ///
    /// Returns the cargo target name to build and the problem metadata to test it
    /// against. An exact match on a registered `bin`/`example` wins. Otherwise a
    /// name ending in [`CROSS_SUFFIX`] is resolved against its base: `e_cross`
    /// borrows `e`'s problem URL and test-case file. Such a target is discovered
    /// by cargo from `src/bin/e_cross.rs` alone, so a brute force never needs a
    /// manifest entry of its own.
    pub(crate) fn bin_like_by_name_or_alias_or_cross(
        &self,
        name_or_alias: &str,
    ) -> anyhow::Result<(String, &PackageMetadataCargoCompeteBinExample)> {
        match self.bin_like_by_name_or_alias(name_or_alias) {
            Ok((name, meta)) => Ok((name.to_owned(), meta)),
            Err(err) => {
                let base = name_or_alias
                    .strip_suffix(CROSS_SUFFIX)
                    .filter(|base| !base.is_empty())
                    .ok_or(err)?;
                let (_, meta) = self.bin_like_by_name_or_alias(base)?;
                Ok((name_or_alias.to_owned(), meta))
            }
        }
    }
}

/// Suffix marking a cross-check companion of another target.
///
/// `cargo compete dup e` writes `src/bin/e_cross.rs`, and `--cross` looks for that
/// name by default. The suffix is also what lets `test`/`submit` accept `e_cross`
/// without a manifest entry.
pub(crate) const CROSS_SUFFIX: &str = "_cross";

#[derive(Debug, PartialEq)]
pub(crate) struct PackageMetadataCargoCompeteBinExample {
    pub(crate) alias: String,
    pub(crate) problem: Url,
}

#[ext(MetadataExt)]
impl cm::Metadata {
    pub(crate) fn all_members(&self) -> Vec<&cm::Package> {
        self.packages
            .iter()
            .filter(|cm::Package { id, .. }| self.workspace_members.contains(id))
            .collect()
    }

    pub(crate) fn query_for_member<S: AsRef<str>>(
        &self,
        spec: Option<S>,
    ) -> anyhow::Result<&cm::Package> {
        if let Some(spec_str) = spec {
            let spec_str = spec_str.as_ref();
            let spec = spec_str.parse::<krates::PkgSpec>()?;

            match *self
                .packages
                .iter()
                .filter(|package| {
                    self.workspace_members.contains(&package.id) && spec.matches(package)
                })
                .collect::<Vec<_>>()
            {
                [] => bail!("package `{}` is not a member of the workspace", spec_str),
                [member] => Ok(member),
                [_, _, ..] => bail!("`{}` matched multiple members?????", spec_str),
            }
        } else {
            let current_member = self
                .resolve
                .as_ref()
                .and_then(|cm::Resolve { root, .. }| root.as_ref())
                .map(|root| &self[root]);

            if let Some(current_member) = current_member {
                Ok(current_member)
            } else {
                match *self.workspace_members.iter().collect::<Vec<_>>() {
                    [] => bail!("this workspace has no members",),
                    [one] => Ok(&self[one]),
                    [..] => {
                        bail!(
                            "this manifest is virtual, and the workspace has {} members. specify \
                             one with `--manifest-path` or `--package`",
                            self.workspace_members.len(),
                        );
                    }
                }
            }
        }
    }
}

#[ext(PackageExt)]
impl cm::Package {
    pub(crate) fn manifest_dir(&self) -> &Utf8Path {
        self.manifest_path
            .parent()
            .expect("`manifest_path` should end with `Cargo.toml`")
    }

    pub(crate) fn read_package_metadata(
        &self,
        shell: &mut Shell,
    ) -> anyhow::Result<PackageMetadataCargoCompete> {
        let unused = &mut indexset!();

        let deserializer = self
            .metadata
            .get("cargo-compete")
            .cloned()
            .unwrap_or_else(|| json!({}))
            .into_deserializer();

        let ret = serde_ignored::deserialize(deserializer, |path| {
            unused.insert(path.to_string());
        })
        .with_context(|| "could not parse `package.metadata.cargo-compete`")?;

        for unused in &*unused {
            shell.warn(format!(
                "unused key in `package.metadata.cargo-compete`: {unused}",
            ))?;
        }

        Ok(ret)
    }

    pub(crate) fn bin_like_target_by_name(
        &self,
        name: impl AsRef<str>,
    ) -> anyhow::Result<&cm::Target> {
        let name = name.as_ref();

        self.targets
            .iter()
            .find(|t| {
                t.name == name
                    && (t.kind == ["bin".to_owned()] || t.kind == ["example".to_owned()])
            })
            .with_context(|| format!("no bin/example target named `{}` in `{}`", name, self.name))
    }

    pub(crate) fn bin_target_by_src_path(
        &self,
        src_path: impl AsRef<Path>,
    ) -> anyhow::Result<&cm::Target> {
        let src_path = src_path.as_ref();

        self.targets
            .iter()
            .find(|t| t.src_path == src_path && t.kind == ["bin".to_owned()])
            .with_context(|| {
                format!(
                    "no bin target which `src_path` is `{}` in `{}`",
                    src_path.display(),
                    self.name,
                )
            })
    }

    pub(crate) fn all_bin_targets_sorted(&self) -> Vec<&cm::Target> {
        self.targets
            .iter()
            .filter(|cm::Target { kind, .. }| *kind == ["bin".to_owned()])
            .sorted_by(|t1, t2| t1.name.cmp(&t2.name))
            .collect()
    }
}

pub(crate) fn locate_project(cwd: impl AsRef<Path>) -> anyhow::Result<PathBuf> {
    let cwd = cwd.as_ref();

    cwd.ancestors()
        .map(|p| p.join("Cargo.toml"))
        .find(|p| p.exists())
        .with_context(|| {
            format!(
                "could not find `Cargo.toml` in `{}` or any parent directory. first, run \
                 `cargo compete init` and `cd` to a workspace",
                cwd.display(),
            )
        })
}

pub(crate) fn cargo_metadata(
    manifest_path: impl AsRef<Path>,
    cwd: impl AsRef<Path>,
) -> cm::Result<cm::Metadata> {
    cm::MetadataCommand::new()
        .manifest_path(manifest_path.as_ref())
        .current_dir(cwd.as_ref())
        .exec()
}

pub(crate) fn cargo_metadata_no_deps(
    manifest_path: impl AsRef<Path>,
    cwd: impl AsRef<Path>,
) -> cm::Result<cm::Metadata> {
    cm::MetadataCommand::new()
        .manifest_path(manifest_path.as_ref())
        .no_deps()
        .current_dir(cwd.as_ref())
        .exec()
}

pub(crate) fn set_cargo_config_build_target_dir(
    dir: &Path,
    shell: &mut Shell,
) -> anyhow::Result<()> {
    crate::fs::create_dir_all(dir.join(".cargo"))?;

    let cargo_config_path = dir.join(".cargo").join("config.toml");

    let mut cargo_config = if cargo_config_path.exists() {
        crate::fs::read_to_string(&cargo_config_path)?
    } else {
        r#"[build]
"#
        .to_owned()
    }
    .parse::<toml_edit::Document>()
    .with_context(|| {
        format!(
            "could not parse the TOML file at `{}`",
            cargo_config_path.display(),
        )
    })?;

    if cargo_config.get("build").is_none() {
        let mut tbl = toml_edit::Table::new();
        tbl.set_implicit(true);
        cargo_config["build"] = toml_edit::Item::Table(tbl);
    }
    let mut dirty = false;
    if { &mut cargo_config["build"]["target-dir"] }.is_none() {
        cargo_config["build"]["target-dir"] = toml_edit::value("target");
        dirty = true;
    }
    if { &mut cargo_config["build"]["build-dir"] }.is_none() {
        cargo_config["build"]["build-dir"] = toml_edit::value("target");
        dirty = true;
    }
    if dirty {
        crate::fs::write(&cargo_config_path, cargo_config.to_string())?;
        shell.status("Wrote", cargo_config_path.display())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::project::{PackageMetadataCargoCompete, PackageMetadataCargoCompeteBinExample};
    use indexmap::indexmap;
    use pretty_assertions::assert_eq;
    use toml::toml;

    fn metadata() -> PackageMetadataCargoCompete {
        toml! {
            [bin]
            abc999-a = { alias = "a", problem = "https://atcoder.jp/contests/abc999/tasks/abc999_a" }
            abc999-e = { alias = "e", problem = "https://atcoder.jp/contests/abc999/tasks/abc999_e" }
        }
        .try_into::<PackageMetadataCargoCompete>()
        .unwrap()
    }

    #[test]
    fn cross_target_borrows_the_base_problem_but_keeps_its_own_name() {
        let md = metadata();
        let (name, meta) = md.bin_like_by_name_or_alias_or_cross("e_cross").unwrap();
        // Build `e_cross`, but test it against `e`'s problem and `e.yml`.
        assert_eq!(name, "e_cross");
        assert_eq!(meta.alias, "e");

        // The `[[bin]]` name resolves the same way, so `--src` works too.
        let (name, meta) = md
            .bin_like_by_name_or_alias_or_cross("abc999-e_cross")
            .unwrap();
        assert_eq!(name, "abc999-e_cross");
        assert_eq!(meta.alias, "e");
    }

    #[test]
    fn a_registered_target_still_wins_and_unrelated_names_do_not_resolve() {
        let md = metadata();
        let (name, meta) = md.bin_like_by_name_or_alias_or_cross("e").unwrap();
        assert_eq!((name.as_str(), &*meta.alias), ("abc999-e", "e"));

        for unknown in ["_cross", "e_copy", "e2", "ex", "z_cross"] {
            assert!(
                md.bin_like_by_name_or_alias_or_cross(unknown).is_err(),
                "`{unknown}` must not resolve to another problem",
            );
        }
    }

    #[test]
    fn deserialize_package_metadata_cargo_compete() -> anyhow::Result<()> {
        let expected = PackageMetadataCargoCompete {
            config: None,
            bin: indexmap!(
                "practice-a".to_owned() => PackageMetadataCargoCompeteBinExample {
                    alias: "a".to_owned(),
                    problem: "https://atcoder.jp/contests/practice/tasks/practice_1"
                        .parse()
                        .unwrap(),
                },
                "practice-b".to_owned() => PackageMetadataCargoCompeteBinExample {
                    alias: "b".to_owned(),
                    problem: "https://atcoder.jp/contests/practice/tasks/practice_2"
                        .parse()
                        .unwrap(),
                },
            ),
            example: indexmap!(),
        };

        assert_eq!(
            expected,
            toml! {
                [bin]
                practice-a = { alias = "a", problem = "https://atcoder.jp/contests/practice/tasks/practice_1" }
                practice-b = { alias = "b", problem = "https://atcoder.jp/contests/practice/tasks/practice_2" }
            }
            .try_into::<PackageMetadataCargoCompete>()?,
        );

        let expected = PackageMetadataCargoCompete {
            config: None,
            bin: indexmap!(
                "aplusb".to_owned() => PackageMetadataCargoCompeteBinExample {
                    alias: "aplusb".to_owned(),
                    problem: "https://judge.yosupo.jp/problem/aplusb".parse().unwrap(),
                },
            ),
            example: indexmap!(),
        };

        assert_eq!(
            expected,
            toml! {
                [bin]
                aplusb = { problem = "https://judge.yosupo.jp/problem/aplusb" }
            }
            .try_into::<PackageMetadataCargoCompete>()?,
        );

        let expected = PackageMetadataCargoCompete {
            config: None,
            bin: indexmap!(),
            example: indexmap!(
                "aplusb".to_owned() => PackageMetadataCargoCompeteBinExample {
                    alias: "aplusb".to_owned(),
                    problem: "https://judge.yosupo.jp/problem/aplusb".parse().unwrap(),
                },
            ),
        };

        assert_eq!(
            expected,
            toml! {
                [example]
                aplusb = { problem = "https://judge.yosupo.jp/problem/aplusb" }
            }
            .try_into::<PackageMetadataCargoCompete>()?,
        );

        Ok(())
    }
}
