use crate::{
    config::CargoCompeteConfigTestProfile,
    project::{MetadataExt as _, PackageExt as _},
    shell::ColorChoice,
};
use anyhow::{anyhow, bail, Context as _};
use human_size::Size;
use std::path::PathBuf;
use structopt::StructOpt;
use strum::VariantNames as _;

#[derive(StructOpt, Debug)]
#[structopt(usage(
    r"cargo compete test [OPTIONS] <bin-name-or-alias>
    cargo compete test [OPTIONS] --src <PATH>",
))]
pub struct OptCompeteTest {
    /// Path to the source code
    #[structopt(
        long,
        value_name("PATH"),
        required_unless("name-or-alias"),
        conflicts_with("name-or-alias")
    )]
    pub src: Option<PathBuf>,

    /// Test for only the test cases
    #[structopt(long, value_name("NAME"))]
    pub testcases: Option<Vec<String>>,

    /// Display limit
    #[structopt(long, value_name("SIZE"), default_value("4KiB"))]
    pub display_limit: Size,

    /// Existing package to retrieving test cases for
    #[structopt(short, long, value_name("SPEC"))]
    pub package: Option<String>,

    /// Build in debug mode. Overrides `test.profile` in compete.toml
    #[structopt(long, conflicts_with("release"))]
    pub debug: bool,

    /// Build in release mode. Overrides `test.profile` in compete.toml
    #[structopt(long)]
    pub release: bool,

    /// Path to Cargo.toml
    #[structopt(long, value_name("PATH"))]
    pub manifest_path: Option<PathBuf>,

    /// Coloring
    #[structopt(
        long,
        value_name("WHEN"),
        possible_values(ColorChoice::VARIANTS),
        default_value("auto")
    )]
    pub color: ColorChoice,

    /// Run N random test cases after samples pass (default 5)
    #[structopt(long, value_name("N"), min_values = 0, max_values = 1)]
    pub random: Option<Vec<u32>>,

    /// Skip sample tests (only valid with --random or --cross)
    #[structopt(long = "no-sample")]
    pub no_sample: bool,

    /// Cross-check against a brute-force target (default `<alias>_cross`), optional case count (default 100)
    #[structopt(long, value_name("[TARGET] [N]"), min_values = 0, max_values = 2)]
    pub cross: Option<Vec<String>>,

    #[structopt(required_unless("src"))]
    /// Name or alias for a `bin`/`example`
    pub name_or_alias: Option<String>,
}

/// Number of cross-check cases when `--cross` is given without one.
const DEFAULT_CROSS_COUNT: u32 = 100;

/// Split `--cross` values into an optional target name and a case count.
///
/// `--cross` and `--cross N` use the default `<alias>_cross` target; `--cross
/// TARGET` and `--cross TARGET N` name it explicitly. A lone all-digit value is
/// read as the count, so a target whose name is only digits needs the two-value
/// form.
fn parse_cross_args(values: &[String]) -> anyhow::Result<(Option<&str>, u32)> {
    fn count(s: &str) -> anyhow::Result<u32> {
        s.parse()
            .with_context(|| format!("invalid case count for `--cross`: `{s}`"))
    }

    match values {
        [] => Ok((None, DEFAULT_CROSS_COUNT)),
        [one] if !one.is_empty() && one.bytes().all(|b| b.is_ascii_digit()) => {
            Ok((None, count(one)?))
        }
        [one] => Ok((Some(cross_target_name(one)?), DEFAULT_CROSS_COUNT)),
        [target, n] => Ok((Some(cross_target_name(target)?), count(n)?)),
        _ => unreachable!("`max_values = 2`"),
    }
}

/// `--cross` used to take a path to a source file. Reject the old form with the
/// rewrite spelled out; otherwise it surfaces as a confusing "no target named
/// `src/bin/e_cross.rs`".
fn cross_target_name(value: &str) -> anyhow::Result<&str> {
    if value.ends_with(".rs") || value.contains('/') || value.contains('\\') {
        bail!(
            "`--cross` takes a target name, not a path: `{value}`\n\
             note: a target name is the file stem of a `src/bin/*.rs`, e.g. `e_cross` for \
             `src/bin/e_cross.rs`",
        );
    }
    Ok(value)
}

pub(crate) fn run(opt: OptCompeteTest, ctx: crate::Context<'_>) -> anyhow::Result<()> {
    let OptCompeteTest {
        src,
        testcases,
        display_limit,
        package,
        debug,
        release,
        manifest_path,
        color,
        random,
        no_sample,
        cross,
        name_or_alias,
    } = opt;

    super::generated_check::validate_generated_check_options(
        false,
        no_sample,
        random.is_some(),
        cross.is_some(),
    )?;

    let crate::Context {
        cwd,
        cookies_path,
        shell,
    } = ctx;

    shell.set_color_choice(color);

    let manifest_path = manifest_path
        .map(|p| Ok(cwd.join(p.strip_prefix(".").unwrap_or(&p))))
        .unwrap_or_else(|| crate::project::locate_project(&cwd))?;
    let metadata = crate::project::cargo_metadata(manifest_path, &cwd)?;
    let member = metadata.query_for_member(package.as_deref())?;
    let package_metadata = member.read_package_metadata(shell)?;
    let (cargo_compete_config, _) = crate::config::load_for_package(member, shell)?;

    let (bin, pkg_md_bin_example) = if let Some(src) = src {
        let src = cwd.join(src.strip_prefix(".").unwrap_or(&src));
        let bin = member.bin_target_by_src_path(src)?;
        let (_, pkg_md_bin) = package_metadata.bin_like_by_name_or_alias_or_cross(&bin.name)?;
        (bin, pkg_md_bin)
    } else if let Some(name_or_alias) = &name_or_alias {
        let (bin_name, pkg_md_bin_example) =
            package_metadata.bin_like_by_name_or_alias_or_cross(name_or_alias)?;
        let bin = member.bin_like_target_by_name(&bin_name)?;
        (bin, pkg_md_bin_example)
    } else {
        unreachable!()
    };

    let (cross_target, cross_count) = if let Some(values) = &cross {
        let (target_override, n) = parse_cross_args(values)?;
        let default_name = format!(
            "{}{}",
            pkg_md_bin_example.alias,
            crate::project::CROSS_SUFFIX,
        );
        let name = target_override.unwrap_or(&default_name);
        let target = member.bin_like_target_by_name(name).map_err(|err| {
            if target_override.is_some() {
                err
            } else {
                // The default target is the one `dup` creates, so point at that
                // rather than leaving the user to guess from cargo's target list.
                anyhow!(
                    "{err}\n\
                     note: `cargo compete dup {}` copies `{}` to `src/bin/{name}.rs`",
                    pkg_md_bin_example.alias,
                    bin.src_path.file_name().unwrap_or(bin.src_path.as_str()),
                )
            }
        })?;
        (Some(target), Some(n))
    } else {
        (None, None)
    };

    crate::testing::test(crate::testing::Args {
        metadata: &metadata,
        member,
        bin,
        bin_alias: &pkg_md_bin_example.alias,
        cargo_compete_config_test_suite: &cargo_compete_config.test_suite,
        problem_url: &pkg_md_bin_example.problem,
        toolchain: cargo_compete_config.test.toolchain.as_deref(),
        release: if debug {
            false
        } else if release {
            true
        } else {
            cargo_compete_config.test.profile == CargoCompeteConfigTestProfile::Release
        },
        test_case_names: testcases.map(|ss| ss.into_iter().collect()),
        display_limit,
        cookies_path: &cookies_path,
        shell,
        no_sample,
        random_count: random.map(|v| v.into_iter().next().unwrap_or(5)),
        cross_target,
        cross_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_test_is_not_a_test_option_alias() {
        assert!(OptCompeteTest::from_iter_safe(&["test", "b", "--no-test"]).is_err());
    }

    #[test]
    fn cross_args_split_into_target_and_count() {
        let v = |ss: &[&str]| ss.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(parse_cross_args(&v(&[])).unwrap(), (None, 100));
        assert_eq!(parse_cross_args(&v(&["30"])).unwrap(), (None, 30));
        assert_eq!(
            parse_cross_args(&v(&["e_cross"])).unwrap(),
            (Some("e_cross"), 100)
        );
        assert_eq!(
            parse_cross_args(&v(&["e_cross", "30"])).unwrap(),
            (Some("e_cross"), 30)
        );
    }

    #[test]
    fn cross_rejects_a_path_and_a_non_numeric_count() {
        let v = |ss: &[&str]| ss.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        // Silently defaulting to 100 cases on a typo hid the mistake entirely.
        assert!(parse_cross_args(&v(&["e_cross", "xyz"])).is_err());
        for path in ["e_cross.rs", "src/bin/e_cross.rs", "./e_cross"] {
            assert!(
                parse_cross_args(&v(&[path])).is_err(),
                "`--cross {path}` must be rejected as a path"
            );
        }
    }
}
