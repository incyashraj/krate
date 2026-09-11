;; A component that is well formed, exports `run` the current way, and asks
;; for a version of krate:time/clock no Krate world provides. The runtime's
;; world selection refuses it by inspection, before anything runs (IC-231).
;; Built with `wasm-tools parse fits-no-world.wat -o fits-no-world.wasm`.
(component
  (import "krate:time/clock@9.9.9" (instance (export "now-millis" (func (result u64)))))
  (core module $m
    (func (export "run") (result i32) i32.const 0))
  (core instance $i (instantiate $m))
  (func $run (result s32) (canon lift (core func $i "run")))
  (export "run" (func $run))
)
