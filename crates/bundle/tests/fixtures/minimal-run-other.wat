;; The same shape as minimal-run.wat with different code (`run` returns 1):
;; a second, distinct, still-valid component for tests that replace one
;; component with another and expect open to accept the result. Built with
;; `wasm-tools parse minimal-run-other.wat -o minimal-run-other.wasm`.
(component
  (core module $m
    (func (export "run") (result i32) i32.const 1))
  (core instance $i (instantiate $m))
  (func $run (result s32) (canon lift (core func $i "run")))
  (export "run" (func $run))
)
