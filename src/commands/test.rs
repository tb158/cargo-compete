use crate::{
    config::CargoCompeteConfigTestProfile,
    project::{MetadataExt as _, PackageExt as _},
    shell::ColorChoice,
};
use human_size::Size;
use std::{
    env,
    path::{Path, PathBuf},
};
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

    /// Cross-check against brute-force binary, optional case count (default 100)
    #[structopt(long, value_name("PATH [N]"), min_values = 1, max_values = 2)]
    pub cross: Option<Vec<String>>,

    #[structopt(required_unless("src"))]
    /// Name or alias for a `bin`/`example`
    pub name_or_alias: Option<String>,
}

fn resolve_cross_source_path(cwd: &Path, raw_src: &Path) -> PathBuf {
    if !raw_src.is_absolute() && raw_src.components().count() == 1 {
        cwd.join("src").join("bin").join(raw_src)
    } else {
        cwd.join(raw_src.strip_prefix(".").unwrap_or(raw_src))
    }
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
        let (_, pkg_md_bin) = package_metadata.bin_like_by_name_or_alias(&bin.name)?;
        (bin, pkg_md_bin)
    } else if let Some(name_or_alias) = &name_or_alias {
        let (bin_name, pkg_md_bin_example) =
            package_metadata.bin_like_by_name_or_alias(name_or_alias)?;
        let bin = member.bin_like_target_by_name(bin_name)?;
        (bin, pkg_md_bin_example)
    } else {
        unreachable!()
    };

    let (cross_artifact, cross_count, cross_bin_alias) = if let Some(ref v) = cross {
        let raw_src = PathBuf::from(&v[0]);
        let cross_src = resolve_cross_source_path(&cwd, &raw_src);
        let n: u32 = v.get(1).and_then(|s| s.parse().ok()).unwrap_or(100);
        match snowchains_core::web::PlatformKind::from_url(&pkg_md_bin_example.problem) {
            Ok(snowchains_core::web::PlatformKind::Atcoder) => {
                let contest =
                    snowchains_core::web::atcoder_contest_id(&pkg_md_bin_example.problem)?;
                let bin_name = crate::random_test::ensure_cross_bin_registered(
                    member.manifest_path.as_std_path(),
                    &cross_src,
                    &contest,
                    pkg_md_bin_example.problem.as_str(),
                    shell,
                )?;
                let alias = cross_src
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_owned();
                crate::process::process(crate::process::cargo_exe()?)
                    .arg("build")
                    .arg("--bin")
                    .arg(&bin_name)
                    .arg("--manifest-path")
                    .arg(&member.manifest_path)
                    .cwd(&metadata.workspace_root)
                    .exec_with_shell_status(shell)?;
                let artifact = metadata
                    .target_directory
                    .join("debug")
                    .join(&bin_name)
                    .with_extension(env::consts::EXE_EXTENSION);
                (Some(artifact), Some(n), Some(alias))
            }
            _ => {
                shell.warn("--cross is only supported for AtCoder problems")?;
                (None, None, None)
            }
        }
    } else {
        (None, None, None)
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
        cross_artifact,
        cross_count,
        cross_bin_alias,
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
    fn bare_cross_filename_resolves_under_src_bin() {
        assert_eq!(
            resolve_cross_source_path(Path::new("/tmp/contest"), Path::new("a copy.rs")),
            PathBuf::from("/tmp/contest/src/bin/a copy.rs")
        );
        assert_eq!(
            resolve_cross_source_path(Path::new("/tmp/contest"), Path::new("src/bin/a copy.rs")),
            PathBuf::from("/tmp/contest/src/bin/a copy.rs")
        );
    }
}
