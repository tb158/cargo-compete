use crate::{project::PackageExt as _, shell::Shell};
use anyhow::ensure;
use az::SaturatingAs as _;
use camino::{Utf8Path, Utf8PathBuf};
use cargo_metadata as cm;
use human_size::{Byte, Size};
use liquid::object;
use maplit::btreemap;
use snowchains_core::{
    judge::CommandExpression,
    testsuite::{BatchTestCase, PartialBatchTestCase, TestSuite},
    web::PlatformKind,
};
use std::{
    collections::{BTreeMap, HashSet},
    env,
    path::Path,
    sync::Arc,
};
use url::Url;

pub(crate) struct Args<'a> {
    pub(crate) metadata: &'a cm::Metadata,
    pub(crate) member: &'a cm::Package,
    pub(crate) bin: &'a cm::Target,
    pub(crate) bin_alias: &'a str,
    pub(crate) cargo_compete_config_test_suite: &'a liquid::Template,
    pub(crate) problem_url: &'a Url,
    pub(crate) toolchain: Option<&'a str>,
    pub(crate) release: bool,
    pub(crate) test_case_names: Option<HashSet<String>>,
    pub(crate) display_limit: Size,
    pub(crate) cookies_path: &'a Path,
    pub(crate) shell: &'a mut Shell,
    /// Skip sample tests and go directly to random/cross tests.
    pub(crate) no_sample: bool,
    /// Number of random cases to run after samples pass (None = skip)
    pub(crate) random_count: Option<u32>,
    /// Pre-built cross (brute-force) binary artifact path
    pub(crate) cross_artifact: Option<Utf8PathBuf>,
    /// Number of cross-check cases to run (None = skip)
    pub(crate) cross_count: Option<u32>,
    /// File stem of the cross binary source (display only)
    pub(crate) cross_bin_alias: Option<String>,
}

pub(crate) fn test(args: Args<'_>) -> anyhow::Result<()> {
    let Args {
        metadata,
        member,
        bin,
        bin_alias,
        cargo_compete_config_test_suite,
        problem_url,
        toolchain,
        release,
        test_case_names,
        display_limit,
        cookies_path,
        shell,
        no_sample,
        random_count,
        cross_artifact,
        cross_count,
        cross_bin_alias,
    } = args;

    let test_suite_path = test_suite_path(
        &metadata.workspace_root,
        member.manifest_dir(),
        cargo_compete_config_test_suite,
        &bin.name,
        bin_alias,
        problem_url,
        shell,
    )?;

    let test_suite = crate::fs::read_yaml(&test_suite_path)?;

    let test_cases = match test_suite {
        TestSuite::Batch(test_suite) => test_suite.load_test_cases(
            test_suite_path.parent().unwrap().as_ref(),
            test_case_names,
            |override_problem_url| {
                fn read(path: &Path) -> anyhow::Result<Arc<str>> {
                    crate::fs::read_to_string(path).map(Into::into)
                }

                let problem_url = override_problem_url.unwrap_or(problem_url);

                let system_test_cases_dir =
                    crate::web::retrieve_testcases::system_test_cases_dir(problem_url)?;

                let text_files = |dir_name: &str| -> anyhow::Result<Vec<_>> {
                    let paths = crate::fs::read_dir(system_test_cases_dir.join(dir_name))?;
                    Ok(paths
                        .into_iter()
                        .filter(|p| p.extension() == Some("txt".as_ref()))
                        .map(|p| {
                            let s = p
                                .file_stem()
                                .expect("should not be empty")
                                .to_string_lossy()
                                .into_owned();
                            (s, p)
                        })
                        .collect())
                };

                if !system_test_cases_dir.join("in").exists() {
                    crate::web::retrieve_testcases::dl_only_system_test_cases(
                        problem_url,
                        cookies_path,
                        &metadata.workspace_root,
                        shell,
                    )?;
                }

                let mut system_test_cases: BTreeMap<_, (Option<_>, Option<_>)> = btreemap!();

                for (name, path) in text_files("in")? {
                    system_test_cases.entry(name).or_default().0 = Some(read(&path)?);
                }
                for (name, path) in text_files("out")? {
                    system_test_cases.entry(name).or_default().1 = Some(read(&path)?);
                }

                Ok(system_test_cases
                    .into_iter()
                    .flat_map(|(name, (r#in, out))| {
                        let r#in = r#in?;
                        Some(PartialBatchTestCase {
                            name: Some(name),
                            r#in,
                            out,
                            timelimit: None,
                            r#match: None,
                        })
                    })
                    .collect())
            },
        )?,
        TestSuite::Interactive(_) => {
            shell.warn("tests for `Interactive` problems are currently not supported")?;
            vec![]
        }
        TestSuite::Unsubmittable => {
            shell.warn("this is `Unsubmittable` problem")?;
            vec![]
        }
    };

    if let Some(toolchain) = toolchain {
        crate::process::process("rustup").args(&["run", toolchain, "cargo"])
    } else {
        crate::process::process(crate::process::cargo_exe()?)
    }
    .arg("build")
    .arg(if bin.kind == ["example".to_owned()] {
        "--example"
    } else {
        "--bin"
    })
    .arg(&bin.name)
    .args(if release { &["--release"] } else { &[] })
    .arg("--manifest-path")
    .arg(&member.manifest_path)
    .cwd(&metadata.workspace_root)
    .exec_with_shell_status(shell)?;

    let artifact = metadata
        .target_directory
        .join(if release { "release" } else { "debug" })
        .join(if bin.kind == ["example".to_owned()] {
            "examples"
        } else {
            ""
        })
        .join(&bin.name)
        .with_extension(env::consts::EXE_EXTENSION);

    ensure!(
        artifact.exists(),
        "`cargo build` succeeded but `{}` was not produced. probably this is a bug",
        artifact,
    );

    let display_limit = display_limit.into::<Byte>().value().saturating_as();

    let sample_result = if !no_sample {
        let outcome = snowchains_core::judge::judge(
            shell.progress_draw_target(),
            tokio::signal::ctrl_c,
            &CommandExpression {
                program: artifact.clone().into(),
                args: vec![],
                cwd: metadata.workspace_root.clone().into(),
                env: btreemap!(),
            },
            &test_cases,
        )?;
        writeln!(shell.err())?;
        outcome.print_pretty(shell.err(), Some(display_limit))?;
        outcome.error_on_fail()
    } else {
        Ok(())
    };

    if sample_result.is_ok() && random_count.is_some() {
        if matches!(
            PlatformKind::from_url(problem_url),
            Ok(PlatformKind::Atcoder)
        ) {
            let timelimit = match crate::fs::read_yaml(&test_suite_path)? {
                TestSuite::Batch(s) => s.timelimit,
                _ => None,
            };
            crate::random_test::run_random_tests(crate::random_test::RandomTestArgs {
                artifact: artifact.as_std_path(),
                yml_path: test_suite_path.as_path(),
                count: random_count.unwrap_or(5),
                timelimit,
                display_limit,
                cwd: metadata.workspace_root.as_std_path(),
                shell,
            })?;
        } else {
            shell.warn("random tests are only supported for AtCoder problems")?;
        }
    }

    if sample_result.is_ok() {
        if let (Some(cross_artifact), Some(cross_count)) = (&cross_artifact, cross_count) {
            // cross binary sample tests
            if !no_sample {
                let cross_cases: Vec<BatchTestCase> = test_cases
                    .iter()
                    .map(|tc| BatchTestCase {
                        timelimit: None,
                        ..tc.clone()
                    })
                    .collect();
                crate::random_test::write_section_banner(
                    shell.err(),
                    "cross-check binary sample tests",
                )?;
                let cross_sample = snowchains_core::judge::judge(
                    shell.progress_draw_target(),
                    tokio::signal::ctrl_c,
                    &CommandExpression {
                        program: cross_artifact.as_std_path().into(),
                        args: vec![],
                        cwd: metadata.workspace_root.clone().into(),
                        env: btreemap!(),
                    },
                    &cross_cases,
                )?;
                writeln!(shell.err())?;
                cross_sample.print_pretty(shell.err(), Some(display_limit))?;
                cross_sample.error_on_fail()?;
            }
            // cross-check random tests
            let timelimit = match crate::fs::read_yaml(&test_suite_path)? {
                TestSuite::Batch(s) => s.timelimit,
                _ => None,
            };
            crate::random_test::run_cross_check(crate::random_test::CrossCheckArgs {
                main_artifact: artifact.as_std_path(),
                cross_artifact: cross_artifact.as_std_path(),
                yml_path: test_suite_path.as_path(),
                count: cross_count,
                timelimit,
                display_limit,
                cwd: metadata.workspace_root.as_std_path(),
                main_bin_name: &bin.name,
                cross_bin_alias: cross_bin_alias.as_deref().unwrap_or(""),
                shell,
            })?;
        }
    }

    sample_result
}

pub(crate) fn test_suite_path(
    workspace_root: &Utf8Path,
    pkg_manifest_dir: &Utf8Path,
    cargo_compete_config_test_suite: &liquid::Template,
    bin_name: &str,
    bin_alias: &str,
    problem_url: &Url,
    shell: &mut Shell,
) -> anyhow::Result<Utf8PathBuf> {
    let contest = match PlatformKind::from_url(problem_url) {
        Ok(PlatformKind::Atcoder) => Some(snowchains_core::web::atcoder_contest_id(problem_url)?),
        Ok(PlatformKind::Codeforces) => {
            Some(snowchains_core::web::codeforces_contest_id(problem_url)?.to_string())
        }
        _ => None,
    };

    let vars = object!({
        "manifest_dir": pkg_manifest_dir,
        "contest": contest,
        "bin_name": bin_name,
        "bin_alias": bin_alias,
    });

    let vars_including_deprecated = object!({
        "manifest_dir": pkg_manifest_dir,
        "contest": contest,
        "bin_name": bin_name,
        "bin_alias": bin_alias,
        "problem": bin_alias,
    });

    let (test_suite_path, uses_deprecated_vars) = cargo_compete_config_test_suite
        .render(&vars)
        .map(|r| (r, false))
        .or_else(|_| {
            cargo_compete_config_test_suite
                .render(&vars_including_deprecated)
                .map(|r| (r, true))
        })?;
    let test_suite_path = Utf8Path::new(&test_suite_path);
    let test_suite_path = test_suite_path.strip_prefix(".").unwrap_or(test_suite_path);

    if uses_deprecated_vars {
        shell.warn("deprecated variables used for `.test-suite` in compete.toml")?;
        shell.warn("- `problem` is deprecated. use `bin_alias` instead.")?;
    }

    Ok(workspace_root.join(test_suite_path))
}
