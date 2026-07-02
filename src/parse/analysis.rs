//! Structural analysis of the persisted `format:` tree.
//!
//! This module is shared by yml lowering and random-test consumers so shape
//! and size-position decisions are derived from one traversal.

use super::types::FormatBlock;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VarShape {
    Scalar,
    Array,
    Rows,
    Jagged,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct FormatAnalysis {
    pub shapes: HashMap<String, VarShape>,
    pub referenced_names: HashSet<String>,
    pub size_exprs: HashSet<String>,
    pub first_test_cases_count: Option<String>,
    pub jagged_len_to_count: HashMap<String, String>,
}

impl FormatAnalysis {
    pub(crate) fn shape_of(&self, name: &str) -> VarShape {
        self.shapes.get(name).copied().unwrap_or(VarShape::Scalar)
    }
}

pub(crate) fn analyze_format(blocks: &[FormatBlock]) -> FormatAnalysis {
    fn mark_scalar(out: &mut FormatAnalysis, name: &str) {
        out.referenced_names.insert(name.to_string());
        out.shapes
            .entry(name.to_string())
            .or_insert(VarShape::Scalar);
    }
    fn size(out: &mut FormatAnalysis, expr: &str) {
        out.size_exprs.insert(expr.to_string());
        mark_scalar(out, expr);
    }
    fn rec(blocks: &[FormatBlock], out: &mut FormatAnalysis) {
        for block in blocks {
            match block {
                FormatBlock::Scalars(s) => {
                    for v in &s.vars {
                        mark_scalar(out, v);
                    }
                }
                FormatBlock::Array(a) => {
                    out.referenced_names.insert(a.base.clone());
                    out.shapes.insert(
                        a.base.clone(),
                        if a.jagged {
                            VarShape::Jagged
                        } else {
                            VarShape::Array
                        },
                    );
                    for expr in a.len.iter().chain(a.height.iter()).chain(a.count.iter()) {
                        size(out, expr);
                    }
                    if a.jagged {
                        if let (Some(len), Some(count)) = (&a.len, &a.count) {
                            out.jagged_len_to_count
                                .entry(len.clone())
                                .or_insert_with(|| count.clone());
                        }
                    }
                }
                FormatBlock::Rows(r) => {
                    for v in &r.vars {
                        out.referenced_names.insert(v.clone());
                        out.shapes.insert(v.clone(), VarShape::Rows);
                    }
                    size(out, &r.len);
                }
                FormatBlock::TestCases(t) => {
                    if out.first_test_cases_count.is_none() {
                        out.first_test_cases_count = Some(t.count.clone());
                    }
                    size(out, &t.count);
                    rec(&t.format, out);
                }
                FormatBlock::Queries(q) => {
                    size(out, &q.count);
                    if let Some(d) = &q.discriminator {
                        mark_scalar(out, d);
                    }
                    for branch in &q.types {
                        rec(&branch.format, out);
                    }
                }
            }
        }
    }
    let mut out = FormatAnalysis::default();
    rec(blocks, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::{ArrayBlock, FormatBlock, RowsBlock, ScalarsBlock, TestCasesBlock};

    #[test]
    fn collects_shapes_sizes_and_denominators() {
        let blocks = vec![
            FormatBlock::Scalars(ScalarsBlock {
                vars: vec!["t".into()],
            }),
            FormatBlock::TestCases(TestCasesBlock {
                count: "t".into(),
                format: vec![
                    FormatBlock::Rows(RowsBlock {
                        vars: vec!["x".into()],
                        len: "n".into(),
                    }),
                    FormatBlock::Array(ArrayBlock {
                        base: "a".into(),
                        len: Some("l".into()),
                        height: None,
                        count: Some("n".into()),
                        jagged: true,
                    }),
                ],
            }),
        ];
        let got = analyze_format(&blocks);
        assert_eq!(got.shape_of("x"), VarShape::Rows);
        assert_eq!(got.shape_of("a"), VarShape::Jagged);
        assert_eq!(got.first_test_cases_count.as_deref(), Some("t"));
        assert_eq!(
            got.jagged_len_to_count.get("l").map(String::as_str),
            Some("n")
        );
        assert!(got.size_exprs.contains("t"));
        assert!(got.size_exprs.contains("n"));
    }
}
