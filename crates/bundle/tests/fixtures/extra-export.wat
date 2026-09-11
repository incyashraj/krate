;; A component that exports `run` AND something no Krate world declares.
;; It instantiates, so nothing before the validator would object; the
;; validator refuses it because the world declares exactly `run` (IC-210).
;; Built with `wasm-tools parse extra-export.wat -o extra-export.wasm`.
(component
  (core module $m
    (func (export "run") (result i32) i32.const 0)
    (func (export "debug-hook") (result i32) i32.const 7))
  (core instance $i (instantiate $m))
  (func $run (result s32) (canon lift (core func $i "run")))
  (export "run" (func $run))
  (func $hook (result s32) (canon lift (core func $i "debug-hook")))
  (export "debug-hook" (func $hook))
)
