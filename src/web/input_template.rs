//! Best-effort input template generator from `random_test:` yml sections.
//!
//! Renders `proconio::input!` declarations from `FormatBlock` values stored in
//! testcase yml files. HTML parsing lives in `crate::parse`.

use crate::parse::{read_random_test_section, snake, FormatBlock, VarConstraint, VarType};
use crate::shell::Shell;
use anyhow::Context as _;
use camino::{Utf8Path, Utf8PathBuf};
use std::collections::{BTreeMap, HashMap};
use std::fs;

struct GuessResult {
    decls: Vec<String>,
    extra_lines: Vec<String>,
}

// ─── format_blocks_to_guess_result ───────────────────────────────────────────

fn var_ty(name: &str, vars: &BTreeMap<String, VarConstraint>) -> (&'static str, bool) {
    match vars.get(name).map(|v| &v.r#type) {
        Some(VarType::I64) => ("i64", false),
        Some(VarType::Chars) => ("Chars", true),
        _ => ("usize", false),
    }
}

/// Convert `FormatBlock` values to `GuessResult` for `proconio::input!` rendering.
fn format_blocks_to_guess_result(
    blocks: &[FormatBlock],
    vars: &BTreeMap<String, VarConstraint>,
) -> GuessResult {
    let mut decls: Vec<String> = Vec::new();
    let extra_lines: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for block in blocks {
        match block {
            FormatBlock::Scalars(sb) => {
                for var in &sb.vars {
                    if seen.insert(var.clone()) {
                        let (ty, _is_str) = var_ty(var, vars);
                        decls.push(format!("{var}: {ty},"));
                    }
                }
            }
            FormatBlock::Array(arr) => {
                let name = &arr.base;
                if seen.insert(name.clone()) {
                    let (elem_ty, is_str) = var_ty(name, vars);
                    // Jagged (proconio jagged syntax `[[T]; n]`) takes precedence.
                    if arr.jagged {
                        let count = arr.count.as_deref().unwrap_or("_");
                        decls.push(format!("{name}: [[{elem_ty}]; {count}],"));
                    } else if is_str {
                        // Chars: the type itself reads one whole line, so the
                        // inner per-string length is NOT a type parameter.
                        match (&arr.height, &arr.count) {
                            (Some(h), Some(f)) => {
                                decls.push(format!("{name}: [[Chars; {h}]; {f}],"));
                            }
                            (None, Some(c)) => {
                                decls.push(format!("{name}: [Chars; {c}],"));
                            }
                            (None, None) => {
                                decls.push(format!("{name}: Chars,"));
                            }
                            _ => {
                                decls.push(format!("{name}: Chars, /* TODO: array */"));
                            }
                        }
                    } else {
                        match (&arr.len, &arr.height, &arr.count) {
                            (Some(w), Some(h), Some(f)) => {
                                decls.push(format!("{name}: [[[{elem_ty}; {w}]; {h}]; {f}],"));
                            }
                            (Some(w), None, Some(h)) => {
                                decls.push(format!("{name}: [[{elem_ty}; {w}]; {h}],"));
                            }
                            (Some(l), None, None) => {
                                decls.push(format!("{name}: [{elem_ty}; {l}],"));
                            }
                            (None, None, Some(c)) => {
                                decls.push(format!("{name}: [{elem_ty}; {c}],"));
                            }
                            _ => {
                                decls.push(format!("{name}: {elem_ty}, /* TODO: array */"));
                            }
                        }
                    }
                }
            }
            FormatBlock::Rows(rows) => {
                let vs = &rows.vars;
                let count_expr = &rows.len;
                if vs.len() == 1 {
                    let name = snake(&vs[0]);
                    let (elem_ty, _is_str) = var_ty(&vs[0], vars);
                    if seen.insert(name.clone()) {
                        decls.push(format!("{name}: [{elem_ty}; {count_expr}],"));
                    }
                } else {
                    let name = snake(&vs.join(""));
                    let tys: Vec<(&str, bool)> = vs.iter().map(|v| var_ty(v, vars)).collect();
                    let ty_str: Vec<&str> = tys.iter().map(|(t, _)| *t).collect();
                    let ty = format!("[({tys}); {count_expr}]", tys = ty_str.join(", "));
                    if seen.insert(name.clone()) {
                        decls.push(format!("{name}: {ty},"));
                    }
                }
            }
            FormatBlock::TestCases(_) | FormatBlock::Queries(_) => {
                // handled at render level
            }
        }
    }

    GuessResult { decls, extra_lines }
}

// ─── render_section_from_format_blocks ───────────────────────────────────────

/// Render a Rust `main` template from `FormatBlock` values.
fn render_section_from_format_blocks(
    blocks: &[FormatBlock],
    vars: &BTreeMap<String, VarConstraint>,
) -> anyhow::Result<String> {
    let header_blocks: Vec<FormatBlock> = blocks
        .iter()
        .filter(|b| !matches!(b, FormatBlock::TestCases(_) | FormatBlock::Queries(_)))
        .cloned()
        .collect();

    let test_cases_block = blocks.iter().find_map(|b| {
        if let FormatBlock::TestCases(tc) = b {
            Some(tc)
        } else {
            None
        }
    });
    let queries_block = blocks.iter().find_map(|b| {
        if let FormatBlock::Queries(q) = b {
            Some(q)
        } else {
            None
        }
    });

    let GuessResult { decls, extra_lines } = format_blocks_to_guess_result(&header_blocks, vars);

    let needs_chars = vars.values().any(|v| matches!(v.r#type, VarType::Chars));

    let mut out: Vec<String> = Vec::new();

    if let Some(tc) = test_cases_block {
        let inner = format_blocks_to_guess_result(&tc.format, vars);
        push_use_line(&mut out, needs_chars);
        out.push("fn main() {".to_string());
        out.push("    input! {".to_string());
        for d in &decls {
            out.push(format!("        {d}"));
        }
        out.push("    }".to_string());
        for l in &extra_lines {
            out.push(format!("    {l}"));
        }
        out.push(format!("    for _ in 0..{} {{", tc.count));
        out.push("        input! {".to_string());
        for d in &inner.decls {
            out.push(format!("            {d}"));
        }
        out.push("        }".to_string());
        for l in &inner.extra_lines {
            out.push(format!("        {l}"));
        }
        out.push("        /* solve testcase */".to_string());
        out.push("    }".to_string());
        out.push("}".to_string());
        return Ok(out.join("\n"));
    }

    if let Some(qb) = queries_block {
        push_use_line(&mut out, needs_chars);
        out.push("fn main() {".to_string());
        out.push("    input! {".to_string());
        for d in &decls {
            out.push(format!("        {d}"));
        }
        out.push("    }".to_string());
        for l in &extra_lines {
            out.push(format!("    {l}"));
        }
        out.push(format!("    for _ in 0..{} {{", qb.count));

        let disc = qb.discriminator.as_deref().unwrap_or("qt");
        let all_numeric = qb.types.iter().all(|t| t.id.parse::<i32>().is_ok());
        if all_numeric && !qb.types.is_empty() {
            out.push(format!("        input! {{ {disc}: usize }}"));
            out.push(format!("        match {disc} {{"));
            for branch in &qb.types {
                let inner = format_blocks_to_guess_result(&branch.format, vars);
                let fields: Vec<String> = inner
                    .decls
                    .iter()
                    .map(|d| d.trim_end_matches(',').to_string())
                    .collect();
                if fields.is_empty() {
                    out.push(format!("            {} => {{}},", branch.id));
                } else {
                    out.push(format!(
                        "            {} => {{ input! {{ {} }} }},",
                        branch.id,
                        fields.join(", ")
                    ));
                }
            }
            out.push("            _ => unreachable!(),".to_string());
            out.push("        }".to_string());
        } else if qb.types.len() == 1 {
            let inner = format_blocks_to_guess_result(&qb.types[0].format, vars);
            let fields: Vec<String> = inner
                .decls
                .iter()
                .map(|d| d.trim_end_matches(',').to_string())
                .collect();
            let all_fields = std::iter::once(format!("{disc}: usize"))
                .chain(fields)
                .collect::<Vec<_>>()
                .join(", ");
            out.push(format!("        input! {{ {all_fields} }}"));
        } else {
            out.push("        /* TODO: per-query fields */".to_string());
        }

        out.push("        /* process query */".to_string());
        out.push("    }".to_string());
        out.push("}".to_string());
        return Ok(out.join("\n"));
    }

    // Plain input
    push_use_line(&mut out, needs_chars);
    out.push("fn main() {".to_string());
    out.push("    input! {".to_string());
    for d in &decls {
        out.push(format!("        {d}"));
    }
    out.push("    }".to_string());
    for l in &extra_lines {
        out.push(format!("    {l}"));
    }
    out.push("}".to_string());
    Ok(out.join("\n"))
}

fn push_use_line(out: &mut Vec<String>, needs_chars: bool) {
    if needs_chars {
        out.push("use proconio::{input, fastout, marker::Chars};".to_string());
    } else {
        out.push("use proconio::{input, fastout};".to_string());
    }
    out.push(String::new());
    out.push("#[fastout]".to_string());
}

// ─── generate_template ────────────────────────────────────────────────────────

/// Generate templates for all tasks that have a `random_test:` section in
/// `dest_dir/testcases/*.yml`.
///
/// Returns a map of destination source paths to generated file contents.
/// Returns `None` if the `testcases/` directory does not exist.
pub(crate) fn generate_template(
    dest_dir: &Utf8Path,
    shell: &mut Shell,
) -> anyhow::Result<Option<HashMap<Utf8PathBuf, String>>> {
    let testcases_dir = dest_dir.join("testcases");
    if !testcases_dir.exists() {
        return Ok(None);
    }
    let src_dir = dest_dir.join("src").join("bin");
    let mut out: HashMap<Utf8PathBuf, String> = HashMap::new();
    let entries =
        fs::read_dir(&testcases_dir).with_context(|| format!("failed to read {testcases_dir}"))?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let Some(ext) = path.extension() else {
            continue;
        };
        if ext != "yml" {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let yml_path = Utf8Path::from_path(&path)
            .ok_or_else(|| anyhow::anyhow!("non-UTF-8 path: {}", path.display()))?;

        let rt = match read_random_test_section(yml_path) {
            Ok(Some(rt)) => rt,
            _ => continue,
        };

        let src_path = src_dir.join(stem).with_extension("rs");
        match render_section_from_format_blocks(&rt.format, &rt.vars) {
            Ok(content) => {
                out.insert(src_path, content);
            }
            Err(err) => {
                shell.warn(format!("render_section failed for {stem}: {err}"))?;
            }
        }
    }
    Ok(Some(out))
}

#[cfg(test)]
#[path = "input_template_tests.rs"]
mod tests;
