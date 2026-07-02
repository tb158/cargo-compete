//! Input format parsing: convert lines from a `<pre>` block into `FormatBlock` values.

use super::normalize::{
    base_var, is_case_placeholder_line, is_concat_hint, is_query_placeholder_line, is_var_name,
    len_expr, normalize_line, snake,
};
use super::types::{ArrayBlock, FormatBlock, RowsBlock, ScalarsBlock, VarConstraint};
use regex::Regex;

pub(crate) fn parse_indexed_token(token: &str) -> Option<(String, Vec<String>)> {
    let t = token.trim();
    let bracket_re = Regex::new(r"^([A-Za-z]+)((?:\[[^\]]+\])+)$").unwrap();
    if let Some(cap) = bracket_re.captures(t) {
        let base = cap.get(1)?.as_str().to_string();
        let rest = cap.get(2)?.as_str();
        let idx_re = Regex::new(r"\[([^\]]+)\]").unwrap();
        let mut idxs = Vec::new();
        for c in idx_re.captures_iter(rest) {
            let idx = c.get(1)?.as_str().trim().to_string();
            if !idx.is_empty() {
                idxs.push(idx);
            }
        }
        if !idxs.is_empty() {
            return Some((base, idxs));
        }
    }

    let us_re = Regex::new(r"^([A-Za-z]+)_(?:\{)?(.+?)(?:\})?$").unwrap();
    if let Some(cap) = us_re.captures(t) {
        let base = cap.get(1)?.as_str().to_string();
        let idxs_raw = cap.get(2)?.as_str();
        let idxs = idxs_raw
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();
        if !idxs.is_empty() {
            return Some((base, idxs));
        }
    }
    None
}

#[derive(Debug, Clone)]
struct IndexedLineSpan {
    base: String,
    first: Vec<String>,
    last: Vec<String>,
}

/// Parse one horizontal indexed sequence regardless of whether intermediate
/// elements are written explicitly or replaced by an ellipsis.
fn parse_indexed_line_span(line: &str) -> Option<IndexedLineSpan> {
    let toks: Vec<&str> = line
        .split_whitespace()
        .filter(|tok| !matches!(*tok, "\\ldots" | "\\vdots"))
        .collect();
    if toks.len() < 2 {
        return None;
    }
    let parsed = toks
        .iter()
        .map(|tok| parse_indexed_token(tok))
        .collect::<Option<Vec<_>>>()?;
    let (base, first) = parsed.first()?.clone();
    let (_, last) = parsed.last()?.clone();
    if first.is_empty()
        || parsed
            .iter()
            .any(|(b, idxs)| b != &base || idxs.len() != first.len())
    {
        return None;
    }
    // A row may vary only along its innermost dimension. Outer indexes name
    // which row/frame is being written.
    if parsed
        .iter()
        .any(|(_, idxs)| idxs[..idxs.len() - 1] != first[..first.len() - 1])
    {
        return None;
    }
    len_expr(first.last()?, last.last()?)?;
    Some(IndexedLineSpan { base, first, last })
}

pub(crate) fn parse_1d_array_line(line: &str) -> Option<(String, String)> {
    let span = parse_indexed_line_span(line)?;
    if span.first.len() != 1 {
        return None;
    }
    Some((
        span.base,
        len_expr(span.first.first()?, span.last.first()?)?,
    ))
}

pub(crate) fn parse_n_repeat(lines: &[String], idx: usize) -> Option<(Vec<String>, String, usize)> {
    fn parse_first_line(line: &str) -> Option<(Vec<String>, String)> {
        let toks: Vec<&str> = line.split_whitespace().collect();
        if toks.len() < 2 {
            return None;
        }
        let mut bases = Vec::new();
        let mut first_idx: Option<String> = None;
        for tok in toks {
            let (base, idxs) = parse_indexed_token(tok)?;
            if idxs.len() != 1 {
                return None;
            }
            let idx = idxs[0].clone();
            if let Some(prev) = &first_idx {
                if *prev != idx {
                    return None;
                }
            } else {
                first_idx = Some(idx);
            }
            bases.push(base);
        }
        Some((bases, first_idx?))
    }

    fn parse_last_line(line: &str, bases: &[String]) -> Option<String> {
        let toks: Vec<&str> = line.split_whitespace().collect();
        if toks.len() != bases.len() {
            return None;
        }
        let mut last_idx: Option<String> = None;
        for (tok, base) in toks.into_iter().zip(bases.iter()) {
            let (b, idxs) = parse_indexed_token(tok)?;
            if &b != base || idxs.len() != 1 {
                return None;
            }
            let idx = idxs[0].clone();
            if let Some(prev) = &last_idx {
                if *prev != idx {
                    return None;
                }
            } else {
                last_idx = Some(idx);
            }
        }
        last_idx
    }

    let (bases, first_idx) = parse_first_line(lines.get(idx)?)?;

    let mut last_idx: Option<String> = None;
    let mut last_found: Option<usize> = None;
    let mut j = idx + 1;
    while j < lines.len() {
        if lines[j].contains("\\vdots")
            || lines[j].contains("\\ldots")
            || lines[j].contains("\\cdots")
            || lines[j].contains("\\dots")
        {
            j += 1;
            continue;
        }
        if let Some(idx_expr) = parse_last_line(&lines[j], &bases) {
            last_idx = Some(idx_expr);
            last_found = Some(j);
            j += 1;
            continue;
        }
        break;
    }
    let last_idx = last_idx?;
    let count_expr = len_expr(&first_idx, &last_idx)?;
    let consumed = last_found.map(|lf| lf + 1 - idx).unwrap_or(1);
    Some((bases, count_expr, consumed))
}

fn parse_vertical_indexed_span(lines: &[String], idx: usize) -> Option<(String, String, usize)> {
    let (base, first) = parse_indexed_token(lines.get(idx)?.trim())?;
    if first.len() != 1 {
        return None;
    }
    let mut last: Option<String> = None;
    let mut last_found: Option<usize> = None;
    let mut j = idx + 1;
    while j < lines.len() {
        if lines[j].contains("\\vdots") {
            j += 1;
            continue;
        }
        if let Some((next_base, indexes)) = parse_indexed_token(lines[j].trim()) {
            if next_base == base && indexes.len() == 1 {
                last = Some(indexes[0].clone());
                last_found = Some(j);
                j += 1;
                continue;
            }
        }
        break;
    }
    let length = len_expr(first.first()?, &last?)?;
    let consumed = last_found.map(|found| found + 1 - idx).unwrap_or(1);
    Some((base, length, consumed))
}

pub(crate) fn parse_vertical_scalars(
    lines: &[String],
    idx: usize,
) -> Option<(String, String, usize)> {
    parse_vertical_indexed_span(lines, idx)
}

/// Parse repeated rows of one rectangular indexed array. The same logic
/// handles explicit and abbreviated inner dimensions: dimensions are derived
/// from each span's endpoints, while vertical ellipses only connect rows.
fn parse_rectangular_block(
    lines: &[String],
    idx: usize,
    dimensions: usize,
) -> Option<(String, Vec<String>, usize)> {
    let first = parse_indexed_line_span(lines.get(idx)?)?;
    if first.first.len() != dimensions || dimensions < 2 {
        return None;
    }
    let inner_len = len_expr(first.first.last()?, first.last.last()?)?;
    let mut last_outer: Option<Vec<String>> = None;
    let mut last_found: Option<usize> = None;
    let mut j = idx + 1;
    while j < lines.len() {
        if lines[j].contains("\\vdots") {
            j += 1;
            continue;
        }
        if let Some(row) = parse_indexed_line_span(&lines[j]) {
            if row.base == first.base
                && row.first.len() == dimensions
                && len_expr(row.first.last()?, row.last.last()?)? == inner_len
            {
                last_outer = Some(row.first[..dimensions - 1].to_vec());
                last_found = Some(j);
                j += 1;
                continue;
            }
        }
        break;
    }
    let last_outer = last_outer?;
    let mut lengths = Vec::with_capacity(dimensions);
    for (from, to) in first.first[..dimensions - 1].iter().zip(last_outer.iter()) {
        lengths.push(len_expr(from, to)?);
    }
    lengths.push(inner_len);
    let consumed = last_found.map(|found| found + 1 - idx).unwrap_or(1);
    Some((first.base, lengths, consumed))
}

pub(crate) fn parse_matrix_block(
    lines: &[String],
    idx: usize,
) -> Option<(String, String, String, usize)> {
    let (base, lengths, consumed) = parse_rectangular_block(lines, idx, 2)?;
    Some((base, lengths[1].clone(), lengths[0].clone(), consumed))
}

pub(crate) fn parse_3d_array_block(
    lines: &[String],
    idx: usize,
) -> Option<(String, String, String, String, usize)> {
    let (base, lengths, consumed) = parse_rectangular_block(lines, idx, 3)?;
    Some((
        base,
        lengths[2].clone(),
        lengths[1].clone(),
        lengths[0].clone(),
        consumed,
    ))
}

pub(crate) fn parse_varlen_rows(
    lines: &[String],
    idx: usize,
) -> Option<(Vec<String>, String, String, String, usize)> {
    struct VarlenRow {
        prefix_bases: Vec<String>,
        row_idx: String,
        len_base: String,
        elem_base: String,
    }

    fn parse_row(line: &str) -> Option<VarlenRow> {
        if !line.contains("\\ldots") {
            return None;
        }
        let toks: Vec<&str> = line
            .split_whitespace()
            .filter(|t| *t != "\\ldots")
            .collect();
        if toks.len() < 3 {
            return None;
        }

        let parsed = toks
            .iter()
            .map(|t| parse_indexed_token(t))
            .collect::<Option<Vec<_>>>()?;

        let first_elem_pos = parsed.iter().position(|(_, idxs)| idxs.len() == 2)?;
        if first_elem_pos == 0 {
            return None;
        }

        let (elem_base, elem_idxs) = &parsed[first_elem_pos];
        if elem_idxs.len() != 2 || !elem_idxs[1].chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let row_idx = elem_idxs[0].clone();

        let mut prefix_bases = Vec::new();
        for (base, idxs) in parsed.iter().take(first_elem_pos) {
            if idxs.len() != 1 || idxs[0] != row_idx {
                return None;
            }
            prefix_bases.push(base.clone());
        }

        let (last_base, last_idxs) = parsed.last()?;
        if last_base != elem_base || last_idxs.len() != 2 || last_idxs[0] != row_idx {
            return None;
        }
        let len_base = base_var(&last_idxs[1])?;
        if !prefix_bases
            .iter()
            .any(|b| base_var(b).as_deref() == Some(len_base.as_str()))
        {
            return None;
        }

        Some(VarlenRow {
            prefix_bases,
            row_idx,
            len_base,
            elem_base: elem_base.clone(),
        })
    }

    let first = parse_row(lines.get(idx)?)?;
    let mut last_row: Option<String> = None;
    let mut last_found: Option<usize> = None;

    let mut j = idx + 1;
    while j < lines.len() {
        if lines[j].contains("\\vdots") {
            j += 1;
            continue;
        }
        if let Some(row) = parse_row(&lines[j]) {
            if row.prefix_bases == first.prefix_bases
                && row.len_base == first.len_base
                && row.elem_base == first.elem_base
            {
                last_row = Some(row.row_idx);
                last_found = Some(j);
                j += 1;
                continue;
            }
        }
        break;
    }

    let last_row = last_row?;
    let count_expr = len_expr(&first.row_idx, &last_row)?;
    let consumed = last_found.map(|lf| lf + 1 - idx).unwrap_or(1);
    Some((
        first.prefix_bases,
        first.len_base,
        first.elem_base,
        count_expr,
        consumed,
    ))
}

/// Fold separate-line Jagged rows into the inline form.
///
/// abc446-b-style input has the per-row length on its own line:
/// ```text
/// L_1
/// X_{1,1} X_{1,2} … X_{1,L_1}
/// ```
/// The inline form (abc457-c) is `L_1 X_{1,1} … X_{1,L_1}`. We detect a line
/// that is a single 1-D-indexed token (`L_1`) immediately followed by an
/// ellipsis row whose trailing element's second subscript references that
/// token's base, and merge the pair. `norm` drives detection; `orig` is folded
/// at the same indices so callers keep a 1:1 norm/orig correspondence.
fn fold_separate_len_rows(norm: &[String], orig: &[String]) -> (Vec<String>, Vec<String>) {
    let mut out_norm: Vec<String> = Vec::with_capacity(norm.len());
    let mut out_orig: Vec<String> = Vec::with_capacity(norm.len());
    let mut i = 0usize;
    while i < norm.len() {
        let cur = norm[i].trim();
        let single = {
            let toks: Vec<&str> = cur.split_whitespace().collect();
            toks.len() == 1 && parse_indexed_token(toks[0]).is_some_and(|(_, idxs)| idxs.len() == 1)
        };
        if single && i + 1 < norm.len() {
            let nxt = norm[i + 1].trim();
            let is_elem_row =
                nxt.contains("\\ldots") || nxt.contains("\\cdots") || nxt.contains("\\dots");
            let len_base_matches = || -> bool {
                let cur_base = parse_indexed_token(cur.split_whitespace().next().unwrap_or(""))
                    .map(|(b, _)| b.to_ascii_lowercase());
                let last_tok = nxt
                    .split_whitespace()
                    .filter(|t| !t.starts_with("\\"))
                    .next_back();
                let last_len_base = last_tok
                    .and_then(parse_indexed_token)
                    .filter(|(_, idxs)| idxs.len() == 2)
                    .and_then(|(_, idxs)| base_var(&idxs[1]))
                    .map(|b| b.to_ascii_lowercase());
                match (cur_base, last_len_base) {
                    (Some(a), Some(b)) => a == b,
                    _ => false,
                }
            };
            if is_elem_row && len_base_matches() {
                out_norm.push(format!("{} {}", cur, nxt));
                out_orig.push(format!("{} {}", orig[i].trim(), orig[i + 1].trim()));
                i += 2;
                continue;
            }
        }
        out_norm.push(norm[i].clone());
        out_orig.push(orig[i].clone());
        i += 1;
    }
    (out_norm, out_orig)
}

/// Inner implementation: collects unrecognised original lines into `skipped`.
/// Indexed arrays retain physical dimensions here; `task.rs` maps those
/// dimensions to type-specific yml fields after variable types are known.
pub(crate) fn lines_to_format_blocks_inner(
    lines: &[String],
    skipped: &mut Vec<String>,
) -> Vec<FormatBlock> {
    let raw_norm: Vec<String> = lines.iter().map(|l| normalize_line(l)).collect();
    // Fold separate-line Jagged rows (`L_i` on its own line followed by
    // `X_{i,1} … X_{i,L_i}`) into the single-line form so the existing
    // `parse_varlen_rows` (which only recognises the inline form) handles
    // both. Per docs/random-test-requirements.md the yml must not distinguish
    // the two HTML layouts.
    let (norm_lines, orig_owned) = fold_separate_len_rows(&raw_norm, lines);
    let orig_lines: &[String] = &orig_owned;
    let mut blocks: Vec<FormatBlock> = Vec::new();
    let mut i = 0usize;

    while i < norm_lines.len() {
        let ln = &norm_lines[i];
        if is_case_placeholder_line(ln) || is_query_placeholder_line(ln) || ln.contains("\\vdots") {
            i += 1;
            continue;
        }

        // Variable-length rows (Jagged): L_i a_{i,1}…a_{i,L_i}
        if let Some((_prefix_bases, len_base, elem_base, count_expr, consumed)) =
            parse_varlen_rows(&norm_lines, i)
        {
            blocks.push(FormatBlock::Array(ArrayBlock {
                base: snake(&elem_base),
                len: Some(snake(&len_base)),
                height: None,
                count: Some(count_expr),
                jagged: true,
            }));
            i += consumed;
            continue;
        }

        // 3-D indexed array: retain physical dimensions until the base type is known.
        if let Some((base, w_expr, h_expr, f_expr, consumed)) = parse_3d_array_block(&norm_lines, i)
        {
            blocks.push(FormatBlock::Array(ArrayBlock {
                base: snake(&base),
                len: Some(w_expr),
                height: Some(h_expr),
                count: Some(f_expr),
                jagged: false,
            }));
            i += consumed;
            continue;
        }

        // 2-D indexed array: retain physical dimensions until the base type is known.
        if let Some((base, w_expr, h_expr, consumed)) = parse_matrix_block(&norm_lines, i) {
            blocks.push(FormatBlock::Array(ArrayBlock {
                base: snake(&base),
                len: Some(w_expr),
                height: None,
                count: Some(h_expr),
                jagged: false,
            }));
            i += consumed;
            continue;
        }

        // N-repeat tuples: x_1 y_1 … x_M y_M
        if let Some((bases, count_expr, consumed)) = parse_n_repeat(&norm_lines, i) {
            blocks.push(FormatBlock::Rows(RowsBlock {
                len: count_expr,
                vars: bases.iter().map(|b| snake(b)).collect(),
            }));
            i += consumed;
            continue;
        }

        // Vertical indexed array: retain its physical length until the base type is known.
        if let Some((base, count_expr, consumed)) = parse_vertical_scalars(&norm_lines, i) {
            blocks.push(FormatBlock::Array(ArrayBlock {
                base: snake(&base),
                len: Some(count_expr),
                height: None,
                count: None,
                jagged: false,
            }));
            i += consumed;
            continue;
        }

        // 1-D indexed array: A_1 … A_N or A_1 A_2 A_3
        if let Some((base, len_e)) = parse_1d_array_line(ln) {
            let concat_hint = is_concat_hint(orig_lines.get(i).unwrap_or(&String::new()), ln);
            if concat_hint {
                blocks.push(FormatBlock::Scalars(ScalarsBlock {
                    vars: vec![snake(&base)],
                }));
            } else {
                blocks.push(FormatBlock::Array(ArrayBlock {
                    base: snake(&base),
                    len: Some(len_e),
                    height: None,
                    count: None,
                    jagged: false,
                }));
            }
            i += 1;
            continue;
        }

        // Plain scalar line: N M K
        if ln.contains(' ')
            && !ln.contains("\\ldots")
            && !ln.contains("\\cdots")
            && !ln.contains("\\dots")
            && !ln.contains('_')
            && !ln.contains('{')
            && !ln.contains('}')
        {
            let vars: Vec<String> = ln.split_whitespace().map(snake).collect();
            blocks.push(FormatBlock::Scalars(ScalarsBlock { vars }));
            i += 1;
            continue;
        }

        // Subscripted scalars on one line: S_x S_y
        if ln.contains(' ') && ln.contains('_') && !ln.contains("\\ldots") {
            let mut ok = true;
            let mut vars: Vec<String> = Vec::new();
            for tok in ln.split_whitespace() {
                if let Some((base, idxs)) = parse_indexed_token(tok) {
                    if idxs.len() != 1 {
                        ok = false;
                        break;
                    }
                    vars.push(snake(&format!("{}_{}", base, idxs[0])));
                } else if tok.chars().all(|c| c.is_ascii_alphanumeric()) {
                    vars.push(snake(tok));
                } else {
                    ok = false;
                    break;
                }
            }
            if ok {
                blocks.push(FormatBlock::Scalars(ScalarsBlock { vars }));
                i += 1;
                continue;
            }
        }

        // Single symbol
        if !ln.contains(' ')
            && !ln.contains("\\ldots")
            && !ln.contains("\\cdots")
            && !ln.contains("\\dots")
        {
            blocks.push(FormatBlock::Scalars(ScalarsBlock {
                vars: vec![snake(ln.trim())],
            }));
            i += 1;
            continue;
        }

        // Unrecognised: keep the original line in `random_test.skipped`.
        skipped.push(orig_lines[i].clone());
        i += 1;
    }

    blocks
}

#[cfg(test)]
pub(crate) fn lines_to_format_blocks(lines: &[String]) -> Vec<FormatBlock> {
    lines_to_format_blocks_inner(lines, &mut Vec::new())
}

/// Recursively collect variable names referenced by a block tree.
pub(crate) fn collect_var_names(blocks: &[FormatBlock]) -> Vec<String> {
    let mut names = Vec::new();
    for block in blocks {
        match block {
            FormatBlock::Scalars(sb) => names.extend(sb.vars.iter().cloned()),
            FormatBlock::Array(ab) => {
                names.push(ab.base.clone());
                if let Some(l) = &ab.len {
                    if is_var_name(l) {
                        names.push(l.clone());
                    }
                }
                if let Some(c) = &ab.count {
                    if is_var_name(c) {
                        names.push(c.clone());
                    }
                }
                if let Some(h) = &ab.height {
                    if is_var_name(h) {
                        names.push(h.clone());
                    }
                }
            }
            FormatBlock::Rows(rb) => {
                if is_var_name(&rb.len) {
                    names.push(rb.len.clone());
                }
                names.extend(rb.vars.iter().cloned());
            }
            FormatBlock::TestCases(tb) => {
                if is_var_name(&tb.count) {
                    names.push(tb.count.clone());
                }
                names.extend(collect_var_names(&tb.format));
            }
            FormatBlock::Queries(qb) => {
                if is_var_name(&qb.count) {
                    names.push(qb.count.clone());
                }
                if let Some(d) = &qb.discriminator {
                    names.push(d.clone());
                }
                for qt in &qb.types {
                    names.extend(collect_var_names(&qt.format));
                }
            }
        }
    }
    names
}

pub(crate) fn add_missing_usize_vars(
    vars: &mut std::collections::BTreeMap<String, VarConstraint>,
    blocks: &[FormatBlock],
) {
    for name in collect_var_names(blocks) {
        vars.entry(name).or_default();
    }
}

#[cfg(test)]
mod parse_1d_tests {
    use super::parse_1d_array_line;

    #[test]
    fn short_form() {
        assert_eq!(
            parse_1d_array_line(r"A_1 \ldots A_N"),
            Some(("A".to_string(), "n".to_string()))
        );
    }

    #[test]
    fn full_form_with_middle() {
        assert_eq!(
            parse_1d_array_line(r"A_1 A_2 \ldots A_N"),
            Some(("A".to_string(), "n".to_string()))
        );
    }

    #[test]
    fn zero_indexed() {
        // 0-origin array: length is N (last index N-1).
        assert_eq!(
            parse_1d_array_line(r"A_0 \ldots A_{N-1}"),
            Some(("A".to_string(), "n".to_string()))
        );
    }

    #[test]
    fn braced_indices() {
        assert_eq!(
            parse_1d_array_line(r"A_{1} \ldots A_{M}"),
            Some(("A".to_string(), "m".to_string()))
        );
    }

    #[test]
    fn fixed_indexed_line_uses_endpoint_span() {
        assert_eq!(
            parse_1d_array_line(r"A_1 A_2 A_3"),
            Some(("A".to_string(), "3".to_string()))
        );
    }

    #[test]
    fn shifted_index_span_keeps_affine_offset() {
        assert_eq!(
            parse_1d_array_line(r"A_{-1} \ldots A_W"),
            Some(("A".to_string(), "w+2".to_string()))
        );
    }

    #[test]
    fn not_an_array_without_indexes() {
        assert_eq!(parse_1d_array_line("A B C"), None);
    }
}

#[cfg(test)]
mod indexed_span_block_tests {
    use super::{lines_to_format_blocks, parse_vertical_scalars};
    use crate::parse::FormatBlock;

    #[test]
    fn vertical_1d_supports_shifted_endpoints() {
        let lines = vec![
            r"A_{-1}".to_string(),
            r"\vdots".to_string(),
            r"A_W".to_string(),
        ];
        assert_eq!(
            parse_vertical_scalars(&lines, 0),
            Some(("A".to_string(), "w+2".to_string(), 3))
        );
    }

    #[test]
    fn fixed_width_matrix_can_have_dynamic_row_span() {
        let lines = vec![
            r"X_{1,1} X_{1,2} X_{1,3}".to_string(),
            r"X_{2,1} X_{2,2} X_{2,3}".to_string(),
            r"\vdots".to_string(),
            r"X_{N,1} X_{N,2} X_{N,3}".to_string(),
        ];
        let blocks = lines_to_format_blocks(&lines);
        match &blocks[0] {
            FormatBlock::Array(a) => {
                assert_eq!(a.base, "x");
                assert_eq!(a.len.as_deref(), Some("3"));
                assert_eq!(a.count.as_deref(), Some("n"));
            }
            other => panic!("expected array, got {:?}", other),
        }
    }

    #[test]
    fn fully_fixed_matrix_remains_literal_sized() {
        let lines = vec![
            r"A_{1,1} A_{1,2} A_{1,3}".to_string(),
            r"A_{2,1} A_{2,2} A_{2,3}".to_string(),
            r"A_{3,1} A_{3,2} A_{3,3}".to_string(),
        ];
        let blocks = lines_to_format_blocks(&lines);
        match &blocks[0] {
            FormatBlock::Array(a) => {
                assert_eq!(a.len.as_deref(), Some("3"));
                assert_eq!(a.count.as_deref(), Some("3"));
            }
            other => panic!("expected array, got {:?}", other),
        }
    }

    #[test]
    fn fixed_inner_3d_uses_outer_endpoint_spans() {
        let lines = vec![
            r"A_{1,1,1} A_{1,1,2}".to_string(),
            r"\vdots".to_string(),
            r"A_{F,H,1} A_{F,H,2}".to_string(),
        ];
        let blocks = lines_to_format_blocks(&lines);
        match &blocks[0] {
            FormatBlock::Array(a) => {
                assert_eq!(a.len.as_deref(), Some("2"));
                assert_eq!(a.height.as_deref(), Some("h"));
                assert_eq!(a.count.as_deref(), Some("f"));
            }
            other => panic!("expected array, got {:?}", other),
        }
    }

    #[test]
    fn indexed_grid_retains_physical_dimension_spans() {
        let lines = vec![
            r"S_{-1,1} S_{-1,2}".to_string(),
            r"\vdots".to_string(),
            r"S_{H,1} S_{H,2}".to_string(),
        ];
        let blocks = super::lines_to_format_blocks_inner(&lines, &mut Vec::new());
        match &blocks[0] {
            FormatBlock::Array(a) => {
                assert_eq!(a.base, "s");
                assert_eq!(a.len.as_deref(), Some("2"));
                assert_eq!(a.count.as_deref(), Some("h+2"));
            }
            other => panic!("expected array, got {:?}", other),
        }
    }

    #[test]
    fn jagged_outer_count_uses_same_dimension_span() {
        let lines = vec![
            r"L_{-1} A_{-1,1} \ldots A_{-1,L_{-1}}".to_string(),
            r"\vdots".to_string(),
            r"L_N A_{N,1} \ldots A_{N,L_N}".to_string(),
        ];
        let got = super::parse_varlen_rows(&lines, 0).expect("must parse");
        assert_eq!(got.1, "l");
        assert_eq!(got.3, "n+2");
    }

    #[test]
    fn vertical_indexed_array_retains_physical_dimension_span() {
        let lines = vec![
            r"S_{-1}".to_string(),
            r"\vdots".to_string(),
            r"S_H".to_string(),
        ];
        assert_eq!(
            super::parse_vertical_scalars(&lines, 0),
            Some(("S".to_string(), "h+2".to_string(), 3))
        );
    }

    #[test]
    fn tuple_rows_count_uses_same_dimension_span() {
        let lines = vec![
            r"X_{-1} Y_{-1}".to_string(),
            r"\vdots".to_string(),
            r"X_M Y_M".to_string(),
        ];
        let got = super::parse_n_repeat(&lines, 0).expect("must parse");
        assert_eq!(got.1, "m+2");
    }
}
