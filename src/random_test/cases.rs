//! Generation orchestration.
//!
//! Bridges the yml (`spec`), the strategy list (`strategy`) and the renderer
//! (`render`) into an ordered list of `(name, input)` cases. No snowchains,
//! judge or CLI here — those are handled by the runner. Huge-input protection lives in the
//! renderer, where the generated case is budgeted as it is emitted.
//!
//! Scope-crossing constraints are enforced by the renderer while each nested
//! block is generated: already-decided parent values narrow inner values, and
//! same-scope pairs are verified before a completed block is retained.

use super::render::{render_case, RenderResult};
use super::spec;
use super::strategy::{case_name, strategy_stream};
use crate::parse::read_random_test_section;
use crate::parse::RandomTestSection;
use camino::Utf8Path;
use rand::{rngs::SmallRng, Rng, SeedableRng};

/// The result of trying to generate random-test cases for one problem.
pub(crate) enum GenerateOutcome {
    /// `random_test:` present and usable.
    Ready {
        /// `(name, input)` in output order. Names are `corner{n}` / `random{n}`
        /// numbered 1.. over emitted cases (dropped corners do not consume a
        /// number).
        cases: Vec<(String, String)>,
        /// Unsupported-constraint passthrough; the runner prints this
        /// as a trailing warning.
        skipped: Vec<String>,
    },
    /// `ResolvedSpec.missing` non-empty, or the renderer budget tripped. The
    /// runner prints these English reasons and aborts this problem's random
    /// test (no cases run).
    Aborted {
        reasons: Vec<String>,
    },
    Interrupted,
}

/// Read the `random_test:` section and generate cases. `Ok(None)` means there
/// is no `random_test:` section (e.g. a non-AtCoder problem) and the caller
/// should skip silently. The RNG is seeded from entropy.
pub(crate) fn generate_cases(
    yml_path: &Utf8Path,
    count: u32,
) -> anyhow::Result<Option<GenerateOutcome>> {
    let Some(section) = read_random_test_section(yml_path)? else {
        return Ok(None);
    };
    let mut rng = SmallRng::from_entropy();
    Ok(Some(generate_cases_with_rng(&section, count, &mut rng)))
}

/// Test seam: deterministic when given a seeded RNG.
fn generate_cases_with_rng(
    section: &RandomTestSection,
    count: u32,
    rng: &mut impl Rng,
) -> GenerateOutcome {
    let spec = spec::resolve(section);

    if !spec.missing.is_empty() {
        return GenerateOutcome::Aborted {
            reasons: spec.missing.clone(),
        };
    }

    // Pull strategies until `count` cases have been *successfully rendered*.
    // A corner that can't be realized for this problem (`render_case` →
    // `Unsatisfied`, only possible for corners — plain `Random` retries
    // internally until it satisfies ordering/not_equal) just advances the
    // stream; the next strategy fills the slot. The stream's tail is mostly
    // plain `Random`, which always renders, so this terminates in O(count);
    // the only non-termination is an unsatisfiable ordering hanging inside
    // `render_case`, gated upstream by `Aborted`.
    let mut stream = strategy_stream(&spec, count, rng);
    let mut corner = 0u32;
    let mut random = 0u32;
    let mut cases: Vec<(String, String)> = Vec::new();

    while (cases.len() as u32) < count {
        if crate::interrupt::requested() {
            return GenerateOutcome::Interrupted;
        }
        let st = stream.next(rng);
        match render_case(&spec, &st, rng) {
            RenderResult::Ready(input) => {
                let name = case_name(&st, &mut corner, &mut random);
                cases.push((name, input));
            }
            RenderResult::Unsatisfied => {}
            RenderResult::Abort(reason) => {
                return GenerateOutcome::Aborted {
                    reasons: vec![reason],
                }
            }
            RenderResult::Interrupted => return GenerateOutcome::Interrupted,
        }
    }

    GenerateOutcome::Ready {
        cases,
        skipped: spec.skipped.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::{
        ArrayBlock, BoundRepr, FormatBlock, RandomTestSection, ScalarsBlock,
        VarConstraint, VarType,
    };
    use std::collections::BTreeMap;

    fn seeded() -> SmallRng {
        SmallRng::seed_from_u64(0xC0FFEE)
    }

    fn vc(lo: i64, hi: i64) -> VarConstraint {
        VarConstraint {
            r#type: VarType::Usize,
            range: Some([BoundRepr::Lit(lo), BoundRepr::Lit(hi)]),
            values: None,
            len: None,
            sum_limit: None,
            all_distinct: false,
        }
    }

    fn section_with(
        vars: Vec<(&str, VarConstraint)>,
        format: Vec<FormatBlock>,
    ) -> RandomTestSection {
        let mut m = BTreeMap::new();
        for (k, v) in vars {
            m.insert(k.to_owned(), v);
        }
        RandomTestSection {
            vars: m,
            format,
            ordering: vec![],
            not_equal: vec![],
            skipped: vec![],
        }
    }

    /// `n` scalars + a 1D int array `a[n]`, all bounds present.
    fn simple_section() -> RandomTestSection {
        section_with(
            vec![("n", vc(1, 5)), ("a", vc(0, 9))],
            vec![
                FormatBlock::Scalars(ScalarsBlock {
                    vars: vec!["n".to_owned()],
                }),
                FormatBlock::Array(ArrayBlock {
                    base: "a".to_owned(),
                    len: Some("n".to_owned()),
                    height: None,
                    count: None,
                    jagged: false,
                }),
            ],
        )
    }

    #[test]
    fn no_section_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.yml");
        std::fs::write(&p, "type: Batch\ncases: []\n").unwrap();
        let yml = Utf8Path::from_path(&p).unwrap();
        let out = generate_cases(yml, 5).unwrap();
        assert!(out.is_none());
    }

    #[test]
    fn missing_aborts_with_reasons() {
        // `n` has no range and no values → recorded in spec.missing.
        let mut bad = vc(0, 0);
        bad.range = None;
        let section = section_with(
            vec![("n", bad)],
            vec![FormatBlock::Scalars(ScalarsBlock {
                vars: vec!["n".to_owned()],
            })],
        );
        let mut rng = seeded();
        match generate_cases_with_rng(&section, 5, &mut rng) {
            GenerateOutcome::Aborted { reasons } => {
                let resolved = spec::resolve(&section);
                assert!(!reasons.is_empty());
                assert_eq!(reasons, resolved.missing);
            }
            GenerateOutcome::Ready { .. } => panic!("expected Aborted"),
            GenerateOutcome::Interrupted => panic!("unexpected interrupt"),
        }
    }

    #[test]
    fn all_cases_emitted() {
        let section = simple_section();
        let mut rng = seeded();
        match generate_cases_with_rng(&section, 5, &mut rng) {
            GenerateOutcome::Ready { cases, .. } => {
                assert_eq!(cases.len(), 5);
                assert!(cases.iter().all(|(_, i)| !i.is_empty()));
                // Names contiguous 1.. within each family.
                let mut c = 0;
                let mut r = 0;
                for (name, _) in &cases {
                    if let Some(n) = name.strip_prefix("corner") {
                        c += 1;
                        assert_eq!(n.parse::<u32>().unwrap(), c);
                    } else if let Some(n) = name.strip_prefix("random") {
                        r += 1;
                        assert_eq!(n.parse::<u32>().unwrap(), r);
                    } else {
                        panic!("unexpected name {}", name);
                    }
                }
            }
            GenerateOutcome::Aborted { reasons } => panic!("aborted: {:?}", reasons),
            GenerateOutcome::Interrupted => panic!("unexpected interrupt"),
        }
    }

    #[test]
    fn impossible_corner_dropped() {
        // a in [5,5], b in [1,1], require a <= b → no corner can satisfy it,
        // but Random retries unboundedly... it also cannot. So ordering must be
        // satisfiable by Random: use a in [1,5], b in [1,5], a <= b (feasible),
        // and an impossible-for-corner pair via not_equal on a fixed var.
        let section = RandomTestSection {
            vars: {
                let mut m = BTreeMap::new();
                m.insert("a".to_owned(), vc(1, 5));
                m.insert("b".to_owned(), vc(1, 5));
                m
            },
            format: vec![FormatBlock::Scalars(ScalarsBlock {
                vars: vec!["a".to_owned(), "b".to_owned()],
            })],
            ordering: vec![["a".to_owned(), "b".to_owned()]],
            not_equal: vec![],
            skipped: vec![],
        };
        let mut rng = seeded();
        match generate_cases_with_rng(&section, 30, &mut rng) {
            GenerateOutcome::Ready { cases, .. } => {
                // Every emitted case must satisfy a <= b.
                for (_name, input) in &cases {
                    let nums: Vec<i64> = input
                        .split_whitespace()
                        .map(|x| x.parse().unwrap())
                        .collect();
                    assert!(nums[0] <= nums[1], "{}: {:?}", _name, input);
                }
            }
            GenerateOutcome::Aborted { reasons } => panic!("aborted: {:?}", reasons),
            GenerateOutcome::Interrupted => panic!("unexpected interrupt"),
        }
    }

    #[test]
    fn exact_count_even_when_corners_fail() {
        // Same shape as `impossible_corner_dropped`: ordering a<=b makes some
        // corner strategies unsatisfiable, but the spec is feasible for plain
        // Random. The emitted total must still equal the requested count.
        let section = RandomTestSection {
            vars: {
                let mut m = BTreeMap::new();
                m.insert("a".to_owned(), vc(1, 5));
                m.insert("b".to_owned(), vc(1, 5));
                m
            },
            format: vec![FormatBlock::Scalars(ScalarsBlock {
                vars: vec!["a".to_owned(), "b".to_owned()],
            })],
            ordering: vec![["a".to_owned(), "b".to_owned()]],
            not_equal: vec![],
            skipped: vec![],
        };
        for &c in &[1u32, 5, 30] {
            let mut rng = seeded();
            match generate_cases_with_rng(&section, c, &mut rng) {
                GenerateOutcome::Ready { cases, .. } => {
                    assert_eq!(cases.len(), c as usize, "count {c}");
                }
                GenerateOutcome::Aborted { reasons } => panic!("aborted: {:?}", reasons),
                GenerateOutcome::Interrupted => panic!("unexpected interrupt"),
            }
        }
    }

    #[test]
    fn count_zero_emits_nothing() {
        let mut rng = seeded();
        match generate_cases_with_rng(&simple_section(), 0, &mut rng) {
            GenerateOutcome::Ready { cases, .. } => {
                assert!(cases.is_empty());
            }
            GenerateOutcome::Aborted { reasons } => panic!("aborted: {:?}", reasons),
            GenerateOutcome::Interrupted => panic!("unexpected interrupt"),
        }
    }

    #[test]
    fn skipped_passthrough() {
        let mut section = simple_section();
        section.skipped = vec!["foo is an integer".to_owned()];
        let mut rng = seeded();
        match generate_cases_with_rng(&section, 3, &mut rng) {
            GenerateOutcome::Ready { skipped, .. } => {
                assert_eq!(skipped, vec!["foo is an integer".to_owned()]);
            }
            GenerateOutcome::Aborted { reasons } => panic!("aborted: {:?}", reasons),
            GenerateOutcome::Interrupted => panic!("unexpected interrupt"),
        }
    }

    #[test]
    fn entropy_smoke() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.yml");
        let yml_text = "\
random_test:
  vars:
    n: { type: usize, range: [1, 5] }
    a: { type: usize, range: [0, 9] }
  format:
    - scalars: { vars: [n] }
    - array: { base: a, len: n }
";
        std::fs::write(&p, yml_text).unwrap();
        let yml = Utf8Path::from_path(&p).unwrap();
        match generate_cases(yml, 5).unwrap() {
            Some(GenerateOutcome::Ready { cases, .. }) => {
                assert_eq!(cases.len(), 5);
                assert!(cases.iter().all(|(_, i)| !i.is_empty()));
            }
            _ => panic!("expected Ready with 5 cases"),
        }
    }
}
