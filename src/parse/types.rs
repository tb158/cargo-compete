//! Types shared across the `random_test` module.

use serde::{Deserialize, Serialize};

/// Minimal representation of a single task section (A, B, C, ...).
#[derive(Debug, Clone)]
pub(crate) struct TaskSection {
    pub(crate) letter: String,
    pub(crate) input_blocks: Vec<Vec<String>>,
    pub(crate) constraints_items: Vec<String>,
}

// ─── FormatBlock types ────────────────────────────────────────────────────────

/// A single block in the `format:` section of the `random_test:` yml entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FormatBlock {
    Scalars(ScalarsBlock),
    Array(ArrayBlock),
    Rows(RowsBlock),
    TestCases(TestCasesBlock),
    Queries(QueriesBlock),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ScalarsBlock {
    pub vars: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ArrayBlock {
    pub base: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub len: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub jagged: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RowsBlock {
    pub vars: Vec<String>,
    pub len: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct TestCasesBlock {
    pub count: String,
    pub format: Vec<FormatBlock>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct QueryBranch {
    pub id: String,
    pub format: Vec<FormatBlock>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct QueriesBlock {
    pub count: String,
    /// Variable name used as the query type discriminator (e.g. "t" or "qt").
    /// None means numeric queries → renderer defaults to "qt".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discriminator: Option<String>,
    pub types: Vec<QueryBranch>,
}

/// Variable type in the `vars:` section.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub(crate) enum VarType {
    #[default]
    Usize,
    I64,
    #[serde(rename = "Chars")]
    Chars,
}

/// A scalar value in the yml schema. Either a literal integer or a free-form
/// expression string (variable name like `n` or `|S|`).
///
/// Serialized untagged so that `Lit(5)` becomes `5` and `Expr("n")` becomes `n`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub(crate) enum BoundRepr {
    Lit(i64),
    Expr(String),
}

/// Per-variable constraint entry in the `vars:` section.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct VarConstraint {
    pub r#type: VarType,
    /// Numeric range `[lo, hi]`. Each side is either a literal integer or the
    /// placeholder `_` (Expr) when the bound could not be inferred. Variable
    /// references are not allowed here — use the top-level `ordering` to
    /// express runtime relationships.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<[BoundRepr; 2]>,
    /// Allowed character values (for `type: Chars`) or a discrete enum set
    /// (for `type: Usize`/`I64`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<String>>,
    /// String length (for `type: Chars`). A literal or regular variable is a
    /// shared exact length; a pipe-wrapped synthetic variable such as `|s|`
    /// is sampled independently for each emitted Chars token.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub len: Option<BoundRepr>,
    /// Sum-bounded variable: in `T` test cases, the sum of this variable
    /// across all cases is at most `sum_limit`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sum_limit: Option<i64>,
    /// All elements of an array of this base must be pairwise distinct.
    #[serde(default, skip_serializing_if = "is_false")]
    pub all_distinct: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// The `random_test:` section in a test suite yml file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct RandomTestSection {
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub vars: std::collections::BTreeMap<String, VarConstraint>,
    pub format: Vec<FormatBlock>,
    /// Pairs `[a, b]` meaning `a <= b` (variables only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ordering: Vec<[String; 2]>,
    /// Pairs `[a, b]` meaning `a != b` (variables only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub not_equal: Vec<[String; 2]>,
    /// Constraint or format lines that could not be parsed. Serialised as a
    /// regular yml list so the random-test command can read and surface them
    /// to the user at runtime.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<String>,
}
