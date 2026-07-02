//! Structural-size decision, effective per-variable ranges, and ctx-free
//! pure value-generation primitives.
//!
//! Everything here is a mechanical function of the resolved yml spec plus the
//! chosen [`CaseStrategy`]. The FormatBlock walker that drives these primitives
//! (general array-length resolution, nesting, ordering / not_equal rejection)
//! lives in [`super::render`].

use super::spec::{Hi, ResolvedSpec, VarInfo};
use super::strategy::{CaseStrategy, DeterministicStrategy, RandomStrategy};
use rand::seq::index;
use rand::Rng;
use std::collections::HashMap;

// ─── structural size decision ─────────────────────────────────────────────────

/// Decided values for the `sum_limit` denominators under the chosen strategy.
#[derive(Debug, Clone, Default)]
pub(crate) struct StructuralSizes {
    /// `TestCases.count` variable name → decided `T`.
    pub test_cases: Option<(String, i64)>,
    /// jagged `Array.count` variable name → decided row count `n`.
    pub jagged_counts: HashMap<String, i64>,
}

fn concrete_hi(info: &VarInfo) -> i64 {
    match info.hi {
        Hi::Fixed(n) => n,
        // A denominator var being itself sum-limited is not expected; fall back
        // to the raw limit so the size stays finite.
        Hi::SumLimited(l) => l,
    }
}

/// `(lo, hi)` bounds for a structural size variable, taken verbatim from the
/// resolved yml (no implicit cap — a faithful mechanical renderer). A reversed
/// pair from a contradictory yml is swapped, not silently clamped. A size var
/// absent from `vars` is an unrecoverable gap that the runner surfaces and
/// aborts on; `(0, 0)` here is never actually rendered.
fn size_bounds(spec: &ResolvedSpec, name: &str) -> (i64, i64) {
    let (lo, hi) = match spec.vars.get(name) {
        Some(i) => (i.lo, concrete_hi(i)),
        None => (0, 0),
    };
    if lo > hi {
        (hi, lo)
    } else {
        (lo, hi)
    }
}

fn decide_one(st: &CaseStrategy, lo: i64, hi: i64, sum_present: bool, rng: &mut impl Rng) -> i64 {
    if sum_present && matches!(st, CaseStrategy::Random(RandomStrategy::MaxSize)) {
        // Concentrate the whole sum budget into a single case / row.
        return 1.max(lo).min(hi);
    }
    strategy_size_value(st, lo, hi, rng)
}

/// Strategy-driven size value over effective bounds `(lo, hi)`; non-size
/// strategies sample uniformly.
pub(super) fn strategy_size_value(
    st: &CaseStrategy,
    lo: i64,
    hi: i64,
    rng: &mut impl Rng,
) -> i64 {
    match st {
        CaseStrategy::Deterministic(DeterministicStrategy::AllMax) => hi,
        CaseStrategy::Deterministic(DeterministicStrategy::AllMin) => lo,
        CaseStrategy::Random(RandomStrategy::SmallSize(k)) => (*k).max(lo).min(hi),
        CaseStrategy::Random(RandomStrategy::MaxSize) => hi,
        _ => gen_int(lo, hi, rng),
    }
}

/// Decide the `sum_limit` denominator sizes (test-case count `T`, jagged row
/// counts) under the chosen strategy.
pub(crate) fn decide_structural_sizes(
    spec: &ResolvedSpec,
    st: &CaseStrategy,
    rng: &mut impl Rng,
) -> StructuralSizes {
    let any_sum_limit = spec.vars.values().any(|v| v.sum_limit.is_some());

    let test_cases = spec.test_cases_count_var.as_ref().map(|tv| {
        let (lo, hi) = size_bounds(spec, tv);
        (tv.clone(), decide_one(st, lo, hi, any_sum_limit, rng))
    });

    let mut jagged_counts = HashMap::new();
    for (len_var, count_var) in &spec.jagged_len_to_count {
        let (lo, hi) = size_bounds(spec, count_var);
        let len_sum_limited = spec
            .vars
            .get(len_var)
            .map(|v| v.sum_limit.is_some())
            .unwrap_or(false);
        jagged_counts.insert(
            count_var.clone(),
            decide_one(st, lo, hi, len_sum_limited, rng),
        );
    }

    StructuralSizes {
        test_cases,
        jagged_counts,
    }
}

// ─── effective range ──────────────────────────────────────────────────────────

/// Resolve a variable's effective `(lo, hi)`. A `sum_limit` `L` caps the upper
/// bound at `L / denom` where `denom` is the variable's sum denominator (its
/// jagged row count, or `T`, or `1` when no such structure exists). This cap
/// applies both to `Hi::SumLimited(L)` and to a `Hi::Fixed(n)` that carries a
/// coexisting `sum_limit` (in which case the smaller of `n` and the cap wins).
pub(crate) fn effective_lo_hi(
    name: &str,
    info: &VarInfo,
    sizes: &StructuralSizes,
    spec: &ResolvedSpec,
) -> (i64, i64) {
    let lo = info.lo;
    let sum_cap = |l: i64| -> i64 {
        let denom = if let Some(cv) = spec.jagged_len_to_count.get(name) {
            sizes.jagged_counts.get(cv).copied().unwrap_or(1)
        } else if let Some((_, t)) = &sizes.test_cases {
            *t
        } else {
            1
        };
        (l / denom.max(1)).max(1)
    };
    // A `range` upper bound forces `Hi::Fixed`, so a coexisting `sum_limit`
    // would otherwise be ignored — apply the dynamic `L/denom` cap here too.
    let hi = match info.hi {
        Hi::Fixed(n) => match info.sum_limit {
            Some(l) => n.min(sum_cap(l)),
            None => n,
        },
        Hi::SumLimited(l) => sum_cap(l),
    };
    if lo > hi {
        (hi, lo)
    } else {
        (lo, hi)
    }
}

// ─── pure value primitives ────────────────────────────────────────────────────

/// Uniform integer in `[lo, hi]` (inclusive); swaps a reversed range.
pub(crate) fn gen_int(lo: i64, hi: i64, rng: &mut impl Rng) -> i64 {
    let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
    if lo == hi {
        lo
    } else {
        rng.gen_range(lo..=hi)
    }
}

fn uniform_vec(lo: i64, hi: i64, len: usize, rng: &mut impl Rng) -> Vec<i64> {
    (0..len).map(|_| gen_int(lo, hi, rng)).collect()
}

/// An integer array of length `len`, shaped by `st`.
///
/// With an enum domain (`values`), every element is selected from that domain.
/// Without one, the array is generated over `[lo, hi]`.
///
/// When `distinct` is set, all elements are pairwise distinct. Only the
/// monotone / mountain strategies keep a meaningful shape on a distinct
/// array; every other strategy (`AllMax`/`AllMin`/`ZeroCorner`/`AllSame`/
/// `AltMaxMin`/`OneMaxRestMin`/`NarrowRange`/`Periodic` and the size /
/// `Random` strategies) cannot impose its shape without producing
/// duplicates, so it emits a plain distinct permutation instead — the
/// strategy still applies normally to scalars / non-distinct arrays.
/// Returns `None` when distinctness is infeasible for this case (value
/// domain narrower than `len`, or too wide to index); the caller resamples
/// the case rather than emitting a constraint-violating array.
pub(crate) fn gen_int_array(
    st: &CaseStrategy,
    lo: i64,
    hi: i64,
    len: usize,
    values: Option<&[i64]>,
    distinct: bool,
    rng: &mut impl Rng,
) -> Option<Vec<i64>> {
    if len == 0 {
        return Some(Vec::new());
    }

    if let Some(vs) = values.filter(|vs| !vs.is_empty()) {
        // An enum domain reduces to its sorted-unique index domain: run the
        // strategy over indices `[0, k-1]`, then map back to values.
        // Multiplicity of duplicate entries in `values` is ignored.
        let mut domain = vs.to_vec();
        domain.sort_unstable();
        domain.dedup();
        // Index 0 always lies in the index range, so ZeroCorner would
        // degenerate to "all domain minimum"; treat the value 0 directly.
        let effective = if matches!(st, CaseStrategy::Random(RandomStrategy::ZeroCorner)) {
            if !distinct && domain.binary_search(&0).is_ok() {
                return Some(vec![0; len]);
            }
            CaseStrategy::Random(RandomStrategy::Random)
        } else {
            st.clone()
        };
        let idxs = gen_int_array(
            &effective,
            0,
            (domain.len() - 1) as i64,
            len,
            None,
            distinct,
            rng,
        )?;
        return Some(idxs.into_iter().map(|i| domain[i as usize]).collect());
    }

    let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };

    if distinct {
        let span_i128 = (hi as i128) - (lo as i128) + 1;
        if span_i128 < len as i128 || span_i128 > usize::MAX as i128 {
            return None;
        }
        let span = span_i128 as usize;
        let mut v: Vec<i64> = index::sample(rng, span, len)
            .into_iter()
            .map(|i| lo + i as i64)
            .collect();
        match st {
            CaseStrategy::Random(RandomStrategy::ArrayMonoInc) => v.sort_unstable(),
            CaseStrategy::Random(RandomStrategy::ArrayMonoDec) => {
                v.sort_unstable_by(|a, b| b.cmp(a));
            }
            CaseStrategy::Random(RandomStrategy::ArrayMountain) => {
                v.sort_unstable();
                let h = len.div_ceil(2);
                v[h..].reverse();
            }
            _ => {}
        }
        return Some(v);
    }

    Some(match st {
        CaseStrategy::Deterministic(DeterministicStrategy::AllMax) => vec![hi; len],
        CaseStrategy::Deterministic(DeterministicStrategy::AllMin) => vec![lo; len],
        CaseStrategy::Random(RandomStrategy::ArrayMonoInc) => {
            let mut v = uniform_vec(lo, hi, len, rng);
            v.sort_unstable();
            v
        }
        CaseStrategy::Random(RandomStrategy::ArrayMonoDec) => {
            let mut v = uniform_vec(lo, hi, len, rng);
            v.sort_unstable_by(|a, b| b.cmp(a));
            v
        }
        CaseStrategy::Random(RandomStrategy::ArrayAllSame) => {
            let x = gen_int(lo, hi, rng);
            vec![x; len]
        }
        CaseStrategy::Random(RandomStrategy::ArrayAltMaxMin) => {
            let phase = rng.gen_range(0..2usize);
            (0..len)
                .map(|i| if (i + phase) % 2 == 0 { hi } else { lo })
                .collect()
        }
        CaseStrategy::Random(RandomStrategy::ArrayMountain) => {
            let mut v = uniform_vec(lo, hi, len, rng);
            let h = len.div_ceil(2);
            v[..h].sort_unstable();
            v[h..].sort_unstable_by(|a, b| b.cmp(a));
            v
        }
        CaseStrategy::Random(RandomStrategy::ArrayOneMaxRestMin) => {
            let mut v = vec![lo; len];
            let p = rng.gen_range(0..len);
            v[p] = hi;
            v
        }
        CaseStrategy::Random(RandomStrategy::ArrayNarrowRange) => {
            if hi > lo {
                let b = rng.gen_range(lo..hi);
                (0..len)
                    .map(|_| if rng.gen_bool(0.5) { b } else { b + 1 })
                    .collect()
            } else {
                vec![lo; len]
            }
        }
        CaseStrategy::Random(RandomStrategy::ArrayPeriodic) => {
            let cap = 5usize.min(len.max(2));
            let p = rng.gen_range(2..=cap);
            let base: Vec<i64> = (0..p).map(|_| gen_int(lo, hi, rng)).collect();
            (0..len).map(|i| base[i % p]).collect()
        }
        CaseStrategy::Random(RandomStrategy::ZeroCorner) if lo <= 0 && 0 <= hi => {
            vec![0; len]
        }
        _ => uniform_vec(lo, hi, len, rng),
    })
}

/// A string of length `len` over `charset` (assumed non-empty). The chosen
/// strategy is applied on the charset index domain `[0, charset.len()-1]`.
/// `row_info = Some((row, total, phase))` makes `ArrayAltMaxMin` form a 2-D
/// checkerboard across rows (shared `phase`, row-dependent parity).
pub(crate) fn gen_string(
    st: &CaseStrategy,
    charset: &[char],
    len: usize,
    row_info: Option<(usize, usize, usize)>,
    rng: &mut impl Rng,
) -> String {
    if len == 0 {
        return String::new();
    }
    let clen = charset.len();
    if clen == 1 {
        return std::iter::repeat_n(charset[0], len).collect();
    }
    match st {
        CaseStrategy::Random(RandomStrategy::ArrayAltMaxMin) => {
            let phase = row_info
                .map(|(_, _, p)| p % 2)
                .unwrap_or_else(|| rng.gen_range(0..2usize));
            let roff = row_info.map(|(r, _, _)| r).unwrap_or(0);
            (0..len)
                .map(|i| {
                    if (i + phase + roff) % 2 == 0 {
                        charset[clen - 1]
                    } else {
                        charset[0]
                    }
                })
                .collect()
        }
        _ => {
            let idxs = gen_int_array(st, 0, (clen - 1) as i64, len, None, false, rng)
                .expect("non-distinct gen_int_array is always Some");
            idxs.iter().map(|&i| charset[i as usize]).collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::spec::resolve;
    use super::*;
    use crate::parse::{
        ArrayBlock, BoundRepr, FormatBlock, RandomTestSection, ScalarsBlock, TestCasesBlock,
        VarConstraint, VarType,
    };
    use rand::rngs::SmallRng;
    use rand::SeedableRng as _;
    use std::collections::BTreeMap;

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

    fn mkspec(vars: BTreeMap<String, VarConstraint>, format: Vec<FormatBlock>) -> ResolvedSpec {
        resolve(&RandomTestSection {
            vars,
            format,
            ..Default::default()
        })
    }

    fn vinfo(lo: i64, hi: Hi) -> VarInfo {
        VarInfo {
            ty: VarType::Usize,
            lo,
            hi,
            values: None,
            charset: None,
            len: None,
            all_distinct: false,
            sum_limit: None,
        }
    }

    #[test]
    fn decide_structural_sizes_per_strategy() {
        let mut v = BTreeMap::new();
        v.insert("t".into(), vc(1, 100));
        let spec = mkspec(
            v,
            vec![FormatBlock::TestCases(TestCasesBlock {
                count: "t".into(),
                format: vec![FormatBlock::Scalars(ScalarsBlock {
                    vars: vec!["x".into()],
                })],
            })],
        );
        let mut r = rng();
        let max = decide_structural_sizes(
            &spec,
            &CaseStrategy::Deterministic(DeterministicStrategy::AllMax),
            &mut r,
        );
        assert_eq!(max.test_cases, Some(("t".into(), 100)));
        let min = decide_structural_sizes(
            &spec,
            &CaseStrategy::Deterministic(DeterministicStrategy::AllMin),
            &mut r,
        );
        assert_eq!(min.test_cases, Some(("t".into(), 1)));
        let small = decide_structural_sizes(
            &spec,
            &CaseStrategy::Random(RandomStrategy::SmallSize(2)),
            &mut r,
        );
        assert_eq!(small.test_cases, Some(("t".into(), 2)));
        // No sum_limit anywhere ⇒ MaxSize keeps T maxed.
        let maxsize = decide_structural_sizes(
            &spec,
            &CaseStrategy::Random(RandomStrategy::MaxSize),
            &mut r,
        );
        assert_eq!(maxsize.test_cases, Some(("t".into(), 100)));
    }

    #[test]
    fn maxsize_concentrates_sum_into_one_case() {
        let mut v = BTreeMap::new();
        v.insert("t".into(), vc(1, 100));
        let mut sumvar = VarConstraint {
            r#type: VarType::Usize,
            range: Some([BoundRepr::Lit(1), BoundRepr::Expr("_".into())]),
            ..Default::default()
        };
        sumvar.sum_limit = Some(50);
        v.insert("k".into(), sumvar);
        let spec = mkspec(
            v,
            vec![FormatBlock::TestCases(TestCasesBlock {
                count: "t".into(),
                format: vec![FormatBlock::Scalars(ScalarsBlock {
                    vars: vec!["k".into()],
                })],
            })],
        );
        let s = decide_structural_sizes(
            &spec,
            &CaseStrategy::Random(RandomStrategy::MaxSize),
            &mut rng(),
        );
        assert_eq!(s.test_cases, Some(("t".into(), 1)));
    }

    #[test]
    fn resolve_populates_sum_denominators() {
        let spec = mkspec(
            BTreeMap::new(),
            vec![FormatBlock::TestCases(TestCasesBlock {
                count: "t".into(),
                format: vec![FormatBlock::Array(ArrayBlock {
                    base: "a".into(),
                    len: Some("l".into()),
                    height: None,
                    count: Some("n".into()),
                    jagged: true,
                })],
            })],
        );
        assert_eq!(spec.test_cases_count_var, Some("t".into()));
        assert_eq!(spec.jagged_len_to_count.get("l"), Some(&"n".to_string()));
    }

    #[test]
    fn effective_lo_hi_variants() {
        // `effective_lo_hi` reads only `spec.jagged_len_to_count`; an empty spec
        // exercises the non-jagged `sum_cap` paths (`T` from `sizes`, else 1).
        let spec = mkspec(BTreeMap::new(), vec![]);

        let fixed = vinfo(2, Hi::Fixed(50));
        assert_eq!(
            effective_lo_hi("x", &fixed, &StructuralSizes::default(), &spec),
            (2, 50)
        );

        let with_t = StructuralSizes {
            test_cases: Some(("t".into(), 4)),
            jagged_counts: HashMap::new(),
        };
        let sl = vinfo(1, Hi::SumLimited(100));
        assert_eq!(effective_lo_hi("x", &sl, &with_t, &spec), (1, 25));

        // No denominator structure ⇒ divide by 1.
        assert_eq!(
            effective_lo_hi("x", &sl, &StructuralSizes::default(), &spec),
            (1, 100)
        );
    }

    #[test]
    fn gen_int_basics() {
        let mut r = rng();
        assert_eq!(gen_int(5, 5, &mut r), 5);
        for _ in 0..200 {
            let v = gen_int(10, 3, &mut r);
            assert!((3..=10).contains(&v));
            let w = gen_int(1, 100, &mut r);
            assert!((1..=100).contains(&w));
        }
    }

    #[test]
    fn gen_int_array_shapes() {
        let mut r = rng();
        let mono_inc = gen_int_array(
            &CaseStrategy::Random(RandomStrategy::ArrayMonoInc),
            1,
            100,
            30,
            None,
            false,
            &mut r,
        )
        .unwrap();
        assert_eq!(mono_inc.len(), 30);
        assert!(mono_inc.windows(2).all(|w| w[0] <= w[1]));

        let mono_dec = gen_int_array(
            &CaseStrategy::Random(RandomStrategy::ArrayMonoDec),
            1,
            100,
            30,
            None,
            false,
            &mut r,
        )
        .unwrap();
        assert!(mono_dec.windows(2).all(|w| w[0] >= w[1]));

        let same = gen_int_array(
            &CaseStrategy::Random(RandomStrategy::ArrayAllSame),
            1,
            100,
            20,
            None,
            false,
            &mut r,
        )
        .unwrap();
        assert!(same.windows(2).all(|w| w[0] == w[1]));

        let alt = gen_int_array(
            &CaseStrategy::Random(RandomStrategy::ArrayAltMaxMin),
            1,
            100,
            20,
            None,
            false,
            &mut r,
        )
        .unwrap();
        assert!(alt.iter().all(|&x| x == 1 || x == 100));

        let zero = gen_int_array(
            &CaseStrategy::Random(RandomStrategy::ZeroCorner),
            -10,
            10,
            15,
            None,
            false,
            &mut r,
        )
        .unwrap();
        assert!(zero.iter().all(|&x| x == 0));

        let zero_with_upper_bound = gen_int_array(
            &CaseStrategy::Random(RandomStrategy::ZeroCorner),
            -10,
            0,
            15,
            None,
            false,
            &mut r,
        )
        .unwrap();
        assert!(zero_with_upper_bound.iter().all(|&x| x == 0));

        let zero_with_lower_bound = gen_int_array(
            &CaseStrategy::Random(RandomStrategy::ZeroCorner),
            0,
            10,
            15,
            None,
            false,
            &mut r,
        )
        .unwrap();
        assert!(zero_with_lower_bound.iter().all(|&x| x == 0));

        let periodic = gen_int_array(
            &CaseStrategy::Random(RandomStrategy::ArrayPeriodic),
            1,
            1000,
            40,
            None,
            false,
            &mut r,
        )
        .unwrap();
        assert_eq!(periodic.len(), 40);
        let p = (2..=5)
            .find(|&p| {
                periodic
                    .iter()
                    .enumerate()
                    .all(|(i, &x)| x == periodic[i % p])
            })
            .expect("array should be periodic with period 2..=5");
        assert!((2..=5).contains(&p));
    }

    #[test]
    fn gen_int_array_enum_values_only() {
        let mut r = rng();
        let vals = [1i64, 2];

        let maxed = gen_int_array(
            &CaseStrategy::Deterministic(DeterministicStrategy::AllMax),
            0,
            0,
            8,
            Some(&vals),
            false,
            &mut r,
        )
        .unwrap();
        assert!(maxed.iter().all(|&x| x == 2));

        let alt = gen_int_array(
            &CaseStrategy::Random(RandomStrategy::ArrayAltMaxMin),
            0,
            0,
            9,
            Some(&vals),
            false,
            &mut r,
        )
        .unwrap();
        assert!(alt.iter().all(|&x| vals.contains(&x)));
        assert!(alt.contains(&1));
        assert!(alt.contains(&2));

        let zero = gen_int_array(
            &CaseStrategy::Random(RandomStrategy::ZeroCorner),
            0,
            0,
            20,
            Some(&vals),
            false,
            &mut r,
        )
        .unwrap();
        assert!(zero.iter().all(|&x| vals.contains(&x)));
    }

    fn is_distinct(v: &[i64]) -> bool {
        let mut s = v.to_vec();
        s.sort_unstable();
        s.dedup();
        s.len() == v.len()
    }

    #[test]
    fn gen_int_array_distinct() {
        let mut r = rng();
        let d = gen_int_array(
            &CaseStrategy::Random(RandomStrategy::Random),
            1,
            10,
            5,
            None,
            true,
            &mut r,
        )
        .unwrap();
        assert_eq!(d.len(), 5);
        assert!(is_distinct(&d));
        assert!(d.iter().all(|&x| (1..=10).contains(&x)));

        // span < len ⇒ infeasible ⇒ None (caller resamples / backfills),
        // never a constraint-violating array.
        assert!(gen_int_array(
            &CaseStrategy::Random(RandomStrategy::Random),
            1,
            3,
            10,
            None,
            true,
            &mut r,
        )
        .is_none());

        let vals = [10i64, 20, 30];
        let e = gen_int_array(
            &CaseStrategy::Random(RandomStrategy::Random),
            0,
            0,
            3,
            Some(&vals),
            true,
            &mut r,
        )
        .unwrap();
        assert!(is_distinct(&e));
        assert!(e.iter().all(|x| vals.contains(x)));

        assert!(gen_int_array(
            &CaseStrategy::Random(RandomStrategy::Random),
            0,
            0,
            4,
            Some(&vals),
            true,
            &mut r,
        )
        .is_none());
    }

    #[test]
    fn gen_int_array_distinct_shapes() {
        let mut r = rng();
        let inc = gen_int_array(
            &CaseStrategy::Random(RandomStrategy::ArrayMonoInc),
            1,
            1000,
            30,
            None,
            true,
            &mut r,
        )
        .unwrap();
        assert!(is_distinct(&inc));
        assert!(inc.windows(2).all(|w| w[0] < w[1]));

        let dec = gen_int_array(
            &CaseStrategy::Random(RandomStrategy::ArrayMonoDec),
            1,
            1000,
            30,
            None,
            true,
            &mut r,
        )
        .unwrap();
        assert!(is_distinct(&dec));
        assert!(dec.windows(2).all(|w| w[0] > w[1]));

        let mtn = gen_int_array(
            &CaseStrategy::Random(RandomStrategy::ArrayMountain),
            1,
            1000,
            21,
            None,
            true,
            &mut r,
        )
        .unwrap();
        assert!(is_distinct(&mtn));
        let peak = mtn.iter().enumerate().max_by_key(|&(_, &x)| x).unwrap().0;
        assert!(mtn[..=peak].windows(2).all(|w| w[0] < w[1]));
        assert!(mtn[peak..].windows(2).all(|w| w[0] > w[1]));

        // A strategy with no distinct-compatible shape (AllMax) must emit a
        // plain distinct permutation, NOT a constant array.
        let neutral = gen_int_array(
            &CaseStrategy::Deterministic(DeterministicStrategy::AllMax),
            1,
            1000,
            10,
            None,
            true,
            &mut r,
        )
        .unwrap();
        assert!(is_distinct(&neutral));
        assert!(neutral.iter().any(|&x| x != 1000));
    }

    #[test]
    fn gen_string_basics() {
        let mut r = rng();
        let cs = ['a', 'b', 'c'];
        let lo = gen_string(
            &CaseStrategy::Deterministic(DeterministicStrategy::AllMin),
            &cs,
            4,
            None,
            &mut r,
        );
        assert_eq!(lo, "aaaa");
        let hi = gen_string(
            &CaseStrategy::Deterministic(DeterministicStrategy::AllMax),
            &cs,
            4,
            None,
            &mut r,
        );
        assert_eq!(hi, "cccc");
        let same = gen_string(
            &CaseStrategy::Random(RandomStrategy::ArrayAllSame),
            &cs,
            6,
            None,
            &mut r,
        );
        let first = same.chars().next().unwrap();
        assert!(same.chars().all(|c| c == first));
        let rnd = gen_string(
            &CaseStrategy::Random(RandomStrategy::Random),
            &cs,
            8,
            None,
            &mut r,
        );
        assert_eq!(rnd.chars().count(), 8);
        assert!(rnd.chars().all(|c| cs.contains(&c)));
    }

    #[test]
    fn gen_string_2d_checkerboard() {
        let mut r = rng();
        let cs = ['a', 'z'];
        let st = CaseStrategy::Random(RandomStrategy::ArrayAltMaxMin);
        let row0 = gen_string(&st, &cs, 4, Some((0, 3, 0)), &mut r);
        let row1 = gen_string(&st, &cs, 4, Some((1, 3, 0)), &mut r);
        let row2 = gen_string(&st, &cs, 4, Some((2, 3, 0)), &mut r);
        // Adjacent rows differ at every column (vertical checkerboard).
        assert!(row0.chars().zip(row1.chars()).all(|(a, b)| a != b));
        // Two rows apart are identical (parity repeats).
        assert_eq!(row0, row2);
    }
}
