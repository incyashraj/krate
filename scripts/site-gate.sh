#!/bin/sh
# Every local check that guards krate.tech, the web Studio and Studio's UI,
# in one run. Used after every step of the 2026-10 redesign so a regression
# points at one change. Prints PASS/FAIL per check and exits non-zero if any
# failed. Run from the repository root:
#
#   sh scripts/site-gate.sh            the checks
#   sh scripts/site-gate.sh -v         the same, printing each failure's output
#
# It does not deploy, push or touch the network beyond what the checks
# themselves do (none of them call live services).
verbose=0
[ "$1" = "-v" ] && verbose=1
fails=0
out=$(mktemp)

run() {
  name="$1"; shift
  if "$@" >"$out" 2>&1; then
    echo "PASS $name"
  else
    echo "FAIL $name"
    fails=$((fails + 1))
    if [ "$verbose" = 1 ]; then sed 's/^/    /' "$out" | tail -40; fi
  fi
}

for t in docs/landing/studio/*-test.mjs; do
  run "$(basename "$t")" node "$t"
done
run "studio hooks" node scripts/check-studio-hooks.mjs
run "studio design layer (self-test)" node scripts/test-studio-redesign.mjs --self-test
run "studio design layer" node scripts/test-studio-redesign.mjs
run "inline scripts parse" node scripts/test-inline-scripts.mjs
run "html balance self-test" python3 scripts/check-html-balance.py --self-test
run "html balance" python3 scripts/check-html-balance.py
run "public facts" python3 scripts/test-public-facts.py
run "contributor docs" python3 scripts/test-contributor-docs.py
run "answer pages self-test" python3 scripts/build-answer-pages.py --self-test
run "site seo tests" python3 scripts/test-site-seo.py
run "landing assets" python3 scripts/test-landing-assets.py
run "first party" python3 scripts/check-first-party.py
run "first party self-test" python3 scripts/check-first-party.py --self-test
run "touch targets self-test" python3 scripts/ensure-touch-targets.py --self-test
run "site kit self-test" python3 scripts/site_kit.py --self-test
run "site kit pages" python3 scripts/site_kit.py --check
run "fingerprint self-test" python3 scripts/fingerprint-css.py --self-test
run "site tests deployed" python3 scripts/check-site-tests-deployed.py

# Browser checks, when a Playwright module is available (it is not in CI).
if [ -n "$KRATE_PLAYWRIGHT_MODULE" ] || node -e "import('playwright')" >/dev/null 2>&1; then
  run "pages readable without JavaScript" node scripts/test-site-nojs.mjs
else
  echo "SKIP pages readable without JavaScript (set KRATE_PLAYWRIGHT_MODULE)"
fi

rm -f "$out"
if [ "$fails" -gt 0 ]; then
  echo "$fails check(s) failed"
  exit 1
fi
echo "all checks passed"
