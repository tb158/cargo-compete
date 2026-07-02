//! Random-test / cross-check generation driven by the `random_test:` section
//! persisted in each `testcases/{problem}.yml`.
//!
//! The HTML is parsed once at retrieve time (`crate::parse`); this module reads
//! the resulting yml back and never re-parses HTML.

mod budget;
mod cargo_reg;
mod cases;
mod context;
mod emitter;
mod gen;
mod proc;
mod relation;
mod render;
mod runner;
mod spec;
mod strategy;

pub(crate) use cargo_reg::ensure_cross_bin_registered;
pub(crate) use runner::{run_cross_check, run_random_tests, CrossCheckArgs, RandomTestArgs};

/// Cross-cutting safety ceiling on generated input elements.
///
/// Random tests keep rendered cases in memory before judging. Inputs above this
/// size are unlikely in normal AtCoder tasks and usually indicate a missing or
/// misparsed constraint, so abort before the renderer allocates unbounded data.
pub(crate) const MAX_INPUT_ELEMENTS: u128 = 100_000_000;

/// Print a section separator banner to stderr and flush so it appears before
/// any progress bar.
pub(crate) fn write_section_banner(
    out: &mut dyn termcolor::WriteColor,
    title: &str,
) -> anyhow::Result<()> {
    writeln!(out)?;
    writeln!(out, "══════════════════════════════════════════")?;
    writeln!(out, "{:^42}", title)?;
    writeln!(out, "══════════════════════════════════════════")?;
    out.flush()?;
    Ok(())
}
