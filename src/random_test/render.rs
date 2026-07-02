//! `FormatBlock` walker that renders one random-test input case.
//!
//! Each case is a single [`CaseStrategy`] applied over a fresh context. The
//! walker drives the pure generation helpers in [`super::gen`]; structural sizes
//! (`T`, jagged row counts) are decided once per case and seeded into the
//! context so the count line printed by the header `Scalars` block stays
//! consistent with the number of body iterations.
//!
//! `ordering` / `not_equal` are enforced against the values available while
//! walking each block. Parent values narrow nested values, and completed
//! `TestCases` / `Queries` iterations are also checked before retention.

use super::budget::{Budget, GenError};
use super::context::{ContextCheckpoint, RenderContext};
use super::emitter::{
    constrained_scalar_value, gen_chars, render_chars_array, render_int_array, render_jagged,
    render_rows, resolve_count, RenderEnv,
};
use super::gen::{decide_structural_sizes, StructuralSizes};
use super::relation::pairs_ok;
use super::spec::ResolvedSpec;
#[cfg(test)]
use super::strategy::DeterministicStrategy;
use super::strategy::{CaseStrategy, RandomStrategy};
use super::MAX_INPUT_ELEMENTS;
use crate::parse::{FormatBlock, VarType};
use rand::Rng;

/// Per-iteration resample budget before the whole case is regenerated.
const INNER_BUDGET: u32 = 20;
/// Whole-case regeneration budget for corner strategies.
const CORNER_BUDGET: u32 = 20;
/// Whole-case regeneration budget for plain random strategies.
const RANDOM_BUDGET: u32 = 100;

pub(crate) enum RenderResult {
    Ready(String),
    Unsatisfied,
    /// Abort this problem's random test with an English reason: the case grew
    /// past the input-size safety ceiling, or plain `Random` exhausted its
    /// retry budget without satisfying the constraints.
    Abort(String),
    Interrupted,
}

impl RenderResult {
    #[cfg(test)]
    fn unwrap(self) -> String {
        match self {
            Self::Ready(input) => input,
            Self::Unsatisfied => panic!("called `RenderResult::unwrap()` on Unsatisfied"),
            Self::Abort(reason) => {
                panic!("called `RenderResult::unwrap()` on Abort: {}", reason)
            }
            Self::Interrupted => panic!("called `RenderResult::unwrap()` on Interrupted"),
        }
    }

    #[cfg(test)]
    fn is_none(&self) -> bool {
        matches!(self, Self::Unsatisfied)
    }
}

/// Render one input case for `st`. Returns `None` for a corner strategy that
/// cannot satisfy the ordering / not_equal constraints (top-level or
/// per-iteration) within [`CORNER_BUDGET`] whole-case retries. Plain `Random`
/// also has a finite retry budget and returns an abort reason when exhausted
/// so impossible or near-impossible constraints cannot loop forever.
pub(crate) fn render_case(
    spec: &ResolvedSpec,
    st: &CaseStrategy,
    rng: &mut impl Rng,
) -> RenderResult {
    let is_corner = !matches!(st, CaseStrategy::Random(RandomStrategy::Random));
    let mut retries = 0u32;
    loop {
        if crate::interrupt::requested() {
            return RenderResult::Interrupted;
        }
        let sizes = decide_structural_sizes(spec, st, rng);
        let mut context = RenderContext::default();
        if let Some((name, t)) = &sizes.test_cases {
            context.scalars.insert(name.clone(), *t);
        }
        for (cv, n) in &sizes.jagged_counts {
            context.scalars.insert(cv.clone(), *n);
        }
        let mut lines: Vec<String> = Vec::new();
        let mut budget = Budget::new(MAX_INPUT_ELEMENTS);
        let ok = match walk(
            &spec.format,
            spec,
            st,
            &sizes,
            &mut context,
            &mut lines,
            &mut budget,
            rng,
        ) {
            Ok(ok) => ok,
            Err(GenError::Interrupted) => return RenderResult::Interrupted,
            Err(GenError::Oversize(reason)) => return RenderResult::Abort(reason),
        };
        if ok
            && pairs_ok(
                spec,
                &context.scalars,
                &context.strings,
                &context.arrays,
                None,
                None,
            )
        {
            if crate::interrupt::requested() {
                return RenderResult::Interrupted;
            }
            return RenderResult::Ready(lines.join("\n") + "\n");
        }
        retries += 1;
        if is_corner && retries >= CORNER_BUDGET {
            return RenderResult::Unsatisfied;
        }
        if !is_corner && retries >= RANDOM_BUDGET {
            return RenderResult::Abort(format!(
                "random test constraints could not be satisfied after {} attempts; \
                 check ordering/not_equal constraints and declared ranges",
                RANDOM_BUDGET
            ));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn walk(
    blocks: &[FormatBlock],
    spec: &ResolvedSpec,
    st: &CaseStrategy,
    sizes: &StructuralSizes,
    context: &mut RenderContext,
    lines: &mut Vec<String>,
    budget: &mut Budget,
    rng: &mut impl Rng,
) -> Result<bool, GenError> {
    for block in blocks {
        if crate::interrupt::requested() {
            return Err(GenError::Interrupted);
        }
        match block {
            FormatBlock::Scalars(b) => {
                let mut parts: Vec<String> = Vec::with_capacity(b.vars.len());
                for v in &b.vars {
                    if let Some(&val) = context.scalars.get(v) {
                        // Seeded structural size or earlier scalar: reuse so the
                        // printed value matches its later use (Chars vars are
                        // never inserted into scalars, so this is always integer).
                        budget.add(1)?;
                        parts.push(val.to_string());
                        continue;
                    }
                    let info = spec
                        .vars
                        .get(v)
                        .expect("format variables are validated at resolve time");
                    if info.ty == VarType::Chars {
                        let env = RenderEnv { spec, st, sizes };
                        let Some(val) = gen_chars(info, &env, &mut context.scalars, budget, rng)?
                        else {
                            return Ok(false);
                        };
                        context.strings.insert(v.clone(), val.clone());
                        parts.push(val);
                    } else {
                        let Some(val) = constrained_scalar_value(
                            spec,
                            v,
                            info,
                            sizes,
                            st,
                            &context.scalars,
                            &context.arrays,
                            rng,
                        ) else {
                            return Ok(false);
                        };
                        context.scalars.insert(v.clone(), val);
                        budget.add(1)?;
                        parts.push(val.to_string());
                    }
                }
                lines.push(parts.join(" "));
            }
            FormatBlock::Array(a) => {
                let ok = if a.jagged {
                    render_jagged(
                        a,
                        spec,
                        st,
                        sizes,
                        &mut context.scalars,
                        &mut context.arrays,
                        lines,
                        budget,
                        rng,
                    )?
                } else if spec
                    .vars
                    .get(&a.base)
                    .map(|i| i.ty == VarType::Chars)
                    .unwrap_or(false)
                {
                    render_chars_array(
                        a,
                        spec,
                        st,
                        sizes,
                        &mut context.scalars,
                        lines,
                        budget,
                        rng,
                    )?
                } else {
                    render_int_array(
                        a,
                        spec,
                        st,
                        sizes,
                        &mut context.scalars,
                        &mut context.arrays,
                        lines,
                        budget,
                        rng,
                    )?
                };
                if !ok {
                    return Ok(false);
                }
            }
            FormatBlock::Rows(b) => {
                if !render_rows(
                    b,
                    spec,
                    st,
                    sizes,
                    &mut context.scalars,
                    &mut context.arrays,
                    lines,
                    budget,
                    rng,
                )? {
                    return Ok(false);
                }
            }
            FormatBlock::TestCases(b) => {
                let Some(t) = resolve_count(&b.count, spec, sizes, st, &mut context.scalars, rng)
                else {
                    return Ok(false);
                };
                let checkpoint = context.checkpoint();
                for i in 0..t.max(0) {
                    if i % 1024 == 0 && crate::interrupt::requested() {
                        return Err(GenError::Interrupted);
                    }
                    if !run_iteration(
                        &b.format,
                        None,
                        spec,
                        st,
                        sizes,
                        context,
                        &checkpoint,
                        lines,
                        budget,
                        rng,
                    )? {
                        return Ok(false);
                    }
                }
            }
            FormatBlock::Queries(b) => {
                let Some(q) = resolve_count(&b.count, spec, sizes, st, &mut context.scalars, rng)
                else {
                    return Ok(false);
                };
                if b.types.is_empty() {
                    continue;
                }
                let checkpoint = context.checkpoint();
                for i in 0..q.max(0) {
                    if i % 1024 == 0 && crate::interrupt::requested() {
                        return Err(GenError::Interrupted);
                    }
                    let bi = rng.gen_range(0..b.types.len());
                    let branch = &b.types[bi];
                    if !run_iteration(
                        &branch.format,
                        Some(&branch.id),
                        spec,
                        st,
                        sizes,
                        context,
                        &checkpoint,
                        lines,
                        budget,
                        rng,
                    )? {
                        return Ok(false);
                    }
                }
            }
        }
    }
    Ok(true)
}

/// Run one `TestCases` / `Queries` iteration with scope-local rejection.
/// `id_token`, when set (Queries), is merged onto the first inner line.
/// Returns `false` if no attempt within [`INNER_BUDGET`] satisfied the
/// iteration-local constraints (caller regenerates the whole case).
#[allow(clippy::too_many_arguments)]
fn run_iteration(
    format: &[FormatBlock],
    id_token: Option<&str>,
    spec: &ResolvedSpec,
    st: &CaseStrategy,
    sizes: &StructuralSizes,
    context: &mut RenderContext,
    checkpoint: &ContextCheckpoint,
    lines: &mut Vec<String>,
    budget: &mut Budget,
    rng: &mut impl Rng,
) -> Result<bool, GenError> {
    for _ in 0..INNER_BUDGET {
        let mut tmp: Vec<String> = Vec::new();
        let mark = budget.used;
        let w = walk(
            format, spec, st, sizes, context, &mut tmp, budget, rng,
        )?;
        let accepted = w
            && pairs_ok(
                spec,
                &context.scalars,
                &context.strings,
                &context.arrays,
                Some(&checkpoint.scalar_names),
                Some(&checkpoint.array_names),
            );
        if accepted {
            match id_token {
                None => lines.append(&mut tmp),
                Some(id) => {
                    budget.add(1)?;
                    if tmp.is_empty() {
                        lines.push(id.to_string());
                    } else {
                        let first = tmp.remove(0);
                        if first.trim().is_empty() {
                            lines.push(id.to_string());
                        } else {
                            lines.push(format!("{} {}", id, first.trim()));
                        }
                        lines.append(&mut tmp);
                    }
                }
            }
        } else {
            budget.used = mark;
        }
        context.restore(checkpoint);
        if accepted {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::super::spec::resolve;
    use super::*;
    use crate::parse::{
        ArrayBlock, BoundRepr, FormatBlock, QueriesBlock, QueryBranch, RandomTestSection,
        RowsBlock, ScalarsBlock, TestCasesBlock, VarConstraint, VarType,
    };
    use rand::rngs::SmallRng;
    use rand::SeedableRng as _;
    use std::collections::{BTreeMap, HashMap, HashSet};

    fn rng() -> SmallRng {
        SmallRng::seed_from_u64(0xC0FFEE)
    }

    fn vc(lo: i64, hi: i64) -> VarConstraint {
        VarConstraint {
            r#type: VarType::Usize,
            range: Some([BoundRepr::Lit(lo), BoundRepr::Lit(hi)]),
            ..Default::default()
        }
    }

    fn vc_enum(values: &[&str]) -> VarConstraint {
        VarConstraint {
            r#type: VarType::Usize,
            values: Some(values.iter().map(|v| (*v).to_string()).collect()),
            ..Default::default()
        }
    }

    fn mkspec(vars: BTreeMap<String, VarConstraint>, format: Vec<FormatBlock>) -> ResolvedSpec {
        resolve(&RandomTestSection {
            vars,
            format,
            ..Default::default()
        })
    }

    fn scalars(vs: &[&str]) -> FormatBlock {
        FormatBlock::Scalars(ScalarsBlock {
            vars: vs.iter().map(|s| s.to_string()).collect(),
        })
    }

    fn lines_of(s: &str) -> Vec<&str> {
        s.lines().collect()
    }

    fn det_max() -> CaseStrategy {
        CaseStrategy::Deterministic(DeterministicStrategy::AllMax)
    }
    fn det_min() -> CaseStrategy {
        CaseStrategy::Deterministic(DeterministicStrategy::AllMin)
    }
    fn random() -> CaseStrategy {
        CaseStrategy::Random(RandomStrategy::Random)
    }

    #[test]
    fn array_len_integer_literal_resolves() {
        let mut v = BTreeMap::new();
        v.insert("a".into(), vc(1, 9));
        let spec = mkspec(
            v,
            vec![FormatBlock::Array(ArrayBlock {
                base: "a".into(),
                len: Some("3".into()),
                height: None,
                count: None,
                jagged: false,
            })],
        );
        let out = render_case(&spec, &det_max(), &mut rng()).unwrap();
        assert_eq!(lines_of(&out)[0].split_whitespace().count(), 3);
    }

    #[test]
    fn array_len_var_plus_offset_resolves() {
        // abc453-b shape: array len `(t)+1`.
        let mut v = BTreeMap::new();
        v.insert("t".into(), vc(1, 100));
        v.insert("x".into(), vc(1, 100));
        v.insert("a".into(), vc(0, 100));
        let spec = mkspec(
            v,
            vec![
                scalars(&["t", "x"]),
                FormatBlock::Array(ArrayBlock {
                    base: "a".into(),
                    len: Some("(t)+1".into()),
                    height: None,
                    count: None,
                    jagged: false,
                }),
            ],
        );
        let out = render_case(&spec, &det_max(), &mut rng()).unwrap();
        let ls = lines_of(&out);
        assert_eq!(ls[0], "100 100");
        assert_eq!(ls[1].split_whitespace().count(), 101);
    }

    #[test]
    fn rows_len_var_minus_offset_resolves_and_consistent() {
        // abc448-d shape: array len `n`, rows len `n-1`. Same `n` must drive
        // both (consistency) and `n-1` must resolve, not collapse to 0.
        let mut v = BTreeMap::new();
        v.insert("n".into(), vc(2, 5));
        v.insert("a".into(), vc(1, 9));
        v.insert("u".into(), vc(1, 5));
        v.insert("v".into(), vc(1, 5));
        let spec = mkspec(
            v,
            vec![
                scalars(&["n"]),
                FormatBlock::Array(ArrayBlock {
                    base: "a".into(),
                    len: Some("n".into()),
                    height: None,
                    count: None,
                    jagged: false,
                }),
                FormatBlock::Rows(RowsBlock {
                    vars: vec!["u".into(), "v".into()],
                    len: "n-1".into(),
                }),
            ],
        );
        let out = render_case(&spec, &det_max(), &mut rng()).unwrap();
        let ls = lines_of(&out);
        assert_eq!(ls[0], "5");
        assert_eq!(ls[1].split_whitespace().count(), 5);
        assert_eq!(ls.len(), 2 + 4);
        for r in &ls[2..] {
            assert_eq!(r.split_whitespace().count(), 2);
        }
    }

    #[test]
    fn rows_numeric_enum_values_are_preserved() {
        // abc440-f shape: enum columns inside Rows must use `values`, not the
        // placeholder numeric range `[0,0]` from spec resolution.
        let mut v = BTreeMap::new();
        v.insert("n".into(), vc(4, 4));
        v.insert("a".into(), vc(1, 100));
        v.insert("b".into(), vc_enum(&["1", "2"]));
        let spec = mkspec(
            v,
            vec![
                scalars(&["n"]),
                FormatBlock::Rows(RowsBlock {
                    vars: vec!["a".into(), "b".into()],
                    len: "n".into(),
                }),
            ],
        );
        let out = render_case(
            &spec,
            &CaseStrategy::Random(RandomStrategy::ArrayAltMaxMin),
            &mut rng(),
        )
        .unwrap();
        let ls = lines_of(&out);
        assert_eq!(ls[0], "4");
        for row in &ls[1..] {
            let vals: Vec<&str> = row.split_whitespace().collect();
            assert_eq!(vals.len(), 2);
            assert!(
                vals[1] == "1" || vals[1] == "2",
                "enum column escaped values: {}",
                row
            );
        }
    }

    #[test]
    fn int_array_numeric_enum_values_are_preserved() {
        let mut v = BTreeMap::new();
        v.insert("a".into(), vc_enum(&["1", "2"]));
        let spec = mkspec(
            v,
            vec![FormatBlock::Array(ArrayBlock {
                base: "a".into(),
                len: Some("8".into()),
                height: None,
                count: None,
                jagged: false,
            })],
        );
        let out = render_case(&spec, &det_max(), &mut rng()).unwrap();
        assert!(out.split_whitespace().all(|x| x == "2"), "{}", out);
    }

    #[test]
    fn scalar_enum_without_range_renders_domain_values() {
        // Regression: enum scalars without `range` used to keep the (0, 0)
        // placeholder bounds, which filtered the whole domain out and made
        // every case unsatisfiable.
        let mut v = BTreeMap::new();
        v.insert("k".into(), vc_enum(&["3", "7"]));
        let spec = mkspec(v, vec![scalars(&["k"])]);
        assert!(spec.missing.is_empty(), "{:?}", spec.missing);

        let out = render_case(&spec, &det_max(), &mut rng()).unwrap();
        assert_eq!(lines_of(&out)[0], "7");
        let out = render_case(&spec, &det_min(), &mut rng()).unwrap();
        assert_eq!(lines_of(&out)[0], "3");
        for _ in 0..20 {
            let out = render_case(&spec, &random(), &mut rng()).unwrap();
            let val = lines_of(&out)[0].to_owned();
            assert!(val == "3" || val == "7", "{}", out);
        }
    }

    #[test]
    fn chars_scalar_string_literal_len_resolves() {
        let mut v = BTreeMap::new();
        v.insert(
            "s".into(),
            VarConstraint {
                r#type: VarType::Chars,
                values: Some(vec!["a".into(), "b".into()]),
                len: Some(BoundRepr::Expr("4".into())),
                ..Default::default()
            },
        );
        let spec = mkspec(v, vec![scalars(&["s"])]);
        let out = render_case(&spec, &det_min(), &mut rng()).unwrap();
        assert_eq!(lines_of(&out)[0], "aaaa");
    }

    #[test]
    fn chars_scalar_offset_len_resolves() {
        let mut v = BTreeMap::new();
        v.insert("n".into(), vc(5, 5));
        v.insert(
            "s".into(),
            VarConstraint {
                r#type: VarType::Chars,
                values: Some(vec!["a".into()]),
                len: Some(BoundRepr::Expr("n-1".into())),
                ..Default::default()
            },
        );
        let spec = mkspec(v, vec![scalars(&["n"]), scalars(&["s"])]);
        let out = render_case(&spec, &det_max(), &mut rng()).unwrap();
        let ls = lines_of(&out);
        assert_eq!(ls[0], "5");
        assert_eq!(ls[1], "aaaa");
    }

    #[test]
    fn chars_scalar_oversize_aborts_before_allocating() {
        let mut v = BTreeMap::new();
        v.insert(
            "s".into(),
            VarConstraint {
                r#type: VarType::Chars,
                values: Some(vec!["a".to_owned()]),
                len: Some(BoundRepr::Lit((MAX_INPUT_ELEMENTS + 1) as i64)),
                ..Default::default()
            },
        );
        let spec = mkspec(v, vec![scalars(&["s"])]);
        match render_case(&spec, &det_max(), &mut rng()) {
            RenderResult::Abort(reason) => {
                assert!(reason.contains("input too large"), "{}", reason);
            }
            RenderResult::Ready(_) => panic!("oversize Chars scalar should not render"),
            RenderResult::Unsatisfied => panic!("oversize Chars scalar is not a constraint miss"),
            RenderResult::Interrupted => panic!("unexpected interrupt"),
        }
    }

    #[test]
    fn int_array_oversize_aborts_before_allocating() {
        let mut v = BTreeMap::new();
        v.insert("a".into(), vc(1, 9));
        let spec = mkspec(
            v,
            vec![FormatBlock::Array(ArrayBlock {
                base: "a".into(),
                len: Some((MAX_INPUT_ELEMENTS + 1).to_string()),
                height: None,
                count: None,
                jagged: false,
            })],
        );
        match render_case(&spec, &det_max(), &mut rng()) {
            RenderResult::Abort(reason) => {
                assert!(reason.contains("input too large"), "{}", reason);
            }
            RenderResult::Ready(_) => panic!("oversize int array should not render"),
            RenderResult::Unsatisfied => panic!("oversize int array is not a constraint miss"),
            RenderResult::Interrupted => panic!("unexpected interrupt"),
        }
    }

    #[test]
    fn test_cases_count_printed_once_and_body_repeats() {
        let mut v = BTreeMap::new();
        v.insert("t".into(), vc(1, 100));
        v.insert("x".into(), vc(1, 5));
        let spec = mkspec(
            v,
            vec![
                scalars(&["t"]),
                FormatBlock::TestCases(TestCasesBlock {
                    count: "t".into(),
                    format: vec![scalars(&["x"])],
                }),
            ],
        );
        let out = render_case(&spec, &det_max(), &mut rng()).unwrap();
        let ls = lines_of(&out);
        assert_eq!(ls[0], "100");
        assert_eq!(ls.len(), 1 + 100);
        assert!(ls[1..].iter().all(|l| *l == "5"));

        let small = render_case(
            &spec,
            &CaseStrategy::Random(RandomStrategy::SmallSize(2)),
            &mut rng(),
        )
        .unwrap();
        let ls = lines_of(&small);
        assert_eq!(ls[0], "2");
        assert_eq!(ls.len(), 1 + 2);
    }

    #[test]
    fn one_d_int_array() {
        let mut v = BTreeMap::new();
        v.insert("n".into(), vc(1, 10));
        v.insert("a".into(), vc(1, 100));
        let spec = mkspec(
            v,
            vec![
                scalars(&["n"]),
                FormatBlock::Array(ArrayBlock {
                    base: "a".into(),
                    len: Some("n".into()),
                    height: None,
                    count: None,
                    jagged: false,
                }),
            ],
        );
        let out = render_case(&spec, &det_max(), &mut rng()).unwrap();
        let ls = lines_of(&out);
        assert_eq!(ls[0], "10");
        let arr: Vec<i64> = ls[1]
            .split_whitespace()
            .map(|x| x.parse().unwrap())
            .collect();
        assert_eq!(arr.len(), 10);
        assert!(arr.iter().all(|&x| x == 100));
    }

    #[test]
    fn two_d_int_grid() {
        let mut v = BTreeMap::new();
        v.insert("h".into(), vc(2, 4));
        v.insert("w".into(), vc(3, 3));
        v.insert("a".into(), vc(1, 9));
        let spec = mkspec(
            v,
            vec![
                scalars(&["h", "w"]),
                FormatBlock::Array(ArrayBlock {
                    base: "a".into(),
                    len: Some("w".into()),
                    height: None,
                    count: Some("h".into()),
                    jagged: false,
                }),
            ],
        );
        let out = render_case(&spec, &det_max(), &mut rng()).unwrap();
        let ls = lines_of(&out);
        assert_eq!(ls[0], "4 3");
        assert_eq!(ls.len(), 1 + 4);
        for row in &ls[1..] {
            let r: Vec<i64> = row.split_whitespace().map(|x| x.parse().unwrap()).collect();
            assert_eq!(r.len(), 3);
            assert!(r.iter().all(|&x| x == 9));
        }
    }

    #[test]
    fn two_d_altmaxmin_is_checkerboard() {
        let mut v = BTreeMap::new();
        v.insert("h".into(), vc(3, 3));
        v.insert("w".into(), vc(4, 4));
        v.insert("a".into(), vc(1, 9));
        let spec = mkspec(
            v,
            vec![
                scalars(&["h", "w"]),
                FormatBlock::Array(ArrayBlock {
                    base: "a".into(),
                    len: Some("w".into()),
                    height: None,
                    count: Some("h".into()),
                    jagged: false,
                }),
            ],
        );
        let out = render_case(
            &spec,
            &CaseStrategy::Random(RandomStrategy::ArrayAltMaxMin),
            &mut rng(),
        )
        .unwrap();
        let ls = lines_of(&out);
        let grid: Vec<Vec<i64>> = ls[1..]
            .iter()
            .map(|r| r.split_whitespace().map(|x| x.parse().unwrap()).collect())
            .collect();
        // Adjacent rows differ at every column; rows two apart are identical.
        for c in 0..4 {
            assert_ne!(grid[0][c], grid[1][c]);
            assert_ne!(grid[1][c], grid[2][c]);
            assert_eq!(grid[0][c], grid[2][c]);
        }
    }

    #[test]
    fn chars_1d_array() {
        let mut v = BTreeMap::new();
        v.insert("h".into(), vc(2, 2));
        v.insert(
            "s".into(),
            VarConstraint {
                r#type: VarType::Chars,
                values: Some(vec!["a".into(), "b".into(), "c".into()]),
                len: Some(BoundRepr::Lit(4)),
                ..Default::default()
            },
        );
        let spec = mkspec(
            v,
            vec![
                scalars(&["h"]),
                FormatBlock::Array(ArrayBlock {
                    base: "s".into(),
                    len: None,
                    height: None,
                    count: Some("h".into()),
                    jagged: false,
                }),
            ],
        );
        let out = render_case(&spec, &det_min(), &mut rng()).unwrap();
        let ls = lines_of(&out);
        assert_eq!(ls[0], "2");
        assert_eq!(ls.len(), 1 + 2);
        assert!(ls[1..].iter().all(|l| *l == "aaaa"));
    }

    #[test]
    fn chars_array_synthetic_length_is_sampled_per_string() {
        let mut v = BTreeMap::new();
        v.insert("|s|".into(), vc(1, 20));
        v.insert(
            "s".into(),
            VarConstraint {
                r#type: VarType::Chars,
                values: Some(vec!["a".into()]),
                len: Some(BoundRepr::Expr("|s|".into())),
                ..Default::default()
            },
        );
        let spec = mkspec(
            v,
            vec![FormatBlock::Array(ArrayBlock {
                base: "s".into(),
                len: None,
                height: None,
                count: Some("12".into()),
                jagged: false,
            })],
        );
        let out = render_case(&spec, &random(), &mut rng()).unwrap();
        let lengths: HashSet<usize> = lines_of(&out).iter().map(|s| s.len()).collect();
        assert!(lengths.len() > 1, "{}", out);
    }

    #[test]
    fn chars_array_synthetic_length_with_height_is_sampled_per_string() {
        let mut v = BTreeMap::new();
        v.insert("|s|".into(), vc(1, 20));
        v.insert(
            "s".into(),
            VarConstraint {
                r#type: VarType::Chars,
                values: Some(vec!["a".into()]),
                len: Some(BoundRepr::Expr("|s|".into())),
                ..Default::default()
            },
        );
        let spec = mkspec(
            v,
            vec![FormatBlock::Array(ArrayBlock {
                base: "s".into(),
                len: None,
                height: Some("3".into()),
                count: Some("4".into()),
                jagged: false,
            })],
        );
        let out = render_case(&spec, &random(), &mut rng()).unwrap();
        let lengths: HashSet<usize> = lines_of(&out).iter().map(|s| s.len()).collect();
        assert!(lengths.len() > 1, "{}", out);
    }

    #[test]
    fn repeated_chars_scalar_synthetic_length_is_sampled_per_string() {
        let mut v = BTreeMap::new();
        v.insert("t".into(), vc(12, 12));
        v.insert("|s|".into(), vc(1, 20));
        v.insert(
            "s".into(),
            VarConstraint {
                r#type: VarType::Chars,
                values: Some(vec!["a".into()]),
                len: Some(BoundRepr::Expr("|s|".into())),
                ..Default::default()
            },
        );
        let spec = mkspec(
            v,
            vec![
                scalars(&["t"]),
                FormatBlock::TestCases(TestCasesBlock {
                    count: "t".into(),
                    format: vec![scalars(&["s"])],
                }),
            ],
        );
        let out = render_case(&spec, &random(), &mut rng()).unwrap();
        let lines = lines_of(&out);
        assert_eq!(lines[0], "12");
        let lengths: HashSet<usize> = lines[1..].iter().map(|s| s.len()).collect();
        assert!(lengths.len() > 1, "{}", out);
    }

    #[test]
    fn rows_chars_synthetic_length_is_sampled_per_string() {
        let mut v = BTreeMap::new();
        v.insert("|s|".into(), vc(1, 20));
        v.insert(
            "s".into(),
            VarConstraint {
                r#type: VarType::Chars,
                values: Some(vec!["a".into()]),
                len: Some(BoundRepr::Expr("|s|".into())),
                ..Default::default()
            },
        );
        let spec = mkspec(
            v,
            vec![FormatBlock::Rows(RowsBlock {
                vars: vec!["s".into()],
                len: "12".into(),
            })],
        );
        let out = render_case(&spec, &random(), &mut rng()).unwrap();
        let lengths: HashSet<usize> = lines_of(&out).iter().map(|s| s.len()).collect();
        assert!(lengths.len() > 1, "{}", out);
    }

    #[test]
    fn rows_block_monoinc_columns_sorted() {
        let mut v = BTreeMap::new();
        v.insert("m".into(), vc(3, 3));
        v.insert("x".into(), vc(1, 100));
        v.insert("y".into(), vc(1, 100));
        let spec = mkspec(
            v,
            vec![
                scalars(&["m"]),
                FormatBlock::Rows(RowsBlock {
                    vars: vec!["x".into(), "y".into()],
                    len: "m".into(),
                }),
            ],
        );
        let out = render_case(
            &spec,
            &CaseStrategy::Random(RandomStrategy::ArrayMonoInc),
            &mut rng(),
        )
        .unwrap();
        let ls = lines_of(&out);
        assert_eq!(ls[0], "3");
        assert_eq!(ls.len(), 1 + 3);
        let col_x: Vec<i64> = ls[1..]
            .iter()
            .map(|r| r.split_whitespace().next().unwrap().parse().unwrap())
            .collect();
        assert!(col_x.windows(2).all(|w| w[0] <= w[1]));
        for r in &ls[1..] {
            assert_eq!(r.split_whitespace().count(), 2);
        }
    }

    #[test]
    fn jagged_sum_limit_respected() {
        let mut v = BTreeMap::new();
        v.insert("n".into(), vc(2, 2));
        let mut lc = VarConstraint {
            r#type: VarType::Usize,
            range: Some([BoundRepr::Lit(1), BoundRepr::Expr("_".into())]),
            ..Default::default()
        };
        lc.sum_limit = Some(5);
        v.insert("l".into(), lc);
        v.insert("a".into(), vc(1, 9));
        let spec = mkspec(
            v,
            vec![
                scalars(&["n"]),
                FormatBlock::Array(ArrayBlock {
                    base: "a".into(),
                    len: Some("l".into()),
                    height: None,
                    count: Some("n".into()),
                    jagged: true,
                }),
            ],
        );
        let out = render_case(&spec, &random(), &mut rng()).unwrap();
        let ls = lines_of(&out);
        assert_eq!(ls[0], "2");
        let mut sum = 0i64;
        for row in &ls[1..] {
            let toks: Vec<i64> = row.split_whitespace().map(|x| x.parse().unwrap()).collect();
            let li = toks[0];
            assert_eq!(toks.len() as i64, 1 + li);
            assert!(toks[1..].iter().all(|&e| (1..=9).contains(&e)));
            sum += li;
        }
        assert!(sum <= 5, "sum of row lengths {} exceeds limit", sum);
    }

    #[test]
    fn two_d_distinct_per_row_only() {
        let mut v = BTreeMap::new();
        v.insert("h".into(), vc(2, 2));
        v.insert("w".into(), vc(5, 5));
        v.insert(
            "a".into(),
            VarConstraint {
                r#type: VarType::Usize,
                range: Some([BoundRepr::Lit(1), BoundRepr::Lit(5)]),
                all_distinct: true,
                ..Default::default()
            },
        );
        let spec = mkspec(
            v,
            vec![
                scalars(&["h", "w"]),
                FormatBlock::Array(ArrayBlock {
                    base: "a".into(),
                    len: Some("w".into()),
                    height: None,
                    count: Some("h".into()),
                    jagged: false,
                }),
            ],
        );
        let out = render_case(&spec, &random(), &mut rng()).unwrap();
        let ls = lines_of(&out);
        for row in &ls[1..] {
            let mut r: Vec<i64> = row.split_whitespace().map(|x| x.parse().unwrap()).collect();
            assert_eq!(r.len(), 5);
            r.sort_unstable();
            r.dedup();
            assert_eq!(r.len(), 5, "row not distinct");
        }
    }

    #[test]
    fn ordering_satisfied_for_random_and_none_for_impossible_corner() {
        let mut v = BTreeMap::new();
        v.insert("a".into(), vc(1, 10));
        v.insert("b".into(), vc(1, 10));
        let mut sec = RandomTestSection {
            vars: v,
            format: vec![scalars(&["a", "b"])],
            ..Default::default()
        };
        sec.ordering = vec![["a".into(), "b".into()]];
        let spec = resolve(&sec);
        for _ in 0..50 {
            let out = render_case(&spec, &random(), &mut rng()).unwrap();
            let toks: Vec<i64> = out
                .lines()
                .next()
                .unwrap()
                .split_whitespace()
                .map(|x| x.parse().unwrap())
                .collect();
            assert!(toks[0] <= toks[1]);
        }

        let mut v2 = BTreeMap::new();
        v2.insert("a".into(), vc(5, 5));
        v2.insert("b".into(), vc(1, 1));
        let mut sec2 = RandomTestSection {
            vars: v2,
            format: vec![scalars(&["a", "b"])],
            ..Default::default()
        };
        sec2.ordering = vec![["a".into(), "b".into()]];
        let spec2 = resolve(&sec2);
        assert!(render_case(&spec2, &det_max(), &mut rng()).is_none());
    }

    #[test]
    fn impossible_random_constraints_abort_after_retry_budget() {
        let mut v = BTreeMap::new();
        v.insert("a".into(), vc(5, 5));
        v.insert("b".into(), vc(1, 1));
        let mut sec = RandomTestSection {
            vars: v,
            format: vec![scalars(&["a", "b"])],
            ..Default::default()
        };
        sec.ordering = vec![["a".into(), "b".into()]];
        let spec = resolve(&sec);

        match render_case(&spec, &random(), &mut rng()) {
            RenderResult::Abort(reason) => {
                assert!(reason.contains("100 attempts"), "{}", reason);
                assert!(reason.contains("ordering/not_equal"), "{}", reason);
            }
            RenderResult::Ready(input) => panic!("unexpected ready input: {}", input),
            RenderResult::Unsatisfied => panic!("plain Random must return an abort reason"),
            RenderResult::Interrupted => panic!("unexpected interrupt"),
        }
    }

    #[test]
    fn ordering_narrows_array_elements_by_existing_scalar() {
        let mut v = BTreeMap::new();
        v.insert("n".into(), vc(3, 3));
        v.insert("a".into(), vc(1, 200_000));
        let mut sec = RandomTestSection {
            vars: v,
            format: vec![
                scalars(&["n"]),
                FormatBlock::Array(ArrayBlock {
                    base: "a".into(),
                    len: Some("n".into()),
                    height: None,
                    count: None,
                    jagged: false,
                }),
            ],
            ..Default::default()
        };
        sec.ordering = vec![["a".into(), "n".into()]];
        let spec = resolve(&sec);

        for _ in 0..20 {
            let out = render_case(&spec, &random(), &mut rng()).unwrap();
            let ls = lines_of(&out);
            let n: i64 = ls[0].parse().unwrap();
            let vals: Vec<i64> = ls[1]
                .split_whitespace()
                .map(|x| x.parse().unwrap())
                .collect();
            assert!(vals.iter().all(|&x| x <= n), "{}", out);
        }
    }

    #[test]
    fn scalar_not_equal_keeps_array_shape_strategy() {
        let mut v = BTreeMap::new();
        v.insert("x".into(), vc(10, 10));
        v.insert("a".into(), vc(1, 9));
        let mut sec = RandomTestSection {
            vars: v,
            format: vec![
                scalars(&["x"]),
                FormatBlock::Array(ArrayBlock {
                    base: "a".into(),
                    len: Some("5".into()),
                    height: None,
                    count: None,
                    jagged: false,
                }),
            ],
            ..Default::default()
        };
        sec.not_equal = vec![["x".into(), "a".into()]];
        let spec = resolve(&sec);
        let out = render_case(
            &spec,
            &CaseStrategy::Random(RandomStrategy::ArrayMonoInc),
            &mut rng(),
        )
        .unwrap();
        let values: Vec<i64> = lines_of(&out)[1]
            .split_whitespace()
            .map(|x| x.parse().unwrap())
            .collect();
        assert!(values.windows(2).all(|w| w[0] <= w[1]), "{}", out);
        assert!(values.iter().all(|&x| x != 10), "{}", out);
    }

    #[test]
    fn prior_array_narrows_later_scalar_lower_bound() {
        let mut v = BTreeMap::new();
        v.insert("a".into(), vc(7, 9));
        v.insert("x".into(), vc(1, 10));
        let mut sec = RandomTestSection {
            vars: v,
            format: vec![
                FormatBlock::Array(ArrayBlock {
                    base: "a".into(),
                    len: Some("3".into()),
                    height: None,
                    count: None,
                    jagged: false,
                }),
                scalars(&["x"]),
            ],
            ..Default::default()
        };
        sec.ordering = vec![["a".into(), "x".into()]];
        let spec = resolve(&sec);

        for _ in 0..20 {
            let out = render_case(&spec, &random(), &mut rng()).unwrap();
            let ls = lines_of(&out);
            let vals: Vec<i64> = ls[0]
                .split_whitespace()
                .map(|x| x.parse().unwrap())
                .collect();
            let x: i64 = ls[1].parse().unwrap();
            assert!(vals.iter().all(|&a| a <= x), "{}", out);
        }
    }

    #[test]
    fn prior_array_narrows_later_scalar_upper_bound() {
        let mut v = BTreeMap::new();
        v.insert("a".into(), vc(3, 5));
        v.insert("x".into(), vc(1, 10));
        let mut sec = RandomTestSection {
            vars: v,
            format: vec![
                FormatBlock::Array(ArrayBlock {
                    base: "a".into(),
                    len: Some("3".into()),
                    height: None,
                    count: None,
                    jagged: false,
                }),
                scalars(&["x"]),
            ],
            ..Default::default()
        };
        sec.ordering = vec![["x".into(), "a".into()]];
        let spec = resolve(&sec);

        for _ in 0..20 {
            let out = render_case(&spec, &random(), &mut rng()).unwrap();
            let ls = lines_of(&out);
            let vals: Vec<i64> = ls[0]
                .split_whitespace()
                .map(|x| x.parse().unwrap())
                .collect();
            let x: i64 = ls[1].parse().unwrap();
            assert!(vals.iter().all(|&a| x <= a), "{}", out);
        }
    }

    #[test]
    fn impossible_prior_array_bound_is_unsatisfied_or_aborted() {
        let mut v = BTreeMap::new();
        v.insert("a".into(), vc(9, 9));
        v.insert("x".into(), vc(1, 5));
        let mut sec = RandomTestSection {
            vars: v,
            format: vec![
                FormatBlock::Array(ArrayBlock {
                    base: "a".into(),
                    len: Some("2".into()),
                    height: None,
                    count: None,
                    jagged: false,
                }),
                scalars(&["x"]),
            ],
            ..Default::default()
        };
        sec.ordering = vec![["a".into(), "x".into()]];
        let spec = resolve(&sec);

        assert!(render_case(&spec, &det_min(), &mut rng()).is_none());
        match render_case(&spec, &random(), &mut rng()) {
            RenderResult::Abort(reason) => {
                assert!(reason.contains("100 attempts"), "{}", reason)
            }
            RenderResult::Ready(input) => panic!("unexpected ready input: {}", input),
            RenderResult::Unsatisfied => panic!("plain Random must return an abort reason"),
            RenderResult::Interrupted => panic!("unexpected interrupt"),
        }
    }

    #[test]
    fn prior_array_narrows_later_array_positionally() {
        let mut v = BTreeMap::new();
        v.insert("a".into(), vc(5, 9));
        v.insert("b".into(), vc(1, 9));
        let mut sec = RandomTestSection {
            vars: v,
            format: vec![
                FormatBlock::Array(ArrayBlock {
                    base: "a".into(),
                    len: Some("4".into()),
                    height: None,
                    count: None,
                    jagged: false,
                }),
                FormatBlock::Array(ArrayBlock {
                    base: "b".into(),
                    len: Some("4".into()),
                    height: None,
                    count: None,
                    jagged: false,
                }),
            ],
            ..Default::default()
        };
        sec.ordering = vec![["a".into(), "b".into()]];
        let spec = resolve(&sec);

        for _ in 0..20 {
            let out = render_case(&spec, &random(), &mut rng()).unwrap();
            let ls = lines_of(&out);
            let a: Vec<i64> = ls[0]
                .split_whitespace()
                .map(|x| x.parse().unwrap())
                .collect();
            let b: Vec<i64> = ls[1]
                .split_whitespace()
                .map(|x| x.parse().unwrap())
                .collect();
            assert!(a.iter().zip(&b).all(|(&x, &y)| x <= y), "{}", out);
        }
    }

    #[test]
    fn prior_array_position_does_not_force_global_max_bound() {
        let mut v = BTreeMap::new();
        v.insert("a".into(), vc(1, 9));
        v.insert("b".into(), vc(1, 9));
        let mut sec = RandomTestSection {
            vars: v,
            format: vec![
                FormatBlock::Array(ArrayBlock {
                    base: "a".into(),
                    len: Some("3".into()),
                    height: None,
                    count: None,
                    jagged: false,
                }),
                FormatBlock::Array(ArrayBlock {
                    base: "b".into(),
                    len: Some("3".into()),
                    height: None,
                    count: None,
                    jagged: false,
                }),
            ],
            ..Default::default()
        };
        sec.ordering = vec![["a".into(), "b".into()]];
        let spec = resolve(&sec);

        let out = render_case(
            &spec,
            &CaseStrategy::Random(RandomStrategy::ArrayOneMaxRestMin),
            &mut rng(),
        )
        .unwrap();
        let ls = lines_of(&out);
        let a: Vec<i64> = ls[0]
            .split_whitespace()
            .map(|x| x.parse().unwrap())
            .collect();
        let b: Vec<i64> = ls[1]
            .split_whitespace()
            .map(|x| x.parse().unwrap())
            .collect();
        assert!(a.iter().zip(&b).all(|(&x, &y)| x <= y), "{}", out);
        assert!(
            b.iter().any(|&x| x < 9),
            "global max narrowing would force all b_i to 9: {}",
            out
        );
    }

    #[test]
    fn impossible_prior_array_to_later_array_position_aborts() {
        let mut v = BTreeMap::new();
        v.insert("a".into(), vc(9, 9));
        v.insert("b".into(), vc(1, 5));
        let mut sec = RandomTestSection {
            vars: v,
            format: vec![
                FormatBlock::Array(ArrayBlock {
                    base: "a".into(),
                    len: Some("2".into()),
                    height: None,
                    count: None,
                    jagged: false,
                }),
                FormatBlock::Array(ArrayBlock {
                    base: "b".into(),
                    len: Some("2".into()),
                    height: None,
                    count: None,
                    jagged: false,
                }),
            ],
            ..Default::default()
        };
        sec.ordering = vec![["a".into(), "b".into()]];
        let spec = resolve(&sec);

        assert!(render_case(&spec, &det_min(), &mut rng()).is_none());
        match render_case(&spec, &random(), &mut rng()) {
            RenderResult::Abort(reason) => {
                assert!(reason.contains("100 attempts"), "{}", reason)
            }
            RenderResult::Ready(input) => panic!("unexpected ready input: {}", input),
            RenderResult::Unsatisfied => panic!("plain Random must return an abort reason"),
            RenderResult::Interrupted => panic!("unexpected interrupt"),
        }
    }

    #[test]
    fn prior_rows_column_narrows_later_rows_column_positionally() {
        let mut v = BTreeMap::new();
        v.insert("a".into(), vc(4, 8));
        v.insert("b".into(), vc(1, 8));
        let mut sec = RandomTestSection {
            vars: v,
            format: vec![FormatBlock::Rows(RowsBlock {
                vars: vec!["a".into(), "b".into()],
                len: "4".into(),
            })],
            ..Default::default()
        };
        sec.ordering = vec![["a".into(), "b".into()]];
        let spec = resolve(&sec);

        for _ in 0..20 {
            let out = render_case(&spec, &random(), &mut rng()).unwrap();
            for row in lines_of(&out) {
                let vals: Vec<i64> = row.split_whitespace().map(|x| x.parse().unwrap()).collect();
                assert!(vals[0] <= vals[1], "{}", out);
            }
        }
    }

    #[test]
    fn prior_rows_column_narrows_later_scalar() {
        let mut v = BTreeMap::new();
        v.insert("a".into(), vc(2, 6));
        v.insert("x".into(), vc(1, 10));
        let mut sec = RandomTestSection {
            vars: v,
            format: vec![
                FormatBlock::Rows(RowsBlock {
                    vars: vec!["a".into()],
                    len: "3".into(),
                }),
                scalars(&["x"]),
            ],
            ..Default::default()
        };
        sec.ordering = vec![["a".into(), "x".into()]];
        let spec = resolve(&sec);

        for _ in 0..20 {
            let out = render_case(&spec, &random(), &mut rng()).unwrap();
            let ls = lines_of(&out);
            let vals: Vec<i64> = ls[..3].iter().map(|x| x.parse().unwrap()).collect();
            let x: i64 = ls[3].parse().unwrap();
            assert!(vals.iter().all(|&a| a <= x), "{}", out);
        }
    }

    #[test]
    fn iteration_reuses_and_restores_parent_array_context() {
        let mut vars = BTreeMap::new();
        vars.insert("a".into(), vc(5, 5));
        vars.insert("b".into(), vc(1, 10));
        vars.insert("x".into(), vc(1, 10));
        let iter_format = vec![
            scalars(&["x"]),
            FormatBlock::Array(ArrayBlock {
                base: "b".into(),
                len: Some("2".into()),
                height: None,
                count: None,
                jagged: false,
            }),
        ];
        let mut section = RandomTestSection {
            vars,
            format: iter_format.clone(),
            ..Default::default()
        };
        section.ordering = vec![["a".into(), "b".into()], ["a".into(), "x".into()]];
        let spec = resolve(&section);

        let mut context = RenderContext {
            arrays: HashMap::from([("a".into(), vec![5, 5, 5])]),
            ..Default::default()
        };
        let checkpoint = context.checkpoint();
        let mut lines = Vec::new();
        let mut budget = Budget::new(MAX_INPUT_ELEMENTS);

        assert!(run_iteration(
            &iter_format,
            None,
            &spec,
            &random(),
            &StructuralSizes::default(),
            &mut context,
            &checkpoint,
            &mut lines,
            &mut budget,
            &mut rng(),
        )
        .unwrap());

        assert!(
            context.scalars.is_empty(),
            "iteration scalars must not leak to parent"
        );
        assert_eq!(context.arrays["a"], vec![5, 5, 5]);
        assert!(
            !context.arrays.contains_key("b"),
            "iteration arrays must not leak to parent"
        );
        assert!(lines[0].parse::<i64>().unwrap() >= 5);
        assert!(lines[1]
            .split_whitespace()
            .map(|x| x.parse::<i64>().unwrap())
            .all(|x| x >= 5));
    }

    #[test]
    fn scope_crossing_ordering_narrows_inner_scalar() {
        let mut v = BTreeMap::new();
        v.insert("n".into(), vc(2, 2));
        v.insert("q".into(), vc(3, 3));
        v.insert("x".into(), vc(1, 200_000));
        let mut sec = RandomTestSection {
            vars: v,
            format: vec![
                scalars(&["n", "q"]),
                FormatBlock::TestCases(TestCasesBlock {
                    count: "q".into(),
                    format: vec![scalars(&["x"])],
                }),
            ],
            ..Default::default()
        };
        sec.ordering = vec![["x".into(), "n".into()]];
        let spec = resolve(&sec);

        let out = render_case(&spec, &random(), &mut rng()).unwrap();
        let ls = lines_of(&out);
        let header: Vec<i64> = ls[0]
            .split_whitespace()
            .map(|x| x.parse().unwrap())
            .collect();
        let n = header[0];
        for row in &ls[1..] {
            let x: i64 = row.parse().unwrap();
            assert!(x <= n, "{}", out);
        }
    }

    #[test]
    fn chars_not_equal_checks_whole_string() {
        let mut v = BTreeMap::new();
        v.insert(
            "s".into(),
            VarConstraint {
                r#type: VarType::Chars,
                values: Some(vec!["a".into(), "b".into()]),
                len: Some(BoundRepr::Lit(4)),
                ..Default::default()
            },
        );
        v.insert(
            "t".into(),
            VarConstraint {
                r#type: VarType::Chars,
                values: Some(vec!["a".into(), "b".into()]),
                len: Some(BoundRepr::Lit(4)),
                ..Default::default()
            },
        );
        let mut sec = RandomTestSection {
            vars: v,
            format: vec![scalars(&["s", "t"])],
            ..Default::default()
        };
        sec.not_equal = vec![["s".into(), "t".into()]];
        let spec = resolve(&sec);

        for _ in 0..20 {
            let out = render_case(&spec, &random(), &mut rng()).unwrap();
            let toks: Vec<&str> = out.split_whitespace().collect();
            assert_eq!(toks.len(), 2);
            assert_ne!(toks[0], toks[1], "{}", out);
        }
    }

    #[test]
    fn not_equal_satisfied_and_impossible_corner_none() {
        let mut v = BTreeMap::new();
        v.insert("a".into(), vc(1, 2));
        v.insert("b".into(), vc(1, 2));
        let mut sec = RandomTestSection {
            vars: v,
            format: vec![scalars(&["a", "b"])],
            ..Default::default()
        };
        sec.not_equal = vec![["a".into(), "b".into()]];
        let spec = resolve(&sec);
        for _ in 0..50 {
            let out = render_case(&spec, &random(), &mut rng()).unwrap();
            let toks: Vec<i64> = out
                .lines()
                .next()
                .unwrap()
                .split_whitespace()
                .map(|x| x.parse().unwrap())
                .collect();
            assert_ne!(toks[0], toks[1]);
        }

        let mut v2 = BTreeMap::new();
        v2.insert("a".into(), vc(3, 3));
        v2.insert("b".into(), vc(3, 3));
        let mut sec2 = RandomTestSection {
            vars: v2,
            format: vec![scalars(&["a", "b"])],
            ..Default::default()
        };
        sec2.not_equal = vec![["a".into(), "b".into()]];
        let spec2 = resolve(&sec2);
        assert!(render_case(&spec2, &det_min(), &mut rng()).is_none());
    }

    #[test]
    fn scalar_not_equal_excludes_prior_array_values() {
        let mut v = BTreeMap::new();
        v.insert("a".into(), vc(1, 2));
        v.insert("x".into(), vc(1, 3));
        let mut sec = RandomTestSection {
            vars: v,
            format: vec![
                FormatBlock::Array(ArrayBlock {
                    base: "a".into(),
                    len: Some("2".into()),
                    height: None,
                    count: None,
                    jagged: false,
                }),
                scalars(&["x"]),
            ],
            ..Default::default()
        };
        sec.not_equal = vec![["a".into(), "x".into()]];
        let spec = resolve(&sec);

        let out = render_case(&spec, &det_max(), &mut rng()).unwrap();
        let ls = lines_of(&out);
        assert_eq!(ls[0], "2 2");
        assert_eq!(ls[1], "3");
    }

    #[test]
    fn array_not_equal_excludes_same_index_values() {
        let mut v = BTreeMap::new();
        v.insert("a".into(), vc(1, 2));
        v.insert("b".into(), vc(1, 2));
        let mut sec = RandomTestSection {
            vars: v,
            format: vec![
                FormatBlock::Array(ArrayBlock {
                    base: "a".into(),
                    len: Some("3".into()),
                    height: None,
                    count: None,
                    jagged: false,
                }),
                FormatBlock::Array(ArrayBlock {
                    base: "b".into(),
                    len: Some("3".into()),
                    height: None,
                    count: None,
                    jagged: false,
                }),
            ],
            ..Default::default()
        };
        sec.not_equal = vec![["a".into(), "b".into()]];
        let spec = resolve(&sec);

        let out = render_case(&spec, &det_max(), &mut rng()).unwrap();
        let ls = lines_of(&out);
        let a: Vec<i64> = ls[0]
            .split_whitespace()
            .map(|x| x.parse().unwrap())
            .collect();
        let b: Vec<i64> = ls[1]
            .split_whitespace()
            .map(|x| x.parse().unwrap())
            .collect();
        assert_eq!(a, vec![2, 2, 2]);
        assert_eq!(b, vec![1, 1, 1]);
        assert!(a.iter().zip(&b).all(|(&x, &y)| x != y), "{}", out);
    }

    #[test]
    fn rows_not_equal_excludes_same_row_values() {
        let mut v = BTreeMap::new();
        v.insert("a".into(), vc(1, 2));
        v.insert("b".into(), vc(1, 2));
        let mut sec = RandomTestSection {
            vars: v,
            format: vec![FormatBlock::Rows(RowsBlock {
                vars: vec!["a".into(), "b".into()],
                len: "3".into(),
            })],
            ..Default::default()
        };
        sec.not_equal = vec![["a".into(), "b".into()]];
        let spec = resolve(&sec);

        let out = render_case(&spec, &det_max(), &mut rng()).unwrap();
        for row in lines_of(&out) {
            let vals: Vec<i64> = row.split_whitespace().map(|x| x.parse().unwrap()).collect();
            assert_ne!(vals[0], vals[1], "{}", out);
        }
    }

    #[test]
    fn per_iteration_ordering_rejected_locally() {
        // a, b are produced inside each test case; ordering a <= b must hold
        // per iteration even though they never reach the top context.
        let mut v = BTreeMap::new();
        v.insert("t".into(), vc(2, 2));
        v.insert("a".into(), vc(1, 10));
        v.insert("b".into(), vc(1, 10));
        let mut sec = RandomTestSection {
            vars: v,
            format: vec![
                scalars(&["t"]),
                FormatBlock::TestCases(TestCasesBlock {
                    count: "t".into(),
                    format: vec![scalars(&["a", "b"])],
                }),
            ],
            ..Default::default()
        };
        sec.ordering = vec![["a".into(), "b".into()]];
        let spec = resolve(&sec);
        let out = render_case(&spec, &random(), &mut rng()).unwrap();
        let ls = lines_of(&out);
        assert_eq!(ls[0], "2");
        assert_eq!(ls.len(), 1 + 2);
        for row in &ls[1..] {
            let toks: Vec<i64> = row.split_whitespace().map(|x| x.parse().unwrap()).collect();
            assert!(toks[0] <= toks[1], "per-iteration ordering violated");
        }

        // Impossible per-iteration constraint under a corner strategy → None.
        let mut v2 = BTreeMap::new();
        v2.insert("t".into(), vc(1, 1));
        v2.insert("a".into(), vc(5, 5));
        v2.insert("b".into(), vc(1, 1));
        let mut sec2 = RandomTestSection {
            vars: v2,
            format: vec![
                scalars(&["t"]),
                FormatBlock::TestCases(TestCasesBlock {
                    count: "t".into(),
                    format: vec![scalars(&["a", "b"])],
                }),
            ],
            ..Default::default()
        };
        sec2.ordering = vec![["a".into(), "b".into()]];
        let spec2 = resolve(&sec2);
        assert!(render_case(&spec2, &det_min(), &mut rng()).is_none());
    }

    #[test]
    fn queries_emit_id_token_and_no_count_line() {
        let mut v = BTreeMap::new();
        v.insert("q".into(), vc(2, 2));
        v.insert("x".into(), vc(1, 5));
        v.insert("y".into(), vc(1, 5));
        let spec = mkspec(
            v,
            vec![
                scalars(&["q"]),
                FormatBlock::Queries(QueriesBlock {
                    count: "q".into(),
                    discriminator: None,
                    types: vec![
                        QueryBranch {
                            id: "1".into(),
                            format: vec![scalars(&["x"])],
                        },
                        QueryBranch {
                            id: "2".into(),
                            format: vec![scalars(&["y"])],
                        },
                    ],
                }),
            ],
        );
        let out = render_case(&spec, &random(), &mut rng()).unwrap();
        let ls = lines_of(&out);
        assert_eq!(ls[0], "2");
        assert_eq!(ls.len(), 1 + 2);
        for q in &ls[1..] {
            let toks: Vec<&str> = q.split_whitespace().collect();
            assert_eq!(toks.len(), 2);
            assert!(toks[0] == "1" || toks[0] == "2");
            assert!(toks[1].parse::<i64>().is_ok());
        }
    }
}
