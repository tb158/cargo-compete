//! Per-problem case-strategy list: decides, for a requested case count, which
//! deterministic / random / corner strategies to emit. RNG is injected so the
//! 30 / 70 post-coverage mix is deterministically testable; production
//! generation passes a `SmallRng::from_entropy()`.

use super::spec::ResolvedSpec;
use rand::seq::SliceRandom as _;
use rand::Rng;

/// Strategies whose rendered input is identical every run; safe to emit at most
/// once (never repeated after full coverage).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DeterministicStrategy {
    AllMax,
    AllMin,
}

/// Strategies with a random element (repeatable after full corner coverage).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RandomStrategy {
    Random,
    /// Size variables forced to `k` (k = 1, 2, 3); non-size random.
    SmallSize(i64),
    /// Size variables maxed, non-size random, `sum_limit` outer var → 1
    /// (behaviour derived from `ResolvedSpec` while rendering each case).
    MaxSize,
    /// Variables whose range spans zero (lo < 0 < hi) set to 0.
    ZeroCorner,
    ArrayMonoInc,
    ArrayMonoDec,
    ArrayAllSame,
    ArrayAltMaxMin,
    ArrayMountain,
    ArrayOneMaxRestMin,
    /// Each element randomly one of two consecutive values from [lo, hi].
    ArrayNarrowRange,
    /// Array elements follow a random repeating pattern of 2–5 values.
    ArrayPeriodic,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CaseStrategy {
    Deterministic(DeterministicStrategy),
    Random(RandomStrategy),
}

/// `true` iff an integer array carries the `all_distinct` constraint. Such a
/// problem additionally pools the distinct-compatible array strategies
/// (`ArrayMonoInc` / `ArrayMonoDec` / `ArrayMountain`).
fn has_distinct_array(spec: &ResolvedSpec) -> bool {
    spec.vars.values().any(|v| v.all_distinct)
}

fn has_inter_array_constraints(spec: &ResolvedSpec) -> bool {
    !spec.inter_array_constrained.is_empty()
}

/// Lazy, unbounded source of case strategies.
///
/// `prefix` is the finite coverage-ordered head: `initial_random` plain
/// `Random` entries followed by the shuffled distinct corner pool. Once the
/// prefix is exhausted, `next` yields an endless tail of 30 % a random-element
/// corner / 70 % plain `Random`. The consumer pulls until it has the required
/// number of *successfully rendered* cases, so a strategy that fails to
/// realize for a given problem simply advances the stream instead of consuming
/// an output slot.
pub(crate) struct StrategyStream {
    prefix: Vec<CaseStrategy>,
    random_corners: Vec<RandomStrategy>,
    pos: usize,
}

/// Build a [`StrategyStream`]. `count` only sizes the leading plain-`Random`
/// run (1 for `count < 10`, else 2, never exceeding `count`); the stream
/// itself is unbounded. The corner pool is shuffled once here, mirroring the
/// previous one-shot list build so seeded sequences are unchanged.
pub(crate) fn strategy_stream(
    spec: &ResolvedSpec,
    count: u32,
    rng: &mut impl Rng,
) -> StrategyStream {
    let n = count as usize;

    let mut corners: Vec<CaseStrategy> = vec![
        CaseStrategy::Deterministic(DeterministicStrategy::AllMax),
        CaseStrategy::Deterministic(DeterministicStrategy::AllMin),
        CaseStrategy::Random(RandomStrategy::SmallSize(1)),
        CaseStrategy::Random(RandomStrategy::SmallSize(2)),
        CaseStrategy::Random(RandomStrategy::SmallSize(3)),
        CaseStrategy::Random(RandomStrategy::ZeroCorner),
        CaseStrategy::Random(RandomStrategy::MaxSize),
    ];
    // The full and distinct-only array-strategy buckets are mutually exclusive
    // on the `all_distinct` axis (requirements §戦略プールの構成): the full set
    // is pooled only when the problem has an array and *no* all_distinct array,
    // the distinct-only set whenever it has an all_distinct array. An
    // inter-array constraint suppresses both.
    let mut array_strats: Vec<RandomStrategy> = Vec::new();
    if !has_inter_array_constraints(spec) {
        if has_distinct_array(spec) {
            array_strats.extend([
                RandomStrategy::ArrayMonoInc,
                RandomStrategy::ArrayMonoDec,
                RandomStrategy::ArrayMountain,
            ]);
        } else if spec.has_array {
            array_strats.extend([
                RandomStrategy::ArrayMonoInc,
                RandomStrategy::ArrayMonoDec,
                RandomStrategy::ArrayAllSame,
                RandomStrategy::ArrayAltMaxMin,
                RandomStrategy::ArrayMountain,
                RandomStrategy::ArrayOneMaxRestMin,
                RandomStrategy::ArrayNarrowRange,
                RandomStrategy::ArrayPeriodic,
            ]);
        }
    }
    corners.extend(array_strats.into_iter().map(CaseStrategy::Random));

    let initial_random = (if n < 10 { 1usize } else { 2usize }).min(n);

    corners.shuffle(rng);

    // Only random-element strategies may repeat in the tail; deterministic
    // ones would produce byte-identical input.
    let random_corners: Vec<RandomStrategy> = corners
        .iter()
        .filter_map(|s| match s {
            CaseStrategy::Random(r) => Some(r.clone()),
            CaseStrategy::Deterministic(_) => None,
        })
        .collect();

    let mut prefix: Vec<CaseStrategy> =
        vec![CaseStrategy::Random(RandomStrategy::Random); initial_random];
    prefix.extend(corners);

    StrategyStream {
        prefix,
        random_corners,
        pos: 0,
    }
}

impl StrategyStream {
    pub(crate) fn next(&mut self, rng: &mut impl Rng) -> CaseStrategy {
        if self.pos < self.prefix.len() {
            let s = self.prefix[self.pos].clone();
            self.pos += 1;
            return s;
        }
        if rng.gen_bool(0.3) && !self.random_corners.is_empty() {
            let idx = rng.gen_range(0..self.random_corners.len());
            CaseStrategy::Random(self.random_corners[idx].clone())
        } else {
            CaseStrategy::Random(RandomStrategy::Random)
        }
    }
}

/// Stable per-case name: plain random cases are `random{n}`, every corner /
/// deterministic case is `corner{n}` (counters are 1-based, caller-owned).
pub(crate) fn case_name(s: &CaseStrategy, corner: &mut u32, random: &mut u32) -> String {
    match s {
        CaseStrategy::Random(RandomStrategy::Random) => {
            *random += 1;
            format!("random{random}")
        }
        _ => {
            *corner += 1;
            format!("corner{corner}")
        }
    }
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
    use std::collections::BTreeMap;

    fn rng() -> SmallRng {
        SmallRng::seed_from_u64(0xC0FFEE)
    }

    /// Pull `count` strategies from a fresh stream. Mirrors the consumer's
    /// failure-free path, so assertions on the old `make_strategy_list`
    /// output carry over unchanged for the same seed.
    fn list(spec: &ResolvedSpec, count: u32, rng: &mut impl Rng) -> Vec<CaseStrategy> {
        let mut s = strategy_stream(spec, count, rng);
        (0..count).map(|_| s.next(rng)).collect()
    }

    fn spec_with(format: Vec<FormatBlock>) -> ResolvedSpec {
        resolve(&RandomTestSection {
            vars: BTreeMap::new(),
            format,
            ..Default::default()
        })
    }

    /// Scalars only ⇒ no `len` anywhere ⇒ `has_array == false`.
    fn scalar_spec() -> ResolvedSpec {
        spec_with(vec![FormatBlock::Scalars(ScalarsBlock {
            vars: vec!["n".into()],
        })])
    }

    fn array_spec() -> ResolvedSpec {
        spec_with(vec![FormatBlock::Array(ArrayBlock {
            base: "a".into(),
            len: Some("n".into()),
            height: None,
            count: None,
            jagged: false,
        })])
    }

    #[test]
    fn count_zero_is_empty() {
        assert!(list(&scalar_spec(), 0, &mut rng()).is_empty());
    }

    #[test]
    fn fewer_than_ten_has_exactly_one_leading_random() {
        let list = list(&array_spec(), 9, &mut rng());
        assert_eq!(list[0], CaseStrategy::Random(RandomStrategy::Random));
        assert_ne!(list[1], CaseStrategy::Random(RandomStrategy::Random));
    }

    #[test]
    fn ten_or_more_has_exactly_two_leading_random() {
        let list = list(&array_spec(), 20, &mut rng());
        assert_eq!(list[0], CaseStrategy::Random(RandomStrategy::Random));
        assert_eq!(list[1], CaseStrategy::Random(RandomStrategy::Random));
        assert_ne!(list[2], CaseStrategy::Random(RandomStrategy::Random));
    }

    #[test]
    fn count_at_or_below_initial_random_is_all_random() {
        // n < 10 ⇒ initial_random == 1.
        let list = list(&array_spec(), 1, &mut rng());
        assert_eq!(list, vec![CaseStrategy::Random(RandomStrategy::Random)]);
    }

    #[test]
    fn no_array_strategies_when_has_array_false() {
        let list = list(&scalar_spec(), 200, &mut rng());
        let array_kinds = [
            RandomStrategy::ArrayMonoInc,
            RandomStrategy::ArrayMonoDec,
            RandomStrategy::ArrayAllSame,
            RandomStrategy::ArrayAltMaxMin,
            RandomStrategy::ArrayMountain,
            RandomStrategy::ArrayOneMaxRestMin,
            RandomStrategy::ArrayNarrowRange,
            RandomStrategy::ArrayPeriodic,
        ];
        assert!(!list.iter().any(|s| matches!(
            s,
            CaseStrategy::Random(r) if array_kinds.contains(r)
        )));
        // MaxSize is a default-pool strategy ⇒ present even without arrays.
        assert!(list.contains(&CaseStrategy::Random(RandomStrategy::MaxSize)));
    }

    #[test]
    fn all_corner_kinds_appear_with_large_count() {
        let list = list(&array_spec(), 200, &mut rng());
        let expected = [
            CaseStrategy::Deterministic(DeterministicStrategy::AllMax),
            CaseStrategy::Deterministic(DeterministicStrategy::AllMin),
            CaseStrategy::Random(RandomStrategy::SmallSize(1)),
            CaseStrategy::Random(RandomStrategy::SmallSize(2)),
            CaseStrategy::Random(RandomStrategy::SmallSize(3)),
            CaseStrategy::Random(RandomStrategy::ZeroCorner),
            CaseStrategy::Random(RandomStrategy::MaxSize),
            CaseStrategy::Random(RandomStrategy::ArrayMonoInc),
            CaseStrategy::Random(RandomStrategy::ArrayMonoDec),
            CaseStrategy::Random(RandomStrategy::ArrayAllSame),
            CaseStrategy::Random(RandomStrategy::ArrayAltMaxMin),
            CaseStrategy::Random(RandomStrategy::ArrayMountain),
            CaseStrategy::Random(RandomStrategy::ArrayOneMaxRestMin),
            CaseStrategy::Random(RandomStrategy::ArrayNarrowRange),
            CaseStrategy::Random(RandomStrategy::ArrayPeriodic),
        ];
        for e in &expected {
            assert!(list.contains(e), "missing corner {:?}", e);
        }
    }

    #[test]
    fn deterministic_corners_never_repeat() {
        let list = list(&array_spec(), 200, &mut rng());
        let max = list
            .iter()
            .filter(|s| **s == CaseStrategy::Deterministic(DeterministicStrategy::AllMax))
            .count();
        let min = list
            .iter()
            .filter(|s| **s == CaseStrategy::Deterministic(DeterministicStrategy::AllMin))
            .count();
        assert_eq!(max, 1);
        assert_eq!(min, 1);
    }

    #[test]
    fn has_array_detection_variants() {
        // Array with explicit len ⇒ true.
        assert!(list(&array_spec(), 200, &mut rng())
            .contains(&CaseStrategy::Random(RandomStrategy::ArrayMonoInc)));

        // Jagged array always carries its length var (`len: Some`) ⇒ true.
        let jagged = spec_with(vec![FormatBlock::Array(ArrayBlock {
            base: "a".into(),
            len: Some("l".into()),
            height: None,
            count: Some("n".into()),
            jagged: true,
        })]);
        assert!(list(&jagged, 200, &mut rng())
            .contains(&CaseStrategy::Random(RandomStrategy::ArrayMonoInc)));

        // The only `len: None` Array shape the parser emits is a 2-D grid
        // (`jagged: false`, width captured on the Chars var instead). With no
        // var carrying a `len` either, `has_array` is false.
        let grid = spec_with(vec![FormatBlock::Array(ArrayBlock {
            base: "g".into(),
            len: None,
            height: None,
            count: Some("h".into()),
            jagged: false,
        })]);
        assert!(!list(&grid, 200, &mut rng())
            .contains(&CaseStrategy::Random(RandomStrategy::ArrayMonoInc)));

        // Rows ⇒ true.
        let rows = spec_with(vec![FormatBlock::Rows(RowsBlock {
            vars: vec!["x".into()],
            len: "m".into(),
        })]);
        assert!(list(&rows, 200, &mut rng())
            .contains(&CaseStrategy::Random(RandomStrategy::ArrayMonoInc)));

        // Nested under TestCases ⇒ true.
        let nested_tc = spec_with(vec![FormatBlock::TestCases(TestCasesBlock {
            count: "t".into(),
            format: vec![FormatBlock::Array(ArrayBlock {
                base: "a".into(),
                len: Some("n".into()),
                height: None,
                count: None,
                jagged: false,
            })],
        })]);
        assert!(list(&nested_tc, 200, &mut rng())
            .contains(&CaseStrategy::Random(RandomStrategy::ArrayMonoInc)));

        // Nested under Queries ⇒ true.
        let nested_q = spec_with(vec![FormatBlock::Queries(QueriesBlock {
            count: "q".into(),
            discriminator: None,
            types: vec![QueryBranch {
                id: "1".into(),
                format: vec![FormatBlock::Rows(RowsBlock {
                    vars: vec!["x".into()],
                    len: "m".into(),
                })],
            }],
        })]);
        assert!(list(&nested_q, 200, &mut rng())
            .contains(&CaseStrategy::Random(RandomStrategy::ArrayMonoInc)));

        // No len in format but a Chars var with a len ⇒ true.
        let mut vars = BTreeMap::new();
        vars.insert(
            "s".into(),
            VarConstraint {
                r#type: VarType::Chars,
                values: Some(vec!["a".into()]),
                len: Some(BoundRepr::Expr("n".into())),
                ..Default::default()
            },
        );
        let chars_len = resolve(&RandomTestSection {
            vars,
            format: vec![FormatBlock::Scalars(ScalarsBlock {
                vars: vec!["n".into()],
            })],
            ..Default::default()
        });
        assert!(list(&chars_len, 200, &mut rng())
            .contains(&CaseStrategy::Random(RandomStrategy::ArrayMonoInc)));
    }

    #[test]
    fn inter_array_constraints_suppress_array_strategies() {
        let mut sec = RandomTestSection {
            format: vec![
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
            ],
            ..Default::default()
        };
        sec.ordering = vec![["a".into(), "b".into()]];
        let spec = resolve(&sec);
        let list = list(&spec, 200, &mut rng());
        let array_kinds = [
            RandomStrategy::ArrayMonoInc,
            RandomStrategy::ArrayMonoDec,
            RandomStrategy::ArrayAllSame,
            RandomStrategy::ArrayAltMaxMin,
            RandomStrategy::ArrayMountain,
            RandomStrategy::ArrayOneMaxRestMin,
            RandomStrategy::ArrayNarrowRange,
            RandomStrategy::ArrayPeriodic,
        ];
        assert!(!list.iter().any(|s| matches!(
            s,
            CaseStrategy::Random(r) if array_kinds.contains(r)
        )));
        assert!(list.contains(&CaseStrategy::Deterministic(DeterministicStrategy::AllMax)));
    }

    #[test]
    fn all_distinct_rows_only_pool_compatible_array_strategies() {
        let mut vars = BTreeMap::new();
        for name in ["a", "b"] {
            vars.insert(
                name.into(),
                VarConstraint {
                    r#type: VarType::Usize,
                    range: Some([BoundRepr::Lit(1), BoundRepr::Lit(10)]),
                    all_distinct: true,
                    ..Default::default()
                },
            );
        }
        let spec = resolve(&RandomTestSection {
            vars,
            format: vec![FormatBlock::Rows(RowsBlock {
                vars: vec!["a".into(), "b".into()],
                len: "3".into(),
            })],
            ..Default::default()
        });
        let list = list(&spec, 200, &mut rng());
        for allowed in [
            RandomStrategy::ArrayMonoInc,
            RandomStrategy::ArrayMonoDec,
            RandomStrategy::ArrayMountain,
        ] {
            assert!(list.contains(&CaseStrategy::Random(allowed)));
        }
        for ignored in [
            RandomStrategy::ArrayAllSame,
            RandomStrategy::ArrayAltMaxMin,
            RandomStrategy::ArrayOneMaxRestMin,
            RandomStrategy::ArrayNarrowRange,
            RandomStrategy::ArrayPeriodic,
        ] {
            assert!(!list.contains(&CaseStrategy::Random(ignored)));
        }
    }

    #[test]
    fn scalar_array_constraint_keeps_array_strategy_pool() {
        let mut sec = RandomTestSection {
            format: vec![
                FormatBlock::Scalars(ScalarsBlock {
                    vars: vec!["x".into()],
                }),
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
        sec.not_equal = vec![["x".into(), "a".into()]];
        let spec = resolve(&sec);
        let list = list(&spec, 200, &mut rng());
        assert!(list.contains(&CaseStrategy::Random(RandomStrategy::ArrayPeriodic)));
    }

    #[test]
    fn mixed_distinct_and_nondistinct_pools_only_distinct_compatible() {
        // A problem with both an all_distinct array `p` and a plain array `a`
        // must pool only the distinct-compatible set (requirements
        // §戦略プールの構成: the full-8 bucket requires *no* all_distinct array).
        let mut vars = BTreeMap::new();
        vars.insert(
            "p".into(),
            VarConstraint {
                r#type: VarType::Usize,
                range: Some([BoundRepr::Lit(1), BoundRepr::Lit(10)]),
                all_distinct: true,
                ..Default::default()
            },
        );
        vars.insert(
            "a".into(),
            VarConstraint {
                r#type: VarType::Usize,
                range: Some([BoundRepr::Lit(1), BoundRepr::Lit(10)]),
                ..Default::default()
            },
        );
        let spec = resolve(&RandomTestSection {
            vars,
            format: vec![
                FormatBlock::Array(ArrayBlock {
                    base: "p".into(),
                    len: Some("n".into()),
                    height: None,
                    count: None,
                    jagged: false,
                }),
                FormatBlock::Array(ArrayBlock {
                    base: "a".into(),
                    len: Some("n".into()),
                    height: None,
                    count: None,
                    jagged: false,
                }),
            ],
            ..Default::default()
        });
        let list = list(&spec, 200, &mut rng());
        for allowed in [
            RandomStrategy::ArrayMonoInc,
            RandomStrategy::ArrayMonoDec,
            RandomStrategy::ArrayMountain,
        ] {
            assert!(list.contains(&CaseStrategy::Random(allowed)));
        }
        for ignored in [
            RandomStrategy::ArrayAllSame,
            RandomStrategy::ArrayAltMaxMin,
            RandomStrategy::ArrayOneMaxRestMin,
            RandomStrategy::ArrayNarrowRange,
            RandomStrategy::ArrayPeriodic,
        ] {
            assert!(!list.contains(&CaseStrategy::Random(ignored)));
        }
    }

    #[test]
    fn case_name_counts_separately() {
        let (mut c, mut r) = (0u32, 0u32);
        assert_eq!(
            case_name(
                &CaseStrategy::Random(RandomStrategy::Random),
                &mut c,
                &mut r
            ),
            "random1"
        );
        assert_eq!(
            case_name(
                &CaseStrategy::Random(RandomStrategy::Random),
                &mut c,
                &mut r
            ),
            "random2"
        );
        assert_eq!(
            case_name(
                &CaseStrategy::Deterministic(DeterministicStrategy::AllMax),
                &mut c,
                &mut r
            ),
            "corner1"
        );
        assert_eq!(
            case_name(
                &CaseStrategy::Random(RandomStrategy::MaxSize),
                &mut c,
                &mut r
            ),
            "corner2"
        );
    }
}
