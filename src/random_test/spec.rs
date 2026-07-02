//! Resolve the yml `random_test:` section into a generation spec.
//!
//! This is a mechanical renderer of the yml: it performs no HTML re-parsing,
//! no inference, and no implicit defaults. All information required for random
//! generation is assumed to be present in the yml. When a variable is missing
//! information that most strategies depend on (numeric bounds, Chars charset /
//! length), the variable is recorded in [`ResolvedSpec::missing`] with an
//! English message naming the variable and pointing at the yml field to edit.
//! The runner surfaces these and aborts the problem's random test,
//! because without bounds almost no strategy (AllMax/AllMin/SmallSize/
//! ZeroCorner/Array*) can run.

use crate::parse::{analyze_format, BoundRepr, FormatBlock, RandomTestSection, VarType};
use std::collections::{HashMap, HashSet};

/// Upper bound of a numeric variable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Hi {
    /// Concrete literal upper bound from `range`.
    Fixed(i64),
    /// `range` upper bound absent but a `sum_limit` is present; the effective
    /// per-case cap (`L / T`) is applied later while rendering.
    SumLimited(i64),
}

/// Resolved per-variable information.
#[derive(Debug, Clone)]
pub(crate) struct VarInfo {
    pub ty: VarType,
    /// Resolved lower bound. For an enum variable without a `range`, derived
    /// from `min(values)`. `0` placeholder when missing (recorded in
    /// [`ResolvedSpec::missing`]; never used because the runner aborts first).
    pub lo: i64,
    pub hi: Hi,
    /// Discrete integer enum domain (`values` for `Usize`/`I64`).
    pub values: Option<Vec<i64>>,
    /// Character set for `Chars` (from `values`).
    pub charset: Option<Vec<char>>,
    /// String length for `Chars` (literal or variable reference; resolved at
    /// generation time against the running context).
    pub len: Option<BoundRepr>,
    pub all_distinct: bool,
    pub sum_limit: Option<i64>,
}

/// The yml `random_test:` section resolved for generation. The `FormatBlock`
/// tree is kept verbatim (no lowering); the generator walks it directly.
#[derive(Debug, Clone)]
pub(crate) struct ResolvedSpec {
    pub format: Vec<FormatBlock>,
    pub vars: HashMap<String, VarInfo>,
    pub size_vars: HashSet<String>,
    /// Parsed once from each distinct persisted size expression.
    pub size_terms: HashMap<String, SizeTerm>,
    /// `a <= b`.
    pub ordering: Vec<(String, String)>,
    /// `a != b`.
    pub not_equal: Vec<(String, String)>,
    /// Array/Rows variables participating in array-to-array constraints.
    /// Jagged and scalar-array pairs are intentionally excluded.
    pub inter_array_constrained: HashSet<String>,
    /// `true` iff the problem contains a strategy-bearing array-like construct
    /// (a len-bearing `Array`, any `Rows`, or a len-bearing `Chars` string).
    /// Precomputed from the `format` tree so the strategy layer never re-walks
    /// `FormatBlock`. The `all_distinct` axis is intentionally excluded.
    pub has_array: bool,
    /// `sum_limit` denominators from the format tree: each jagged `Array.len`
    /// variable → its row-count variable. Read by the generator instead of
    /// re-analysing the format.
    pub jagged_len_to_count: HashMap<String, String>,
    /// First `TestCases.count` variable (the `T` sum denominator), if any.
    pub test_cases_count_var: Option<String>,
    /// Constraint/format lines the parser could not handle (surfaced as a
    /// trailing warning by the runner).
    pub skipped: Vec<String>,
    /// Unrecoverable yml gaps. Non-empty ⇒ the runner prints these English
    /// warnings and aborts this problem's random test.
    pub missing: Vec<String>,
}

fn lit(b: Option<&BoundRepr>) -> Option<i64> {
    match b {
        Some(BoundRepr::Lit(n)) => Some(*n),
        _ => None,
    }
}

/// A size-field value (`Array.len/height/count`, `Rows.len`,
/// `TestCases.count`, `Queries.count`). The yml schema stores these as raw
/// strings; only the three shapes below are supported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SizeTerm {
    /// Integer literal, e.g. `"3"`.
    Lit(i64),
    /// An exact `vars` key, e.g. `n` or `|S|` (pipes are part of the key).
    Var(String),
    /// An exact `vars` key plus a literal offset, e.g. `n-1` or `(t)+1`.
    VarOffset(String, i64),
}

/// Strip one matching outer pair of parentheses, then trim.
fn strip_one_paren(s: &str) -> &str {
    let t = s.trim();
    if let Some(inner) = t.strip_prefix('(').and_then(|x| x.strip_suffix(')')) {
        inner.trim()
    } else {
        t
    }
}

/// Parse a size-field string into a [`SizeTerm`].
///
/// Recognised shapes only (everything else ⇒ `None`, treated by the caller as
/// a yml gap, exactly like a missing `range`):
/// - integer literal: `3`, `2`
/// - exact `vars` key: `n`, `t`, `|S|`
/// - exact `vars` key with a trailing `±<int>`, with an optional single paren
///   pair around the name: `n-1`, `(t)+1`, `(n)-1`
///
/// `vars` is the resolved variable map; a name is valid iff it is an exact
/// key (no tokenisation, so `|S|` matches verbatim).
pub(crate) fn parse_size(expr: &str, vars: &HashMap<String, VarInfo>) -> Option<SizeTerm> {
    let e = expr.trim();
    if let Ok(n) = e.parse::<i64>() {
        return Some(SizeTerm::Lit(n));
    }
    if vars.contains_key(e) {
        return Some(SizeTerm::Var(e.to_owned()));
    }
    // Trailing `±<int>`: find the rightmost `+`/`-` (not at index 0) whose
    // suffix is all digits.
    let split = e.char_indices().rev().find_map(|(i, c)| {
        if i == 0 || (c != '+' && c != '-') {
            return None;
        }
        let num = &e[i + 1..];
        if !num.is_empty() && num.bytes().all(|b| b.is_ascii_digit()) {
            Some((i, c, num))
        } else {
            None
        }
    });
    if let Some((i, sign, num)) = split {
        let base = strip_one_paren(&e[..i]);
        if vars.contains_key(base) {
            if let Ok(mag) = num.parse::<i64>() {
                let off = if sign == '-' { -mag } else { mag };
                return Some(SizeTerm::VarOffset(base.to_owned(), off));
            }
        }
    }
    None
}

/// `true` iff `blocks` contain any strategy-bearing array-like construct: a
/// plain `Array` carrying an explicit `len`, or any `Rows` block (recursing
/// into `TestCases` / `Queries`). A 2-D grid `Array` (`len: None`, width on a
/// `Chars` var) is intentionally *not* counted here; the resolve-time
/// `has_array` additionally ORs in `Chars`-with-`len` string variables.
/// Collect every value-variable name referenced by the format tree
/// (`Scalars.vars`, `Array.base`, `Rows.vars`), recursing into `TestCases` /
/// `Queries`. Size fields are validated separately via [`parse_size`].
fn collect_format_value_vars<'a>(blocks: &'a [FormatBlock], out: &mut Vec<&'a str>) {
    for b in blocks {
        match b {
            FormatBlock::Scalars(s) => out.extend(s.vars.iter().map(String::as_str)),
            FormatBlock::Array(a) => out.push(&a.base),
            FormatBlock::Rows(r) => out.extend(r.vars.iter().map(String::as_str)),
            FormatBlock::TestCases(tc) => collect_format_value_vars(&tc.format, out),
            FormatBlock::Queries(q) => {
                for t in &q.types {
                    collect_format_value_vars(&t.format, out);
                }
            }
        }
    }
}

fn format_has_array(blocks: &[FormatBlock]) -> bool {
    blocks.iter().any(|b| match b {
        FormatBlock::Array(a) => a.len.is_some(),
        FormatBlock::Rows(_) => true,
        FormatBlock::TestCases(tc) => format_has_array(&tc.format),
        FormatBlock::Queries(q) => q.types.iter().any(|t| format_has_array(&t.format)),
        FormatBlock::Scalars(_) => false,
    })
}

/// Resolve a `RandomTestSection` into a [`ResolvedSpec`].
pub(crate) fn resolve(section: &RandomTestSection) -> ResolvedSpec {
    let mut vars: HashMap<String, VarInfo> = HashMap::new();
    let mut missing: Vec<String> = Vec::new();

    for (name, vc) in &section.vars {
        let range = vc.range.as_ref();
        let lo_lit = lit(range.map(|r| &r[0]));
        let hi_lit = lit(range.map(|r| &r[1]));

        let values: Option<Vec<i64>> = if matches!(vc.r#type, VarType::Usize | VarType::I64) {
            match &vc.values {
                Some(vs) => {
                    let mut parsed = Vec::with_capacity(vs.len());
                    let mut bad = false;
                    for v in vs {
                        match v.parse::<i64>() {
                            Ok(n) => parsed.push(n),
                            Err(_) => bad = true,
                        }
                    }
                    if bad || parsed.is_empty() {
                        missing.push(format!(
                            "variable `{name}`: `values` is {} — edit \
                             `random_test.vars.{name}.values` in the yml",
                            if bad { "non-integer" } else { "empty" }
                        ));
                        None
                    } else {
                        Some(parsed)
                    }
                }
                None => None,
            }
        } else {
            None
        };

        let (lo, hi);
        match vc.r#type {
            VarType::Chars => {
                // Numeric bounds are not applicable to Chars.
                lo = 0;
                hi = Hi::Fixed(0);
            }
            VarType::Usize | VarType::I64 => {
                if let Some(vs) = &values {
                    // Enum fully specifies the domain; absent bounds derive
                    // from the domain itself (no implicit default).
                    lo = lo_lit.unwrap_or_else(|| *vs.iter().min().expect("non-empty"));
                    hi = Hi::Fixed(hi_lit.unwrap_or_else(|| *vs.iter().max().expect("non-empty")));
                } else if vc.values.is_some() {
                    // Unusable `values` already recorded in `missing`; the
                    // runner aborts before these placeholders are read.
                    lo = lo_lit.unwrap_or(0);
                    hi = Hi::Fixed(hi_lit.unwrap_or(0));
                } else {
                    lo = match lo_lit {
                        Some(n) => n,
                        None => {
                            missing.push(format!(
                                "variable `{name}`: lower bound missing — edit \
                                 `random_test.vars.{name}.range` in the yml"
                            ));
                            0
                        }
                    };
                    hi = match hi_lit {
                        Some(n) => Hi::Fixed(n),
                        None => match vc.sum_limit {
                            Some(l) => Hi::SumLimited(l),
                            None => {
                                missing.push(format!(
                                    "variable `{name}`: upper bound missing — edit \
                                     `random_test.vars.{name}.range` in the yml"
                                ));
                                Hi::Fixed(0)
                            }
                        },
                    };
                }
            }
        }

        let charset: Option<Vec<char>> = if vc.r#type == VarType::Chars {
            let cs: Vec<char> = vc
                .values
                .iter()
                .flatten()
                .filter_map(|s| s.chars().next())
                .collect();
            if cs.is_empty() {
                missing.push(format!(
                    "variable `{name}`: Chars charset missing — edit \
                     `random_test.vars.{name}.values` in the yml"
                ));
                None
            } else {
                Some(cs)
            }
        } else {
            None
        };

        if vc.r#type == VarType::Chars && vc.len.is_none() {
            missing.push(format!(
                "variable `{name}`: Chars length missing — edit \
                 `random_test.vars.{name}.len` in the yml"
            ));
        }

        vars.insert(
            name.clone(),
            VarInfo {
                ty: vc.r#type.clone(),
                lo,
                hi,
                values,
                charset,
                len: vc.len.clone(),
                all_distinct: vc.all_distinct,
                sum_limit: vc.sum_limit,
            },
        );
    }

    let analysis = analyze_format(&section.format);
    let mut size_vars = analysis.size_exprs.clone();
    for vc in section.vars.values() {
        if vc.r#type == VarType::Chars {
            if let Some(BoundRepr::Expr(expr)) = &vc.len {
                size_vars.insert(expr.clone());
            }
        }
    }
    let size_terms: HashMap<String, SizeTerm> = size_vars
        .iter()
        .filter_map(|expr| parse_size(expr, &vars).map(|term| (expr.clone(), term)))
        .collect();

    // Every size field must be an integer literal, an exact `vars` key, or
    // such a key with a literal offset. Anything else (unsupported expression
    // such as `2*n`, or a reference to a variable absent from `vars`) is a yml
    // gap: record it so the runner aborts this problem with a warning, exactly
    // like a missing `range`. Never silently treat it as 0.
    let mut unresolved: Vec<&String> = size_vars
        .iter()
        .filter(|s| parse_size(s, &vars).is_none())
        .collect();
    unresolved.sort();
    for s in unresolved {
        missing.push(format!(
            "size expression `{s}`: not an integer literal, a `random_test.vars` \
             key, or `<var>±<int>` — fix the format/expression or add the \
             variable in the yml"
        ));
    }

    // Every value variable the format reads must be declared in `vars`;
    // otherwise the renderer would have no bounds to draw from. Record it as
    // a yml gap so the runner aborts, never render a made-up value.
    let mut format_value_vars = Vec::new();
    collect_format_value_vars(&section.format, &mut format_value_vars);
    format_value_vars.sort_unstable();
    format_value_vars.dedup();
    for name in format_value_vars {
        if !vars.contains_key(name) {
            missing.push(format!(
                "variable `{name}`: referenced in the format but missing from \
                 `random_test.vars` — add it in the yml"
            ));
        }
    }

    let is_array_like = |name: &str| {
        matches!(
            analysis.shape_of(name),
            crate::parse::VarShape::Array | crate::parse::VarShape::Rows
        )
    };

    // not_equal between string arrays is not enforced by the generator; warn
    // instead of silently ignoring the constraint.
    let mut skipped = section.skipped.clone();
    for pair in &section.not_equal {
        for name in pair {
            let is_chars = vars.get(name).is_some_and(|v| v.ty == VarType::Chars);
            if is_chars && is_array_like(name) {
                skipped.push(format!(
                    "not_equal involving string array `{name}` is not enforced"
                ));
            }
        }
    }
    let inter_array_constrained = section
        .ordering
        .iter()
        .chain(&section.not_equal)
        .filter(|[a, b]| is_array_like(a) && is_array_like(b))
        .flat_map(|[a, b]| [a.clone(), b.clone()])
        .collect();

    let has_array =
        format_has_array(&section.format) || vars.values().any(|v| v.len.is_some());

    ResolvedSpec {
        format: section.format.clone(),
        vars,
        size_vars,
        size_terms,
        ordering: section
            .ordering
            .iter()
            .map(|[a, b]| (a.clone(), b.clone()))
            .collect(),
        not_equal: section
            .not_equal
            .iter()
            .map(|[a, b]| (a.clone(), b.clone()))
            .collect(),
        inter_array_constrained,
        has_array,
        jagged_len_to_count: analysis.jagged_len_to_count,
        test_cases_count_var: analysis.first_test_cases_count,
        skipped,
        missing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::{
        ArrayBlock, QueriesBlock, QueryBranch, RowsBlock, ScalarsBlock, TestCasesBlock,
        VarConstraint,
    };
    use std::collections::BTreeMap;

    fn vc_num(ty: VarType, lo: Option<i64>, hi: Option<i64>) -> VarConstraint {
        let range = match (lo, hi) {
            (None, None) => None,
            (l, h) => Some([
                l.map(BoundRepr::Lit).unwrap_or(BoundRepr::Expr("_".into())),
                h.map(BoundRepr::Lit).unwrap_or(BoundRepr::Expr("_".into())),
            ]),
        };
        VarConstraint {
            r#type: ty,
            range,
            ..Default::default()
        }
    }

    fn section(
        vars: BTreeMap<String, VarConstraint>,
        format: Vec<FormatBlock>,
    ) -> RandomTestSection {
        RandomTestSection {
            vars,
            format,
            ..Default::default()
        }
    }

    #[test]
    fn numeric_literal_range_resolves() {
        let mut v = BTreeMap::new();
        v.insert("n".into(), vc_num(VarType::Usize, Some(1), Some(100)));
        let r = resolve(&section(v, vec![]));
        let n = &r.vars["n"];
        assert_eq!(n.lo, 1);
        assert_eq!(n.hi, Hi::Fixed(100));
        assert!(r.missing.is_empty());
    }

    #[test]
    fn missing_hi_with_sum_limit_is_sum_limited() {
        let mut v = BTreeMap::new();
        let mut c = vc_num(VarType::Usize, Some(1), None);
        c.sum_limit = Some(200_000);
        v.insert("n".into(), c);
        let r = resolve(&section(v, vec![]));
        assert_eq!(r.vars["n"].hi, Hi::SumLimited(200_000));
        assert!(r.missing.is_empty());
    }

    #[test]
    fn missing_hi_without_sum_limit_is_recorded() {
        let mut v = BTreeMap::new();
        v.insert("n".into(), vc_num(VarType::Usize, Some(1), None));
        let r = resolve(&section(v, vec![]));
        assert_eq!(r.missing.len(), 1);
        assert!(r.missing[0].contains("`n`"));
        assert!(r.missing[0].contains("upper bound missing"));
        assert!(r.missing[0].contains("random_test.vars.n.range"));
    }

    #[test]
    fn missing_lo_is_recorded() {
        let mut v = BTreeMap::new();
        v.insert("n".into(), vc_num(VarType::Usize, None, Some(10)));
        let r = resolve(&section(v, vec![]));
        assert_eq!(r.missing.len(), 1);
        assert!(r.missing[0].contains("lower bound missing"));
    }

    #[test]
    fn chars_with_values_builds_charset() {
        let mut v = BTreeMap::new();
        v.insert("n".into(), vc_num(VarType::Usize, Some(1), Some(10)));
        v.insert(
            "s".into(),
            VarConstraint {
                r#type: VarType::Chars,
                values: Some(vec!["a".into(), "b".into(), "c".into()]),
                len: Some(BoundRepr::Expr("n".into())),
                ..Default::default()
            },
        );
        let r = resolve(&section(v, vec![]));
        assert_eq!(r.vars["s"].charset, Some(vec!['a', 'b', 'c']));
        assert!(r.missing.is_empty());
    }

    #[test]
    fn chars_without_values_is_recorded_no_default() {
        let mut v = BTreeMap::new();
        v.insert(
            "s".into(),
            VarConstraint {
                r#type: VarType::Chars,
                len: Some(BoundRepr::Expr("n".into())),
                ..Default::default()
            },
        );
        let r = resolve(&section(v, vec![]));
        assert_eq!(r.vars["s"].charset, None);
        assert!(r
            .missing
            .iter()
            .any(|m| m.contains("Chars charset missing")));
    }

    #[test]
    fn chars_without_len_is_recorded() {
        let mut v = BTreeMap::new();
        v.insert(
            "s".into(),
            VarConstraint {
                r#type: VarType::Chars,
                values: Some(vec!["a".into()]),
                ..Default::default()
            },
        );
        let r = resolve(&section(v, vec![]));
        assert!(r.missing.iter().any(|m| m.contains("Chars length missing")));
    }

    #[test]
    fn int_enum_values_parsed() {
        let mut v = BTreeMap::new();
        v.insert(
            "b".into(),
            VarConstraint {
                r#type: VarType::Usize,
                values: Some(vec!["1".into(), "2".into()]),
                ..Default::default()
            },
        );
        let r = resolve(&section(v, vec![]));
        assert_eq!(r.vars["b"].values, Some(vec![1, 2]));
        assert!(r.missing.is_empty());
    }

    #[test]
    fn size_vars_collected_from_all_positions() {
        let format = vec![
            FormatBlock::Array(ArrayBlock {
                base: "a".into(),
                len: Some("w".into()),
                height: Some("h".into()),
                count: Some("f".into()),
                jagged: false,
            }),
            FormatBlock::Rows(RowsBlock {
                vars: vec!["x".into(), "y".into()],
                len: "m".into(),
            }),
            FormatBlock::TestCases(TestCasesBlock {
                count: "t".into(),
                format: vec![FormatBlock::Array(ArrayBlock {
                    base: "l".into(),
                    len: Some("ll".into()),
                    height: None,
                    count: Some("n".into()),
                    jagged: true,
                })],
            }),
            FormatBlock::Queries(QueriesBlock {
                count: "q".into(),
                discriminator: None,
                types: vec![QueryBranch {
                    id: "1".into(),
                    format: vec![FormatBlock::Array(ArrayBlock {
                        base: "z".into(),
                        len: Some("zl".into()),
                        height: None,
                        count: None,
                        jagged: false,
                    })],
                }],
            }),
            FormatBlock::Scalars(ScalarsBlock {
                vars: vec!["k".into()],
            }),
        ];
        let r = resolve(&section(BTreeMap::new(), format));
        for expected in ["w", "h", "f", "m", "t", "ll", "n", "q", "zl"] {
            assert!(
                r.size_vars.contains(expected),
                "missing size var {}",
                expected
            );
        }
        assert!(!r.size_vars.contains("k"));
        assert!(!r.size_vars.contains("a"));
    }

    fn vars_with(names: &[&str]) -> HashMap<String, VarInfo> {
        let mut m = HashMap::new();
        for n in names {
            m.insert(
                (*n).to_owned(),
                VarInfo {
                    ty: VarType::Usize,
                    lo: 1,
                    hi: Hi::Fixed(10),
                    values: None,
                    charset: None,
                    len: None,
                    all_distinct: false,
                    sum_limit: None,
                },
            );
        }
        m
    }

    #[test]
    fn parse_size_shapes() {
        let v = vars_with(&["n", "t", "|S|"]);
        assert_eq!(parse_size("3", &v), Some(SizeTerm::Lit(3)));
        assert_eq!(parse_size(" 2 ", &v), Some(SizeTerm::Lit(2)));
        assert_eq!(parse_size("n", &v), Some(SizeTerm::Var("n".into())));
        // `|S|` is an exact key — pipes are part of the name, not stripped.
        assert_eq!(parse_size("|S|", &v), Some(SizeTerm::Var("|S|".into())));
        assert_eq!(
            parse_size("n-1", &v),
            Some(SizeTerm::VarOffset("n".into(), -1))
        );
        assert_eq!(
            parse_size("(t)+1", &v),
            Some(SizeTerm::VarOffset("t".into(), 1))
        );
        assert_eq!(
            parse_size("(n)-2", &v),
            Some(SizeTerm::VarOffset("n".into(), -2))
        );
        // Unsupported / unknown ⇒ None (caller turns this into a yml gap).
        assert_eq!(parse_size("2*n", &v), None);
        assert_eq!(parse_size("n*m", &v), None);
        assert_eq!(parse_size("n+m", &v), None);
        assert_eq!(parse_size("k", &v), None);
        assert_eq!(parse_size("k-1", &v), None);
    }

    #[test]
    fn unsupported_size_expr_is_recorded() {
        let mut v = BTreeMap::new();
        v.insert("n".into(), vc_num(VarType::Usize, Some(1), Some(10)));
        let format = vec![FormatBlock::Array(ArrayBlock {
            base: "a".into(),
            len: Some("2*n".into()),
            height: None,
            count: None,
            jagged: false,
        })];
        let r = resolve(&section(v, format));
        assert!(
            r.missing
                .iter()
                .any(|m| m.contains("size expression `2*n`")),
            "missing: {:?}",
            r.missing
        );
    }

    #[test]
    fn size_expr_referencing_unknown_var_is_recorded() {
        // `n-1` but `n` is not declared in vars.
        let format = vec![FormatBlock::Rows(RowsBlock {
            vars: vec!["u".into(), "v".into()],
            len: "n-1".into(),
        })];
        let r = resolve(&section(BTreeMap::new(), format));
        assert!(
            r.missing
                .iter()
                .any(|m| m.contains("size expression `n-1`")),
            "missing: {:?}",
            r.missing
        );
    }

    #[test]
    fn chars_len_size_exprs_are_validated_like_format_sizes() {
        let mut v = BTreeMap::new();
        v.insert("n".into(), vc_num(VarType::Usize, Some(2), Some(10)));
        v.insert(
            "s".into(),
            VarConstraint {
                r#type: VarType::Chars,
                values: Some(vec!["a".into()]),
                len: Some(BoundRepr::Expr("17".into())),
                ..Default::default()
            },
        );
        v.insert(
            "t".into(),
            VarConstraint {
                r#type: VarType::Chars,
                values: Some(vec!["a".into()]),
                len: Some(BoundRepr::Expr("n-1".into())),
                ..Default::default()
            },
        );
        let r = resolve(&section(v, vec![]));
        assert!(r.missing.is_empty(), "unexpected missing: {:?}", r.missing);
        assert_eq!(r.size_terms.get("17"), Some(&SizeTerm::Lit(17)));
        assert_eq!(
            r.size_terms.get("n-1"),
            Some(&SizeTerm::VarOffset("n".into(), -1))
        );
    }

    #[test]
    fn unsupported_chars_len_size_expr_is_recorded() {
        let mut v = BTreeMap::new();
        v.insert("n".into(), vc_num(VarType::Usize, Some(2), Some(10)));
        v.insert(
            "s".into(),
            VarConstraint {
                r#type: VarType::Chars,
                values: Some(vec!["a".into()]),
                len: Some(BoundRepr::Expr("2*n".into())),
                ..Default::default()
            },
        );
        let r = resolve(&section(v, vec![]));
        assert!(
            r.missing
                .iter()
                .any(|m| m.contains("size expression `2*n`")),
            "missing: {:?}",
            r.missing
        );
    }

    #[test]
    fn supported_size_exprs_do_not_record() {
        let mut v = BTreeMap::new();
        v.insert("n".into(), vc_num(VarType::Usize, Some(2), Some(10)));
        v.insert("t".into(), vc_num(VarType::Usize, Some(1), Some(10)));
        v.insert("a".into(), vc_num(VarType::Usize, Some(1), Some(10)));
        v.insert("u".into(), vc_num(VarType::Usize, Some(1), Some(10)));
        v.insert("v".into(), vc_num(VarType::Usize, Some(1), Some(10)));
        let format = vec![
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
            FormatBlock::Array(ArrayBlock {
                base: "a".into(),
                len: Some("(t)+1".into()),
                height: None,
                count: Some("3".into()),
                jagged: false,
            }),
        ];
        let r = resolve(&section(v, format));
        assert!(r.missing.is_empty(), "unexpected missing: {:?}", r.missing);
    }

    #[test]
    fn enum_var_without_range_derives_bounds_from_values() {
        let mut v = BTreeMap::new();
        v.insert(
            "b".into(),
            VarConstraint {
                r#type: VarType::Usize,
                values: Some(vec!["2".into(), "1".into()]),
                ..Default::default()
            },
        );
        let format = vec![FormatBlock::Scalars(ScalarsBlock {
            vars: vec!["b".into()],
        })];
        let r = resolve(&section(v, format));
        assert!(r.missing.is_empty(), "unexpected missing: {:?}", r.missing);
        let info = &r.vars["b"];
        assert_eq!(info.lo, 1);
        assert_eq!(info.hi, Hi::Fixed(2));
    }

    #[test]
    fn empty_enum_values_recorded_as_missing() {
        let mut v = BTreeMap::new();
        v.insert(
            "b".into(),
            VarConstraint {
                r#type: VarType::Usize,
                values: Some(vec![]),
                ..Default::default()
            },
        );
        let r = resolve(&section(v, vec![]));
        assert!(
            r.missing.iter().any(|m| m.contains("`values` is empty")),
            "{:?}",
            r.missing
        );
    }

    #[test]
    fn undeclared_format_variable_recorded_as_missing() {
        let format = vec![FormatBlock::Scalars(ScalarsBlock {
            vars: vec!["x".into()],
        })];
        let r = resolve(&section(BTreeMap::new(), format));
        assert!(
            r.missing
                .iter()
                .any(|m| m.contains("variable `x`") && m.contains("missing from")),
            "{:?}",
            r.missing
        );
    }

    #[test]
    fn chars_array_not_equal_becomes_skipped_warning() {
        let chars_vc = || VarConstraint {
            r#type: VarType::Chars,
            values: Some(vec!["a".into(), "b".into()]),
            len: Some(BoundRepr::Lit(4)),
            ..Default::default()
        };
        let mut v = BTreeMap::new();
        v.insert("s".into(), chars_vc());
        v.insert("t".into(), chars_vc());
        let arr = |base: &str| {
            FormatBlock::Array(ArrayBlock {
                base: base.into(),
                len: None,
                height: None,
                count: Some("2".into()),
                jagged: false,
            })
        };
        let mut s = section(v, vec![arr("s"), arr("t")]);
        s.not_equal = vec![["s".into(), "t".into()]];
        let r = resolve(&s);
        assert!(
            r.skipped
                .iter()
                .any(|m| m.contains("not_equal involving string array `s`")),
            "{:?}",
            r.skipped
        );
    }

    #[test]
    fn ordering_and_not_equal_passthrough() {
        let mut s = section(BTreeMap::new(), vec![]);
        s.ordering = vec![["m".into(), "n".into()]];
        s.not_equal = vec![["a".into(), "b".into()]];
        s.skipped = vec!["weird constraint".into()];
        let r = resolve(&s);
        assert_eq!(r.ordering, vec![("m".into(), "n".into())]);
        assert_eq!(r.not_equal, vec![("a".into(), "b".into())]);
        assert_eq!(r.skipped, vec!["weird constraint".to_string()]);
    }

    #[test]
    fn jagged_arrays_excluded_from_inter_array_constraint() {
        // A jagged array that appears in a not_equal clause must NOT be
        // classified as inter-array-constrained: jagged shapes keep their full
        // Array-strategy pool. The plain-array pair `b`/`c` is still classified,
        // confirming the filter rejects only the jagged side.
        let format = vec![
            FormatBlock::Array(ArrayBlock {
                base: "a".into(),
                len: Some("3".into()),
                height: None,
                count: Some("2".into()),
                jagged: true,
            }),
            FormatBlock::Array(ArrayBlock {
                base: "b".into(),
                len: Some("3".into()),
                height: None,
                count: None,
                jagged: false,
            }),
            FormatBlock::Array(ArrayBlock {
                base: "c".into(),
                len: Some("3".into()),
                height: None,
                count: None,
                jagged: false,
            }),
        ];
        let mut s = section(BTreeMap::new(), format);
        s.not_equal = vec![["a".into(), "b".into()], ["b".into(), "c".into()]];
        let r = resolve(&s);
        // jagged `a` excluded even though it appears in a not_equal clause.
        assert!(!r.inter_array_constrained.contains("a"));
        // the plain-array pair is still classified.
        assert!(r.inter_array_constrained.contains("b"));
        assert!(r.inter_array_constrained.contains("c"));
    }
}
