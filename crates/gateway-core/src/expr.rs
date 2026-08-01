//! Tier-1 extensibility: sandboxed CEL expressions (docs/02-architecture.md).
//!
//! Three uses, one wrapper: derived attribution values (`team = <jwt claim
//! transform>`), route-match conditions beyond the path prefix, and GB-8
//! label value expressions. Expressions COMPILE AT CONFIG LOAD — a typo'd
//! expression is a validation error at startup/reload, never a request-time
//! surprise — and evaluate per request against a small, documented context:
//!
//! - `request.method` — the HTTP method, uppercase (`"POST"`)
//! - `request.path`   — the normalized request path (`"/openai/v1/chat"`)
//! - `request.headers` — map of lowercase header name → first value
//! - `jwt.claims`     — claims of the VERIFIED token (empty map when no
//!   token verified; guard with `has(jwt.claims.x)` or `"x" in jwt.claims`)
//! - `attribution`    — map of resolved attribution key → value; only
//!   available to label expressions (attribution is resolved after route
//!   match and derivation, so conditions and derivations cannot see it)
//!
//! Besides the CEL standard library (`has`, `size`, `contains`,
//! `startsWith`, `endsWith`, `matches`, …) one custom helper is provided:
//! `"a-b".split("-")` → list of parts, for claim transforms.
//!
//! The interpreter (the `cel` crate, the maintained continuation of
//! `cel-interpreter`) is sandboxed by construction — no I/O, no syscalls,
//! pure functions over the context. On top of that, compile-time cost and
//! depth limits apply: source length ≤ [`MAX_SOURCE_LEN`], bracket nesting
//! ≤ [`MAX_NESTING`], and every referenced variable must be one this
//! module's context actually provides (an unknown variable fails the
//! CONFIG, not the request).
//!
//! RUNTIME cost is bounded structurally: comprehension macros (`map`,
//! `filter`, `all`, `exists`, `exists_one`) — CEL's only iteration
//! construct — are REJECTED at compile. A comprehension over
//! caller-controlled input (e.g. `.split("").map(…)` on a request header)
//! can be made superlinear and wedge a request-hot-path worker; without
//! comprehensions every AST node evaluates at most once and every stdlib
//! operation is linear in its operand, so evaluation stays microseconds
//! no matter what the caller sends. `has(…)` is NOT a comprehension (it
//! compiles to a presence-test field select) and stays available. Like
//! every other limit here, this fails the CONFIG, not the request.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use cel::common::ast::{EntryExpr, Expr, IdedExpr};
use cel::extractors::This;
use cel::{Context, Program, Value};

/// Cost limit: the longest expression source accepted at config load.
pub const MAX_SOURCE_LEN: usize = 512;

/// Depth limit: maximum `(`/`[`/`{` nesting accepted at config load
/// (string literals excluded from the scan).
pub const MAX_NESTING: usize = 16;

/// Which context variables an expression is allowed to reference —
/// conditions and derivations run before attribution is resolved, so only
/// label expressions may reference `attribution`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExprKind {
    /// Route-match condition: `request` + `jwt`. Must produce a bool.
    Condition,
    /// Derived attribution value: `request` + `jwt`. Must produce a scalar.
    Derived,
    /// GB-8 label value: `request` + `jwt` + `attribution`. Scalar.
    Label,
}

impl ExprKind {
    fn allowed_variables(self) -> &'static [&'static str] {
        match self {
            ExprKind::Condition | ExprKind::Derived => &["request", "jwt"],
            ExprKind::Label => &["request", "jwt", "attribution"],
        }
    }
}

/// A CEL expression compiled at config load. Immutable and request-safe:
/// evaluation takes the per-request context by reference and shares nothing.
pub struct CompiledExpr {
    source: String,
    kind: ExprKind,
    program: Program,
}

impl fmt::Debug for CompiledExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompiledExpr")
            .field("source", &self.source)
            .field("kind", &self.kind)
            .finish()
    }
}

impl CompiledExpr {
    /// Compile with the limits on: length, nesting depth, and the
    /// allowed-variable check. Every failure names the offending source.
    pub fn compile(source: &str, kind: ExprKind) -> Result<CompiledExpr, String> {
        if source.trim().is_empty() {
            return Err("empty CEL expression".to_string());
        }
        if source.len() > MAX_SOURCE_LEN {
            return Err(format!(
                "CEL expression exceeds {MAX_SOURCE_LEN} bytes ({} bytes): cost limit",
                source.len()
            ));
        }
        let depth = max_nesting(source);
        if depth > MAX_NESTING {
            return Err(format!(
                "CEL expression nests {depth} levels deep (limit {MAX_NESTING}): depth limit"
            ));
        }
        let program = Program::compile(source)
            .map_err(|e| format!("CEL parse error in {source:?}: {e}"))?;
        if contains_comprehension(program.expression()) {
            return Err(format!(
                "CEL expression {source:?} uses a comprehension macro \
                 (map/filter/all/exists/exists_one): comprehensions iterate and \
                 can be made superlinear in request-controlled input, so they \
                 are rejected at load — runtime cost limit"
            ));
        }
        let refs = program.references();
        let allowed = kind.allowed_variables();
        let mut unknown: Vec<&str> = refs
            .variables()
            .into_iter()
            .filter(|v| !allowed.contains(v))
            .collect();
        if !unknown.is_empty() {
            unknown.sort_unstable();
            return Err(format!(
                "CEL expression {source:?} references unknown variable(s) {}; \
                 this context provides: {}",
                unknown.join(", "),
                allowed.join(", ")
            ));
        }
        Ok(CompiledExpr {
            source: source.to_string(),
            kind,
            program,
        })
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    /// Evaluate to a raw CEL value against the documented context.
    fn eval(&self, ctx: &EvalCtx) -> Result<Value, String> {
        let mut cel_ctx = Context::default();
        // One custom helper beyond the CEL stdlib: `"a-b".split("-")` —
        // the canonical claim-transform derivation. Pure, sandboxed.
        cel_ctx.add_function(
            "split",
            |This(s): This<Arc<String>>, sep: Arc<String>| -> Result<Value, cel::ExecutionError> {
                let parts: Vec<Value> = s
                    .split(sep.as_str())
                    .map(|p| Value::String(Arc::new(p.to_string())))
                    .collect();
                Ok(Value::List(Arc::new(parts)))
            },
        );
        let request = serde_json::json!({
            "method": ctx.method,
            "path": ctx.path,
            "headers": ctx.headers,
        });
        cel_ctx
            .add_variable("request", request)
            .map_err(|e| format!("context build error: {e}"))?;
        let claims = ctx
            .claims
            .clone()
            .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
        cel_ctx
            .add_variable("jwt", serde_json::json!({ "claims": claims }))
            .map_err(|e| format!("context build error: {e}"))?;
        if self.kind == ExprKind::Label {
            cel_ctx
                .add_variable("attribution", serde_json::json!(ctx.attribution))
                .map_err(|e| format!("context build error: {e}"))?;
        }
        self.program
            .execute(&cel_ctx)
            .map_err(|e| format!("CEL eval error in {:?}: {e}", self.source))
    }

    /// Evaluate a condition: `true`/`false`, anything else is an error the
    /// caller treats as "does not match" (an erroring condition can never
    /// SELECT a route — fail-closed for routing).
    pub fn eval_bool(&self, ctx: &EvalCtx) -> Result<bool, String> {
        match self.eval(ctx)? {
            Value::Bool(b) => Ok(b),
            other => Err(format!(
                "CEL condition {:?} produced {:?}, expected bool",
                self.source,
                other.type_of()
            )),
        }
    }

    /// Evaluate to a string tag/label value. Scalars stringify (string,
    /// int, uint, float, bool); anything else — null, lists, maps — is an
    /// error so misattribution fails closed, never coerces.
    pub fn eval_string(&self, ctx: &EvalCtx) -> Result<String, String> {
        match self.eval(ctx)? {
            Value::String(s) => Ok(s.as_ref().clone()),
            Value::Int(i) => Ok(i.to_string()),
            Value::UInt(u) => Ok(u.to_string()),
            Value::Float(f) => Ok(f.to_string()),
            Value::Bool(b) => Ok(b.to_string()),
            other => Err(format!(
                "CEL expression {:?} produced {:?}, expected a scalar",
                self.source,
                other.type_of()
            )),
        }
    }
}

/// The per-request evaluation context — request metadata, verified claims,
/// and (for label expressions) the resolved attribution map. Small and
/// explicit: this struct IS the documented CEL API surface.
#[derive(Debug, Default, Clone)]
pub struct EvalCtx {
    pub method: String,
    pub path: String,
    /// Lowercase header name → first value.
    pub headers: BTreeMap<String, String>,
    /// Claims of the verified JWT (`None` → `jwt.claims` is `{}`).
    pub claims: Option<serde_json::Value>,
    /// Resolved attribution key → value; consulted by Label expressions.
    pub attribution: BTreeMap<String, String>,
}

/// Runtime cost limit's measurement: does the compiled AST contain a
/// comprehension node? The parser expands `map`/`filter`/`all`/`exists`/
/// `exists_one` macros into [`Expr::Comprehension`] (NOT function calls,
/// so `references().functions()` never sees them); `has(…)` expands to a
/// presence-test [`Expr::Select`] and passes. The match is exhaustive on
/// purpose — a `cel` upgrade that adds an AST variant must be re-audited
/// here before it compiles.
fn contains_comprehension(expr: &IdedExpr) -> bool {
    match &expr.expr {
        Expr::Comprehension(_) => true,
        Expr::Call(call) => {
            call.target.as_deref().is_some_and(contains_comprehension)
                || call.args.iter().any(contains_comprehension)
        }
        Expr::List(list) => list.elements.iter().any(contains_comprehension),
        Expr::Map(map) => map.entries.iter().any(|e| entry_contains_comprehension(&e.expr)),
        Expr::Struct(st) => st.entries.iter().any(|e| entry_contains_comprehension(&e.expr)),
        Expr::Select(sel) => contains_comprehension(&sel.operand),
        Expr::Ident(_) | Expr::Literal(_) | Expr::Unspecified => false,
    }
}

fn entry_contains_comprehension(entry: &EntryExpr) -> bool {
    match entry {
        EntryExpr::StructField(f) => contains_comprehension(&f.value),
        EntryExpr::MapEntry(m) => {
            contains_comprehension(&m.key) || contains_comprehension(&m.value)
        }
    }
}

/// Maximum bracket nesting outside string literals — the depth limit's
/// measurement. Quotes toggle a "in string" state so bracket characters in
/// literals don't count.
fn max_nesting(source: &str) -> usize {
    let mut depth: usize = 0;
    let mut max = 0;
    let mut in_string: Option<char> = None;
    let mut escaped = false;
    for c in source.chars() {
        if let Some(quote) = in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == quote {
                in_string = None;
            }
            continue;
        }
        match c {
            '"' | '\'' => in_string = Some(c),
            '(' | '[' | '{' => {
                depth += 1;
                max = max.max(depth);
            }
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    max
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> EvalCtx {
        EvalCtx {
            method: "POST".to_string(),
            path: "/openai/v1/chat".to_string(),
            headers: [("x-tenant".to_string(), "acme".to_string())].into(),
            claims: Some(serde_json::json!({"sub": "alice", "team_id": "ml-research-42"})),
            attribution: [("team".to_string(), "ml".to_string())].into(),
        }
    }

    #[test]
    fn condition_evaluates_request_meta() {
        let e = CompiledExpr::compile(r#"request.method == "POST""#, ExprKind::Condition).unwrap();
        assert!(e.eval_bool(&ctx()).unwrap());
        let e =
            CompiledExpr::compile(r#"request.headers["x-tenant"] == "acme""#, ExprKind::Condition)
                .unwrap();
        assert!(e.eval_bool(&ctx()).unwrap());
    }

    #[test]
    fn derived_transforms_jwt_claims() {
        // The doc's example: team from a claim transform.
        let e = CompiledExpr::compile(
            r#"jwt.claims.team_id.split("-")[0]"#,
            ExprKind::Derived,
        )
        .unwrap();
        assert_eq!(e.eval_string(&ctx()).unwrap(), "ml");
    }

    #[test]
    fn label_expressions_see_resolved_attribution() {
        let e = CompiledExpr::compile(r#"attribution["team"] + "-gw""#, ExprKind::Label).unwrap();
        assert_eq!(e.eval_string(&ctx()).unwrap(), "ml-gw");
    }

    #[test]
    fn condition_and_derived_cannot_reference_attribution() {
        let err =
            CompiledExpr::compile(r#"attribution["team"]"#, ExprKind::Condition).unwrap_err();
        assert!(err.contains("unknown variable"), "{err}");
        let err = CompiledExpr::compile(r#"attribution["team"]"#, ExprKind::Derived).unwrap_err();
        assert!(err.contains("unknown variable"), "{err}");
    }

    #[test]
    fn unknown_variable_fails_at_compile_not_at_eval() {
        let err = CompiledExpr::compile("budget.remaining > 0", ExprKind::Condition).unwrap_err();
        assert!(err.contains("budget"), "{err}");
        assert!(err.contains("this context provides"), "{err}");
    }

    #[test]
    fn parse_error_fails_fast_with_the_source_named() {
        let err = CompiledExpr::compile("request.method ==", ExprKind::Condition).unwrap_err();
        assert!(err.contains("parse error"), "{err}");
        assert!(err.contains("request.method =="), "{err}");
    }

    #[test]
    fn missing_jwt_claims_default_to_empty_map() {
        let mut c = ctx();
        c.claims = None;
        let e = CompiledExpr::compile(r#""sub" in jwt.claims"#, ExprKind::Condition).unwrap();
        assert!(!e.eval_bool(&c).unwrap());
        // An unguarded claim access errors — the caller fails closed.
        let e = CompiledExpr::compile("jwt.claims.sub", ExprKind::Derived).unwrap();
        assert!(e.eval_string(&c).is_err());
    }

    #[test]
    fn non_scalar_results_fail_closed() {
        let e = CompiledExpr::compile("request.headers", ExprKind::Derived).unwrap();
        let err = e.eval_string(&ctx()).unwrap_err();
        assert!(err.contains("expected a scalar"), "{err}");
        let e = CompiledExpr::compile(r#"request.path"#, ExprKind::Condition).unwrap();
        assert!(e.eval_bool(&ctx()).is_err());
    }

    #[test]
    fn length_and_depth_limits_reject_at_compile() {
        let long = format!(r#""{}""#, "x".repeat(MAX_SOURCE_LEN + 1));
        let err = CompiledExpr::compile(&long, ExprKind::Condition).unwrap_err();
        assert!(err.contains("cost limit"), "{err}");

        let deep = format!("{}1{}", "(".repeat(MAX_NESTING + 1), ")".repeat(MAX_NESTING + 1));
        let err = CompiledExpr::compile(&deep, ExprKind::Condition).unwrap_err();
        assert!(err.contains("depth limit"), "{err}");

        // Brackets inside string literals don't count toward depth.
        let bracket_string = format!(r#""{}" == "x""#, "(".repeat(MAX_NESTING + 5));
        assert!(CompiledExpr::compile(&bracket_string, ExprKind::Condition).is_ok());
    }

    #[test]
    fn empty_expression_is_rejected() {
        assert!(CompiledExpr::compile("  ", ExprKind::Condition).is_err());
    }

    #[test]
    fn comprehension_macros_reject_at_compile() {
        // All five macros — CEL's only loops — die at load, not on the
        // request hot path.
        for src in [
            "[1, 2].map(x, x + 1) == [2, 3]",
            "[1, 2].filter(x, x > 1) == [2]",
            "[1, 2].all(x, x > 0)",
            "[1, 2].exists(x, x > 1)",
            "[1, 2].exists_one(x, x > 1)",
        ] {
            let err = CompiledExpr::compile(src, ExprKind::Condition).unwrap_err();
            assert!(err.contains("comprehension"), "{src}: {err}");
            assert!(err.contains("runtime cost limit"), "{src}: {err}");
        }
    }

    #[test]
    fn comprehension_over_request_input_rejected_the_adversarial_probe() {
        // The exact shape that hung the proxy before this limit: short,
        // shallow, load-acceptable, but quadratic in a caller-controlled
        // header. It must never reach evaluation.
        let src = r#"string(size(request.headers["x-p"].split("").map(a, request.headers["x-p"].split(""))))"#;
        assert!(src.len() <= MAX_SOURCE_LEN && max_nesting(src) <= MAX_NESTING);
        let err = CompiledExpr::compile(src, ExprKind::Label).unwrap_err();
        assert!(err.contains("runtime cost limit"), "{err}");
    }

    #[test]
    fn comprehension_nested_inside_calls_and_literals_is_still_found() {
        // Buried in a call argument, a list literal, and a map value — the
        // AST walk sees through all of them.
        for src in [
            "size([1, 2].filter(x, x > 1)) > 0",
            "[[1].map(x, x)][0] == [1]",
            r#"{"k": [1].map(x, x)}["k"] == [1]"#,
        ] {
            let err = CompiledExpr::compile(src, ExprKind::Condition).unwrap_err();
            assert!(err.contains("comprehension"), "{src}: {err}");
        }
    }

    #[test]
    fn has_is_not_a_comprehension_and_still_compiles() {
        // `has(…)` expands to a presence-test select, not a loop — the
        // documented claim-guard idiom keeps working.
        let e = CompiledExpr::compile(r#"has(jwt.claims.sub)"#, ExprKind::Condition).unwrap();
        assert!(e.eval_bool(&ctx()).unwrap());
        let mut c = ctx();
        c.claims = None;
        assert!(!e.eval_bool(&c).unwrap());
    }
}
