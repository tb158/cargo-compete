use super::{
    extract_ascii_inequality_fragments, extract_var_names, parse_constraints, resolve_lit,
    BoundExpr, NumBound, Side,
};

fn b_lit(n: i64) -> BoundExpr {
    BoundExpr::Lit(n)
}

fn b_var(name: &str, offset: i64) -> BoundExpr {
    BoundExpr::Var {
        name: name.to_string(),
        offset,
    }
}

#[test]
fn simple_range() {
    let p = parse_constraints(&["1 \\leq N \\leq 100".to_string()]);
    let n = p.num_vars.get("n").expect("n");
    assert_eq!(n.lo, Some(b_lit(1)));
    assert_eq!(n.hi, Some(b_lit(100)));
}

#[test]
fn multi_var_range() {
    let p = parse_constraints(&["1 \\leq A,B \\leq N".to_string()]);
    let a = p.num_vars.get("a").expect("a");
    assert_eq!(a.lo, Some(b_lit(1)));
    assert_eq!(a.hi, Some(b_var("n", 0)));
    let b = p.num_vars.get("b").expect("b");
    assert_eq!(b.lo, Some(b_lit(1)));
    assert_eq!(b.hi, Some(b_var("n", 0)));
}

#[test]
fn chain_inequality() {
    let p = parse_constraints(&["1 \\leq M \\leq N \\leq 10".to_string()]);
    assert_eq!(p.num_vars.get("m").unwrap().lo, Some(b_lit(1)));
    assert_eq!(p.num_vars.get("n").unwrap().hi, Some(b_lit(10)));
    assert!(p.var_le.contains(&("m".into(), "n".into())));
}

#[test]
fn inequality_ignores_html_tags() {
    let p = parse_constraints(&["<var>1 \\leq H,W\\leq 1000</var>".to_string()]);
    assert_eq!(p.num_vars["h"].lo, Some(b_lit(1)));
    assert_eq!(p.num_vars["h"].hi, Some(b_lit(1000)));
    assert_eq!(p.num_vars["w"].lo, Some(b_lit(1)));
    assert_eq!(p.num_vars["w"].hi, Some(b_lit(1000)));
    assert!(!p.num_vars.contains_key("var"));
}

#[test]
fn code_tags_do_not_create_inequalities() {
    let p = parse_constraints(&[
        "<var>S_{i,j}</var> は <code>#</code>, <code>.</code>, <code>o</code>, <code>x</code>, <code>S</code>, <code>G</code> のいずれかである。".to_string(),
        "<var>S_{i,j}=</var><code>S</code>, <code>G</code> となる <var>(i,j)</var> がちょうど一つずつ存在する。".to_string(),
    ]);
    assert!(p.str_vars.contains_key("s"));
    assert!(
        p.num_vars.is_empty(),
        "unexpected numeric vars: {:?}",
        p.num_vars
    );
    assert!(p.var_le.is_empty(), "unexpected ordering: {:?}", p.var_le);
    assert_eq!(p.skipped.len(), 1);
    assert!(p.skipped[0].contains("ちょうど"));
}

#[test]
fn offset_bound() {
    let p = parse_constraints(&["1 \\leq K \\leq N-1".to_string()]);
    let k = p.num_vars.get("k").expect("k");
    assert_eq!(k.lo, Some(b_lit(1)));
    assert_eq!(k.hi, Some(b_var("n", -1)));
}

#[test]
fn negative_lower_bound() {
    let p = parse_constraints(&["-10^9 \\leq X \\leq 10^9".to_string()]);
    let x = p.num_vars.get("x").expect("x");
    assert_eq!(x.lo, Some(b_lit(-1_000_000_000)));
    assert_eq!(x.hi, Some(b_lit(1_000_000_000)));
}

#[test]
fn not_equal_pair_does_not_create_numeric_vars() {
    let p = parse_constraints(&["A \\neq B".to_string()]);
    assert!(p.var_ne.contains(&("a".into(), "b".into())));
    assert!(!p.num_vars.contains_key("a"));
    assert!(!p.num_vars.contains_key("b"));
}

#[test]
fn not_equal_subscripted_can_receive_later_ranges() {
    // Element-wise inequality remains a candidate; later range constraints own vars.
    let p = parse_constraints(&[
        "A_j \\neq B_j".to_string(),
        "0 \\leq A_j, B_j \\leq 10".to_string(),
    ]);
    assert!(p.var_ne.contains(&("a".into(), "b".into())));
    assert!(p.num_vars.contains_key("a"));
    assert!(p.num_vars.contains_key("b"));
}

#[test]
fn not_equal_tuple_skipped() {
    // Tuple inequality cannot be expressed as a base-pair → goes to skipped.
    let p = parse_constraints(&["(X_i,Y_i) \\neq (0,0)".to_string()]);
    assert!(p.var_ne.is_empty());
    assert_eq!(p.skipped.len(), 1);
}

#[test]
fn enum_alternative() {
    let p = parse_constraints(&["B は 1, 2 のいずれか".to_string()]);
    assert_eq!(p.enum_values.get("b"), Some(&vec![1, 2]));
    // enum vars do not carry numeric bounds.
    assert!(p.num_vars.get("b").is_none());
}

#[test]
fn enum_alternative_applies_to_all_extracted_variables() {
    let p = parse_constraints(&["B, C は 1, 2 のいずれか".to_string()]);
    assert_eq!(p.enum_values.get("b"), Some(&vec![1, 2]));
    assert_eq!(p.enum_values.get("c"), Some(&vec![1, 2]));
}

#[test]
fn enum_in_set() {
    let p = parse_constraints(&["X \\in \\{1, 2, 3\\}".to_string()]);
    assert_eq!(p.enum_values.get("x"), Some(&vec![1, 2, 3]));
    assert!(p.num_vars.get("x").is_none());
}

#[test]
fn enum_in_set_applies_to_all_extracted_variables() {
    let p = parse_constraints(&["X,Y \\in \\{1, 2, 3\\}".to_string()]);
    assert_eq!(p.enum_values.get("x"), Some(&vec![1, 2, 3]));
    assert_eq!(p.enum_values.get("y"), Some(&vec![1, 2, 3]));
}

#[test]
fn enum_blocks_subsequent_inequality() {
    // An inequality that mentions an already-enum variable must not register a range.
    let p = parse_constraints(&[
        "B は 1, 2 のいずれか".to_string(),
        "1 \\leq B \\leq 2".to_string(),
    ]);
    assert!(p.enum_values.contains_key("b"));
    assert!(p.num_vars.get("b").is_none());
}

#[test]
fn all_distinct() {
    let p = parse_constraints(&["A_1, A_2, ..., A_N はすべて相異なる".to_string()]);
    assert!(p.all_distinct.contains("a"));
}

#[test]
fn all_distinct_applies_to_all_extracted_variables() {
    let p = parse_constraints(&["A_i, B_i はすべて相異なる".to_string()]);
    assert!(p.all_distinct.contains("a"));
    assert!(p.all_distinct.contains("b"));
}

#[test]
fn extract_var_names_shapes() {
    assert_eq!(extract_var_names("N"), vec!["n"]);
    assert_eq!(extract_var_names("A_i"), vec!["a"]);
    assert_eq!(extract_var_names("A_{i,j}"), vec!["a"]);
    assert_eq!(extract_var_names("A_i, B_i"), vec!["a", "b"]);
    assert_eq!(extract_var_names("A_i、B_i"), vec!["a", "b"]);
    assert_eq!(extract_var_names("|A|"), vec!["|a|"]);
    assert_eq!(extract_var_names("|A| + |B|"), vec!["|a|", "|b|"]);
    assert_eq!(extract_var_names("A, |A|"), vec!["a", "|a|"]);
    assert_eq!(extract_var_names("|A_i|"), vec!["|a|"]);
    assert_eq!(extract_var_names("min(A,B)"), vec!["a", "b"]);
    assert_eq!(extract_var_names("max(N,M)"), vec!["n", "m"]);
    assert_eq!(
        extract_var_names("1 つの入力に含まれるテストケースについて、N"),
        vec!["n"]
    );
}

#[test]
fn extract_var_names_ignores_min_max_keywords() {
    assert!(extract_var_names("min").is_empty());
    assert!(extract_var_names("max").is_empty());
}

#[test]
fn min_max_operands_keep_cartesian_ordering_semantics() {
    let p = parse_constraints(&["X \\leq \\max(A, B)".to_string()]);
    assert!(p.var_le.contains(&("x".into(), "a".into())));
    assert!(p.var_le.contains(&("x".into(), "b".into())));
    assert!(!p.num_vars.contains_key("max"));
}

#[test]
fn extract_var_names_ignores_arithmetic_expressions() {
    assert!(extract_var_names("N-1").is_empty());
    assert!(extract_var_names("N^2").is_empty());
    assert!(extract_var_names("2*N").is_empty());
    assert!(extract_var_names("\\dots").is_empty());
}

#[test]
fn sum_limit() {
    let p = parse_constraints(&["N の総和は 200000 以下".to_string()]);
    assert_eq!(p.sum_limits.get("n"), Some(&200_000));
}

#[test]
fn sum_limit_with_japanese_prose_prefix() {
    let p = parse_constraints(&[
        "1 つの入力に含まれるテストケースについて、N の総和は 2 \\times 10^5 以下".to_string(),
        "1 つの入力に含まれるテストケースについて、M の総和は 2 \\times 10^5 以下".to_string(),
    ]);
    assert_eq!(p.sum_limits.get("n"), Some(&200_000));
    assert_eq!(p.sum_limits.get("m"), Some(&200_000));
    assert!(p.skipped.is_empty());
}

#[test]
fn sum_limit_applies_to_all_extracted_variables() {
    let p = parse_constraints(&["N, M の総和は 2 \\times 10^5 以下".to_string()]);
    assert_eq!(p.sum_limits.get("n"), Some(&200_000));
    assert_eq!(p.sum_limits.get("m"), Some(&200_000));
}

#[test]
fn sum_limit_of_string_length_targets_synthetic_length_var() {
    let p =
        parse_constraints(&["全てのテストケースにおける S の長さの総和は 10^6 以下".to_string()]);
    assert_eq!(p.sum_limits.get("|s|"), Some(&1_000_000));
    assert!(!p.sum_limits.contains_key("s"));
}

#[test]
fn sum_limit_keeps_pipe_wrapped_length_variables_distinct() {
    let p = parse_constraints(&["|A| + |B| の総和は 2000000 以下".to_string()]);
    assert_eq!(p.sum_limits.get("|a|"), Some(&2_000_000));
    assert_eq!(p.sum_limits.get("|b|"), Some(&2_000_000));
    assert!(!p.sum_limits.contains_key("a"));
    assert!(!p.sum_limits.contains_key("b"));
}

#[test]
fn sum_limit_ignores_exponent_target() {
    let p = parse_constraints(&["N^2 の総和は 2 \\times 10^5 以下".to_string()]);
    assert!(p.sum_limits.is_empty());
}

#[test]
fn aggregate_rhs_is_skipped_without_fake_ordering() {
    let p = parse_constraints(&[r"\displaystyle 1\le K\le \sum_{i=1}^N C_iL_i".to_string()]);
    assert!(p.var_le.is_empty(), "unexpected ordering: {:?}", p.var_le);
    assert!(!p.num_vars.contains_key("i"));
    assert!(p.skipped.iter().any(|item| item.contains("\\sum")));
}

#[test]
fn string_with_lower_alpha() {
    let p = parse_constraints(&["S は英小文字からなる長さ N の文字列".to_string()]);
    let s = p.str_vars.get("s").expect("s");
    assert_eq!(s.charset, ('a'..='z').collect::<Vec<_>>());
    assert_eq!(s.len_lo, Some(b_var("n", 0)));
    assert_eq!(s.len_hi, Some(b_var("n", 0)));
}

#[test]
fn string_decl_applies_to_all_extracted_variables() {
    let p = parse_constraints(&["S, T は英小文字からなる長さ N の文字列".to_string()]);
    assert!(p.str_vars.contains_key("s"));
    assert!(p.str_vars.contains_key("t"));
}

#[test]
fn abs_length_update_applies_to_all_extracted_variables() {
    let p = parse_constraints(&[
        "A, B は英小文字からなる文字列".to_string(),
        r"1 \leq |A|, |B| \leq N".to_string(),
    ]);
    for name in ["a", "b"] {
        let spec = p.str_vars.get(name).expect(name);
        assert_eq!(spec.len_lo, Some(b_lit(1)), "{name} lower bound");
        assert_eq!(spec.len_hi, Some(b_var("n", 0)), "{name} upper bound");
    }
}

#[test]
fn abs_length_update_supports_multi_character_variable_name() {
    let p = parse_constraints(&[
        "STR は英小文字からなる文字列".to_string(),
        r"1 \leq |STR| \leq N".to_string(),
    ]);
    let spec = p.str_vars.get("str").expect("str");
    assert_eq!(spec.len_lo, Some(b_lit(1)));
    assert_eq!(spec.len_hi, Some(b_var("n", 0)));
}

#[test]
fn string_length_range() {
    let p = parse_constraints(&["S は英小文字からなる長さ 1 以上 100 以下の文字列".to_string()]);
    let s = p.str_vars.get("s").expect("s");
    assert_eq!(s.len_lo, Some(b_lit(1)));
    assert_eq!(s.len_hi, Some(b_lit(100)));
}

#[test]
fn abs_length_update() {
    // S declared as a string in pass 1, then `1 <= |S| <= N` updates the length.
    let p = parse_constraints(&[
        "S は英小文字からなる文字列".to_string(),
        "1 \\leq |S| \\leq N".to_string(),
    ]);
    let s = p.str_vars.get("s").expect("s");
    assert_eq!(s.len_lo, Some(b_lit(1)));
    assert_eq!(s.len_hi, Some(b_var("n", 0)));
}

#[test]
fn explicit_charset() {
    let p =
        parse_constraints(&["G は <code>#</code> か <code>.</code> からなる文字列".to_string()]);
    let g = p.str_vars.get("g").expect("g");
    assert_eq!(g.charset, vec!['#', '.']);
}

#[test]
fn ignorable_item() {
    let p = parse_constraints(&["入力は全て整数".to_string()]);
    assert!(p.skipped.is_empty());
}

#[test]
fn unparseable_goes_to_skipped() {
    let p = parse_constraints(&["何か特殊な \\dfrac{1}{2} 制約".to_string()]);
    assert_eq!(p.skipped.len(), 1);
}

#[test]
fn resolve_lit_literal() {
    let ctx = std::collections::HashMap::new();
    assert_eq!(resolve_lit(&b_lit(5), &ctx, Side::Hi), Some(5));
}

#[test]
fn resolve_lit_var_with_offset() {
    let mut ctx = std::collections::HashMap::new();
    ctx.insert(
        "n".to_string(),
        NumBound {
            lo: Some(b_lit(1)),
            hi: Some(b_lit(100_000)),
        },
    );
    assert_eq!(resolve_lit(&b_var("n", 0), &ctx, Side::Hi), Some(100_000));
    assert_eq!(resolve_lit(&b_var("n", -1), &ctx, Side::Hi), Some(99_999));
    // Lo-side resolution returns n's lo
    assert_eq!(resolve_lit(&b_var("n", 0), &ctx, Side::Lo), Some(1));
}

#[test]
fn extract_ascii_inequality_fragments_japanese_prefix() {
    // After normalize, `\leq` → `<=` and spaces removed.
    let frags = extract_ascii_inequality_fragments("1種類目のクエリについて、1<=x<=N-1");
    assert_eq!(frags.len(), 1);
    assert_eq!(frags[0], "1<=x<=N-1");
}

#[test]
fn extract_ascii_inequality_fragments_pure_ascii() {
    let frags = extract_ascii_inequality_fragments("1<=M<=N<=10");
    assert_eq!(frags.len(), 1);
    assert_eq!(frags[0], "1<=M<=N<=10");
}

#[test]
fn extract_ascii_inequality_fragments_no_op_returns_empty() {
    let frags = extract_ascii_inequality_fragments("入力は全て整数");
    assert!(frags.is_empty());
}

#[test]
fn try_inequality_chain_japanese_prefix() {
    let p = parse_constraints(&["1 種類目のクエリについて、1 \\leq x \\leq N-1".to_string()]);
    let x = p.num_vars.get("x").expect("x");
    assert_eq!(x.lo, Some(b_lit(1)));
    assert_eq!(x.hi, Some(b_var("n", -1)));
}

#[test]
fn string_decl_japanese_prose_prefix() {
    let p = parse_constraints(&[
        "全ての 2 種類目のクエリについて、s は英小文字からなる長さ 1 以上の文字列".to_string(),
    ]);
    let s = p.str_vars.get("s").expect("s should be detected as string");
    assert_eq!(s.charset, ('a'..='z').collect::<Vec<_>>());
    assert_eq!(s.len_lo, Some(b_lit(1)));
    assert_eq!(s.len_hi, None);
}

#[test]
fn enum_japanese_prose_prefix() {
    let p = parse_constraints(&["全ての i について、B は 1, 2 のいずれか".to_string()]);
    assert_eq!(p.enum_values.get("b"), Some(&vec![1, 2]));
}

#[test]
fn parse_length_spec_lo_only_via_constraint() {
    let p = parse_constraints(&["S は英小文字からなる長さ 1 以上の文字列".to_string()]);
    let s = p.str_vars.get("s").expect("s");
    assert_eq!(s.len_lo, Some(b_lit(1)));
    assert_eq!(s.len_hi, None);
}

#[test]
fn parse_length_spec_hi_only_via_constraint() {
    let p = parse_constraints(&["S は英小文字からなる長さ 100 以下の文字列".to_string()]);
    let s = p.str_vars.get("s").expect("s");
    assert_eq!(s.len_lo, None);
    assert_eq!(s.len_hi, Some(b_lit(100)));
}

#[test]
fn natural_form_both_sided() {
    let p = parse_constraints(&["N は 1 以上 11 以下の整数".to_string()]);
    let n = p.num_vars.get("n").expect("n");
    assert_eq!(n.lo, Some(b_lit(1)));
    assert_eq!(n.hi, Some(b_lit(11)));
}

#[test]
fn natural_form_both_sided_applies_to_all_extracted_variables() {
    let p = parse_constraints(&["H,W は 1 以上 500 以下の整数".to_string()]);
    for name in ["h", "w"] {
        let bound = p.num_vars.get(name).expect(name);
        assert_eq!(bound.lo, Some(b_lit(1)), "{name} lower bound");
        assert_eq!(bound.hi, Some(b_lit(500)), "{name} upper bound");
    }
}

#[test]
fn natural_form_lo_only() {
    let p = parse_constraints(&["N は 1 以上の整数".to_string()]);
    let n = p.num_vars.get("n").expect("n");
    assert_eq!(n.lo, Some(b_lit(1)));
    assert_eq!(n.hi, None);
}

#[test]
fn natural_form_lo_only_applies_to_all_extracted_variables() {
    let p = parse_constraints(&["H,W は 1 以上の整数".to_string()]);
    for name in ["h", "w"] {
        let bound = p.num_vars.get(name).expect(name);
        assert_eq!(bound.lo, Some(b_lit(1)), "{name} lower bound");
        assert_eq!(bound.hi, None, "{name} upper bound");
    }
}

#[test]
fn natural_form_hi_only() {
    let p = parse_constraints(&["N は 100 以下の整数".to_string()]);
    let n = p.num_vars.get("n").expect("n");
    assert_eq!(n.lo, None);
    assert_eq!(n.hi, Some(b_lit(100)));
}

#[test]
fn natural_form_hi_only_applies_to_all_extracted_variables() {
    let p = parse_constraints(&["H,W は 500 以下の整数".to_string()]);
    for name in ["h", "w"] {
        let bound = p.num_vars.get(name).expect(name);
        assert_eq!(bound.lo, None, "{name} lower bound");
        assert_eq!(bound.hi, Some(b_lit(500)), "{name} upper bound");
    }
}

#[test]
fn subscripted_var_in_bound_chain() {
    // `1 ≤ l_i ≤ u_i ≤ 10^9` → l.hi resolves transitively through u.hi.
    let p = parse_constraints(&["1 \\leq l_i \\leq u_i \\leq 10^9".to_string()]);
    let l = p.num_vars.get("l").expect("l");
    let u = p.num_vars.get("u").expect("u");
    assert_eq!(l.lo, Some(b_lit(1)));
    assert_eq!(l.hi, Some(b_var("u", 0)));
    assert_eq!(u.hi, Some(b_lit(1_000_000_000)));
}

#[test]
fn strict_lt_chain_resolves_endpoints() {
    // `1 \leq A_1 \lt A_2 \lt ... \lt A_M \leq N` — only the outermost `\leq`
    // pieces matter for A; the strict middle is ignored.
    let p = parse_constraints(&[
        "1 \\leq A_1 \\lt A_2 \\lt \\dots \\lt A_M \\leq N".to_string(),
        "1 \\leq N \\leq 10".to_string(),
    ]);
    let a = p.num_vars.get("a").expect("a");
    assert_eq!(a.lo, Some(b_lit(1)));
    // a.hi resolves via Var{n, 0}, n.hi=10 → 10
    assert!(a.hi.is_some());
}

#[test]
fn transitive_upper_bound_through_chain() {
    // chain `a <= b <= c <= d` plus a separate `d <= 100` should let
    // a, b, c all resolve their upper bound to 100 via recursion.
    let p = parse_constraints(&[
        "A \\leq B \\leq C \\leq D".to_string(),
        "D \\leq 100".to_string(),
    ]);
    let ctx = &p.num_vars;
    let a = p.num_vars.get("a").expect("a");
    let b = p.num_vars.get("b").expect("b");
    let c = p.num_vars.get("c").expect("c");
    let d = p.num_vars.get("d").expect("d");
    assert_eq!(
        resolve_lit(a.hi.as_ref().unwrap(), ctx, Side::Hi),
        Some(100)
    );
    assert_eq!(
        resolve_lit(b.hi.as_ref().unwrap(), ctx, Side::Hi),
        Some(100)
    );
    assert_eq!(
        resolve_lit(c.hi.as_ref().unwrap(), ctx, Side::Hi),
        Some(100)
    );
    assert_eq!(
        resolve_lit(d.hi.as_ref().unwrap(), ctx, Side::Hi),
        Some(100)
    );
}

#[test]
fn register_num_bounds_lit_kept_over_var() {
    // Item 1 sets n.lo = Lit(2). Item 2 has chain `1 ≤ l ≤ r ≤ n` which would
    // otherwise overwrite n.lo to Var{r, 0}. The Lit must win.
    let p = parse_constraints(&[
        "2 \\leq N \\leq 200000".to_string(),
        "1 \\leq l \\leq r \\leq N".to_string(),
    ]);
    let n = p.num_vars.get("n").expect("n");
    assert_eq!(n.lo, Some(b_lit(2)));
    assert_eq!(n.hi, Some(b_lit(200_000)));
}

#[test]
fn string_var_no_numeric_pollution() {
    // S declared as string. `S` should NOT have numeric bounds even if `S = N` appears.
    let p = parse_constraints(&[
        "S は英小文字からなる長さ N の文字列".to_string(),
        "1 \\leq N \\leq 10".to_string(),
    ]);
    assert!(p.num_vars.get("s").is_none());
    assert!(p.str_vars.contains_key("s"));
}

#[test]
fn length_spec_fuzzy_lo_with_particle() {
    // `長さが N 以上` — the particle `が` between 長さ and N must be skipped.
    let (lo, hi) = super::parse_length_spec("長さが N 以上");
    assert_eq!(lo, Some(b_var("n", 0)));
    assert_eq!(hi, None);
}

#[test]
fn length_spec_fuzzy_hi_with_prose() {
    // `S の長さは M 以下` — prose `は` after 長さ must be skipped.
    let (lo, hi) = super::parse_length_spec("S の長さは M 以下");
    assert_eq!(lo, None);
    assert_eq!(hi, Some(b_var("m", 0)));
}

#[test]
fn length_spec_fuzzy_range_with_comma() {
    // `長さは 1 以上、10^5 以下` — comma between bounds must be skipped.
    let (lo, hi) = super::parse_length_spec("長さは 1 以上、10^5 以下");
    assert_eq!(lo, Some(b_lit(1)));
    assert_eq!(hi, Some(b_lit(100_000)));
}

#[test]
fn natural_inequality_fuzzy_prose_prefix() {
    // `N の値は 1 以上 10^5 以下` — `の値は` instead of bare `は`.
    let p = parse_constraints(&["N の値は 1 以上 10^5 以下".to_string()]);
    let n = p.num_vars.get("n").expect("n");
    assert_eq!(n.lo, Some(b_lit(1)));
    assert_eq!(n.hi, Some(b_lit(100_000)));
}
