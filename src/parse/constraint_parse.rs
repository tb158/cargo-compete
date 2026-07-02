//! Constraint parsing: HTML `<li>` constraint items → structured representation.
//!
//! The output `ConstraintParse` is an intermediate representation that is later
//! merged into the yml `vars` / `ordering` / `not_equal` sections by
//! `task.rs::enrich_vars_with_constraints`.
//!
//! Design: accumulator pattern. Each `try_*` recogniser takes `&mut ConstraintParse`
//! and returns `bool` indicating whether the item was consumed. `is_*` helpers are
//! pure read-only checks. Patterns are tried in priority order in `parse_constraints`.

use super::normalize::normalize_constraint;
use regex::Regex;
use std::collections::{HashMap, HashSet};

// ─── Public types ─────────────────────────────────────────────────────────────

/// Accumulator collecting all constraint information from a task's `<li>` items.
#[derive(Debug, Clone, Default)]
pub(crate) struct ConstraintParse {
    pub num_vars: HashMap<String, NumBound>,
    /// Discrete value sets from `X は a, b のいずれか` / `X \in {a, b}` constraints.
    /// Variables present here are excluded from `num_vars` (no range needed).
    pub enum_values: HashMap<String, Vec<i64>>,
    pub str_vars: HashMap<String, StrSpec>,
    pub var_le: HashSet<(String, String)>,
    pub var_ne: HashSet<(String, String)>,
    pub all_distinct: HashSet<String>,
    pub sum_limits: HashMap<String, i64>,
    pub skipped: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct NumBound {
    pub lo: Option<BoundExpr>,
    pub hi: Option<BoundExpr>,
}

/// Internal bound expression. Resolved to literal `i64` at yml-write time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BoundExpr {
    Lit(i64),
    /// Variable reference with optional integer offset (e.g. `n`, `n-1`).
    Var {
        name: String,
        offset: i64,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct StrSpec {
    /// Allowed characters. Currently informational only — `task.rs::detect_string_vars`
    /// is the canonical source of `values:` in the yml, kept verbatim to preserve
    /// existing output. May be used by a future random-test generator.
    #[allow(dead_code)]
    pub charset: Vec<char>,
    pub len_lo: Option<BoundExpr>,
    pub len_hi: Option<BoundExpr>,
}

/// Side selector used by `resolve_lit`.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Side {
    Lo,
    Hi,
}

// ─── Top-level entry point ────────────────────────────────────────────────────

pub(crate) fn parse_constraints(items: &[String]) -> ConstraintParse {
    let mut acc = ConstraintParse::default();

    // Pass 1: detect string-variable declarations (`S は ... からなる ...`).
    // Runs first so subsequent passes can recognise constraints involving
    // already-known string variables and skip them as numeric constraints.
    detect_string_decls(items, &mut acc);

    // Pass 2: `1 ≤ |S| ≤ N`-style abs-length updates for already-detected strings.
    apply_abs_length_updates(items, &mut acc);

    // Pass 3: every other pattern.
    for item in items {
        let handled = try_enum(item, &mut acc)
            || try_all_distinct(item, &mut acc)
            || try_sum_limit(item, &mut acc)
            || is_already_string_constraint(item, &acc)
            || try_inequality_chain(item, &mut acc)
            || is_ignorable(item);
        if !handled {
            acc.skipped.push(item.clone());
        }
    }

    // Numeric bounds for variables that are also string variables are not
    // meaningful; remove them.
    for name in acc.str_vars.keys().cloned().collect::<Vec<_>>() {
        acc.num_vars.remove(&name);
    }

    acc
}

// ─── Resolver: BoundExpr → i64 ────────────────────────────────────────────────

/// Resolve a `BoundExpr` to a literal `i64` using already-parsed bounds for
/// referenced variables. Returns `None` when no literal answer is available.
pub(crate) fn resolve_lit(
    expr: &BoundExpr,
    ctx: &HashMap<String, NumBound>,
    side: Side,
) -> Option<i64> {
    resolve_lit_with_depth(expr, ctx, side, 8)
}

fn resolve_lit_with_depth(
    expr: &BoundExpr,
    ctx: &HashMap<String, NumBound>,
    side: Side,
    depth: usize,
) -> Option<i64> {
    if depth == 0 {
        return None;
    }
    match expr {
        BoundExpr::Lit(n) => Some(*n),
        BoundExpr::Var { name, offset } => {
            let target = ctx.get(name)?;
            let inner = match side {
                Side::Lo => target.lo.as_ref()?,
                Side::Hi => target.hi.as_ref()?,
            };
            let n = resolve_lit_with_depth(inner, ctx, side, depth - 1)?;
            Some(n + offset)
        }
    }
}

// ─── Numeric expression evaluation ────────────────────────────────────────────

/// Evaluate a constant integer expression (`10^5`, `2*10^5`, `-10^9` etc.).
/// Returns `None` for variable-containing expressions.
fn eval_expr(expr: &str) -> Option<i64> {
    let expr = expr.trim();
    if let Ok(n) = expr.parse::<i64>() {
        return Some(n);
    }
    // Leading unary minus: `-10^6` is `-(10^6)`, not `(-10)^6`. eval_expr has
    // no binary subtraction (only `*` / `^` / parens), so a leading `-` is
    // always a sign.
    if let Some(rest) = expr.strip_prefix('-') {
        return eval_expr(rest).map(|v| -v);
    }
    if let Some(pos) = expr.rfind('*') {
        let left = eval_expr(&expr[..pos])?;
        let right = eval_expr(&expr[pos + 1..])?;
        return left.checked_mul(right);
    }
    if let Some(pos) = expr.find('^') {
        let base = eval_expr(&expr[..pos])?;
        let exp = eval_expr(&expr[pos + 1..])?;
        if (0..=30).contains(&exp) {
            return Some(base.pow(exp as u32));
        }
    }
    if let Some(stripped) = expr.strip_prefix('(').and_then(|s| s.strip_suffix(')')) {
        return eval_expr(stripped);
    }
    // LaTeX braces around grouped expressions: `10^{12}` → exponent fragment `{12}`
    if let Some(stripped) = expr.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
        return eval_expr(stripped);
    }
    None
}

/// Parse a single bound expression (literal, variable, or `var±k`). Returns
/// `None` for non-recognised expressions.
fn parse_bound_expr(expr: &str) -> Option<BoundExpr> {
    let expr = expr.trim();
    if let Some(n) = eval_expr(expr) {
        return Some(BoundExpr::Lit(n));
    }
    // var ± integer offset
    for (sep, sign) in &[('-', -1i64), ('+', 1i64)] {
        if let Some(idx) = expr.rfind(*sep) {
            // skip leading `-` of negative numbers
            if idx == 0 {
                continue;
            }
            let var = expr[..idx].trim();
            let k = expr[idx + 1..].trim();
            if let Some(name) = extract_var_base(var) {
                if let Ok(k) = k.parse::<i64>() {
                    return Some(BoundExpr::Var {
                        name,
                        offset: sign * k,
                    });
                }
            }
        }
    }
    if let Some(name) = extract_var_base(expr) {
        return Some(BoundExpr::Var { name, offset: 0 });
    }
    None
}

/// Extract a base variable name from an expression. Accepts plain names
/// (`u`, `N`) and subscripted forms (`u_i`, `t_N`, `A_{i,j}`), returning the
/// lowercase base before the first `_`.
fn extract_var_base(s: &str) -> Option<String> {
    let names = extract_var_names(s);
    (names.len() == 1).then(|| names[0].clone())
}

/// Extract base variable names from a comma-separated token like `A,B` or
/// `A_i,B_j`. Used for left/right sides of inequalities.
fn extract_var_names(tok: &str) -> Vec<String> {
    super::normalize::extract_var_names(tok)
}

// ─── String-declaration pass ──────────────────────────────────────────────────

fn detect_string_decls(items: &[String], acc: &mut ConstraintParse) {
    for item in items {
        let Some((vars, spec)) = parse_string_decl(item) else {
            continue;
        };
        for v in vars {
            acc.str_vars.insert(v, spec.clone());
        }
    }
}

/// Decide the charset for a string-declaration constraint by looking at the
/// whole item. Position-independent so we don't depend on where the keyword
/// appears relative to `は`.
fn determine_charset(item: &str) -> Option<Vec<char>> {
    if item.contains("英小文字") {
        Some(('a'..='z').collect())
    } else if item.contains("英大文字") {
        Some(('A'..='Z').collect())
    } else if item.contains("数字列") {
        Some(('0'..='9').collect())
    } else if item.matches("<code>").count() >= 2 {
        let code_re = Regex::new(r"<code>(.)</code>").unwrap();
        let chars: Vec<char> = code_re
            .captures_iter(item)
            .filter_map(|c| c.get(1).unwrap().as_str().chars().next())
            .collect();
        if chars.is_empty() {
            None
        } else {
            Some(chars)
        }
    } else if item.contains("文字列") {
        Some(('A'..='Z').chain('a'..='z').collect())
    } else {
        None
    }
}

fn parse_string_decl(item: &str) -> Option<(Vec<String>, StrSpec)> {
    let charset = determine_charset(item)?;

    let (len_lo, len_hi) = if item.contains("長さ") || item.contains("からなる") {
        parse_length_spec(item)
    } else {
        // Single character (grid cell)
        (Some(BoundExpr::Lit(1)), Some(BoundExpr::Lit(1)))
    };

    // Use the position-independent helper to find `<vars> は` anywhere in the
    // constraint (handles Japanese-prose prefixes like `全ての ... 、s は ...`).
    let decl_text = strip_html_tags(item);
    let (var_names, _) = super::normalize::find_var_decls(&decl_text)
        .into_iter()
        .next()?;

    Some((
        var_names,
        StrSpec {
            charset,
            len_lo,
            len_hi,
        },
    ))
}

/// Parse `長さ ...` patterns. Returns `(len_lo, len_hi)` either of which can be
/// `None` when only one side is specified.
///
/// Supported forms:
/// - `長さ X 以上 Y 以下` → (Some(X), Some(Y))
/// - `長さ X 以上`        → (Some(X), None)
/// - `長さ X 以下`        → (None, Some(X))
/// - `長さ X の`           → (Some(X), Some(X))  (fixed length)
fn parse_length_spec(item: &str) -> (Option<BoundExpr>, Option<BoundExpr>) {
    // Fuzzy strategy: do not anchor the whole sentence. Locate the `長さ`
    // marker, then the `以上` / `以下` keywords after it, and read the maximal
    // run of expression characters that sits next to each keyword. Japanese
    // particles / prose ("が", "は", "の長さは", "文字列 ... からなる") are
    // non-expression characters, so they are skipped automatically.
    let Some(len_pos) = item.find("長さ") else {
        return (None, None);
    };
    let after = &item["長さ".len() + len_pos..];

    let ge = after.find("以上");
    let le = after.find("以下");

    match (ge, le) {
        (Some(g), Some(l)) if g < l => {
            let lo = expr_ending_at(after, g);
            let hi_seg = &after[g + "以上".len()..l];
            let hi = expr_ending_at(hi_seg, hi_seg.len());
            (lo, hi)
        }
        (Some(g), _) => (expr_ending_at(after, g), None),
        (None, Some(l)) => (None, expr_ending_at(after, l)),
        (None, None) => {
            // Fixed length: `長さ X の ...`
            let stop = after.find('の').unwrap_or(after.len());
            let raw: String = after[..stop].chars().filter(|c| is_expr_char(*c)).collect();
            let bv = parse_bound_expr_simple(raw.trim());
            (bv.clone(), bv)
        }
    }
}

/// Characters that can appear in a bound expression (`2*10^5`, `N-1`, `100000`).
fn is_expr_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || " ^*+-{}()._\\".contains(c)
}

/// Read the maximal trailing run of expression characters in `s[..end]` and
/// parse it as a bound. Returns `None` when no expression precedes `end`.
fn expr_ending_at(s: &str, end: usize) -> Option<BoundExpr> {
    let head = &s[..end];
    let start = head
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_expr_char(*c))
        .last()
        .map(|(i, _)| i)?;
    parse_bound_expr_simple(head[start..].trim())
}

fn parse_bound_expr_simple(raw: &str) -> Option<BoundExpr> {
    let norm = normalize_constraint(raw);
    parse_bound_expr(&norm)
}

// ─── |S| <= N pass ────────────────────────────────────────────────────────────

fn apply_abs_length_updates(items: &[String], acc: &mut ConstraintParse) {
    let op_re = Regex::new(r"(<=|>=|<|>)").unwrap();

    for item in items {
        if !item.contains('|') {
            continue;
        }
        let norm = normalize_constraint(item);
        let (tokens, ops) = tokenize_inequality(&norm, &op_re);
        if ops.is_empty() {
            continue;
        }
        for i in 0..tokens.len() {
            let tok = tokens[i].trim();
            let var_names = extract_var_names(tok)
                .into_iter()
                .filter_map(|name| {
                    name.strip_prefix('|')
                        .and_then(|name| name.strip_suffix('|'))
                        .map(str::to_string)
                })
                .collect::<Vec<_>>();
            for var_name in var_names {
                let Some(spec) = acc.str_vars.get_mut(&var_name) else {
                    continue;
                };
                if i > 0 && (ops[i - 1] == "<=" || ops[i - 1] == "<") {
                    if let Some(b) = parse_bound_expr(tokens[i - 1].trim()) {
                        spec.len_lo = Some(b);
                    }
                }
                if i < ops.len() && (ops[i] == "<=" || ops[i] == "<") {
                    if let Some(b) = parse_bound_expr(tokens[i + 1].trim()) {
                        spec.len_hi = Some(b);
                    }
                }
            }
        }
    }
}

// ─── Tokenisation ─────────────────────────────────────────────────────────────

fn tokenize_inequality(norm: &str, op_re: &Regex) -> (Vec<String>, Vec<String>) {
    let mut tokens = Vec::new();
    let mut ops = Vec::new();
    let mut last = 0usize;
    for m in op_re.find_iter(norm) {
        tokens.push(norm[last..m.start()].to_string());
        ops.push(m.as_str().to_string());
        last = m.end();
    }
    tokens.push(norm[last..].to_string());
    (tokens, ops)
}

// ─── try_enum: B は 1, 2 のいずれか / X \in {1, 2} ──────────────────────────────

fn try_enum(item: &str, acc: &mut ConstraintParse) -> bool {
    if let Some((vars, vals)) = parse_either_alternative(item) {
        register_enum(acc, &vars, &vals);
        return true;
    }
    if let Some((vars, vals)) = parse_in_set(item) {
        register_enum(acc, &vars, &vals);
        return true;
    }
    false
}

fn parse_either_alternative(item: &str) -> Option<(Vec<String>, Vec<i64>)> {
    if !item.contains("のいずれか") && !item.contains("いずれかの") {
        return None;
    }
    let (var_names, rest) = super::normalize::find_var_decls(item).into_iter().next()?;
    let nums_raw = rest
        .find("のいずれか")
        .or_else(|| rest.find("いずれかの"))
        .map(|p| &rest[..p])?;
    let nums: Vec<i64> = nums_raw
        .split([',', '、', ' '])
        .filter_map(|p| p.trim().parse::<i64>().ok())
        .collect();
    if nums.is_empty() {
        return None;
    }
    Some((var_names, nums))
}

fn parse_in_set(item: &str) -> Option<(Vec<String>, Vec<i64>)> {
    let in_pos = item.find("\\in")?;
    let vars_str = item[..in_pos].trim();
    let rest = item[in_pos..].trim_start_matches("\\in").trim();
    let content = rest
        .trim_start_matches("\\lbrace")
        .trim_start_matches("\\{")
        .trim()
        .trim_end_matches("\\rbrace")
        .trim_end_matches("\\}")
        .trim();
    let nums: Vec<i64> = content
        .split(',')
        .filter_map(|p| p.trim().parse::<i64>().ok())
        .collect();
    if nums.is_empty() {
        return None;
    }
    let var_names = extract_var_names(vars_str);
    if var_names.is_empty() {
        return None;
    }
    Some((var_names, nums))
}

fn register_enum(acc: &mut ConstraintParse, vars: &[String], values: &[i64]) {
    for var in vars {
        acc.enum_values.insert(var.clone(), values.to_vec());
        // Remove any pre-existing numeric bound: enum supersedes range.
        acc.num_vars.remove(var);
    }
}

// ─── try_all_distinct: A_1, ..., A_N は相異なる ───────────────────────────────

fn try_all_distinct(item: &str, acc: &mut ConstraintParse) -> bool {
    if !item.contains("相異なる") {
        return false;
    }
    let var_part = item
        .split("はすべて相異なる")
        .next()
        .or_else(|| item.split("は相異なる").next())
        .unwrap_or("");
    let vars = extract_var_names(var_part);
    if vars.is_empty() {
        return false;
    }
    for var in vars {
        acc.all_distinct.insert(var);
    }
    true
}

// ─── try_sum_limit: X の総和は Y 以下 ─────────────────────────────────────────

fn try_sum_limit(item: &str, acc: &mut ConstraintParse) -> bool {
    // LaTeX form: `\sum_{i=1}^N L_i \le X` → sum_limits[l] = X.
    // Per docs/random-test-requirements.md ("Jagged 配列の仕様"): no special
    // Σ handling — the summed variable's base gets the RHS as its sum_limit.
    if item.contains("\\sum") && try_latex_sum_limit(item, acc) {
        return true;
    }
    let Some(pos) = item.find("の総和は") else {
        return false;
    };
    // Skip exponent-bearing constraints (`N^2 の総和`) which are not simple sums.
    let before = &item[..pos];
    if before.contains('^') {
        return false;
    }
    let var_names = extract_var_names(before);
    if var_names.is_empty() {
        return false;
    }
    let after = &item[pos + "の総和は".len()..];
    let limit_raw = ["以下", "を超えない"]
        .iter()
        .find_map(|d| after.find(d).map(|i| &after[..i]))
        .unwrap_or("")
        .trim();
    let Some(limit) = eval_expr(&normalize_constraint(limit_raw)) else {
        return false;
    };
    let targets_length = before.trim_end().ends_with("の長さ");
    for var_name in var_names {
        let target = if targets_length && !var_name.starts_with('|') {
            format!("|{var_name}|")
        } else {
            var_name
        };
        acc.sum_limits.insert(target, limit);
    }
    true
}

/// Parse `\sum_{...}^{...} <var>_<idx> ... \le <RHS>` (e.g. abc457-c
/// `\displaystyle \sum_{i=1}^N L_i \le 2\times 10^5`) into
/// `sum_limits[base(var)] = eval(RHS)`. Only the `\sum ... <= RHS` direction is
/// handled here; `T \le \sum ...` (an upper bound on T, not a sum limit) is
/// not representable by the current generator and is retained as `skipped`.
fn try_latex_sum_limit(item: &str, acc: &mut ConstraintParse) -> bool {
    let norm = normalize_constraint(item).replace("\\displaystyle", "");
    // Need `\sum` left of a `<=`/`<` comparator.
    let Some(cmp) = norm.find("<=").or_else(|| norm.find('<')) else {
        return false;
    };
    let (lhs, rhs_raw) = norm.split_at(cmp);
    let rhs = rhs_raw.trim_start_matches("<=").trim_start_matches('<');
    let Some(sum_pos) = lhs.find("\\sum") else {
        return false;
    };
    // Strip `\sum`, then an optional `_{...}` lower-index and `^...` upper-index.
    let after_sum = &lhs[sum_pos + "\\sum".len()..];
    let sub_re = Regex::new(r"^_\{[^}]*\}|^_[^\\^{]+").unwrap();
    let body = sub_re.replace(after_sum, "");
    let sup_re = Regex::new(r"^\^\{[^}]*\}|^\^[^\\^{]").unwrap();
    let body = sup_re.replace(&body, "");
    let vars = extract_var_names(&body);
    if vars.is_empty() {
        return false;
    }
    let Some(limit) = eval_expr(rhs) else {
        return false;
    };
    for var in vars {
        acc.sum_limits.insert(var, limit);
    }
    true
}

// ─── String-constraint follow-up detection ────────────────────────────────────

/// Constraint items that mention only an already-known string variable should
/// be considered handled (e.g. "S に含まれる文字は ..."). Without this they
/// would fall through to numeric inequality parsing, which could pollute the
/// numeric bounds.
fn is_already_string_constraint(item: &str, acc: &ConstraintParse) -> bool {
    let item = strip_html_tags(item);
    let Some(ha_pos) = item.find(" は") else {
        return false;
    };
    let vars_str = item[..ha_pos].trim();
    let vars = extract_var_names(vars_str);
    !vars.is_empty() && vars.iter().all(|v| acc.str_vars.contains_key(v))
}

// ─── try_inequality_chain: 1 ≤ N ≤ 10^5 / 1 ≤ A,B ≤ N etc. ────────────────────

/// Split an item into ASCII-only fragments containing inequality operators.
///
/// AtCoder constraints sometimes embed inequality chains within Japanese prose,
/// e.g. `1 種類目のクエリについて、1 ≤ x ≤ N-1`. The inequality body itself is
/// always ASCII, so we discard non-ASCII regions and keep contiguous ASCII spans
/// that contain at least one comparison operator.
pub(crate) fn extract_ascii_inequality_fragments(item: &str) -> Vec<String> {
    let replaced: String = item
        .chars()
        .map(|c| if c.is_ascii() { c } else { '\0' })
        .collect();
    replaced
        .split('\0')
        .filter(|chunk| {
            chunk.contains("<=")
                || chunk.contains(">=")
                || chunk.contains("!=")
                || chunk.contains('<')
                || chunk.contains('>')
        })
        .map(str::to_string)
        .collect()
}

fn try_inequality_chain(item: &str, acc: &mut ConstraintParse) -> bool {
    // A simple sum limit on the left side is already consumed by
    // `try_latex_sum_limit`. Remaining aggregate inequalities (for example
    // `K <= \sum C_i L_i`) cannot be represented as scalar ordering; parsing
    // their summation index would invent input variables such as `i`.
    if item.contains("\\sum") {
        return false;
    }

    // Normalize first so `\leq` becomes `<=`, then split on non-ASCII so the
    // fragment-with-op detection works on the canonicalised form.
    let norm = normalize_for_inequality(item);
    let mut handled = false;
    for fragment in extract_ascii_inequality_fragments(&norm) {
        if try_inequality_chain_single(&fragment, acc) {
            handled = true;
        }
    }
    handled
}

fn try_inequality_chain_single(norm: &str, acc: &mut ConstraintParse) -> bool {
    if !is_safe_for_inequality(norm) {
        return false;
    }
    let op_re = Regex::new(r"(<=|>=|!=|<|>)").unwrap();
    let (tokens, ops) = tokenize_inequality(norm, &op_re);
    if ops.is_empty() {
        return false;
    }

    let chain = InequalityChain { tokens, ops };
    let mut handled = false;
    if register_var_ne(&chain, acc) {
        handled = true;
    }
    if register_var_le(&chain, acc) {
        handled = true;
    }
    if register_num_bounds(&chain, acc) {
        handled = true;
    }
    handled
}

fn normalize_for_inequality(item: &str) -> String {
    let mut t = normalize_constraint(&strip_html_tags(item));
    t = t.replace("\\neq", "!=").replace("\\ne", "!=");
    t = t.replace("\\left(", "(").replace("\\right)", ")");

    // Convert Japanese natural-language range forms into inequality form so
    // the existing chain parser can handle them. Apply both-sided form first
    // to avoid partial overlap with the lo-only / hi-only fallbacks.
    // Fuzzy: allow Japanese prose/particles ("は", "の値は", "、" …) between the
    // variable and the bound instead of requiring a literal `は`. The bound is
    // the adjacent run of expression characters; the gap matches anything that
    // is not a digit or comparison operator (so it skips prose, not numbers).
    let ident = r"[A-Za-z][A-Za-z0-9]*(?:_(?:\{[^}]*\}|[A-Za-z0-9]+))?";
    let abs_ident = format!(r"\|{ident}\|");
    let var = format!(r"(?:{abs_ident}|{ident})");
    let vars = format!(r"{var}(?:[、,]{var})*");
    let expr = r"[0-9A-Za-z^*+\-{}().\\]+";
    let gap = r"[^0-9<>=]*?";
    let nat_both = Regex::new(&format!(r"({vars}){gap}({expr})以上{gap}({expr})以下")).unwrap();
    t = nat_both.replace_all(&t, "$2<=$1<=$3").to_string();
    let nat_lo = Regex::new(&format!(r"({vars}){gap}({expr})以上")).unwrap();
    t = nat_lo.replace_all(&t, "$2<=$1").to_string();
    let nat_hi = Regex::new(&format!(r"({vars}){gap}({expr})以下")).unwrap();
    t = nat_hi.replace_all(&t, "$1<=$2").to_string();

    // Strip parenthesised index ranges like `(1<=i<=N)` that follow the actual
    // inequality (e.g. `1<=A_i<=10^9(1<=i<=N)`). Such groups always contain
    // their own comparison operators, which lets us identify them safely.
    let index_range_re = Regex::new(r"\([^()]*(?:<=|>=|<|>)[^()]*\)").unwrap();
    t = index_range_re.replace_all(&t, "").to_string();
    t
}

fn strip_html_tags(item: &str) -> String {
    let tag_re = Regex::new(r"</?[A-Za-z][^>]*>").unwrap();
    tag_re.replace_all(item, "").to_string()
}

fn is_safe_for_inequality(norm: &str) -> bool {
    !norm.contains("dfrac") && !norm.contains("frac") && !norm.contains("sqrt")
}

struct InequalityChain {
    tokens: Vec<String>,
    ops: Vec<String>,
}

fn register_var_ne(chain: &InequalityChain, acc: &mut ConstraintParse) -> bool {
    let mut handled = false;
    for j in 0..chain.ops.len() {
        if chain.ops[j] != "!=" {
            continue;
        }
        let lt = chain.tokens[j].trim();
        let rt = chain.tokens[j + 1].trim();
        // Tuple forms like `(X_i,Y_i)` → skip; we can't represent tuple inequality.
        if (lt.starts_with('(') && lt.ends_with(')')) || (rt.starts_with('(') && rt.ends_with(')'))
        {
            continue;
        }
        let lv = extract_var_names(lt);
        let rv = extract_var_names(rt);
        if lv.len() == 1 && rv.len() == 1 {
            let pair = (lv[0].clone(), rv[0].clone());
            acc.var_ne.insert(pair);
            handled = true;
        }
    }
    handled
}

fn register_var_le(chain: &InequalityChain, acc: &mut ConstraintParse) -> bool {
    let mut handled = false;
    for j in 0..chain.ops.len() {
        let lv;
        let rv;
        match chain.ops[j].as_str() {
            "<=" | "<" => {
                lv = extract_var_names(chain.tokens[j].trim());
                rv = extract_var_names(chain.tokens[j + 1].trim());
            }
            ">=" | ">" => {
                lv = extract_var_names(chain.tokens[j + 1].trim());
                rv = extract_var_names(chain.tokens[j].trim());
            }
            _ => continue,
        }
        for l in &lv {
            for r in &rv {
                acc.var_le.insert((l.clone(), r.clone()));
                handled = true;
            }
        }
    }
    handled
}

fn register_num_bounds(chain: &InequalityChain, acc: &mut ConstraintParse) -> bool {
    let mut handled = false;
    for i in 0..chain.tokens.len() {
        let token_str = chain.tokens[i].trim();
        // Skip subscript-range tokens like "N,1" from "1 <= i <= N,1 <= j <= N".
        if token_str.contains(',') {
            let has_numeric_part = token_str
                .split(',')
                .any(|p| p.trim().starts_with(|c: char| c.is_ascii_digit()));
            if has_numeric_part {
                continue;
            }
        }
        let vars = extract_var_names(token_str);
        if vars.is_empty() {
            continue;
        }
        let lo = if i > 0 && (chain.ops[i - 1] == "<=" || chain.ops[i - 1] == "<") {
            parse_bound_expr(chain.tokens[i - 1].trim())
        } else if i < chain.ops.len() && (chain.ops[i] == ">=" || chain.ops[i] == ">") {
            parse_bound_expr(chain.tokens[i + 1].trim())
        } else {
            None
        };
        let hi = if i < chain.ops.len() && (chain.ops[i] == "<=" || chain.ops[i] == "<") {
            parse_bound_expr(chain.tokens[i + 1].trim())
        } else if i > 0 && (chain.ops[i - 1] == ">=" || chain.ops[i - 1] == ">") {
            parse_bound_expr(chain.tokens[i - 1].trim())
        } else {
            None
        };
        if lo.is_none() && hi.is_none() {
            continue;
        }
        for var in vars {
            // Variables already registered as enum: range is unnecessary, skip.
            if acc.enum_values.contains_key(&var) {
                continue;
            }
            let entry = acc.num_vars.entry(var).or_default();
            if let Some(b) = lo.clone() {
                if assign_bound(&mut entry.lo, b) {
                    handled = true;
                }
            }
            if let Some(b) = hi.clone() {
                if assign_bound(&mut entry.hi, b) {
                    handled = true;
                }
            }
        }
    }
    handled
}

/// Update a bound slot, preferring an existing literal over a variable reference.
/// Returns whether the slot changed.
fn assign_bound(slot: &mut Option<BoundExpr>, new: BoundExpr) -> bool {
    match (slot.as_ref(), &new) {
        // Existing literal beats incoming variable reference.
        (Some(BoundExpr::Lit(_)), BoundExpr::Var { .. }) => false,
        _ => {
            *slot = Some(new);
            true
        }
    }
}

// ─── Ignorable items ──────────────────────────────────────────────────────────

fn is_ignorable(item: &str) -> bool {
    item.contains('は')
        && (item.contains("整数") || item.contains("正整数") || item.contains("入力は全て"))
}

#[cfg(test)]
#[path = "constraint_parse_tests.rs"]
mod tests;
