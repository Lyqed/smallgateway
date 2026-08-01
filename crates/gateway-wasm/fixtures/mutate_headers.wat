;; A policy module that ENFORCES via a header transform: on_request returns a
;; MutateHeaders decision setting `x-policy: enforced`; the other hooks
;; continue. Proves a signed module can mutate the upstream request (the
;; header-transform proof in the demo).
;;
;; Like the trivial module it ignores its input and returns a constant
;; decision — enough to prove the decision plumbing end to end without a full
;; JSON parser in wat.
(module
  (memory (export "memory") 1)

  ;; on_request response: set x-policy=enforced.
  (data (i32.const 1024)
    "{\"decision\":\"mutate_headers\",\"set\":{\"x-policy\":\"enforced\"},\"remove\":[]}")
  ;; continue, for the other hooks.
  (data (i32.const 1280) "{\"decision\":\"continue\"}")

  (global $bump (mut i32) (i32.const 2048))

  (func (export "alloc") (param $n i32) (result i32)
    (local $p i32)
    (local.set $p (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $n)))
    (local.get $p))

  (func $continue (result i64)
    (i64.or (i64.shl (i64.const 1280) (i64.const 32)) (i64.const 23)))

  ;; The mutate response is 71 bytes at offset 1024.
  (func (export "on_request") (param i32 i32) (result i64)
    (i64.or (i64.shl (i64.const 1024) (i64.const 32)) (i64.const 71)))
  (func (export "on_response_event") (param i32 i32) (result i64) (call $continue))
  (func (export "on_response_end") (param i32 i32) (result i64) (call $continue)))
