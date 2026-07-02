//! Value generation and output emission for a walked format block.

use super::budget::{Budget, GenError};
use super::context::{ArrayCtx, Ctx};
use super::gen::{effective_lo_hi, gen_string, strategy_size_value, StructuralSizes};
use super::relation::{
    bounded_distinct_int, effective_array_strategy, gen_int_array_with_positional_bounds,
    gen_positionally_bounded_int, has_array_element_constraints, narrow_bounds_from_scalars,
    narrow_scalar_bounds, not_equal_forbidden_scalar, record_array_values,
};
use super::spec::{ResolvedSpec, SizeTerm, VarInfo};
use super::strategy::{CaseStrategy, RandomStrategy};
use crate::parse::{ArrayBlock, BoundRepr, RowsBlock, VarType};
use rand::Rng;
use std::collections::HashSet;

pub(super) struct RenderEnv<'a> {
    pub(super) spec: &'a ResolvedSpec,
    pub(super) st: &'a CaseStrategy,
    pub(super) sizes: &'a StructuralSizes,
}

// ─── value helpers ────────────────────────────────────────────────────────────

// Scalars reuse the positional integer picker so enum domains, ordering,
// not_equal, and fallback strategy behaviour stay identical to constrained
// array elements. A scalar is the only element in that synthetic sequence.
const SCALAR_POSITION: usize = 0;
const SCALAR_SEQUENCE_LEN: usize = 1;

#[allow(clippy::too_many_arguments)]
pub(super) fn constrained_scalar_value(
    spec: &ResolvedSpec,
    name: &str,
    info: &VarInfo,
    sizes: &StructuralSizes,
    st: &CaseStrategy,
    ctx: &Ctx,
    array_ctx: &ArrayCtx,
    rng: &mut impl Rng,
) -> Option<i64> {
    let (lo, hi) = effective_lo_hi(name, info, sizes, spec);
    let (lo, hi) = narrow_scalar_bounds(name, lo, hi, spec, ctx, array_ctx)?;
    let forbidden = not_equal_forbidden_scalar(name, spec, ctx, array_ctx);
    let used = HashSet::new();
    let is_size = spec.size_vars.contains(name);
    let candidate = match st {
        CaseStrategy::Random(RandomStrategy::SmallSize(k)) if is_size => Some((*k).max(lo).min(hi)),
        CaseStrategy::Random(RandomStrategy::MaxSize) if is_size => Some(hi),
        _ => None,
    };
    if let Some(x) = candidate {
        if !forbidden.contains(&x) {
            return Some(x);
        }
        return bounded_distinct_int(st, lo, hi, &used, &forbidden, rng);
    }
    gen_positionally_bounded_int(
        st,
        SCALAR_POSITION,
        SCALAR_SEQUENCE_LEN,
        lo,
        hi,
        info.values.as_deref(),
        false,
        &used,
        &forbidden,
        rng,
    )
}

/// One size scalar, decided without prior context (fresh scope). `None` is a
/// constraint miss: the caller resamples the case, never invents a value.
fn size_value(
    spec: &ResolvedSpec,
    name: &str,
    sizes: &StructuralSizes,
    st: &CaseStrategy,
    rng: &mut impl Rng,
) -> Option<i64> {
    let info = spec
        .vars
        .get(name)
        .expect("size expressions are validated at resolve time");
    constrained_scalar_value(
        spec,
        name,
        info,
        sizes,
        st,
        &Ctx::new(),
        &ArrayCtx::new(),
        rng,
    )
}

/// Resolve a count / size variable: reuse the context value if present
/// (seeded structural size or earlier scalar), else decide it now and cache it
/// so a later reference stays consistent. `None` is a constraint miss.
#[allow(clippy::too_many_arguments)]
pub(super) fn resolve_count(
    name: &str,
    spec: &ResolvedSpec,
    sizes: &StructuralSizes,
    st: &CaseStrategy,
    ctx: &mut Ctx,
    rng: &mut impl Rng,
) -> Option<i64> {
    if let Some(&v) = ctx.get(name) {
        return Some(v);
    }
    // Size fields were parsed once while resolving the persisted spec.
    let v = match spec.size_terms.get(name) {
        Some(SizeTerm::Lit(n)) => *n,
        Some(SizeTerm::Var(vn)) => size_value(spec, vn, sizes, st, rng)?,
        Some(SizeTerm::VarOffset(vn, off)) => {
            resolve_count(vn, spec, sizes, st, ctx, rng)? + *off
        }
        None => size_value(spec, name, sizes, st, rng)?,
    };
    ctx.insert(name.to_string(), v);
    Some(v)
}

/// Resolve the length of one emitted Chars token (`vars[s].len`).
///
/// Pipe-wrapped names are parser-created length domains, not input size
/// variables. Each emitted string samples that domain independently; regular
/// expressions keep using the cached structural value shared by the input.
#[allow(clippy::too_many_arguments)]
fn resolve_len(
    repr: &Option<BoundRepr>,
    spec: &ResolvedSpec,
    sizes: &StructuralSizes,
    st: &CaseStrategy,
    ctx: &mut Ctx,
    rng: &mut impl Rng,
) -> Option<usize> {
    let len = match repr
        .as_ref()
        .expect("Chars `len` is validated at resolve time")
    {
        BoundRepr::Lit(n) => *n,
        BoundRepr::Expr(name) if is_synthetic_chars_len(name) => {
            size_value(spec, name, sizes, st, rng)?
        }
        BoundRepr::Expr(expr) => match spec
            .size_terms
            .get(expr)
            .expect("size expressions are validated at resolve time")
        {
            SizeTerm::Lit(n) => *n,
            SizeTerm::Var(name) => resolve_count(name, spec, sizes, st, ctx, rng)?,
            SizeTerm::VarOffset(name, off) => {
                resolve_count(name, spec, sizes, st, ctx, rng)? + *off
            }
        },
    };
    Some(len.max(0) as usize)
}

fn is_synthetic_chars_len(name: &str) -> bool {
    name.len() >= 2 && name.starts_with('|') && name.ends_with('|')
}

pub(super) fn gen_chars(
    info: &VarInfo,
    env: &RenderEnv<'_>,
    ctx: &mut Ctx,
    budget: &mut Budget,
    rng: &mut impl Rng,
) -> Result<Option<String>, GenError> {
    let Some(len) = resolve_len(&info.len, env.spec, env.sizes, env.st, ctx, rng) else {
        return Ok(None);
    };
    budget.add(len as u128)?;
    let cs = info
        .charset
        .as_deref()
        .expect("Chars charset is validated at resolve time");
    Ok(Some(gen_string(env.st, cs, len, None, rng)))
}

fn join_ints(v: &[i64]) -> String {
    let mut s = String::new();
    for (i, x) in v.iter().enumerate() {
        if i > 0 {
            s.push(' ');
        }
        s.push_str(&x.to_string());
    }
    s
}

fn is_altmaxmin(st: &CaseStrategy) -> bool {
    matches!(st, CaseStrategy::Random(RandomStrategy::ArrayAltMaxMin))
}

/// Checkerboard endpoints: an enum domain overrides the plain range bounds.
fn checkerboard_bounds(values: Option<&[i64]>, lo: i64, hi: i64) -> (i64, i64) {
    values
        .filter(|vs| !vs.is_empty())
        .map(|vs| (*vs.iter().min().unwrap(), *vs.iter().max().unwrap()))
        .unwrap_or((lo, hi))
}

/// One checkerboard cell: the parity of the combined grid index (plus a random
/// phase) picks the upper or lower endpoint.
fn checkerboard_cell(lo: i64, hi: i64, idx: usize, phase: usize) -> i64 {
    if (idx + phase) % 2 == 0 {
        hi
    } else {
        lo
    }
}

// ─── array renderers ──────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
pub(super) fn render_jagged(
    a: &ArrayBlock,
    spec: &ResolvedSpec,
    st: &CaseStrategy,
    sizes: &StructuralSizes,
    ctx: &mut Ctx,
    array_ctx: &mut ArrayCtx,
    lines: &mut Vec<String>,
    budget: &mut Budget,
    rng: &mut impl Rng,
) -> Result<bool, GenError> {
    let n = match &a.count {
        // The parser only marks a block jagged when it carries a count.
        Some(c) => match resolve_count(c, spec, sizes, st, ctx, rng) {
            Some(v) => v,
            None => return Ok(false),
        },
        None => 0,
    }
    .max(0);
    let len_var = a
        .len
        .as_ref()
        .expect("the parser only marks a block jagged when it carries a len");
    let info = spec
        .vars
        .get(&a.base)
        .expect("format variables are validated at resolve time");
    let (elo, ehi) = effective_lo_hi(&a.base, info, sizes, spec);
    let Some((elo, ehi)) = narrow_bounds_from_scalars(&a.base, elo, ehi, spec, ctx) else {
        return Ok(false);
    };
    let values = info.values.as_deref();
    let distinct = info.all_distinct;
    // Each row samples its length independently — never through `resolve_count`,
    // whose ctx cache would freeze every row to one value.
    let (len_vn, len_off) = match spec
        .size_terms
        .get(len_var)
        .expect("size expressions are validated at resolve time")
    {
        SizeTerm::Lit(v) => (None, *v),
        SizeTerm::Var(vn) => (Some(vn), 0),
        SizeTerm::VarOffset(vn, off) => (Some(vn), *off),
    };
    let len_bounds = len_vn.map(|vn| {
        let info = spec
            .vars
            .get(vn)
            .expect("size expressions are validated at resolve time");
        effective_lo_hi(vn, info, sizes, spec)
    });
    for _ in 0..n {
        let li = match len_bounds {
            Some((llo, lhi)) => strategy_size_value(st, llo, lhi, rng) + len_off,
            None => len_off,
        }
        .max(0);
        budget.add((li as u128).checked_add(1).ok_or_else(|| {
            GenError::Oversize(
                "input too large: generated jagged row element count overflows 128-bit range"
                    .to_owned(),
            )
        })?)?;
        let start = array_ctx.get(&a.base).map_or(0, Vec::len);
        let elems = match gen_int_array_with_positional_bounds(
            st,
            &a.base,
            elo,
            ehi,
            li as usize,
            values,
            distinct,
            spec,
            ctx,
            array_ctx,
            start,
            rng,
        ) {
            Some(e) => e,
            None => return Ok(false),
        };
        let mut line = li.to_string();
        record_array_values(array_ctx, &a.base, &elems);
        for e in &elems {
            line.push(' ');
            line.push_str(&e.to_string());
        }
        lines.push(line);
    }
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_chars_array(
    a: &ArrayBlock,
    spec: &ResolvedSpec,
    st: &CaseStrategy,
    sizes: &StructuralSizes,
    ctx: &mut Ctx,
    lines: &mut Vec<String>,
    budget: &mut Budget,
    rng: &mut impl Rng,
) -> Result<bool, GenError> {
    let count = match &a.count {
        Some(c) => match resolve_count(c, spec, sizes, st, ctx, rng) {
            Some(v) => v.max(0) as usize,
            None => return Ok(false),
        },
        None => 1,
    };
    let height = match &a.height {
        Some(h) => match resolve_count(h, spec, sizes, st, ctx, rng) {
            Some(v) => v.max(1) as usize,
            None => return Ok(false),
        },
        None => 1,
    };
    let total = count.saturating_mul(height);
    let info = spec
        .vars
        .get(&a.base)
        .expect("format variables are validated at resolve time");
    let charset = info
        .charset
        .as_deref()
        .expect("Chars charset is validated at resolve time");
    let phase = if is_altmaxmin(st) {
        rng.gen_range(0..2usize)
    } else {
        0
    };
    for idx in 0..total {
        let Some(slen) = resolve_len(&info.len, spec, sizes, st, ctx, rng) else {
            return Ok(false);
        };
        budget.add(slen as u128)?;
        lines.push(gen_string(st, charset, slen, Some((idx, total, phase)), rng));
    }
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_int_array(
    a: &ArrayBlock,
    spec: &ResolvedSpec,
    st: &CaseStrategy,
    sizes: &StructuralSizes,
    ctx: &mut Ctx,
    array_ctx: &mut ArrayCtx,
    lines: &mut Vec<String>,
    budget: &mut Budget,
    rng: &mut impl Rng,
) -> Result<bool, GenError> {
    let info = spec
        .vars
        .get(&a.base)
        .expect("format variables are validated at resolve time");
    let (lo, hi) = effective_lo_hi(&a.base, info, sizes, spec);
    let Some((lo, hi)) = narrow_bounds_from_scalars(&a.base, lo, hi, spec, ctx) else {
        return Ok(false);
    };
    let values = info.values.as_deref();
    let distinct = info.all_distinct;
    let len = match &a.len {
        Some(l) => match resolve_count(l, spec, sizes, st, ctx, rng) {
            Some(v) => v.max(0) as usize,
            None => return Ok(false),
        },
        None => 0,
    };

    let count = match &a.count {
        Some(c) => match resolve_count(c, spec, sizes, st, ctx, rng) {
            Some(v) => Some(v.max(0) as usize),
            None => return Ok(false),
        },
        None => None,
    };
    let height = match &a.height {
        Some(h) => match resolve_count(h, spec, sizes, st, ctx, rng) {
            Some(v) => Some(v.max(0) as usize),
            None => return Ok(false),
        },
        None => None,
    };

    let rows = match (count, height) {
        (None, None) => {
            budget.add(len as u128)?;
            let start = array_ctx.get(&a.base).map_or(0, Vec::len);
            match gen_int_array_with_positional_bounds(
                st, &a.base, lo, hi, len, values, distinct, spec, ctx, array_ctx, start, rng,
            ) {
                Some(e) => {
                    record_array_values(array_ctx, &a.base, &e);
                    lines.push(join_ints(&e));
                }
                None => return Ok(false),
            }
            return Ok(true);
        }
        (Some(c), None) => c,
        (None, Some(h)) => h,
        (Some(c), Some(h)) => c.saturating_mul(h),
    };
    budget.add((rows as u128).checked_mul(len as u128).ok_or_else(|| {
        GenError::Oversize(
            "input too large: generated array element count overflows 128-bit range".to_owned(),
        )
    })?)?;

    let effective = effective_array_strategy(st, &a.base, distinct, spec);
    if is_altmaxmin(&effective)
        && !has_array_element_constraints(
            &a.base,
            0,
            rows.saturating_mul(len),
            spec,
            ctx,
            array_ctx,
        )
    {
        let phase = rng.gen_range(0..2usize);
        let (lo, hi) = checkerboard_bounds(values, lo, hi);
        for r in 0..rows {
            let row: Vec<i64> = (0..len)
                .map(|c| checkerboard_cell(lo, hi, r + c, phase))
                .collect();
            record_array_values(array_ctx, &a.base, &row);
            lines.push(join_ints(&row));
        }
    } else {
        for _ in 0..rows {
            let start = array_ctx.get(&a.base).map_or(0, Vec::len);
            match gen_int_array_with_positional_bounds(
                st, &a.base, lo, hi, len, values, distinct, spec, ctx, array_ctx, start, rng,
            ) {
                Some(e) => {
                    record_array_values(array_ctx, &a.base, &e);
                    lines.push(join_ints(&e));
                }
                None => return Ok(false),
            }
        }
    }
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_rows(
    b: &RowsBlock,
    spec: &ResolvedSpec,
    st: &CaseStrategy,
    sizes: &StructuralSizes,
    ctx: &mut Ctx,
    array_ctx: &mut ArrayCtx,
    lines: &mut Vec<String>,
    budget: &mut Budget,
    rng: &mut impl Rng,
) -> Result<bool, GenError> {
    let rows = match resolve_count(&b.len, spec, sizes, st, ctx, rng) {
        Some(v) => v.max(0) as usize,
        None => return Ok(false),
    };
    if rows == 0 {
        return Ok(true);
    }
    let altmm = is_altmaxmin(st);
    let phase = if altmm { rng.gen_range(0..2usize) } else { 0 };

    let mut cols: Vec<Vec<String>> = Vec::with_capacity(b.vars.len());
    for v in &b.vars {
        let info = spec
            .vars
            .get(v)
            .expect("format variables are validated at resolve time");
        match info {
            info if info.ty == VarType::Chars => {
                let charset = info
                    .charset
                    .as_deref()
                    .expect("Chars charset is validated at resolve time");
                let lenrepr = info.len.clone();
                let mut col = Vec::with_capacity(rows);
                for i in 0..rows {
                    let Some(slen) = resolve_len(&lenrepr, spec, sizes, st, ctx, rng) else {
                        return Ok(false);
                    };
                    budget.add(slen as u128)?;
                    col.push(gen_string(st, charset, slen, Some((i, rows, phase)), rng));
                }
                cols.push(col);
            }
            info => {
                let (lo, hi) = effective_lo_hi(v, info, sizes, spec);
                let Some((lo, hi)) = narrow_bounds_from_scalars(v, lo, hi, spec, ctx) else {
                    return Ok(false);
                };
                budget.add(rows as u128)?;
                let start = array_ctx.get(v).map_or(0, Vec::len);
                let effective = effective_array_strategy(st, v, info.all_distinct, spec);
                let col: Vec<i64> = if is_altmaxmin(&effective)
                    && !has_array_element_constraints(v, start, rows, spec, ctx, array_ctx)
                {
                    let (lo, hi) = checkerboard_bounds(info.values.as_deref(), lo, hi);
                    (0..rows)
                        .map(|i| checkerboard_cell(lo, hi, i, phase))
                        .collect()
                } else {
                    match gen_int_array_with_positional_bounds(
                        st,
                        v,
                        lo,
                        hi,
                        rows,
                        info.values.as_deref(),
                        info.all_distinct,
                        spec,
                        ctx,
                        array_ctx,
                        start,
                        rng,
                    ) {
                        Some(e) => e,
                        None => return Ok(false),
                    }
                };
                record_array_values(array_ctx, v, &col);
                cols.push(col.iter().map(|x| x.to_string()).collect());
            }
        }
    }
    for i in 0..rows {
        let line: Vec<&str> = cols.iter().map(|c| c[i].as_str()).collect();
        lines.push(line.join(" "));
    }
    Ok(true)
}
