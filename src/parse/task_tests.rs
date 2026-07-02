//! Integration tests for `task_to_format_blocks` using real task.html fixtures.
//!
//! Each test reads a contest's `task.html`, picks one task by letter, runs the
//! full pipeline, and asserts properties of the resulting `RandomTestSection`.
//!
//! Tests are skipped silently when the fixture file is not present (so this
//! module remains compilable on machines without the contest directory).

use super::super::constraint_parse::{BoundExpr, ConstraintParse, StrSpec};
use super::super::types::{
    ArrayBlock, BoundRepr, FormatBlock, RandomTestSection, ScalarsBlock, TaskSection,
    VarConstraint, VarType,
};
use super::{
    apply_ordering_range_bounds, canonicalize_chars_array_dimensions,
    enrich_section_with_constraints, parse_task_sections, task_to_format_blocks,
};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

fn lit_range(lo: i64, hi: i64) -> Option<[BoundRepr; 2]> {
    Some([BoundRepr::Lit(lo), BoundRepr::Lit(hi)])
}

const CONTEST_BASE: &str = "/workspaces/atcoder-rust-devcontainer/src/contest";

fn load_section(contest: &str, letter: char) -> Option<RandomTestSection> {
    let path = format!("{}/{}/task.html", CONTEST_BASE, contest);
    if !Path::new(&path).exists() {
        return None;
    }
    let html = std::fs::read_to_string(&path).ok()?;
    let sections = parse_task_sections(&html);
    let upper = letter.to_ascii_uppercase().to_string();
    let task = sections.iter().find(|s| s.letter == upper)?;
    Some(task_to_format_blocks(task))
}

#[test]
fn constraint_pair_filters_match_random_generation_scope() {
    let mut parsed = ConstraintParse::default();
    parsed.var_le = HashSet::from([
        ("n".to_string(), "m".to_string()),
        ("m".to_string(), "p".to_string()),
        ("s".to_string(), "t".to_string()),
        ("n".to_string(), "s".to_string()),
    ]);
    parsed.var_ne = HashSet::from([
        ("n".to_string(), "m".to_string()),
        ("s".to_string(), "t".to_string()),
        ("n".to_string(), "s".to_string()),
    ]);

    let mut vars = BTreeMap::new();
    vars.insert(
        "n".to_string(),
        VarConstraint {
            r#type: VarType::Usize,
            ..Default::default()
        },
    );
    vars.insert(
        "m".to_string(),
        VarConstraint {
            r#type: VarType::I64,
            ..Default::default()
        },
    );
    vars.insert(
        "p".to_string(),
        VarConstraint {
            r#type: VarType::Usize,
            ..Default::default()
        },
    );
    vars.insert(
        "s".to_string(),
        VarConstraint {
            r#type: VarType::Chars,
            ..Default::default()
        },
    );
    vars.insert(
        "t".to_string(),
        VarConstraint {
            r#type: VarType::Chars,
            ..Default::default()
        },
    );
    let mut section = RandomTestSection {
        vars,
        format: vec![],
        ..Default::default()
    };

    enrich_section_with_constraints(&parsed, &mut section);

    assert_eq!(
        section.ordering,
        vec![
            ["m".to_string(), "p".to_string()],
            ["n".to_string(), "m".to_string()],
            ["n".to_string(), "p".to_string()],
        ]
    );
    assert_eq!(
        section.not_equal,
        vec![
            ["n".to_string(), "m".to_string()],
            ["s".to_string(), "t".to_string()],
        ]
    );
}

#[test]
fn constraint_pair_shape_filters_match_requirements() {
    let mut parsed = ConstraintParse::default();
    parsed.var_le = HashSet::from([
        ("a".to_string(), "b".to_string()),
        ("j".to_string(), "b".to_string()),
    ]);
    parsed.var_ne = HashSet::from([
        ("a".to_string(), "b".to_string()),
        ("j".to_string(), "b".to_string()),
        ("s".to_string(), "t".to_string()),
        ("u".to_string(), "v".to_string()),
    ]);

    let mut vars = BTreeMap::new();
    for name in ["a", "b", "j"] {
        vars.insert(
            name.to_string(),
            VarConstraint {
                r#type: VarType::Usize,
                ..Default::default()
            },
        );
    }
    for name in ["s", "t", "u", "v"] {
        vars.insert(
            name.to_string(),
            VarConstraint {
                r#type: VarType::Chars,
                ..Default::default()
            },
        );
    }
    let mut section = RandomTestSection {
        vars,
        format: vec![
            FormatBlock::Scalars(ScalarsBlock {
                vars: vec!["s".into(), "t".into()],
            }),
            FormatBlock::Array(ArrayBlock {
                base: "a".into(),
                len: Some("n".into()),
                height: None,
                count: None,
                jagged: false,
            }),
            FormatBlock::Array(ArrayBlock {
                base: "b".into(),
                len: Some("n".into()),
                height: None,
                count: None,
                jagged: false,
            }),
            FormatBlock::Array(ArrayBlock {
                base: "j".into(),
                len: Some("l".into()),
                height: None,
                count: Some("n".into()),
                jagged: true,
            }),
            FormatBlock::Array(ArrayBlock {
                base: "u".into(),
                len: None,
                height: None,
                count: Some("n".into()),
                jagged: false,
            }),
            FormatBlock::Array(ArrayBlock {
                base: "v".into(),
                len: None,
                height: None,
                count: Some("n".into()),
                jagged: false,
            }),
        ],
        ..Default::default()
    };

    enrich_section_with_constraints(&parsed, &mut section);

    assert_eq!(section.ordering, vec![["a".to_string(), "b".to_string()]]);
    assert_eq!(
        section.not_equal,
        vec![
            ["a".to_string(), "b".to_string()],
            ["s".to_string(), "t".to_string()],
        ]
    );
}

#[test]
fn not_equal_only_index_names_are_not_emitted_as_vars_or_pairs() {
    let task = TaskSection {
        letter: "F".into(),
        input_blocks: vec![vec![
            "N".into(),
            "X_1 Y_1".into(),
            r"\vdots".into(),
            "X_N Y_N".into(),
        ]],
        constraints_items: vec![
            r"1 \leq N \leq 6 \times 10^4".into(),
            r"0 \leq X_i \leq 2 \times 10^7".into(),
            r"0 \leq Y_i \leq 2 \times 10^7".into(),
            r"i \neq j ならば (X_i,Y_i) \neq (X_j,Y_j)".into(),
        ],
    };

    let section = task_to_format_blocks(&task);
    assert!(section.vars.contains_key("n"));
    assert!(section.vars.contains_key("x"));
    assert!(section.vars.contains_key("y"));
    assert!(!section.vars.contains_key("i"));
    assert!(!section.vars.contains_key("j"));
    assert!(!section.not_equal.contains(&["i".into(), "j".into()]));
}

#[test]
fn not_equal_before_later_ranges_is_still_emitted() {
    let task = TaskSection {
        letter: "A".into(),
        input_blocks: vec![vec![
            "N".into(),
            "A_1 B_1".into(),
            r"\vdots".into(),
            "A_N B_N".into(),
        ]],
        constraints_items: vec![
            r"A_i \neq B_i".into(),
            r"1 \leq N \leq 10".into(),
            r"0 \leq A_i,B_i \leq N".into(),
        ],
    };

    let section = task_to_format_blocks(&task);
    assert!(section.vars.contains_key("a"));
    assert!(section.vars.contains_key("b"));
    assert!(section.not_equal.contains(&["a".into(), "b".into()]));
    assert!(section.ordering.contains(&["a".into(), "n".into()]));
    assert!(section.ordering.contains(&["b".into(), "n".into()]));
}

#[test]
fn abc448_f_does_not_emit_predicate_index_vars() {
    let Some(section) = load_section("abc448", 'f') else {
        return;
    };

    assert!(section.vars.contains_key("n"));
    assert!(section.vars.contains_key("x"));
    assert!(section.vars.contains_key("y"));
    assert!(!section.vars.contains_key("i"));
    assert!(!section.vars.contains_key("j"));
    assert!(!section.not_equal.contains(&["i".into(), "j".into()]));
}

#[test]
fn abc455_f_ordering_closure_reaches_n() {
    let Some(section) = load_section("abc455", 'f') else {
        return;
    };

    assert!(section.ordering.contains(&["l".into(), "r".into()]));
    assert!(section.ordering.contains(&["r".into(), "n".into()]));
    assert!(section.ordering.contains(&["l".into(), "n".into()]));
}

#[test]
fn ordering_ranges_are_materialized_from_persisted_pairs() {
    let mut section = RandomTestSection {
        vars: BTreeMap::from([
            (
                "a".into(),
                VarConstraint {
                    r#type: VarType::Usize,
                    range: lit_range(1, 1_000),
                    ..Default::default()
                },
            ),
            (
                "b".into(),
                VarConstraint {
                    r#type: VarType::Usize,
                    range: lit_range(0, 500),
                    ..Default::default()
                },
            ),
            (
                "c".into(),
                VarConstraint {
                    r#type: VarType::Usize,
                    range: lit_range(0, 100),
                    ..Default::default()
                },
            ),
        ]),
        ordering: vec![["a".into(), "b".into()], ["b".into(), "c".into()]],
        ..Default::default()
    };

    apply_ordering_range_bounds(&mut section);

    assert_eq!(section.vars["a"].range, lit_range(1, 100));
    assert_eq!(section.vars["b"].range, lit_range(1, 100));
    assert_eq!(section.vars["c"].range, lit_range(1, 100));
}

#[test]
fn abc431_c_min_operands_materialize_k_upper_bound() {
    let Some(section) = load_section("abc431", 'c') else {
        return;
    };

    assert!(section.ordering.contains(&["k".into(), "m".into()]));
    assert!(section.ordering.contains(&["k".into(), "n".into()]));
    assert_eq!(section.vars["k"].range, lit_range(1, 200_000));
}

// ─── abc450/a: simple literal range ───────────────────────────────────────────

#[test]
fn abc450_a_simple_range() {
    let Some(section) = load_section("abc450", 'a') else {
        return;
    };
    let n = section.vars.get("n").expect("n");
    assert_eq!(n.r#type, VarType::Usize);
    assert_eq!(n.range, lit_range(1, 9));
}

#[test]
fn abc461_d_multi_var_natural_range() {
    let Some(section) = load_section("abc461", 'd') else {
        return;
    };
    assert_eq!(section.vars["h"].range, lit_range(1, 500));
    assert_eq!(section.vars["w"].range, lit_range(1, 500));
}

// ─── abc442/e: range + i64 + var dep + != + ordering ──────────────────────────

#[test]
fn abc442_e_full_constraints() {
    let Some(section) = load_section("abc442", 'e') else {
        return;
    };

    // Numeric ranges, including i64 negative bounds
    let n = section.vars.get("n").expect("n");
    assert_eq!(n.r#type, VarType::Usize);
    assert_eq!(n.range, lit_range(2, 200_000));

    let q = section.vars.get("q").expect("q");
    assert_eq!(q.range, lit_range(1, 200_000));

    let x = section.vars.get("x").expect("x");
    assert_eq!(x.r#type, VarType::I64);
    assert_eq!(x.range, lit_range(-1_000_000_000, 1_000_000_000));

    let a = section.vars.get("a").expect("a");
    assert_eq!(a.r#type, VarType::Usize);
    // A_j ≤ N → range hi resolves to N's max literal
    assert_eq!(a.range, lit_range(1, 200_000));

    // ordering covers a ≤ n and b ≤ n (a, b implicitly bounded by n)
    assert!(section.ordering.contains(&["a".into(), "n".into()]));
    assert!(section.ordering.contains(&["b".into(), "n".into()]));

    // not_equal: A_j ≠ B_j → registered as base pair (a, b)
    assert!(section.not_equal.contains(&["a".into(), "b".into()]));

    // (X_i, Y_i) ≠ (0, 0) is a tuple constraint we can't represent → skipped
    assert!(
        section
            .skipped
            .iter()
            .any(|s| s.contains("(X_i,Y_i)") || s.contains("(0,0)")),
        "expected tuple constraint in skipped, got: {:?}",
        section.skipped
    );
}

// ─── abc441/b: Chars + len direct reference ───────────────────────────────────

#[test]
fn abc441_b_chars_with_len_ref() {
    let Some(section) = load_section("abc441", 'b') else {
        return;
    };

    let s = section.vars.get("s").expect("s");
    assert_eq!(s.r#type, VarType::Chars);
    assert!(
        s.values.as_ref().is_some_and(|vs| vs.len() == 26),
        "s should have 26-char charset (a-z), got: {:?}",
        s.values
    );
    // |S| = N → len: n (direct reference)
    assert_eq!(s.len, Some(BoundRepr::Expr("n".into())));

    // format: s appears as a scalar (not array)
    let has_s_scalar = section
        .format
        .iter()
        .any(|b| matches!(b, FormatBlock::Scalars(sb) if sb.vars.contains(&"s".to_string())));
    assert!(
        has_s_scalar,
        "s should appear as a scalar in format, got: {:?}",
        section.format
    );
}

#[test]
fn pipe_wrapped_chars_length_var_is_lowercase_usize_with_sum_limit() {
    let mut parsed = ConstraintParse::default();
    parsed.str_vars.insert(
        "a".into(),
        StrSpec {
            charset: vec!['x'],
            len_lo: Some(BoundExpr::Lit(1)),
            len_hi: Some(BoundExpr::Lit(2_000_000)),
        },
    );
    parsed.sum_limits.insert("|a|".into(), 2_000_000);

    let mut vars = BTreeMap::new();
    vars.insert(
        "a".into(),
        VarConstraint {
            r#type: VarType::Chars,
            values: Some(vec!["x".into()]),
            ..Default::default()
        },
    );
    let mut section = RandomTestSection {
        vars,
        format: vec![FormatBlock::Scalars(ScalarsBlock {
            vars: vec!["a".into()],
        })],
        ..Default::default()
    };

    enrich_section_with_constraints(&parsed, &mut section);

    let a = section.vars.get("a").expect("a");
    assert_eq!(a.len, Some(BoundRepr::Expr("|a|".into())));

    let len = section.vars.get("|a|").expect("|a|");
    assert_eq!(len.r#type, VarType::Usize);
    assert_eq!(len.sum_limit, Some(2_000_000));
    assert_eq!(len.range, lit_range(1, 2_000_000));
}

// ─── Existing format unchanged: abc450/a generates a simple scalar ────────────

#[test]
fn abc450_a_format_unchanged() {
    let Some(section) = load_section("abc450", 'a') else {
        return;
    };
    // Format should be a single scalars block with [n]
    assert_eq!(section.format.len(), 1);
    match &section.format[0] {
        FormatBlock::Scalars(sb) => assert_eq!(sb.vars, vec!["n".to_string()]),
        other => panic!("expected scalars, got {:?}", other),
    }
}

// ─── abc442/b: enum constraint produces values without range ──────────────────

#[test]
fn abc442_b_enum_no_range() {
    let Some(section) = load_section("abc442", 'b') else {
        return;
    };
    let a = section.vars.get("a").expect("a");
    assert_eq!(a.r#type, VarType::Usize);
    assert!(
        a.values.is_some(),
        "a should have values from enum constraint"
    );
    assert!(
        a.range.is_none(),
        "a should NOT have range when values is set, got: {:?}",
        a.range
    );
}

// ─── abc442/d: Japanese-prefixed inequality + Lit-over-Var preservation ───────

#[test]
fn abc442_d_japanese_prefix_and_lit_preservation() {
    let Some(section) = load_section("abc442", 'd') else {
        return;
    };
    // n.range must NOT be overwritten by `1 ≤ l ≤ r ≤ N` chain.
    let n = section.vars.get("n").expect("n");
    assert_eq!(n.range, lit_range(2, 200_000));

    // x's range comes from `1 ≤ x ≤ N-1` (after Japanese prefix stripping).
    let x = section.vars.get("x").expect("x");
    assert_eq!(x.range, lit_range(1, 199_999));

    // l's lo from the chain prefix `1`.
    let l = section.vars.get("l").expect("l");
    assert_eq!(
        l.range.as_ref().map(|r| r[0].clone()),
        Some(BoundRepr::Lit(1))
    );

    // r resolves through `r ≤ N` to the literal upper bound.
    let r = section.vars.get("r").expect("r");
    assert_eq!(
        r.range.as_ref().map(|x| x[1].clone()),
        Some(BoundRepr::Lit(200_000))
    );
}

// ─── abc457/c: Jagged array with sum_limit ────────────────────────────────────

fn find_jagged_array(blocks: &[FormatBlock]) -> Option<&ArrayBlock> {
    for b in blocks {
        match b {
            FormatBlock::Array(ab) if ab.jagged => return Some(ab),
            FormatBlock::TestCases(tb) => {
                if let Some(ab) = find_jagged_array(&tb.format) {
                    return Some(ab);
                }
            }
            FormatBlock::Queries(qb) => {
                for qt in &qb.types {
                    if let Some(ab) = find_jagged_array(&qt.format) {
                        return Some(ab);
                    }
                }
            }
            _ => {}
        }
    }
    None
}

#[test]
fn abc457_c_jagged_with_sum_limit() {
    let Some(section) = load_section("abc457", 'c') else {
        return;
    };
    assert!(
        !section.vars.contains_key("i"),
        "aggregate constraint index must not become an input variable"
    );
    assert!(
        !section.ordering.contains(&["k".into(), "i".into()]),
        "aggregate constraint must not become scalar ordering"
    );
    let ab = find_jagged_array(&section.format).expect("jagged array block");
    assert!(ab.jagged, "jagged flag should be true");
    let l_var = ab.len.as_ref().expect("len variable (each row's length)");
    assert!(ab.count.is_some(), "count (number of rows) should be set");
    // The per-row length variable must exist in vars and carry sum_limit.
    let l = section
        .vars
        .get(l_var.as_str())
        .unwrap_or_else(|| panic!("var {} missing", l_var));
    assert!(
        l.sum_limit.is_some(),
        "expected sum_limit on jagged length var {}, got vars[{}]={:?}",
        l_var,
        l_var,
        l
    );
}

// ─── abc446/b: separate-line Jagged, no sum_limit ─────────────────────────────

#[test]
fn abc446_b_jagged_without_sum_limit() {
    let Some(section) = load_section("abc446", 'b') else {
        return;
    };
    let ab = find_jagged_array(&section.format).expect("jagged array block");
    assert!(ab.jagged);
    let l_var = ab.len.as_ref().expect("len variable");
    let l = section.vars.get(l_var.as_str()).expect("len var entry");
    assert!(
        l.sum_limit.is_none(),
        "abc446-b has no Σ constraint, so jagged length var must have no sum_limit, got: {:?}",
        l.sum_limit
    );
}

// ─── abc453/d: Chars 2D grid → vars[s].len = w ────────────────────────────────

#[test]
fn abc453_d_chars_grid_inner_len() {
    let Some(section) = load_section("abc453", 'd') else {
        return;
    };
    assert!(
        !section.vars.contains_key("code") && !section.vars.contains_key("g"),
        "HTML <code> tags must not create numeric vars, got vars: {:?}",
        section.vars.keys().collect::<Vec<_>>()
    );
    assert!(
        section
            .ordering
            .iter()
            .all(|pair| pair.iter().all(|v| v != "code" && v != "g")),
        "HTML <code> tags must not create ordering pairs, got: {:?}",
        section.ordering
    );
    let s = section.vars.get("s").expect("s var");
    assert_eq!(s.r#type, VarType::Chars);
    assert_eq!(s.len, Some(BoundRepr::Expr("w".into())));
    // The grid block itself must not carry the inner width any more
    // (post-migration ArrayBlock has no `width` field).
    let grid = section
        .format
        .iter()
        .find_map(|b| match b {
            FormatBlock::Array(ab) if ab.base == "s" && ab.count.is_some() && !ab.jagged => {
                Some(ab)
            }
            _ => None,
        })
        .expect("Chars grid array block");
    assert!(
        grid.len.is_none(),
        "Chars 2D grid block should NOT carry an inner len on the format side; got: {:?}",
        grid.len
    );
}

// ─── Chars indexed arrays: physical dimensions are type-mapped at assembly ───

#[test]
fn abc459_d_string_length_sum_limit_is_attached_to_synthetic_len() {
    let Some(section) = load_section("abc459", 'd') else {
        return;
    };

    assert_eq!(section.vars["s"].len, Some(BoundRepr::Expr("|s|".into())));
    assert_eq!(section.vars["s"].sum_limit, None);
    assert_eq!(section.vars["|s|"].range, lit_range(1, 1_000_000));
    assert_eq!(section.vars["|s|"].sum_limit, Some(1_000_000));
}

#[test]
fn abc459_b_horizontal_chars_array_uses_count_and_keeps_string_len() {
    let Some(section) = load_section("abc459", 'b') else {
        return;
    };
    let s = section.vars.get("s").expect("s var");
    assert_eq!(s.r#type, VarType::Chars);
    assert_eq!(s.len, Some(BoundRepr::Expr("|s|".into())));
    let array = section
        .format
        .iter()
        .find_map(|block| match block {
            FormatBlock::Array(array) if array.base == "s" => Some(array),
            _ => None,
        })
        .expect("s array block");
    assert_eq!(array.len, None);
    assert_eq!(array.height, None);
    assert_eq!(array.count.as_deref(), Some("n"));
}

#[test]
fn abc440_g_chars_3d_inner_len_comes_from_format_width() {
    let Some(section) = load_section("abc440", 'g') else {
        return;
    };
    let s = section.vars.get("s").expect("s var");
    assert_eq!(s.r#type, VarType::Chars);
    assert_eq!(s.len, Some(BoundRepr::Expr("w".into())));
    let array = section
        .format
        .iter()
        .find_map(|block| match block {
            FormatBlock::Array(array) if array.base == "s" => Some(array),
            _ => None,
        })
        .expect("s array block");
    assert_eq!(array.len, None);
    assert_eq!(array.height.as_deref(), Some("h"));
    assert_eq!(array.count.as_deref(), Some("f"));
}

#[test]
fn chars_dimension_mapping_depends_on_type_not_base_name() {
    let mut vars = BTreeMap::from([(
        "word".to_string(),
        VarConstraint {
            r#type: VarType::Chars,
            len: Some(BoundRepr::Expr("1".into())),
            ..Default::default()
        },
    )]);
    let mut blocks = vec![FormatBlock::Array(ArrayBlock {
        base: "word".into(),
        len: Some("w".into()),
        height: None,
        count: Some("h".into()),
        jagged: false,
    })];

    canonicalize_chars_array_dimensions(&mut blocks, &mut vars);

    assert_eq!(vars["word"].len, Some(BoundRepr::Expr("w".into())));
    match &blocks[0] {
        FormatBlock::Array(array) => {
            assert_eq!(array.len, None);
            assert_eq!(array.count.as_deref(), Some("h"));
        }
        other => panic!("expected array, got {:?}", other),
    }
}

// ─── abc449/d: chain-propagated i64 (R/U via Var lower bound) ─────────────────

#[test]
fn abc449_d_chain_i64() {
    let Some(section) = load_section("abc449", 'd') else {
        return;
    };
    // Constraints: `-10^6 <= L <= R <= 10^6`, `-10^6 <= D <= U <= 10^6`.
    // L/D have a literal-negative lo; R/U only get a `Var` lo that resolves
    // transitively to -10^6. All four must end up I64 with the same range.
    for v in ["l", "r", "d", "u"] {
        let c = section.vars.get(v).unwrap_or_else(|| panic!("{}", v));
        assert_eq!(c.r#type, VarType::I64, "{v} type");
        assert_eq!(c.range, lit_range(-1_000_000, 1_000_000), "{v} range");
    }
}

// ─── Step 9: exact sum_limit-derived count upper bound ────────────────────────

mod sum_count_bounds {
    use super::super::super::types::{
        ArrayBlock, BoundRepr, FormatBlock, RandomTestSection, ScalarsBlock, TestCasesBlock,
        VarConstraint,
    };
    use std::collections::BTreeMap;

    /// `[Lit(lo), hi]` where `hi=None` ⇒ unresolved placeholder `_`.
    fn vc(lo: i64, hi: Option<i64>, sum: Option<i64>) -> VarConstraint {
        let hi = hi
            .map(BoundRepr::Lit)
            .unwrap_or(BoundRepr::Expr("_".into()));
        VarConstraint {
            range: Some([BoundRepr::Lit(lo), hi]),
            sum_limit: sum,
            ..Default::default()
        }
    }

    fn sect(vars: Vec<(&str, VarConstraint)>, format: Vec<FormatBlock>) -> RandomTestSection {
        let vars: BTreeMap<String, VarConstraint> =
            vars.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        RandomTestSection {
            vars,
            format,
            ..Default::default()
        }
    }

    fn scalars(vs: &[&str]) -> FormatBlock {
        FormatBlock::Scalars(ScalarsBlock {
            vars: vs.iter().map(|s| s.to_string()).collect(),
        })
    }

    fn hi_of(s: &RandomTestSection, name: &str) -> BoundRepr {
        s.vars.get(name).unwrap().range.as_ref().unwrap()[1].clone()
    }

    // 1. F-Centipede shape: `q` test cases, `3 ≤ n`, `Σ n ≤ 200000`,
    //    q upper bound absent → q.hi = floor(200000 / 3) = 66666.
    #[test]
    fn testcases_count_derived_from_sum_limit() {
        let mut s = sect(
            vec![
                ("q", vc(1, None, None)),
                ("n", vc(3, Some(200_000), Some(200_000))),
            ],
            vec![FormatBlock::TestCases(TestCasesBlock {
                count: "q".into(),
                format: vec![scalars(&["n"])],
            })],
        );
        super::super::apply_sum_count_bounds(&mut s);
        assert_eq!(hi_of(&s, "q"), BoundRepr::Lit(66_666));
    }

    // 2. Jagged: rows count `m` absent, per-row len `k` lo=1, Σk ≤ 5000;
    //    m.range was None entirely → becomes [Lit(0), Lit(5000)].
    #[test]
    fn jagged_count_derived_and_range_created() {
        let mut k = vc(1, None, Some(5000));
        k.range = Some([BoundRepr::Lit(1), BoundRepr::Expr("_".into())]);
        let mut m = VarConstraint::default(); // range: None
        m.range = None;
        let mut s = sect(
            vec![("m", m), ("k", k)],
            vec![FormatBlock::Array(ArrayBlock {
                base: "a".into(),
                len: Some("k".into()),
                height: None,
                count: Some("m".into()),
                jagged: true,
            })],
        );
        super::super::apply_sum_count_bounds(&mut s);
        assert_eq!(
            s.vars.get("m").unwrap().range,
            Some([BoundRepr::Lit(0), BoundRepr::Lit(5000)])
        );
    }

    // 3. An explicit upper bound stated in the HTML is never overridden.
    #[test]
    fn explicit_upper_bound_preserved() {
        let mut s = sect(
            vec![
                ("q", vc(1, Some(50), None)),
                ("n", vc(3, Some(200_000), Some(200_000))),
            ],
            vec![FormatBlock::TestCases(TestCasesBlock {
                count: "q".into(),
                format: vec![scalars(&["n"])],
            })],
        );
        super::super::apply_sum_count_bounds(&mut s);
        assert_eq!(hi_of(&s, "q"), BoundRepr::Lit(50)); // unchanged
    }

    // 4. Size var lower bound is 0 (usize default, no stated `≥1`) → condition
    //    unmet, q stays a placeholder.
    #[test]
    fn size_var_lo_zero_not_derived() {
        let mut s = sect(
            vec![
                ("q", vc(1, None, None)),
                ("n", vc(0, Some(200_000), Some(200_000))),
            ],
            vec![FormatBlock::TestCases(TestCasesBlock {
                count: "q".into(),
                format: vec![scalars(&["n"])],
            })],
        );
        super::super::apply_sum_count_bounds(&mut s);
        assert_eq!(hi_of(&s, "q"), BoundRepr::Expr("_".into()));
    }

    // 5. Two sum-limited vars share one count → tightest (min) wins.
    #[test]
    fn multiple_size_vars_take_min() {
        let mut s = sect(
            vec![
                ("q", vc(1, None, None)),
                ("n", vc(10, None, Some(1_000_000_000))), // floor=1e8
                ("p", vc(2, None, Some(200_000))),        // floor=1e5
            ],
            vec![FormatBlock::TestCases(TestCasesBlock {
                count: "q".into(),
                format: vec![scalars(&["n", "p"])],
            })],
        );
        super::super::apply_sum_count_bounds(&mut s);
        assert_eq!(hi_of(&s, "q"), BoundRepr::Lit(100_000));
    }

    // 6. L < lo ⇒ floor = 0 (contradictory): leave placeholder, write nothing.
    #[test]
    fn contradictory_leaves_placeholder() {
        let mut s = sect(
            vec![("q", vc(1, None, None)), ("n", vc(10, None, Some(5)))],
            vec![FormatBlock::TestCases(TestCasesBlock {
                count: "q".into(),
                format: vec![scalars(&["n"])],
            })],
        );
        super::super::apply_sum_count_bounds(&mut s);
        assert_eq!(hi_of(&s, "q"), BoundRepr::Expr("_".into()));
    }
}

// ─── Round-trip: serialize then deserialize ───────────────────────────────────

#[test]
fn serde_roundtrip_abc442_e() {
    let Some(section) = load_section("abc442", 'e') else {
        return;
    };
    let yaml = serde_yaml::to_string(&section).expect("serialize");
    let restored: RandomTestSection = serde_yaml::from_str(&yaml).expect("deserialize");
    assert_eq!(restored.vars.len(), section.vars.len());
    assert_eq!(restored.ordering, section.ordering);
    assert_eq!(restored.not_equal, section.not_equal);
}

#[test]
fn abc453_g_query_operands_use_base_variables() {
    let Some(section) = load_section("abc453", 'g') else {
        return;
    };

    for name in ["x_i", "y_i", "z_i", "l_i", "r_i"] {
        assert!(
            !section.vars.contains_key(name),
            "query operand index name must not be emitted: {}",
            name
        );
    }
    for name in ["x", "y", "z", "l", "r"] {
        assert!(
            section.vars.contains_key(name),
            "missing base variable: {}",
            name
        );
    }

    let queries = section
        .format
        .iter()
        .find_map(|block| match block {
            FormatBlock::Queries(q) => Some(q),
            _ => None,
        })
        .expect("queries block");
    let operands: Vec<Vec<String>> = queries
        .types
        .iter()
        .map(|branch| match branch.format.first() {
            Some(FormatBlock::Scalars(s)) => s.vars.clone(),
            _ => Vec::new(),
        })
        .collect();
    assert_eq!(
        operands,
        vec![
            vec!["x".to_string(), "y".to_string()],
            vec!["x".to_string(), "y".to_string(), "z".to_string()],
            vec!["x".to_string(), "l".to_string(), "r".to_string()],
        ]
    );
}
