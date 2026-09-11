;; The smallest component Krate will accept: no imports, and exactly the
;; `run` export every Krate world declares. Built into minimal-run.wasm with
;; `wasm-tools parse minimal-run.wat -o minimal-run.wasm`. Test fixtures that
;; only need "a real component" use this instead of a bare 8-byte header,
;; which is a component that could never run (IC-210).
(component
  (core module $m
    (func (export "run") (result i32) i32.const 0))
  (core instance $i (instantiate $m))
  (func $run (result s32) (canon lift (core func $i "run")))
  (export "run" (func $run))
)
