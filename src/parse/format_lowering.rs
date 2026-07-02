//! Type-directed lowering from physical indexed dimensions to persisted yml fields.

use super::types::{BoundRepr, FormatBlock, VarConstraint, VarType};
use std::collections::BTreeMap;

pub(crate) fn lower_typed_format_dimensions(
    blocks: &mut [FormatBlock],
    vars: &mut BTreeMap<String, VarConstraint>,
) {
    for block in blocks {
        match block {
            FormatBlock::Array(arr) if !arr.jagged => {
                if !vars
                    .get(&arr.base)
                    .is_some_and(|var| var.r#type == VarType::Chars)
                {
                    continue;
                }
                if arr.height.is_none() && arr.count.is_none() {
                    arr.count = arr.len.take();
                } else if arr.count.is_some() {
                    if let Some(width) = arr.len.take() {
                        vars.get_mut(&arr.base).expect("Chars base exists").len =
                            Some(BoundRepr::Expr(width));
                    }
                }
            }
            FormatBlock::TestCases(tc) => lower_typed_format_dimensions(&mut tc.format, vars),
            FormatBlock::Queries(q) => {
                for branch in &mut q.types {
                    lower_typed_format_dimensions(&mut branch.format, vars);
                }
            }
            _ => {}
        }
    }
}
