;; A per-event policy module that CUTS the stream: on_response_event returns
;; a CutStream decision. Proves a signed module can enforce mid-stream via the
;; SAME GB-4 terminal-event machinery GB-5 uses. Its on_request continues, so
;; the request reaches the upstream and starts streaming before the cut fires.
(module
  (memory (export "memory") 1)

  ;; {"decision":"cut_stream","reason":"module-cut"} — 47 bytes.
  (data (i32.const 1024) "{\"decision\":\"cut_stream\",\"reason\":\"module-cut\"}")
  (data (i32.const 1280) "{\"decision\":\"continue\"}")

  (global $bump (mut i32) (i32.const 2048))

  (func (export "alloc") (param $n i32) (result i32)
    (local $p i32)
    (local.set $p (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $n)))
    (local.get $p))

  (func $continue (result i64)
    (i64.or (i64.shl (i64.const 1280) (i64.const 32)) (i64.const 23)))

  (func (export "on_request") (param i32 i32) (result i64) (call $continue))
  (func (export "on_response_event") (param i32 i32) (result i64)
    (i64.or (i64.shl (i64.const 1024) (i64.const 32)) (i64.const 47)))
  (func (export "on_response_end") (param i32 i32) (result i64) (call $continue)))
