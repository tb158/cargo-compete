//! Parse `task.html` and persist the parsed input format / constraints to each
//! `testcases/*.yml` `random_test:` section.
//!
//! Downstream consumers (`web::input_template`, future `random_test` runner)
//! read the yml back via `serde_yaml`; they don't re-parse the HTML.

mod analysis;
mod constraint_parse;
mod format_lowering;
mod format_parse;
mod normalize;
mod task;
mod types;
mod yml_io;

pub(crate) use analysis::{analyze_format, VarShape};
pub(crate) use normalize::snake;
pub(crate) use task::annotate_ymls_with_format;
pub(crate) use types::{
    ArrayBlock, BoundRepr, FormatBlock, RandomTestSection, RowsBlock, VarConstraint, VarType,
};
pub(crate) use yml_io::read_random_test_section;

#[cfg(test)]
pub(crate) use format_parse::{lines_to_format_blocks, parse_varlen_rows};
#[cfg(test)]
pub(crate) use task::{parse_task_sections, task_to_format_blocks};
#[cfg(test)]
pub(crate) use types::{QueriesBlock, QueryBranch, ScalarsBlock, TaskSection, TestCasesBlock};
