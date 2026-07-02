//! Top-level orchestrator: HTML → `TaskSection` → `RandomTestSection` and
//! the multi-yml `annotate_ymls_with_format` driver.

use super::analysis::{analyze_format, VarShape};
use super::constraint_parse::{
    parse_constraints, resolve_lit, BoundExpr, ConstraintParse, NumBound, Side, StrSpec,
};
use super::format_lowering::lower_typed_format_dimensions;
use super::format_parse::{add_missing_usize_vars, lines_to_format_blocks_inner};
use super::normalize::{
    base_var, extract_case_subscript, extract_query_subscript, is_case_placeholder_line,
    is_query_placeholder_line, snake,
};
use super::types::{
    BoundRepr, FormatBlock, QueriesBlock, QueryBranch, RandomTestSection, ScalarsBlock,
    TaskSection, TestCasesBlock, VarConstraint, VarType,
};
use super::yml_io::append_format_to_yml;
use std::collections::{HashMap, HashSet};

#[cfg(test)]
#[path = "task_tests.rs"]
mod tests;
use crate::shell::Shell;
use anyhow::Context as _;
use camino::{Utf8Path, Utf8PathBuf};
use regex::Regex;
use std::fs;

// ─── HTML helpers ─────────────────────────────────────────────────────────────

fn transitive_closure_ordering(ordering: HashSet<(String, String)>) -> HashSet<(String, String)> {
    let mut adj: HashMap<String, Vec<String>> = HashMap::new();
    for (a, b) in &ordering {
        if a != b {
            adj.entry(a.clone()).or_default().push(b.clone());
        }
    }

    let mut closed = ordering;
    let starts: Vec<String> = adj.keys().cloned().collect();
    for start in starts {
        let mut seen: HashSet<String> = HashSet::new();
        let mut stack = adj.get(&start).cloned().unwrap_or_default();
        while let Some(next) = stack.pop() {
            if !seen.insert(next.clone()) {
                continue;
            }
            if start != next {
                closed.insert((start.clone(), next.clone()));
            }
            if let Some(ns) = adj.get(&next) {
                stack.extend(ns.iter().cloned());
            }
        }
    }
    closed
}

fn strip_tags(html: &str) -> String {
    let re = Regex::new(r"(?s)<.*?>").expect("invalid regex");
    let mut s = re.replace_all(html, "").to_string();
    s = s.replace("&lt;", "<");
    s = s.replace("&gt;", ">");
    s = s.replace("&amp;", "&");
    s
}

fn strip_tags_keep_code(html: &str) -> String {
    let any_tag_re = Regex::new(r"(?s)<[^>]*>").unwrap();
    let s = html
        .replace("<code>", "\x00CODEOPEN\x00")
        .replace("</code>", "\x00CODECLOSE\x00");
    let stripped = any_tag_re.replace_all(&s, "");
    let mut result = stripped
        .replace("\x00CODEOPEN\x00", "<code>")
        .replace("\x00CODECLOSE\x00", "</code>");
    result = result
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&");
    result
}

pub(crate) fn extract_constraints_items(seg: &str) -> Vec<String> {
    let li_re = Regex::new(r"(?s)<li>(.*?)</li>").unwrap();
    for key in ["制約", "Constraints"] {
        // Scope from `<h3>制約</h3>` to the next section boundary (next <h3> or
        // </section>). This handles nested <ul>s correctly because we no longer
        // bound the scope to a single </ul>.
        let scope_re = Regex::new(&format!(
            r"(?s)<h3>{}</h3>(.*?)(?:<h3>|</section>)",
            regex::escape(key)
        ))
        .unwrap();
        let Some(cap) = scope_re.captures(seg) else {
            continue;
        };
        let scope = cap.get(1).unwrap().as_str();
        let mut items = Vec::new();
        for li in li_re.captures_iter(scope) {
            let li_html = li.get(1).unwrap().as_str();
            let txt = strip_tags_keep_code(li_html).trim().to_string();
            if !txt.is_empty() {
                items.push(txt);
            }
        }
        if !items.is_empty() {
            return items;
        }
    }
    Vec::new()
}

pub(crate) fn parse_task_sections(task_html: &str) -> Vec<TaskSection> {
    let span_re = Regex::new(r#"(?s)<span class="h2">\s*([A-Z])\s*-\s*([^<]+)</span>"#)
        .expect("invalid regex");
    let mut spans: Vec<(usize, usize, String, String)> = Vec::new();
    for cap in span_re.captures_iter(task_html) {
        let m = cap.get(0).unwrap();
        let letter = cap.get(1).unwrap().as_str().trim().to_string();
        let title = cap.get(2).unwrap().as_str().trim().to_string();
        spans.push((m.start(), m.end(), letter, title));
    }

    let mut out = Vec::new();
    let pre_re = Regex::new(r"(?s)<pre>(.*?)</pre>").expect("invalid regex");
    for idx in 0..spans.len() {
        let (start, _end, letter, _title) = spans[idx].clone();
        let end = if idx + 1 < spans.len() {
            spans[idx + 1].0
        } else {
            task_html.len()
        };
        let seg = &task_html[start..end];

        let in_pos = seg.find(r"<h3>入力</h3>");
        if in_pos.is_none() {
            continue;
        }
        let in_pos = in_pos.unwrap();
        let out_pos = seg.find(r"<h3>出力</h3>").unwrap_or(seg.len());
        let inp = &seg[in_pos..out_pos];

        let mut blocks: Vec<Vec<String>> = Vec::new();
        for cap in pre_re.captures_iter(inp) {
            let pre = cap.get(1).unwrap().as_str();
            let txt = strip_tags(pre);
            let lines: Vec<String> = txt
                .lines()
                .map(|l| l.trim())
                .filter(|l| !l.is_empty())
                .map(|l| l.to_string())
                .collect();
            blocks.push(lines);
        }
        let constraints_items = extract_constraints_items(seg);
        out.push(TaskSection {
            letter,
            input_blocks: blocks,
            constraints_items,
        });
    }
    out
}

// ─── task_to_format_blocks ────────────────────────────────────────────────────

/// Convert a `TaskSection` to a `RandomTestSection`.
///
/// Pipeline:
/// 1. Run `parse_constraints` over the constraint items to obtain structured
///    bound / charset / sum-limit / etc. information.
/// 2. Initialise `vars` from the parsed output (Chars from `str_vars`, every
///    numeric var as Usize).
/// 3. Parse the input format; add any vars referenced only by format blocks as
///    `usize`.
/// 4. Enrich `vars` with range / len / sum_limit / all_distinct / ordering /
///    not_equal from the parsed constraints, and promote a numeric var to I64
///    when its resolved lower bound is negative.
pub(crate) fn task_to_format_blocks(task: &TaskSection) -> RandomTestSection {
    let parsed = parse_constraints(&task.constraints_items);
    let mut section = build_section(task, &parsed);
    enrich_section_with_constraints(&parsed, &mut section);
    lower_typed_format_dimensions(&mut section.format, &mut section.vars);
    section
}

/// Convert physical indexed-array dimensions into the yml representation for
/// Chars once constraint parsing has established each base variable's type.
#[cfg(test)]
fn canonicalize_chars_array_dimensions(
    blocks: &mut [FormatBlock],
    vars: &mut std::collections::BTreeMap<String, VarConstraint>,
) {
    lower_typed_format_dimensions(blocks, vars);
}

fn build_section(task: &TaskSection, parsed: &ConstraintParse) -> RandomTestSection {
    // Initialise vars from parsed constraints. Chars (string) takes precedence
    // over I64/Usize; I64 is detected from a literal negative lower bound.
    let mut vars: std::collections::BTreeMap<String, VarConstraint> =
        std::collections::BTreeMap::new();
    for (name, str_spec) in &parsed.str_vars {
        vars.insert(
            name.clone(),
            VarConstraint {
                r#type: VarType::Chars,
                values: Some(str_spec.charset.iter().map(|c| c.to_string()).collect()),
                ..Default::default()
            },
        );
    }
    for name in parsed.num_vars.keys() {
        if vars.contains_key(name) {
            continue; // Chars wins
        }
        // All numeric vars start as Usize. I64 is decided later in
        // `enrich_section_with_constraints`, once each var's range has been
        // resolved (a negative resolved lower bound implies a signed domain).
        vars.insert(
            name.clone(),
            VarConstraint {
                r#type: VarType::Usize,
                ..Default::default()
            },
        );
    }

    let all_lines: Vec<String> = task.input_blocks.iter().flatten().cloned().collect();
    let has_cases = all_lines.iter().any(|l| is_case_placeholder_line(l));
    let has_queries = all_lines.iter().any(|l| is_query_placeholder_line(l));

    let first = match task.input_blocks.first() {
        Some(f) => f,
        None => {
            return RandomTestSection {
                ordering: vec![],
                not_equal: vec![],
                vars,
                format: vec![],
                skipped: vec![],
            };
        }
    };

    if !has_cases && !has_queries {
        let mut skipped = Vec::new();
        let blocks = lines_to_format_blocks_inner(first, &mut skipped);
        add_missing_usize_vars(&mut vars, &blocks);
        return RandomTestSection {
            ordering: vec![],
            not_equal: vec![],
            vars,
            format: blocks,
            skipped,
        };
    }

    if has_cases {
        let mut skipped = Vec::new();
        let header_blocks = lines_to_format_blocks_inner(first, &mut skipped);
        let inner_blocks = match task.input_blocks.get(1) {
            Some(b) => lines_to_format_blocks_inner(b, &mut skipped),
            None => {
                skipped.push("per-testcase fields".to_string());
                vec![]
            }
        };

        let count_var = all_lines
            .iter()
            .filter(|l| is_case_placeholder_line(l))
            .find_map(|l| extract_case_subscript(l))
            .unwrap_or_else(|| "t".to_string());

        let tc = FormatBlock::TestCases(TestCasesBlock {
            count: count_var,
            format: inner_blocks,
        });
        let mut all = header_blocks;
        all.push(tc);
        add_missing_usize_vars(&mut vars, &all);
        return RandomTestSection {
            ordering: vec![],
            not_equal: vec![],
            vars,
            format: all,
            skipped,
        };
    }

    // Queries
    let mut skipped = Vec::new();
    let header_blocks = lines_to_format_blocks_inner(first, &mut skipped);

    let count_var = all_lines
        .iter()
        .filter(|l| is_query_placeholder_line(l))
        .find_map(|l| extract_query_subscript(l))
        .unwrap_or_else(|| "q".to_string());

    // Single-block queries (header + one per-query format block) are structurally
    // identical to test_cases: just a body to read each iteration, no branching.
    if task.input_blocks.len() <= 2 {
        let inner_blocks = match task.input_blocks.get(1) {
            Some(b) => lines_to_format_blocks_inner(b, &mut skipped),
            None => {
                skipped.push("per-query fields".to_string());
                vec![]
            }
        };
        let tc = FormatBlock::TestCases(TestCasesBlock {
            count: count_var,
            format: inner_blocks,
        });
        let mut all = header_blocks;
        all.push(tc);
        add_missing_usize_vars(&mut vars, &all);
        return RandomTestSection {
            ordering: vec![],
            not_equal: vec![],
            vars,
            format: all,
            skipped,
        };
    }

    // Multi-block queries: branch by query type (numeric ID or symbol).
    let mut query_types: Vec<QueryBranch> = Vec::new();
    let mut sym_types: Vec<(String, Vec<String>)> = Vec::new();
    let mut sym_name: Option<String> = None;
    let mut mixed_symbol = false;
    for b in task.input_blocks.iter().skip(1) {
        if b.len() != 1 {
            continue;
        }
        let toks: Vec<&str> = b[0].split_whitespace().collect();
        if toks.is_empty() {
            continue;
        }
        if toks[0].parse::<i32>().is_ok() {
            let id = toks[0].to_string();
            let fmt: Vec<FormatBlock> = if toks.len() > 1 {
                let vars: Vec<String> = toks[1..]
                    .iter()
                    .map(|t| base_var(t).unwrap_or_else(|| snake(t)))
                    .collect();
                vec![FormatBlock::Scalars(ScalarsBlock { vars })]
            } else {
                vec![]
            };
            query_types.push(QueryBranch { id, format: fmt });
        } else {
            let name = snake(toks[0]);
            if let Some(prev) = &sym_name {
                if *prev != name {
                    mixed_symbol = true;
                }
            } else {
                sym_name = Some(name.clone());
            }
            let rest: Vec<String> = toks[1..]
                .iter()
                .map(|t| base_var(t).unwrap_or_else(|| snake(t)))
                .collect();
            sym_types.push((name, rest));
        }
    }

    let has_numeric = !query_types.is_empty();
    let discriminator = if has_numeric { None } else { sym_name };
    let types: Vec<QueryBranch> = if has_numeric {
        query_types
    } else if !sym_types.is_empty() && !mixed_symbol {
        sym_types
            .iter()
            .enumerate()
            .map(|(idx, (_, toks))| QueryBranch {
                id: (idx + 1).to_string(),
                format: if toks.is_empty() {
                    vec![]
                } else {
                    vec![FormatBlock::Scalars(ScalarsBlock { vars: toks.clone() })]
                },
            })
            .collect()
    } else {
        skipped.push("per-query fields".to_string());
        vec![]
    };
    let qb = FormatBlock::Queries(QueriesBlock {
        count: count_var,
        discriminator,
        types,
    });
    let mut all = header_blocks;
    all.push(qb);
    add_missing_usize_vars(&mut vars, &all);
    RandomTestSection {
        ordering: vec![],
        not_equal: vec![],
        vars,
        format: all,
        skipped,
    }
}

// ─── Constraint enrichment (new) ──────────────────────────────────────────────

/// Add range / len / sum_limit / all_distinct / ordering / not_equal information
/// from a parsed constraint set to an existing section. May promote a numeric
/// var's `type` from Usize to I64 when its resolved lower bound is negative;
/// does NOT modify `values`.
fn enrich_section_with_constraints(parsed: &ConstraintParse, section: &mut RandomTestSection) {
    // 1. enum_values: discrete sets become `values`. These vars carry no range.
    for (name, values) in &parsed.enum_values {
        if let Some(entry) = section.vars.get_mut(name) {
            entry.values = Some(values.iter().map(|v| v.to_string()).collect());
        }
    }

    // 2. sum_limit
    for (name, limit) in &parsed.sum_limits {
        let entry = section
            .vars
            .entry(name.clone())
            .or_insert_with(|| VarConstraint {
                r#type: VarType::Usize,
                ..Default::default()
            });
        entry.sum_limit = Some(*limit);
    }

    // 3. all_distinct
    for name in &parsed.all_distinct {
        if let Some(entry) = section.vars.get_mut(name) {
            entry.all_distinct = true;
        }
    }

    // 4. len: determine the kind of length representation for each Chars var.
    //    Synthetic vars are added to section.vars (as usize) and registered as
    //    NumBound entries below so they participate in range resolution like
    //    any other usize variable.
    let mut extra_ordering: Vec<[String; 2]> = Vec::new();
    let mut synthetic_bounds: std::collections::HashMap<String, NumBound> =
        std::collections::HashMap::new();
    for (name, str_spec) in &parsed.str_vars {
        if !section.vars.contains_key(name) {
            continue;
        }
        match resolve_len_kind(name, str_spec) {
            LenResolution::DirectRef(varname) => {
                if let Some(entry) = section.vars.get_mut(name) {
                    entry.len = Some(BoundRepr::Expr(varname));
                }
            }
            LenResolution::FixedLit(n) => {
                if let Some(entry) = section.vars.get_mut(name) {
                    entry.len = Some(BoundRepr::Expr(n.to_string()));
                }
            }
            LenResolution::Synthetic(syn_name) => {
                if let Some(entry) = section.vars.get_mut(name) {
                    entry.len = Some(BoundRepr::Expr(syn_name.clone()));
                }
                section
                    .vars
                    .entry(syn_name.clone())
                    .or_insert_with(|| VarConstraint {
                        r#type: VarType::Usize,
                        ..Default::default()
                    });
                synthetic_bounds.insert(
                    syn_name.clone(),
                    NumBound {
                        lo: str_spec.len_lo.clone(),
                        hi: str_spec.len_hi.clone(),
                    },
                );
                if let Some(BoundExpr::Var { name: dep, .. }) = &str_spec.len_hi {
                    extra_ordering.push([syn_name, dep.clone()]);
                }
            }
            LenResolution::None => {}
        }
    }

    // 5. range: combined bounds from constraints + synthetic length vars,
    //    then resolve each numeric var's range with usize-default fallback.
    let mut combined_bounds = parsed.num_vars.clone();
    combined_bounds.extend(synthetic_bounds);

    let var_names: Vec<String> = section
        .vars
        .iter()
        .filter(|(_, v)| v.r#type != VarType::Chars && v.values.is_none())
        .map(|(n, _)| n.clone())
        .collect();
    for name in var_names {
        let bound = combined_bounds.get(&name).cloned().unwrap_or_default();
        let mut var_type = section.vars.get(&name).map(|v| v.r#type.clone()).unwrap();
        let range = resolve_or_placeholder(&bound, &combined_bounds, &var_type);
        // Decide I64 from the resolved range, not the raw syntactic bound: a
        // chain like `-10^6 <= L <= R <= 10^6` only gives R a `Var{L}` lower
        // bound, which resolves transitively to a negative literal. hi<0
        // implies lo<0, so checking lo alone is sufficient.
        let negative = matches!(&range[0], BoundRepr::Lit(n) if *n < 0);
        let range = if negative && var_type == VarType::Usize {
            var_type = VarType::I64;
            // Re-resolve so the lo default is the I64 placeholder, not 0.
            resolve_or_placeholder(&bound, &combined_bounds, &var_type)
        } else {
            range
        };
        if let Some(entry) = section.vars.get_mut(&name) {
            entry.r#type = var_type;
            entry.range = Some(range);
        }
    }

    let var_types: std::collections::HashMap<String, VarType> = section
        .vars
        .iter()
        .map(|(name, var)| (name.clone(), var.r#type.clone()))
        .collect();
    let analysis = analyze_format(&section.format);
    let var_type = |name: &str| var_types.get(name);
    let var_shape = |name: &str| analysis.shape_of(name);
    let is_numeric_var = |name: &str| matches!(var_type(name), Some(VarType::Usize | VarType::I64));
    let is_jagged_var = |name: &str| var_shape(name) == VarShape::Jagged;
    let is_chars_scalar = |name: &str| {
        matches!(var_type(name), Some(VarType::Chars)) && var_shape(name) == VarShape::Scalar
    };
    let is_allowed_ordering = |a: &str, b: &str| {
        is_numeric_var(a) && is_numeric_var(b) && !is_jagged_var(a) && !is_jagged_var(b)
    };
    let is_allowed_not_equal = |a: &str, b: &str| {
        (is_numeric_var(a) && is_numeric_var(b) && !is_jagged_var(a) && !is_jagged_var(b))
            || (is_chars_scalar(a) && is_chars_scalar(b))
    };

    // 6. ordering: union of explicit var_le and implicit Var-bound references.
    let mut ordering: HashSet<(String, String)> = parsed.var_le.clone();
    for (name, num_bound) in &combined_bounds {
        if let Some(BoundExpr::Var { name: dep, .. }) = &num_bound.hi {
            ordering.insert((name.clone(), dep.clone()));
        }
    }
    for pair in extra_ordering {
        ordering.insert((pair[0].clone(), pair[1].clone()));
    }
    // Drop self-pairs and only keep relationships where both sides exist in `vars`.
    let ordering: HashSet<(String, String)> = ordering
        .into_iter()
        .filter(|(a, b)| a != b && is_allowed_ordering(a, b))
        .collect();
    let mut ordering: Vec<[String; 2]> = transitive_closure_ordering(ordering)
        .into_iter()
        .filter(|(a, b)| a != b)
        .map(|(a, b)| [a, b])
        .collect();
    ordering.sort();
    section.ordering = ordering;

    // 7. not_equal
    let mut not_equal: Vec<[String; 2]> = parsed
        .var_ne
        .iter()
        .filter(|(a, b)| a != b && is_allowed_not_equal(a, b))
        .map(|(a, b)| [a.clone(), b.clone()])
        .collect();
    not_equal.sort();
    section.not_equal = not_equal;

    // 8. skipped: append parser's skipped items.
    section.skipped.extend(parsed.skipped.iter().cloned());

    // 9. Derive an explicit upper bound for a sum_limit denominator count var
    //    (test-case count / jagged row count) when the HTML stated none.
    apply_sum_count_bounds(section);

    // 10. Materialize bounds implied by the persisted relations after every
    //     parser-side range derivation is available. This covers relations
    //     extracted from function operands, such as `K <= min(N, M)`, which
    //     are representable in `ordering` but not as one scalar BoundExpr.
    apply_ordering_range_bounds(section);
}

/// Tighten numeric ranges using the same relations persisted in `ordering`.
///
/// For `a <= b`, a concrete upper bound of `b` limits `a`, and a concrete
/// lower bound of `a` limits `b`. Iterate to a fixed point because ordering
/// may contain independently extracted or transitively closed relations.
fn apply_ordering_range_bounds(section: &mut RandomTestSection) {
    fn lit_bound(repr: &BoundRepr) -> Option<i64> {
        match repr {
            BoundRepr::Lit(n) => Some(*n),
            BoundRepr::Expr(_) => None,
        }
    }

    loop {
        let current: std::collections::HashMap<String, (Option<i64>, Option<i64>)> = section
            .vars
            .iter()
            .filter_map(|(name, var)| {
                var.range
                    .as_ref()
                    .map(|range| (name.clone(), (lit_bound(&range[0]), lit_bound(&range[1]))))
            })
            .collect();
        let mut changed = false;
        for [lower, upper] in &section.ordering {
            let Some((lower_lo, _)) = current.get(lower).copied() else {
                continue;
            };
            let Some((_, upper_hi)) = current.get(upper).copied() else {
                continue;
            };
            if let Some(hi) = upper_hi {
                if let Some(var) = section.vars.get_mut(lower) {
                    if let Some(range) = &mut var.range {
                        if lit_bound(&range[1]).is_none_or(|old| hi < old) {
                            range[1] = BoundRepr::Lit(hi);
                            changed = true;
                        }
                    }
                }
            }
            if let Some(lo) = lower_lo {
                if let Some(var) = section.vars.get_mut(upper) {
                    if let Some(range) = &mut var.range {
                        if lit_bound(&range[0]).is_none_or(|old| lo > old) {
                            range[0] = BoundRepr::Lit(lo);
                            changed = true;
                        }
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
}

/// Step 9: write an exact, entailed upper bound onto each sum_limit denominator
/// count variable that the HTML left unbounded.
///
/// Not a default: in `C` repetitions each contributing a size var
/// `S >= lo >= 1` whose total is bounded by `Σ S <= L`, we have
/// `C·lo <= Σ S <= L`, hence `C <= floor(L / lo)`. The bound is written
/// verbatim into the yml so it stays auditable; the generator still renders
/// the yml as-is (the no-implicit-defaults invariant binds the generator, not
/// this parser-side exact derivation).
fn apply_sum_count_bounds(section: &mut RandomTestSection) {
    let mut derived: std::collections::BTreeMap<String, i64> = std::collections::BTreeMap::new();
    collect_sum_count_bounds(&section.format, &section.vars, &mut derived);
    for (c, hi) in derived {
        if hi < 1 {
            continue; // L < lo: constraints are contradictory; leave the
                      // placeholder so the runtime surfaces it via `missing`.
        }
        let Some(entry) = section.vars.get_mut(&c) else {
            continue;
        };
        match &entry.range {
            None => {
                entry.range = Some([BoundRepr::Lit(0), BoundRepr::Lit(hi)]);
            }
            Some([lo, BoundRepr::Expr(p)]) if p == "_" => {
                // Don't write a reversed/contradictory range.
                if let BoundRepr::Lit(lc) = lo {
                    if hi < *lc {
                        continue;
                    }
                }
                let lo = lo.clone();
                entry.range = Some([lo, BoundRepr::Lit(hi)]);
            }
            _ => {} // Explicit upper bound stated in the HTML: never override.
        }
    }
}

/// Collect, per sum_limit denominator count variable `C`, the tightest exact
/// upper bound `min_i floor(L_i / lo_i)` derivable from the sum-limited size
/// variables `S_i` it governs. Mirrors the runtime
/// `random_test::gen::collect_sum_denominators` pairing semantics:
/// `TestCases.count` governs sum-limited vars in its subtree; a jagged
/// `Array.count` governs its `len` variable.
fn collect_sum_count_bounds(
    blocks: &[FormatBlock],
    vars: &std::collections::BTreeMap<String, VarConstraint>,
    out: &mut std::collections::BTreeMap<String, i64>,
) {
    let consider =
        |out: &mut std::collections::BTreeMap<String, i64>, count_var: &str, size_var: &str| {
            let Some(s) = vars.get(size_var) else { return };
            let Some(limit) = s.sum_limit else { return };
            let Some([BoundRepr::Lit(lo), _]) = &s.range else {
                return; // size var lower bound not a concrete literal
            };
            if *lo < 1 {
                return; // condition: size var lower bound must be >= 1
            }
            let cand = limit / *lo; // floor division (i64)
            out.entry(count_var.to_string())
                .and_modify(|e| *e = (*e).min(cand))
                .or_insert(cand);
        };
    for b in blocks {
        match b {
            FormatBlock::TestCases(tc) => {
                let inner = analyze_format(&tc.format);
                for name in &inner.referenced_names {
                    consider(out, &tc.count, name);
                }
                collect_sum_count_bounds(&tc.format, vars, out);
            }
            FormatBlock::Array(ab) if ab.jagged => {
                if let (Some(c), Some(l)) = (&ab.count, &ab.len) {
                    consider(out, c, l);
                }
            }
            FormatBlock::Queries(q) => {
                for br in &q.types {
                    collect_sum_count_bounds(&br.format, vars, out);
                }
            }
            _ => {}
        }
    }
}

/// Resolve `bound` into a `[lo, hi]` pair. Each side is either a literal
/// integer (when resolution succeeds) or the `_` placeholder (when no literal
/// can be derived even via transitive Var resolution).
///
/// As a single typed exception, `usize` lower bound defaults to `0` instead of
/// `_`, since negative usize values are nonsensical.
fn resolve_or_placeholder(
    bound: &NumBound,
    ctx: &std::collections::HashMap<String, NumBound>,
    var_type: &VarType,
) -> [BoundRepr; 2] {
    let lo_default = match var_type {
        VarType::Usize => BoundRepr::Lit(0),
        _ => BoundRepr::Expr("_".to_string()),
    };
    let hi_default = BoundRepr::Expr("_".to_string());
    let lo = bound
        .lo
        .as_ref()
        .and_then(|e| resolve_lit(e, ctx, Side::Lo))
        .map(BoundRepr::Lit)
        .unwrap_or(lo_default);
    let hi = bound
        .hi
        .as_ref()
        .and_then(|e| resolve_lit(e, ctx, Side::Hi))
        .map(BoundRepr::Lit)
        .unwrap_or(hi_default);
    [lo, hi]
}

/// What kind of yml `len:` representation a string variable's length spec resolves to.
enum LenResolution {
    /// `len: <varname>` — both bounds are the same simple Var reference.
    DirectRef(String),
    /// `len: "<int>"` — both bounds are the same literal integer, stored as
    /// the same size-expression string form used by format len/count fields.
    FixedLit(i64),
    /// `len: |var|` — synthetic length variable; its bounds (possibly partial)
    /// are taken from the spec and resolved later via the standard usize
    /// `resolve_or_placeholder` path.
    Synthetic(String),
    /// No length information available.
    None,
}

fn resolve_len_kind(var_name: &str, spec: &StrSpec) -> LenResolution {
    let lo = &spec.len_lo;
    let hi = &spec.len_hi;

    // (a) Both bounds are the same `Var(name, offset=0)` → direct reference.
    if let (
        Some(BoundExpr::Var {
            name: ln,
            offset: lo_off,
        }),
        Some(BoundExpr::Var {
            name: hn,
            offset: hi_off,
        }),
    ) = (lo, hi)
    {
        if ln == hn && *lo_off == 0 && *hi_off == 0 {
            return LenResolution::DirectRef(ln.clone());
        }
    }

    // (b) Both bounds are the same literal → fixed length.
    if let (Some(BoundExpr::Lit(a)), Some(BoundExpr::Lit(b))) = (lo, hi) {
        if a == b {
            return LenResolution::FixedLit(*a);
        }
    }

    // (c) Any partial / mixed information → synthetic. The synthetic var's
    // range is computed downstream by `resolve_or_placeholder` against
    // `NumBound { lo: spec.len_lo, hi: spec.len_hi }`.
    if lo.is_some() || hi.is_some() {
        return LenResolution::Synthetic(format!("|{}|", var_name));
    }

    LenResolution::None
}

// ─── annotate_ymls_with_format ────────────────────────────────────────────────

/// Parse `task.html` in `dest_dir` and append `random_test:` sections to each
/// yml file in `yml_paths` whose stem matches a task letter.
pub(crate) fn annotate_ymls_with_format(
    dest_dir: &Utf8Path,
    yml_paths: &[Utf8PathBuf],
    shell: &mut Shell,
) -> anyhow::Result<()> {
    let task_path = dest_dir.join("task.html");
    if !task_path.exists() {
        return Ok(());
    }
    let html =
        fs::read_to_string(&task_path).with_context(|| format!("failed to read {task_path}"))?;
    let sections = parse_task_sections(&html);

    for yml_path in yml_paths {
        let stem = match yml_path.file_stem() {
            Some(s) => s,
            None => continue,
        };
        let letter_upper = stem.to_ascii_uppercase();

        let task = match sections.iter().find(|t| t.letter == letter_upper) {
            Some(t) => t,
            None => continue,
        };

        let section = task_to_format_blocks(task);

        if let Err(e) = append_format_to_yml(yml_path, &section) {
            shell.warn(format!("annotate_ymls_with_format: {e}"))?;
        }
    }
    Ok(())
}
