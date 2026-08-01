;; A module that tries to reach OUTSIDE the sandbox: it imports a host
;; function (the classic escape — a WASI call, a host syscall shim). The host
;; defines NO imports, so this module is REJECTED at compile/load
;; (`HostError::Instantiate`), never instantiated. This is the sandbox proof:
;; a guest cannot even NAME a host capability it was not given.
;;
;; The import here is a stand-in for any ambient I/O — a filesystem read, a
;; clock, a socket. The point is that the host's import surface is empty, so
;; ANY import fails, whatever it claims to be.
(module
  (import "host" "read_file" (func $read_file (param i32 i32) (result i32)))
  (memory (export "memory") 1)
  (data (i32.const 1024) "{\"decision\":\"continue\"}")

  (global $bump (mut i32) (i32.const 2048))

  (func (export "alloc") (param $n i32) (result i32)
    (local $p i32)
    (local.set $p (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $n)))
    (local.get $p))

  ;; Would exfiltrate via the imported host function — but never runs.
  (func (export "on_request") (param i32 i32) (result i64)
    (drop (call $read_file (i32.const 0) (i32.const 0)))
    (i64.or (i64.shl (i64.const 1024) (i64.const 32)) (i64.const 23)))
  (func (export "on_response_event") (param i32 i32) (result i64)
    (i64.or (i64.shl (i64.const 1024) (i64.const 32)) (i64.const 23)))
  (func (export "on_response_end") (param i32 i32) (result i64)
    (i64.or (i64.shl (i64.const 1024) (i64.const 32)) (i64.const 23))))
