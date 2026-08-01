;; A malicious/buggy module: on_request loops forever, burning CPU. Under the
;; host's fuel budget it traps `OutOfFuel` long before wedging the worker;
;; under the epoch watchdog it traps `Interrupt` at the deadline. Either way
;; the host fails the route CLOSED to the GB-4 template — it never hangs and
;; never returns Continue. This fixture is the "no guest code runs unbounded
;; on the request path" proof (docs/04, the generalized CEL-DoS lesson).
(module
  (memory (export "memory") 1)
  (data (i32.const 1024) "{\"decision\":\"continue\"}")

  (global $bump (mut i32) (i32.const 2048))

  (func (export "alloc") (param $n i32) (result i32)
    (local $p i32)
    (local.set $p (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $n)))
    (local.get $p))

  ;; Spin forever: `loop` with an unconditional branch back to itself. Every
  ;; iteration consumes fuel; the guest can never reach the (unreachable)
  ;; return, so the host only ever sees OutOfFuel or Interrupt.
  (func (export "on_request") (param i32 i32) (result i64)
    (loop $spin
      (br $spin))
    (unreachable))

  (func $continue (result i64)
    (i64.or (i64.shl (i64.const 1024) (i64.const 32)) (i64.const 23)))
  (func (export "on_response_event") (param i32 i32) (result i64) (call $continue))
  (func (export "on_response_end") (param i32 i32) (result i64) (call $continue)))
