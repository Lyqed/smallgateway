;; A policy module that REJECTS on_request with a custom reason — proving a
;; signed module can enforce a bespoke org rule and fail the request to the
;; operator's GB-4 template (the custom-rejection proof in the demo). The
;; `reason` flows into the GB-4 {{key}} substitution.
(module
  (memory (export "memory") 1)

  ;; {"decision":"reject","reason":"blocked-by-policy"} — 50 bytes.
  (data (i32.const 1024) "{\"decision\":\"reject\",\"reason\":\"blocked-by-policy\"}")
  (data (i32.const 1280) "{\"decision\":\"continue\"}")

  (global $bump (mut i32) (i32.const 2048))

  (func (export "alloc") (param $n i32) (result i32)
    (local $p i32)
    (local.set $p (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $n)))
    (local.get $p))

  (func $continue (result i64)
    (i64.or (i64.shl (i64.const 1280) (i64.const 32)) (i64.const 23)))

  (func (export "on_request") (param i32 i32) (result i64)
    (i64.or (i64.shl (i64.const 1024) (i64.const 32)) (i64.const 50)))
  (func (export "on_response_event") (param i32 i32) (result i64) (call $continue))
  (func (export "on_response_end") (param i32 i32) (result i64) (call $continue)))
