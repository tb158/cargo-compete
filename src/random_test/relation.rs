//! Enforcement and generation support for persisted `ordering` / `not_equal` pairs.

use super::context::{ArrayCtx, Ctx, StrCtx};
use super::gen::{gen_int, gen_int_array};
use super::spec::ResolvedSpec;
use super::strategy::{CaseStrategy, DeterministicStrategy, RandomStrategy};
use rand::Rng;
use std::collections::HashSet;

/// The pair member opposite `name`, or `None` when `name` is not in the pair.
fn pair_other<'a>(name: &str, pair: &'a (String, String)) -> Option<&'a String> {
    if pair.0 == name {
        Some(&pair.1)
    } else if pair.1 == name {
        Some(&pair.0)
    } else {
        None
    }
}

pub(super) fn effective_array_strategy(
    st: &CaseStrategy,
    name: &str,
    distinct: bool,
    spec: &ResolvedSpec,
) -> CaseStrategy {
    let zero = matches!(st, CaseStrategy::Random(RandomStrategy::ZeroCorner));
    // Only the monotone / mountain shapes survive distinctness; every other
    // deterministic or shape strategy would force duplicates.
    let distinct_compatible = matches!(
        st,
        CaseStrategy::Random(
            RandomStrategy::ArrayMonoInc
                | RandomStrategy::ArrayMonoDec
                | RandomStrategy::ArrayMountain
        )
    );
    let ignored_for_distinct = distinct
        && (matches!(st, CaseStrategy::Deterministic(_))
            || zero
            || (is_array_shape_strategy(st) && !distinct_compatible));
    let ignored_for_inter_array = spec.inter_array_constrained.contains(name)
        && (zero || is_array_shape_strategy(st));
    if ignored_for_distinct || ignored_for_inter_array {
        CaseStrategy::Random(RandomStrategy::Random)
    } else {
        st.clone()
    }
}

/// Check `ordering` / `not_equal` pairs for values already rendered in this
/// scope. Array pairs are compared by flattened position; scalar-array pairs
/// compare the scalar against every known element.
pub(super) fn pairs_ok(
    spec: &ResolvedSpec,
    sc: &Ctx,
    strings: &StrCtx,
    arrays: &ArrayCtx,
    parent: Option<&HashSet<String>>,
    parent_arrays: Option<&HashSet<String>>,
) -> bool {
    let is_local = |v: &str| parent.map(|p| !p.contains(v)).unwrap_or(true);
    let is_local_array = |v: &str| parent_arrays.map(|p| !p.contains(v)).unwrap_or(true);
    let int_val = |v: &str| -> Option<i64> {
        if !is_local(v) {
            return None;
        }
        sc.get(v).copied()
    };
    let str_val = |v: &str| -> Option<&str> {
        if !is_local(v) {
            return None;
        }
        strings.get(v).map(String::as_str)
    };
    let arr_val = |v: &str| -> Option<&[i64]> {
        if !is_local_array(v) {
            return None;
        }
        arrays.get(v).map(Vec::as_slice)
    };

    for (a, b) in &spec.ordering {
        if let (Some(x), Some(y)) = (int_val(a), int_val(b)) {
            if x > y {
                return false;
            }
        }
        if let (Some(x), Some(ys)) = (int_val(a), arr_val(b)) {
            if ys.iter().any(|&y| x > y) {
                return false;
            }
        }
        if let (Some(xs), Some(y)) = (arr_val(a), int_val(b)) {
            if xs.iter().any(|&x| x > y) {
                return false;
            }
        }
        if let (Some(xs), Some(ys)) = (arr_val(a), arr_val(b)) {
            if xs.iter().zip(ys).any(|(&x, &y)| x > y) {
                return false;
            }
        }
    }
    for (a, b) in &spec.not_equal {
        if let (Some(x), Some(y)) = (int_val(a), int_val(b)) {
            if x == y {
                return false;
            }
        }
        if let (Some(x), Some(ys)) = (int_val(a), arr_val(b)) {
            if ys.contains(&x) {
                return false;
            }
        }
        if let (Some(xs), Some(y)) = (arr_val(a), int_val(b)) {
            if xs.contains(&y) {
                return false;
            }
        }
        if let (Some(xs), Some(ys)) = (arr_val(a), arr_val(b)) {
            if xs.iter().zip(ys).any(|(&x, &y)| x == y) {
                return false;
            }
        }
        if let (Some(x), Some(y)) = (str_val(a), str_val(b)) {
            if x == y {
                return false;
            }
        }
    }
    true
}

pub(super) fn narrow_scalar_bounds(
    name: &str,
    lo: i64,
    hi: i64,
    spec: &ResolvedSpec,
    ctx: &Ctx,
    array_ctx: &ArrayCtx,
) -> Option<(i64, i64)> {
    let (mut lo, mut hi) = narrow_bounds_from_scalars(name, lo, hi, spec, ctx)?;
    for (a, b) in &spec.ordering {
        if a == name {
            if let Some(bound) = array_ctx.get(b).and_then(|xs| xs.iter().min()) {
                hi = hi.min(*bound);
            }
        }
        if b == name {
            if let Some(bound) = array_ctx.get(a).and_then(|xs| xs.iter().max()) {
                lo = lo.max(*bound);
            }
        }
    }
    valid_bounds(lo, hi)
}

pub(super) fn narrow_bounds_from_scalars(
    name: &str,
    lo: i64,
    hi: i64,
    spec: &ResolvedSpec,
    ctx: &Ctx,
) -> Option<(i64, i64)> {
    let mut lo = lo;
    let mut hi = hi;
    for (a, b) in &spec.ordering {
        if a == name {
            if let Some(&bound) = ctx.get(b) {
                hi = hi.min(bound);
            }
        }
        if b == name {
            if let Some(&bound) = ctx.get(a) {
                lo = lo.max(bound);
            }
        }
    }
    valid_bounds(lo, hi)
}

fn narrow_element_bounds(
    name: &str,
    index: usize,
    lo: i64,
    hi: i64,
    spec: &ResolvedSpec,
    array_ctx: &ArrayCtx,
) -> Option<(i64, i64)> {
    let mut lo = lo;
    let mut hi = hi;
    for (a, b) in &spec.ordering {
        if a == name {
            if let Some(&bound) = array_ctx.get(b).and_then(|xs| xs.get(index)) {
                hi = hi.min(bound);
            }
        }
        if b == name {
            if let Some(&bound) = array_ctx.get(a).and_then(|xs| xs.get(index)) {
                lo = lo.max(bound);
            }
        }
    }
    valid_bounds(lo, hi)
}

fn valid_bounds(lo: i64, hi: i64) -> Option<(i64, i64)> {
    if lo > hi {
        None
    } else {
        Some((lo, hi))
    }
}

pub(super) fn record_array_values(array_ctx: &mut ArrayCtx, name: &str, values: &[i64]) {
    array_ctx
        .entry(name.to_string())
        .or_default()
        .extend_from_slice(values);
}

fn has_positional_array_bounds(
    name: &str,
    start: usize,
    len: usize,
    spec: &ResolvedSpec,
    array_ctx: &ArrayCtx,
) -> bool {
    spec.ordering.iter().any(|pair| {
        pair_other(name, pair)
            .and_then(|v| array_ctx.get(v))
            .is_some_and(|xs| start < xs.len() && start.saturating_add(len) > 0)
    })
}

pub(super) fn has_array_element_constraints(
    name: &str,
    start: usize,
    len: usize,
    spec: &ResolvedSpec,
    ctx: &Ctx,
    array_ctx: &ArrayCtx,
) -> bool {
    if has_positional_array_bounds(name, start, len, spec, array_ctx) {
        return true;
    }
    spec.not_equal.iter().any(|pair| {
        let Some(other) = pair_other(name, pair) else {
            return false;
        };
        ctx.contains_key(other)
            || array_ctx
                .get(other)
                .is_some_and(|xs| start < xs.len() && start.saturating_add(len) > 0)
    })
}

pub(super) fn not_equal_forbidden_scalar(
    name: &str,
    spec: &ResolvedSpec,
    ctx: &Ctx,
    array_ctx: &ArrayCtx,
) -> HashSet<i64> {
    let mut forbidden = HashSet::new();
    for pair in &spec.not_equal {
        let Some(other) = pair_other(name, pair) else {
            continue;
        };
        if let Some(&x) = ctx.get(other) {
            forbidden.insert(x);
        }
        if let Some(xs) = array_ctx.get(other) {
            forbidden.extend(xs.iter().copied());
        }
    }
    forbidden
}

fn element_satisfies_not_equal(
    name: &str,
    index: usize,
    value: i64,
    spec: &ResolvedSpec,
    ctx: &Ctx,
    array_ctx: &ArrayCtx,
) -> bool {
    spec.not_equal.iter().all(|pair| {
        let Some(other) = pair_other(name, pair) else {
            return true;
        };
        ctx.get(other).is_none_or(|&x| x != value)
            && array_ctx
                .get(other)
                .and_then(|xs| xs.get(index))
                .is_none_or(|&x| x != value)
    })
}

fn not_equal_forbidden_element(
    name: &str,
    index: usize,
    spec: &ResolvedSpec,
    ctx: &Ctx,
    array_ctx: &ArrayCtx,
) -> HashSet<i64> {
    let mut forbidden = HashSet::new();
    for pair in &spec.not_equal {
        let Some(other) = pair_other(name, pair) else {
            continue;
        };
        if let Some(&x) = ctx.get(other) {
            forbidden.insert(x);
        }
        if let Some(&x) = array_ctx.get(other).and_then(|xs| xs.get(index)) {
            forbidden.insert(x);
        }
    }
    forbidden
}

#[allow(clippy::too_many_arguments)]
pub(super) fn gen_int_array_with_positional_bounds(
    st: &CaseStrategy,
    name: &str,
    lo: i64,
    hi: i64,
    len: usize,
    values: Option<&[i64]>,
    distinct: bool,
    spec: &ResolvedSpec,
    ctx: &Ctx,
    array_ctx: &ArrayCtx,
    start: usize,
    rng: &mut impl Rng,
) -> Option<Vec<i64>> {
    let effective = effective_array_strategy(st, name, distinct, spec);
    if !has_array_element_constraints(name, start, len, spec, ctx, array_ctx) {
        return gen_int_array(&effective, lo, hi, len, values, distinct, rng);
    }

    // For arrays constrained only by not_equal (and a uniform domain),
    // sampling a whole array at once is linear and usually succeeds within a
    // few attempts. Scalar orderings are already folded into `lo`/`hi` by the
    // caller (`narrow_bounds_from_scalars`), so generating inside `[lo, hi]`
    // satisfies them automatically; only array-to-array positional ordering
    // (which narrows bounds per index) forces the element-by-element fallback.
    let has_active_ordering = has_positional_array_bounds(name, start, len, spec, array_ctx);
    let is_plain_random = matches!(effective, CaseStrategy::Random(RandomStrategy::Random));
    if !has_active_ordering && (distinct || is_array_shape_strategy(&effective)) {
        let attempts = if is_plain_random { 32 } else { 1 };
        for _ in 0..attempts {
            if crate::interrupt::requested() {
                return None;
            }
            let candidate = gen_int_array(&effective, lo, hi, len, values, distinct, rng)?;
            let valid = candidate.iter().enumerate().all(|(offset, &x)| {
                (offset % 1024 != 0 || !crate::interrupt::requested())
                    && element_satisfies_not_equal(name, start + offset, x, spec, ctx, array_ctx)
            });
            if valid {
                return Some(candidate);
            }
        }
        if !is_plain_random {
            return None;
        }
    }

    let mut out = Vec::with_capacity(len);
    let mut used = HashSet::new();
    for offset in 0..len {
        if offset % 1024 == 0 && crate::interrupt::requested() {
            return None;
        }
        let index = start + offset;
        let (elo, ehi) = narrow_element_bounds(name, index, lo, hi, spec, array_ctx)?;
        let forbidden = not_equal_forbidden_element(name, index, spec, ctx, array_ctx);
        let x = gen_positionally_bounded_int(
            &effective, offset, len, elo, ehi, values, distinct, &used, &forbidden, rng,
        )?;
        if distinct {
            used.insert(x);
        }
        out.push(x);
    }
    Some(out)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn gen_positionally_bounded_int(
    st: &CaseStrategy,
    index: usize,
    len: usize,
    lo: i64,
    hi: i64,
    values: Option<&[i64]>,
    distinct: bool,
    used: &HashSet<i64>,
    forbidden: &HashSet<i64>,
    rng: &mut impl Rng,
) -> Option<i64> {
    if let Some(vs) = values.filter(|vs| !vs.is_empty()) {
        let mut domain: Vec<i64> = vs
            .iter()
            .copied()
            .filter(|&x| {
                lo <= x && x <= hi && !forbidden.contains(&x) && (!distinct || !used.contains(&x))
            })
            .collect();
        domain.sort_unstable();
        domain.dedup();
        return choose_from_domain(st, index, len, &domain, rng);
    }

    if !distinct {
        if matches!(
            st,
            CaseStrategy::Deterministic(DeterministicStrategy::AllMax)
        ) {
            let mut x = hi;
            loop {
                if !forbidden.contains(&x) {
                    return Some(x);
                }
                if x == lo {
                    return None;
                }
                x -= 1;
            }
        }
        if matches!(
            st,
            CaseStrategy::Deterministic(DeterministicStrategy::AllMin)
        ) {
            let mut x = lo;
            loop {
                if !forbidden.contains(&x) {
                    return Some(x);
                }
                if x == hi {
                    return None;
                }
                x += 1;
            }
        }
        let candidate = match st {
            CaseStrategy::Random(RandomStrategy::ArrayAltMaxMin) => {
                if index % 2 == 0 {
                    hi
                } else {
                    lo
                }
            }
            CaseStrategy::Random(RandomStrategy::ArrayOneMaxRestMin) => {
                if index + 1 == len {
                    hi
                } else {
                    lo
                }
            }
            CaseStrategy::Random(RandomStrategy::ZeroCorner) if lo <= 0 && 0 <= hi => 0,
            _ => gen_int(lo, hi, rng),
        };
        if !forbidden.contains(&candidate) {
            return Some(candidate);
        }
        return bounded_distinct_int(st, lo, hi, &HashSet::new(), forbidden, rng);
    }

    bounded_distinct_int(st, lo, hi, used, forbidden, rng)
}

fn choose_from_domain(
    st: &CaseStrategy,
    index: usize,
    len: usize,
    domain: &[i64],
    rng: &mut impl Rng,
) -> Option<i64> {
    if domain.is_empty() {
        return None;
    }
    Some(match st {
        CaseStrategy::Deterministic(DeterministicStrategy::AllMax) => *domain.last().unwrap(),
        CaseStrategy::Deterministic(DeterministicStrategy::AllMin) => domain[0],
        CaseStrategy::Random(RandomStrategy::ArrayAltMaxMin) => {
            if index % 2 == 0 {
                *domain.last().unwrap()
            } else {
                domain[0]
            }
        }
        CaseStrategy::Random(RandomStrategy::ArrayOneMaxRestMin) => {
            if index + 1 == len {
                *domain.last().unwrap()
            } else {
                domain[0]
            }
        }
        CaseStrategy::Random(RandomStrategy::ZeroCorner) if domain.binary_search(&0).is_ok() => 0,
        _ => domain[rng.gen_range(0..domain.len())],
    })
}

pub(super) fn bounded_distinct_int(
    st: &CaseStrategy,
    lo: i64,
    hi: i64,
    used: &HashSet<i64>,
    forbidden: &HashSet<i64>,
    rng: &mut impl Rng,
) -> Option<i64> {
    let blocked_count = |lo: i64, hi: i64, used: &HashSet<i64>, forbidden: &HashSet<i64>| {
        used.union(forbidden)
            .filter(|&&x| lo <= x && x <= hi)
            .count()
    };
    let span = (hi as i128) - (lo as i128) + 1;
    let max_blocked = used.len().saturating_add(forbidden.len()) as i128;
    if span <= max_blocked && span <= blocked_count(lo, hi, used, forbidden) as i128 {
        return None;
    }
    let prefer_min = matches!(
        st,
        CaseStrategy::Deterministic(DeterministicStrategy::AllMin)
    );
    let prefer_max = matches!(
        st,
        CaseStrategy::Deterministic(DeterministicStrategy::AllMax)
    );
    if prefer_min || prefer_max {
        let mut x = if prefer_min { lo } else { hi };
        loop {
            if !used.contains(&x) && !forbidden.contains(&x) {
                return Some(x);
            }
            if prefer_min {
                if x == hi {
                    return None;
                }
                x += 1;
            } else {
                if x == lo {
                    return None;
                }
                x -= 1;
            }
        }
    }
    for _ in 0..1000 {
        let x = gen_int(lo, hi, rng);
        if !used.contains(&x) && !forbidden.contains(&x) {
            return Some(x);
        }
    }
    let mut x = lo;
    let mut steps = 0u64;
    while x <= hi {
        if steps % 1024 == 0 && crate::interrupt::requested() {
            return None;
        }
        steps += 1;
        if !used.contains(&x) && !forbidden.contains(&x) {
            return Some(x);
        }
        if x == i64::MAX {
            break;
        }
        x += 1;
    }
    None
}

fn is_array_shape_strategy(st: &CaseStrategy) -> bool {
    matches!(
        st,
        CaseStrategy::Random(
            RandomStrategy::ArrayMonoInc
                | RandomStrategy::ArrayMonoDec
                | RandomStrategy::ArrayAllSame
                | RandomStrategy::ArrayAltMaxMin
                | RandomStrategy::ArrayMountain
                | RandomStrategy::ArrayOneMaxRestMin
                | RandomStrategy::ArrayNarrowRange
                | RandomStrategy::ArrayPeriodic
        )
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::{
        ArrayBlock, BoundRepr, FormatBlock, RandomTestSection, VarConstraint, VarType,
    };
    use crate::random_test::spec::resolve;
    use rand::rngs::SmallRng;
    use rand::SeedableRng as _;
    use std::collections::{BTreeMap, HashMap, HashSet};

    fn vc(distinct: bool) -> VarConstraint {
        VarConstraint {
            r#type: VarType::Usize,
            range: Some([BoundRepr::Lit(1), BoundRepr::Lit(10)]),
            all_distinct: distinct,
            ..Default::default()
        }
    }

    fn spec(distinct: bool, inter_array: bool) -> ResolvedSpec {
        let mut vars = BTreeMap::new();
        vars.insert("a".into(), vc(distinct));
        vars.insert("b".into(), vc(false));
        let mut section = RandomTestSection {
            vars,
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
        if inter_array {
            section.not_equal = vec![["a".into(), "b".into()]];
        }
        resolve(&section)
    }

    #[test]
    fn all_distinct_ignores_only_incompatible_strategies() {
        let spec = spec(true, false);
        let random = CaseStrategy::Random(RandomStrategy::Random);
        for st in [
            CaseStrategy::Deterministic(DeterministicStrategy::AllMax),
            CaseStrategy::Deterministic(DeterministicStrategy::AllMin),
            CaseStrategy::Random(RandomStrategy::ZeroCorner),
            CaseStrategy::Random(RandomStrategy::ArrayAllSame),
            CaseStrategy::Random(RandomStrategy::ArrayAltMaxMin),
            CaseStrategy::Random(RandomStrategy::ArrayOneMaxRestMin),
            CaseStrategy::Random(RandomStrategy::ArrayNarrowRange),
            CaseStrategy::Random(RandomStrategy::ArrayPeriodic),
        ] {
            assert_eq!(effective_array_strategy(&st, "a", true, &spec), random);
        }
        for st in [
            CaseStrategy::Random(RandomStrategy::ArrayMonoInc),
            CaseStrategy::Random(RandomStrategy::ArrayMonoDec),
            CaseStrategy::Random(RandomStrategy::ArrayMountain),
            CaseStrategy::Random(RandomStrategy::MaxSize),
        ] {
            assert_eq!(effective_array_strategy(&st, "a", true, &spec), st);
        }
    }

    #[test]
    fn inter_array_ignores_array_shapes_but_keeps_extremes() {
        let spec = spec(false, true);
        let random = CaseStrategy::Random(RandomStrategy::Random);
        for st in [
            CaseStrategy::Random(RandomStrategy::ZeroCorner),
            CaseStrategy::Random(RandomStrategy::ArrayMonoInc),
            CaseStrategy::Random(RandomStrategy::ArrayMonoDec),
            CaseStrategy::Random(RandomStrategy::ArrayAllSame),
            CaseStrategy::Random(RandomStrategy::ArrayAltMaxMin),
            CaseStrategy::Random(RandomStrategy::ArrayMountain),
            CaseStrategy::Random(RandomStrategy::ArrayOneMaxRestMin),
            CaseStrategy::Random(RandomStrategy::ArrayNarrowRange),
            CaseStrategy::Random(RandomStrategy::ArrayPeriodic),
        ] {
            assert_eq!(effective_array_strategy(&st, "a", false, &spec), random);
        }
        for st in [
            CaseStrategy::Deterministic(DeterministicStrategy::AllMax),
            CaseStrategy::Deterministic(DeterministicStrategy::AllMin),
        ] {
            assert_eq!(effective_array_strategy(&st, "a", false, &spec), st);
        }
    }

    #[test]
    fn all_distinct_rule_wins_for_inter_array_extremes() {
        let spec = spec(true, true);
        let random = CaseStrategy::Random(RandomStrategy::Random);
        for st in [
            CaseStrategy::Deterministic(DeterministicStrategy::AllMax),
            CaseStrategy::Deterministic(DeterministicStrategy::AllMin),
        ] {
            assert_eq!(effective_array_strategy(&st, "a", true, &spec), random);
        }
    }

    #[test]
    fn distinct_inter_array_scale_stays_near_linear() {
        use std::time::{Duration, Instant};

        // abc435-d core: x and y are length-m all_distinct arrays in [1, n] with
        // x[i] != y[i]. The scalar orderings x <= n / y <= n are uniform (the
        // caller already folds them into [lo, hi]); they must NOT push the
        // distinct array onto the O(m^2) element-by-element fallback. At m == n
        // both columns are full permutations, the worst case for that fallback,
        // so the whole-array fast path is what keeps this near-linear.
        let m = 300_000usize;
        let n = 300_000i64;
        let distinct_var = VarConstraint {
            r#type: VarType::Usize,
            range: Some([BoundRepr::Lit(1), BoundRepr::Lit(n)]),
            all_distinct: true,
            ..Default::default()
        };
        let mut vars = BTreeMap::new();
        vars.insert("x".into(), distinct_var.clone());
        vars.insert("y".into(), distinct_var);
        let mut section = RandomTestSection {
            vars,
            format: vec![
                FormatBlock::Array(ArrayBlock {
                    base: "x".into(),
                    len: Some("m".into()),
                    height: None,
                    count: None,
                    jagged: false,
                }),
                FormatBlock::Array(ArrayBlock {
                    base: "y".into(),
                    len: Some("m".into()),
                    height: None,
                    count: None,
                    jagged: false,
                }),
            ],
            ..Default::default()
        };
        section.ordering = vec![["x".into(), "n".into()], ["y".into(), "n".into()]];
        section.not_equal = vec![["x".into(), "y".into()]];
        let spec = resolve(&section);
        assert!(spec.inter_array_constrained.contains("y"));

        // x already emitted as the identity permutation; n known as a scalar so
        // the scalar ordering y <= n is "active" in the sense the old gate used.
        let mut ctx: Ctx = HashMap::new();
        ctx.insert("n".into(), n);
        let mut array_ctx: ArrayCtx = HashMap::new();
        array_ctx.insert("x".into(), (1..=n).collect());

        let mut rng = SmallRng::seed_from_u64(0xABC435);
        let start = Instant::now();
        let y = gen_int_array_with_positional_bounds(
            &CaseStrategy::Random(RandomStrategy::Random),
            "y",
            1,
            n,
            m,
            None,
            true,
            &spec,
            &ctx,
            &array_ctx,
            0,
            &mut rng,
        )
        .expect("distinct y with x[i] != y[i] must be generatable");
        let elapsed = start.elapsed();

        assert!(
            elapsed < Duration::from_secs(20),
            "generation took {:?}; expected near-linear (whole-array) time",
            elapsed
        );
        assert_eq!(y.len(), m);
        assert_eq!(
            y.iter().copied().collect::<HashSet<_>>().len(),
            m,
            "y must stay all_distinct"
        );
        for (i, &v) in y.iter().enumerate() {
            assert!((1..=n).contains(&v), "y[{}] = {} out of [1, {}]", i, v, n);
            assert_ne!(v, (i as i64) + 1, "y[{}] must differ from x[{}]", i, i);
        }
    }
}
