;; The trivial policy module: every hook returns {"decision":"continue"}.
;;
;; This is the fixture the HOT-PATH BENCHMARK runs per event to measure the
;; irreducible per-event WASM cost (instantiate + marshal in + call + marshal
;; out) with a guest that does the minimum real work: read nothing, return the
;; continue decision. It also backs the host/loader sandbox tests.
;;
;; ABI (see src/abi.rs, src/host.rs):
;;   - export `memory`
;;   - export `alloc(i32) -> i32`: bump-allocate `n` bytes, return the offset.
;;     The host writes the input JSON there. (This trivial guest ignores the
;;     input; it still must allocate so the host can hand bytes across.)
;;   - export `on_request` / `on_response_event` / `on_response_end`
;;     `(ptr i32, len i32) -> i64`: return the response as a packed
;;     (out_ptr << 32) | out_len. Here every hook returns the same constant.
;;
;; No imports: the module calls nothing in the host — the sandbox's first wall.
(module
  (memory (export "memory") 1)

  ;; The constant response, placed at a fixed offset well above the bump area.
  (data (i32.const 1024) "{\"decision\":\"continue\"}")

  ;; A bump pointer for alloc, starting after the input scratch region.
  (global $bump (mut i32) (i32.const 2048))

  ;; alloc(n): return the current bump pointer, then advance it by n.
  (func (export "alloc") (param $n i32) (result i32)
    (local $p i32)
    (local.set $p (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $n)))
    (local.get $p))

  ;; Pack the constant {"decision":"continue"} (23 bytes at offset 1024).
  (func $continue (result i64)
    (i64.or
      (i64.shl (i64.const 1024) (i64.const 32))
      (i64.const 23)))

  (func (export "on_request") (param i32 i32) (result i64) (call $continue))
  (func (export "on_response_event") (param i32 i32) (result i64) (call $continue))
  (func (export "on_response_end") (param i32 i32) (result i64) (call $continue)))
