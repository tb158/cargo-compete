//! Text normalization helpers for constraint and format-line parsing.

use regex::Regex;

pub(crate) fn snake(s: &str) -> String {
    let mut out = String::new();
    let mut prev_is_underscore = false;
    for ch in s.chars() {
        let c = if ch.is_ascii_alphanumeric() { ch } else { '_' };
        if c == '_' {
            if !prev_is_underscore {
                out.push('_');
            }
            prev_is_underscore = true;
        } else {
            out.push(c.to_ascii_lowercase());
            prev_is_underscore = false;
        }
    }
    out.trim_matches('_').to_string()
}

pub(crate) fn normalize_constraint(s: &str) -> String {
    let mut t = s.to_string();
    t = t
        .replace("≤", "<=")
        .replace("≦", "<=")
        .replace("≧", ">=")
        .replace("≥", ">=");
    t = t.replace("\\leq", "<=").replace("\\le", "<=");
    t = t.replace("\\geq", ">=").replace("\\ge", ">=");
    t = t.replace("\\lt", "<").replace("\\gt", ">");
    t = t.replace("−", "-");
    t = t.replace("\\times", "*").replace("×", "*");
    // LaTeX escapes for braces and explicit space.
    t = t.replace("\\lbrace", "{").replace("\\rbrace", "}");
    t = t.replace("\\ ", "");
    t = t.replace(' ', "");
    t
}

pub(crate) fn normalize_line(line: &str) -> String {
    let mut s = line.trim().to_string();
    s = s.replace("\\cdots", "\\ldots").replace("\\dots", "\\ldots");
    s = s.replace("\\vdots", " \\vdots ");
    s = s.replace("\\ldots", " \\ldots ");

    let latex_cmd_re = Regex::new(r"\\(?:mathrm|mathit|text|rm|textit|textbf)\{([^}]+)\}").unwrap();
    s = latex_cmd_re.replace_all(&s, "$1").to_string();

    let underscore_re = Regex::new(r"\s*_\s*").unwrap();
    s = underscore_re.replace_all(&s, "_").to_string();

    let comma_re = Regex::new(r",\s+").unwrap();
    s = comma_re.replace_all(&s, ",").to_string();
    let brace_left_re = Regex::new(r"\{\s+").unwrap();
    s = brace_left_re.replace_all(&s, "{").to_string();
    let brace_right_re = Regex::new(r"\s+\}").unwrap();
    s = brace_right_re.replace_all(&s, "}").to_string();

    let brace_concat_re = Regex::new(r"\}([A-Za-z\\])").unwrap();
    s = brace_concat_re.replace_all(&s, "} $1").to_string();
    let bracket_concat_re = Regex::new(r"\]([A-Za-z\\])").unwrap();
    s = bracket_concat_re.replace_all(&s, "] $1").to_string();
    let digit_concat_re = Regex::new(r"([A-Za-z]_\d+)([A-Za-z\\])").unwrap();
    s = digit_concat_re.replace_all(&s, "$1 $2").to_string();

    let ws_re = Regex::new(r"\s+").unwrap();
    s = ws_re.replace_all(&s, " ").to_string();
    s.trim().to_string()
}

pub(crate) fn is_concat_hint(orig: &str, norm: &str) -> bool {
    let o = orig.split_whitespace().count();
    let n = norm.split_whitespace().count();
    n > o
}

pub(crate) fn is_case_placeholder_line(line: &str) -> bool {
    let l = line.to_ascii_lowercase();
    l.contains("case") && (l.contains('_') || l.contains("\\mathrm"))
}

pub(crate) fn is_query_placeholder_line(line: &str) -> bool {
    let l = line.to_ascii_lowercase();
    l.contains("query") && (l.contains('_') || l.contains("\\mathrm") || l.contains("\\text"))
}

/// Extract the loop variable from a placeholder line such as
/// `\text{case}_T`, `\mathrm{query}_Q`, or `case_T`.
///
/// `keyword` is the placeholder body (e.g. `"case"`, `"query"`).
/// Returns the snake-cased subscript on match (e.g. `"t"`, `"q"`).
pub(crate) fn extract_keyword_subscript(line: &str, keyword: &str) -> Option<String> {
    let pattern = format!(
        r"(?i)(?:\\(?:text|mathrm|rm)\s*\{{\s*{kw}\s*\}}|{kw})_\s*\{{?\s*([A-Za-z][A-Za-z0-9]*)\s*\}}?",
        kw = regex::escape(keyword),
    );
    let re = Regex::new(&pattern).unwrap();
    let cap = re.captures(line)?;
    Some(snake(cap.get(1)?.as_str()))
}

pub(crate) fn extract_case_subscript(line: &str) -> Option<String> {
    extract_keyword_subscript(line, "case")
}

pub(crate) fn extract_query_subscript(line: &str) -> Option<String> {
    extract_keyword_subscript(line, "query")
}

pub(crate) fn sym_expr(s: &str) -> String {
    let mut t = s.trim().replace(' ', "");
    t = t.replace('\\', "");
    if let Some((a, b)) = t.split_once('-') {
        if b.chars().all(|c| c.is_ascii_digit()) {
            return format!("{}-{}", snake(a), b);
        }
    }
    let coef_re = Regex::new(r"^(\d+)([A-Za-z]+)$").unwrap();
    if let Some(cap) = coef_re.captures(&t) {
        return format!("{}*{}", &cap[1], snake(&cap[2]));
    }
    if t.chars().all(|c| c.is_ascii_alphabetic()) {
        return snake(&t);
    }
    t
}

/// Return the size of an indexed span as `last - first + 1`.
///
/// Size fields can represent a literal or one variable plus a literal offset,
/// so spans such as `-1..W` become `w+2` and `0..N-1` become `n`. Spans that
/// would require subtracting independent variables are intentionally rejected.
pub(crate) fn len_expr(first_idx: &str, last_raw: &str) -> Option<String> {
    #[derive(Debug)]
    enum Affine {
        Lit(i64),
        Var(String, i64),
    }

    fn parse_affine(raw: &str) -> Option<Affine> {
        let expr = sym_expr(raw);
        if let Ok(n) = expr.parse::<i64>() {
            return Some(Affine::Lit(n));
        }
        let re = Regex::new(r"^\(?([a-z][a-z0-9_]*)\)?(?:([+-])(\d+))?$").unwrap();
        let cap = re.captures(&expr)?;
        let name = cap.get(1)?.as_str().to_string();
        let offset = match (cap.get(2), cap.get(3)) {
            (Some(sign), Some(n)) => {
                let n = n.as_str().parse::<i64>().ok()?;
                if sign.as_str() == "-" {
                    -n
                } else {
                    n
                }
            }
            _ => 0,
        };
        Some(Affine::Var(name, offset))
    }

    fn render_var_offset(name: String, offset: i64) -> String {
        match offset.cmp(&0) {
            std::cmp::Ordering::Equal => name,
            std::cmp::Ordering::Greater => format!("{name}+{offset}"),
            std::cmp::Ordering::Less => format!("{name}{offset}"),
        }
    }

    match (parse_affine(first_idx)?, parse_affine(last_raw)?) {
        (Affine::Lit(first), Affine::Lit(last)) => {
            let len = last - first + 1;
            (len >= 0).then(|| len.to_string())
        }
        (Affine::Lit(first), Affine::Var(name, last_off)) => {
            Some(render_var_offset(name, last_off - first + 1))
        }
        (Affine::Var(first_name, first_off), Affine::Var(last_name, last_off))
            if first_name == last_name =>
        {
            let len = last_off - first_off + 1;
            (len >= 0).then(|| len.to_string())
        }
        _ => None,
    }
}

pub(crate) fn base_var(tok: &str) -> Option<String> {
    let mut t = tok.to_string();
    t = t
        .replace("\\mathrm", "")
        .replace("\\text", "")
        .replace("\\rm", "");
    t = t.replace(['{', '}'], "");
    t = t.replace('|', "");
    t = t.replace('\\', "");
    if t.is_empty() {
        return None;
    }
    let start = t
        .char_indices()
        .find(|(_, c)| c.is_ascii_alphabetic())
        .map(|(i, _)| i)?;
    let t = &t[start..];
    let mut base = t;
    if let Some((b, _)) = base.split_once('_') {
        base = b;
    }
    if let Some((b, _)) = base.split_once('[') {
        base = b;
    }
    if base.is_empty() {
        return None;
    }
    Some(snake(base))
}

pub(crate) fn is_var_name(s: &str) -> bool {
    !s.is_empty()
        && s.parse::<u64>().is_err()
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Extract base variable names from a text slice that is already scoped by the
/// caller to a variable-list position.
///
/// Accepts plain and subscripted names (`N`, `A_i`, `A_{i,j}`), absolute-length
/// names (`|S|`, `|A_i|`), multiple names separated by ASCII/Japanese
/// punctuation, and Japanese prose prefixes. Plain names adjacent to arithmetic
/// operators are ignored so expressions like `N-1` or `N^2` are not mistaken for
/// variable lists; `|S|+|T|` keeps the pipe-wrapped names because `|S|` and `S`
/// are distinct variables in the random-test yml.
pub(crate) fn extract_var_names(s: &str) -> Vec<String> {
    let ident = r"[A-Za-z][A-Za-z0-9]*(?:_(?:\{[^}]*\}|[A-Za-z0-9]+))?";
    let re = Regex::new(&format!(r"\|{ident}\| | {ident}").replace(' ', "")).unwrap();
    let mut names = Vec::new();
    for m in re.find_iter(s) {
        let raw = m.as_str();
        let pipe_wrapped = raw.starts_with('|') && raw.ends_with('|');
        let before = s[..m.start()].chars().next_back();
        let after = s[m.end()..].chars().next();
        let adjacent_to_non_separator = |c: char| c.is_ascii_alphanumeric() || c == '\\';
        if pipe_wrapped {
            if before.is_some_and(adjacent_to_non_separator)
                || after.is_some_and(adjacent_to_non_separator)
            {
                continue;
            }
        } else if before.is_some_and(|c| c.is_ascii_alphanumeric() || "\\+-*^".contains(c))
            || after.is_some_and(|c| c.is_ascii_alphanumeric() || "+-*^".contains(c))
        {
            continue;
        }
        let inner = if pipe_wrapped {
            raw.trim_matches('|')
        } else {
            raw
        };
        let base = inner
            .split('_')
            .next()
            .unwrap_or(inner)
            .trim_matches(|c| c == '{' || c == '}');
        if base.is_empty() {
            continue;
        }
        let base = snake(base);
        if base == "min" || base == "max" {
            continue;
        }
        let name = if pipe_wrapped {
            format!("|{}|", base)
        } else {
            base
        };
        if !name.is_empty() && !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// Find all `<vars> は` occurrences anywhere in the constraint item.
///
/// Returns `(var_base_names, rest_after_は)` for each match. The variable
/// list may use `,` or `、` (Japanese full-width comma) as separator, and
/// each variable may have a subscript (`X_i`, `S_{i,j}`) which is stripped
/// to its base name.
///
/// Designed to be position-independent: matches a `<var(s)> は` pattern even
/// when preceded by Japanese prose like `全ての ... について、`.
pub(crate) fn find_var_decls(item: &str) -> Vec<(Vec<String>, String)> {
    let ident = r"(?:\|[A-Za-z][A-Za-z0-9]*(?:_(?:\{[^}]*\}|[A-Za-z0-9]+))?\||[A-Za-z][A-Za-z0-9]*(?:_(?:\{[^}]*\}|[A-Za-z0-9]+))?)";
    let re = Regex::new(&format!(r"({ident}(?:\s*[、,]\s*{ident})*)\s*は")).unwrap();
    re.captures_iter(item)
        .filter_map(|cap| {
            let vars_str = cap.get(1)?.as_str();
            let names = extract_var_names(vars_str);
            if names.is_empty() {
                return None;
            }
            let rest = item[cap.get(0)?.end()..].to_string();
            Some((names, rest))
        })
        .collect()
}
