//! Random-test and cross-check runners.
//!
//! Consumes the generation layer (`super::cases::generate_cases`) and drives
//! snowchains `judge()`. yml-only — no HTML re-parse (the constraints were
//! persisted at retrieve time by `crate::parse`).

use super::cases::{generate_cases, GenerateOutcome};
use super::proc::{run_with_input, RunResult};
use crate::shell::Shell;
use camino::Utf8Path;
use maplit::btreemap;
use snowchains_core::{
    judge::{CommandExpression, Verdict},
    testsuite::{BatchTestCase, DeterministicExpectedOutput, ExpectedOutput},
};
use std::{path::Path, sync::Arc, time::Duration};
use termcolor::Color;

const DISPLAY_LIMIT_NOTE: &str = "output beyond --display-limit (default: 4KiB; e.g. 152834 B) is truncated; change the limit with --display-limit";

pub(crate) struct RandomTestArgs<'a> {
    pub artifact: &'a Path,
    pub yml_path: &'a Utf8Path,
    pub count: u32,
    pub timelimit: Option<Duration>,
    pub display_limit: usize,
    pub cwd: &'a Path,
    pub shell: &'a mut Shell,
}

type NamedCases = Vec<(String, String)>;

fn load_generated_cases(
    yml_path: &Utf8Path,
    count: u32,
    empty_warning: &str,
    shell: &mut Shell,
) -> anyhow::Result<Option<(NamedCases, Vec<String>)>> {
    let (cases, skipped) = match generate_cases(yml_path, count)? {
        None => return Ok(None),
        Some(GenerateOutcome::Aborted { reasons }) => {
            for reason in reasons {
                shell.warn(reason)?;
            }
            return Ok(None);
        }
        Some(GenerateOutcome::Interrupted) => return Err(crate::interrupt::Interrupted.into()),
        Some(GenerateOutcome::Ready { cases, skipped }) => (cases, skipped),
    };
    if cases.is_empty() {
        shell.warn(empty_warning)?;
        return Ok(None);
    }
    Ok(Some((cases, skipped)))
}

fn print_skipped_constraints(shell: &mut Shell, skipped: &[String]) -> anyhow::Result<()> {
    if !skipped.is_empty() {
        shell.err_label(
            Color::Yellow,
            "warning",
            format!(
                "skipped {} unsupported constraint(s): {}",
                skipped.len(),
                skipped.join("; ")
            ),
        )?;
    }
    Ok(())
}

pub(crate) fn run_random_tests(args: RandomTestArgs<'_>) -> anyhow::Result<()> {
    let RandomTestArgs {
        artifact,
        yml_path,
        count,
        timelimit,
        display_limit,
        cwd,
        shell,
    } = args;

    let _interrupt_guard = crate::interrupt::activate()?;
    let Some((cases, skipped)) =
        load_generated_cases(yml_path, count, "no test cases generated", shell)?
    else {
        return Ok(());
    };

    let test_cases: Vec<BatchTestCase> = cases
        .into_iter()
        .map(|(name, input)| BatchTestCase {
            name: Some(name),
            timelimit,
            input: Arc::from(input.as_str()),
            output: ExpectedOutput::Deterministic(DeterministicExpectedOutput::Pass),
        })
        .collect();

    super::write_section_banner(shell.err(), "random tests")?;

    let outcome = snowchains_core::judge::judge(
        shell.progress_draw_target(),
        tokio::signal::ctrl_c,
        &CommandExpression {
            program: artifact.into(),
            args: vec![],
            cwd: cwd.into(),
            env: btreemap!(),
        },
        &test_cases,
    )
    .map_err(|err| {
        if crate::interrupt::requested() {
            anyhow::Error::from(crate::interrupt::Interrupted)
        } else {
            err
        }
    })?;
    crate::interrupt::check()?;

    writeln!(shell.err())?;
    outcome.print_pretty(shell.err(), Some(display_limit))?;
    writeln!(shell.err())?;

    let failures = outcome
        .verdicts
        .iter()
        .filter(|v| !matches!(v, Verdict::Accepted { .. }))
        .count();
    let has_accepted = failures < outcome.verdicts.len();

    shell.err_label(Color::Cyan, "note", DISPLAY_LIMIT_NOTE)?;
    if has_accepted {
        shell.err_label(
            Color::Cyan,
            "note",
            "Accepted means no crash or TLE; output correctness is not verified",
        )?;
    }
    print_skipped_constraints(shell, &skipped)?;
    if failures > 0 {
        anyhow::bail!("{}/{} tests failed", failures, test_cases.len());
    }
    Ok(())
}

pub(crate) struct CrossCheckArgs<'a> {
    pub main_artifact: &'a Path,
    pub cross_artifact: &'a Path,
    pub yml_path: &'a Utf8Path,
    pub count: u32,
    /// Applied to the main binary only; cross binary runs without timelimit.
    pub timelimit: Option<Duration>,
    pub display_limit: usize,
    pub cwd: &'a Path,
    pub main_bin_name: &'a str,
    pub cross_bin_alias: &'a str,
    pub shell: &'a mut Shell,
}

pub(crate) fn run_cross_check(args: CrossCheckArgs<'_>) -> anyhow::Result<()> {
    let CrossCheckArgs {
        main_artifact,
        cross_artifact,
        yml_path,
        count,
        timelimit,
        display_limit,
        cwd,
        main_bin_name,
        cross_bin_alias,
        shell,
    } = args;

    let _interrupt_guard = crate::interrupt::activate()?;
    let Some((cases, skipped)) = load_generated_cases(
        yml_path,
        count,
        "no test cases generated for cross-check",
        shell,
    )?
    else {
        return Ok(());
    };

    super::write_section_banner(shell.err(), "cross-check tests")?;

    // Run the cross (brute-force) binary on each case to obtain expected output.
    let mut adopted: Vec<BatchTestCase> = Vec::new();
    for (name, input) in &cases {
        crate::interrupt::check()?;
        match run_with_input(cross_artifact, input, None, cwd)? {
            RunResult::Ok(output) => {
                adopted.push(BatchTestCase {
                    name: Some(name.clone()),
                    timelimit,
                    input: Arc::from(input.as_str()),
                    output: ExpectedOutput::Deterministic(DeterministicExpectedOutput::Exact {
                        text: Arc::from(output.as_str()),
                    }),
                });
            }
            RunResult::RuntimeError(code) => {
                shell.warn(format!(
                    "cross binary RE (exit {code}) on case {name}, skipping"
                ))?;
            }
            RunResult::TimeLimitExceeded => {
                shell.warn(format!("cross binary TLE on case {name}, skipping"))?;
            }
        }
    }

    if adopted.is_empty() {
        shell.warn("all cross-check cases skipped (cross binary RE/TLE on every input)")?;
        return Ok(());
    }

    let mut outcome = snowchains_core::judge::judge(
        shell.progress_draw_target(),
        tokio::signal::ctrl_c,
        &CommandExpression {
            program: main_artifact.into(),
            args: vec![],
            cwd: cwd.into(),
            env: btreemap!(),
        },
        &adopted,
    )
    .map_err(|err| {
        if crate::interrupt::requested() {
            anyhow::Error::from(crate::interrupt::Interrupted)
        } else {
            err
        }
    })?;
    crate::interrupt::check()?;

    let failure_count = outcome
        .verdicts
        .iter()
        .filter(|v| !matches!(v, Verdict::Accepted { .. }))
        .count();
    if failure_count > 0 {
        outcome
            .verdicts
            .retain(|v| !matches!(v, Verdict::Accepted { .. }));
        writeln!(shell.err())?;
        outcome.print_pretty(shell.err(), Some(display_limit))?;
        writeln!(shell.err())?;
        shell.err_label(Color::Yellow, "expected", cross_bin_alias)?;
        shell.err_label(Color::Yellow, "actual", main_bin_name)?;
        writeln!(shell.err())?;
    } else {
        // No detail block is printed for accepted cross-check cases; retain the
        // visual separator before the trailing notes.
        writeln!(shell.err())?;
    }

    shell.err_label(Color::Cyan, "note", DISPLAY_LIMIT_NOTE)?;
    print_skipped_constraints(shell, &skipped)?;

    if failure_count > 0 {
        anyhow::bail!(
            "{}/{} cross-check tests failed",
            failure_count,
            adopted.len()
        );
    }
    Ok(())
}
