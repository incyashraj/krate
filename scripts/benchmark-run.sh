#!/usr/bin/env bash
# The Krate App Benchmark: measure whether an authored app is USABLE, not just
# whether authoring succeeded.
#
# scripts/reliability-run.sh answers "did something build and run". This answers
# the harder question a real person feels: does the app do the thing, survive a
# resize, answer a click, and stay open. An app can pass every check-app stage
# and still be an app nobody can use, and that gap is the whole point of this
# harness.
#
# THE FIVE GATES, in the order they are checked. First failure wins and is
# recorded as the reason.
#
#   1. authored   `krate create` produced a .krate at all
#   2. imports    the component imports only krate:* (check-app stage 13)
#   3. does       the per-request asserts in the corpus all hold
#   4. resize     the app survives being told the window is a different size
#   5. click      the app responds to a synthetic activation
#   6. stays      the app did not close itself before it was asked to
#
# Gates 4-6 are folded into one run because they are all observations of the
# same self-exercise pass, not separate launches. See ASSERTIONS below.
#
# ASSERTIONS -- how "does what was asked" is machine-checkable.
#
# Each corpus row carries semicolon-separated asserts over key:value lines the
# app prints on stdout during `krate run <app> -- quick`. That `quick` argument
# is not invented here: 27 of the 34 apps in apps/ already implement it as a
# self-exercise path that drives the app through its own interactions and
# reports the resulting state. This harness makes the house convention a
# requirement. A human writes the asserts once, they are locked in the corpus,
# and every run after that needs no human.
#
# Rejected alternatives, and why:
#   - A locked reference screenshot. It fails on any font, scale, or theme
#     change, so it would need re-judging constantly and would rot into a rubber
#     stamp. It also cannot distinguish "the counter incremented" from "a
#     counter is painted on screen", which is exactly the distinction that
#     separates a working app from a picture of one.
#   - A model judging the screenshot. Not reproducible, and the number stops
#     being something we are accountable to.
#   - Exit code only. That is what reliability-run.sh already does, and it is
#     what let a 1511-line mail client over invented data score as a pass.
#
# RESUMABLE. Every finished request appends one TSV row keyed by its corpus id,
# and a re-run skips any id already present. A full run is hours. Stopping and
# resuming is the normal way to use this.
#
# RATE LIMITS ARE `skipped`, NEVER `fail`. Learned the hard way: counting quota
# rejections as failures once turned a 14/14 pass rate into a reported 23%,
# which was a lie about the system. A rejection means no code was ever written.
# The run records it as skipped and STOPS, because once quota is gone every
# remaining request fails in two seconds and the corpus burns in a minute.
#
# Usage:
#   scripts/benchmark-run.sh                  # everything not yet done
#   scripts/benchmark-run.sh --count 10
#   scripts/benchmark-run.sh --only 1,5,39
#   scripts/benchmark-run.sh --tier easy
#   scripts/benchmark-run.sh --summary        # print the table, run nothing
#   scripts/benchmark-run.sh --dry-run        # show what would run
#
# Do not rebuild the krate binary mid-run. Half the results would measure one
# build and half another, and the score would then be a number about nothing.
# Copy the binary somewhere and point KRATE_BIN at the copy.
#
# Environment:
#   KRATE_BIN     binary under test (default: target/release/krate)
#   RESULTS       TSV to append to (default: evidence/benchmark/results-<date>.tsv)
#   WORK_ROOT     per-request work dirs (default: a tmp dir, kept for inspection)
#   AGENT         authoring agent (default: claude)
#   TIMEOUT_SECS  per-request agent budget (default: 900)

set -uo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
corpus="$repo_root/evidence/benchmark/corpus.tsv"
krate_bin="${KRATE_BIN:-$repo_root/target/release/krate}"
results="${RESULTS:-$repo_root/evidence/benchmark/results-$(date +%Y-%m-%d).tsv}"
work_root="${WORK_ROOT:-${TMPDIR:-/tmp}/krate-benchmark}"
agent="${AGENT:-claude}"
timeout_secs="${TIMEOUT_SECS:-900}"

count=0
only=""
tier_filter=""
summary_only=0
dry_run=0

while [ $# -gt 0 ]; do
  case "$1" in
    --count) count="$2"; shift 2 ;;
    --only) only="$2"; shift 2 ;;
    --tier) tier_filter="$2"; shift 2 ;;
    --summary) summary_only=1; shift ;;
    --dry-run) dry_run=1; shift ;;
    --results) results="$2"; shift 2 ;;
    --corpus) corpus="$2"; shift 2 ;;
    -h|--help) sed -n '2,74p' "${BASH_SOURCE[0]}"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

if [ ! -f "$corpus" ]; then
  echo "no corpus at $corpus" >&2
  exit 1
fi

# Strip comments and blanks. Unlike the reliability corpus, ids are explicit in
# field 1, so this file may be reordered without breaking any results file.
rows="$(mktemp)"
trap 'rm -f "$rows"' EXIT
grep -v '^[[:space:]]*#' "$corpus" | grep -v '^[[:space:]]*$' > "$rows"
total_rows="$(wc -l < "$rows" | tr -d ' ')"

header=$'id\ttier\trequest\tresult\tgate\tseconds\tkrate_bytes\tasserts_passed\tasserts_total\tmissing\tnote'
if [ ! -f "$results" ]; then
  mkdir -p "$(dirname "$results")"
  printf '%s\n' "$header" > "$results"
fi

print_summary() {
  if [ ! -f "$results" ]; then
    echo "no results yet at $results"
    return
  fi
  awk -F'\t' '
    NR == 1 { next }
    {
      total++
      if ($4 == "skipped") { skipped++; secs += $6; next }
      # Tier counts track ATTEMPTED requests only, for the same reason the score
      # does: a request the AI never saw says nothing about the tier.
      tiers[$2]++
      if ($4 == "pass") { pass++; tierpass[$2]++ }
      else { fail++; gate[$5]++ }
      secs += $6
    }
    END {
      if (total == 0) { print "no results yet"; exit }
      printf "\n  requests run      %d\n", total
      printf "  usable            %d\n", pass
      printf "  not usable        %d\n", fail
      if (skipped > 0) printf "  skipped (account) %d\n", skipped
      # The score counts only requests that actually reached the AI. A quota
      # rejection means nothing was authored, so including it would understate
      # the product rather than measure it.
      attempted = pass + fail
      if (attempted > 0)
        printf "\n  SCORE             %d/%d  (%.0f%%)\n", pass, attempted, (pass * 100) / attempted
      printf "  mean wall time    %.0fs\n", secs / total
      if (length(tiers) > 0) {
        printf "\n  by tier\n"
        for (t in tiers) printf "    %-8s %d/%d\n", t, tierpass[t] + 0, tiers[t]
      }
      if (fail > 0) {
        printf "\n  failures by gate\n"
        for (g in gate) printf "    %-10s %d\n", g, gate[g]
      }
      print ""
    }
  ' "$results"
}

if [ "$summary_only" = "1" ]; then
  print_summary
  exit 0
fi

# A dry run reads the corpus and nothing else, so it must work before anything
# is built -- that is how you check a corpus edit without a toolchain.
if [ "$dry_run" != "1" ] && [ ! -x "$krate_bin" ]; then
  echo "no krate binary at $krate_bin -- run: cargo build --release -p krate-cli" >&2
  exit 1
fi

# Evaluate one assert against the app's stdout. Returns 0 if it holds.
#
# Grammar is deliberately tiny -- five operators over "key:value" lines -- so
# that a person writing a corpus row cannot express something the harness
# silently mis-evaluates. Anything richer would need a parser, and a parser
# nobody trusts is worse than a bar nobody can game.
#
# A key may name alternatives with `|`, and the assert holds if ANY of them
# holds: `count|clicks>=1`, `cols|columns>=2`, `recorded|logged>=1`.
#
# This is not a loosening of the bar. An app that never reports a property
# still fails; this only stops two names for the SAME reported property
# counting as a miss. The 2026-08-12 run made the case: ten of twenty-eight
# failures turned on a key name, and a base64 encoder that verified its own
# round trip against known vectors failed on `round_trip` versus
# `roundtrip` -- one underscore (K-105).
check_assert() {
  local assert="$1" out="$2" keys op want key

  case "$assert" in
    *'>='*) keys="${assert%%>=*}"; op=">="; want="${assert##*>=}" ;;
    *'<='*) keys="${assert%%<=*}"; op="<="; want="${assert##*<=}" ;;
    *'=='*) keys="${assert%%==*}"; op="=="; want="${assert##*==}" ;;
    *'!='*) keys="${assert%%!=*}"; op="!="; want="${assert##*!=}" ;;
    *'~'*)  keys="${assert%%~*}";  op="~";  want="${assert##*~}" ;;
    *'?'*)  keys="${assert%\?}";   op="?";  want="" ;;
    *) return 1 ;;
  esac

  # Try each alternative in turn; the first that holds wins. `!=` is the one
  # operator where this needs care: it must hold for every name the app
  # actually printed, not merely for one, or `roundtrip!=no` would pass on an
  # app that printed `roundtrip:no` alongside some other spelling that is
  # absent. So `!=` requires a present key AND the inequality.
  local IFS='|'
  for key in $keys; do
    unset IFS
    check_one_key "$key" "$op" "$want" "$out" && return 0
    local IFS='|'
  done
  unset IFS
  return 1
}

# One key, one operator. Split out of `check_assert` so the alternatives loop
# above reads as a loop rather than as a rewrite of the operator table.
check_one_key() {
  local key="$1" op="$2" want="$3" out="$4" value

  # Last wins: an app that prints a key repeatedly as it exercises itself is
  # reporting its final state on the last line, which is the state we mean.
  value="$(grep -E "^${key}:" "$out" 2>/dev/null | tail -1 | cut -d: -f2- )"
  [ -z "$value" ] && [ "$op" != "?" ] && return 1

  case "$op" in
    '?')  grep -qE "^${key}:" "$out" 2>/dev/null ;;
    '~')  printf '%s' "$value" | grep -qF -- "$want" ;;
    '==') [ "$value" = "$want" ] ;;
    '!=') [ "$value" != "$want" ] ;;
    '>='|'<=')
      # Numeric. A non-numeric value fails rather than comparing as a string,
      # because "12 items" >= 3 must not quietly pass on lexical order.
      case "$value" in
        ''|*[!0-9.-]*) return 1 ;;
      esac
      awk -v a="$value" -v b="$want" -v o="$op" \
        'BEGIN { exit !(o == ">=" ? a >= b : a <= b) }'
      ;;
    *) return 1 ;;
  esac
}

# Which ids to attempt, in corpus order.
targets=""
if [ -n "$only" ]; then
  targets="$(printf '%s' "$only" | tr ',' ' ')"
else
  targets="$(awk -F'\t' -v t="$tier_filter" '(t == "" || $2 == t) { print $1 }' "$rows")"
fi

mkdir -p "$work_root"

# Refuse to start beside another authoring run. Two `krate create` processes on
# one machine fight over the same cargo target cache and the same AI quota, and
# a request that fails because another workstation exhausted the quota is
# recorded against the product rather than against the machine. That is exactly
# the confusion that once turned a 14/14 pass rate into a reported 23%.
#
# Found the hard way: the first attempt at this run started while another
# workstation was authoring "a tip calculator that splits the bill by number of
# people" from ~/.local/bin/krate into ~/krate-outsider. Nothing was corrupted,
# because each run keeps its own work dir -- but it cost real time to prove
# that, and a louder failure up front is worth more than a quiet coincidence.
#
# Matched on the binary invocation (`.../krate create`) rather than the bare
# words, because `pgrep -f 'krate create'` also matches this harness's own
# wrapper shell and any grep whose pattern happens to contain the phrase. A
# guard with false positives gets disabled, and then it protects nothing.
others=0
if [ "$dry_run" != "1" ]; then
  others="$(pgrep -laf 'krate create' 2>/dev/null \
    | grep -E '/krate create ' | grep -vc 'grep -E' || true)"
fi
if [ "${others:-0}" -gt 0 ] && [ "${ALLOW_CONCURRENT_RUNS:-0}" != "1" ]; then
  echo "another 'krate create' is already running on this machine:" >&2
  pgrep -laf 'krate create' 2>/dev/null | grep -E '/krate create ' | grep -v 'grep -E' | cut -c1-120 >&2
  echo >&2
  echo "Two authoring runs share one AI quota and one build cache, so the" >&2
  echo "score would measure the collision rather than the product. Wait for" >&2
  echo "it to finish, or set ALLOW_CONCURRENT_RUNS=1 if you are certain." >&2
  exit 3
fi

echo "krate binary : $krate_bin"
echo "corpus       : $corpus ($total_rows requests)"
echo "results      : $results"
echo "work dirs    : $work_root"
echo "agent        : $agent"
echo

attempted=0
for id in $targets; do
  row="$(awk -F'\t' -v id="$id" '$1 == id { print; exit }' "$rows")"
  if [ -z "$row" ]; then
    echo "[$id] no such request in the corpus, skipping"
    continue
  fi
  tier="$(printf '%s' "$row" | cut -f2)"
  request="$(printf '%s' "$row" | cut -f3)"
  asserts="$(printf '%s' "$row" | cut -f4)"

  if [ -n "$tier_filter" ] && [ "$tier" != "$tier_filter" ]; then
    continue
  fi
  # Resumable: an id already in the results file is done, however it went.
  if awk -F'\t' -v id="$id" 'NR > 1 && $1 == id { found = 1 } END { exit !found }' "$results"; then
    echo "[$id] already recorded, skipping"
    continue
  fi
  if [ "$dry_run" = "1" ]; then
    echo "[$id] ($tier) $request"
    echo "      asserts: $asserts"
    continue
  fi

  attempted=$(( attempted + 1 ))
  dir="$work_root/req-$id"
  rm -rf "$dir"
  mkdir -p "$dir"
  out="$dir/app.krate"
  log="$dir/create.log"

  echo "[$id] ($tier) $request"
  start="$(date +%s)"
  KRATE_AUTHOR_TIMEOUT_SECS="$timeout_secs" \
    "$krate_bin" create "$request" \
      --output "$out" \
      --agent "$agent" \
      --work-dir "$dir/work" \
      --json \
      > "$log" 2>"$dir/create.err"
  create_exit=$?
  end="$(date +%s)"
  seconds=$(( end - start ))

  app_dir="$(find "$dir/work" -maxdepth 2 -name Cargo.toml -print 2>/dev/null | head -1 | xargs -I{} dirname {})"
  krate_bytes=0
  [ -f "$out" ] && krate_bytes="$(wc -c < "$out" | tr -d ' ')"

  result=""
  gate="-"
  note="-"
  passed=0
  total_asserts=0
  missing="-"

  # --- account check, before anything is judged ---
  #
  # A request the AI never saw is not a product failure. Two ways that happens,
  # and both must be `skipped` rather than `fail`:
  #
  #   quota  the API refused the request; no code was ever written. Counting
  #          these as failures once turned a 14/14 pass rate into a reported
  #          23%, which was a lie about the system.
  #   auth   the account is not signed in at all. Measured here: `--agent
  #          claude` failed request 2 in 4 seconds with "OAuth session expired"
  #          in the transcript (K-007). Four seconds is far too fast to have
  #          authored anything, and recording it as a failed app would have put
  #          this machine's broken login into the product's score.
  rate_limited=0
  skip_reason=""
  for f in "$log" "$dir/create.err" "${app_dir:+$app_dir/.agent-transcript.txt}"; do
    [ -n "$f" ] && [ -f "$f" ] || continue
    if grep -q '"status":"rejected"' "$f" 2>/dev/null; then
      rate_limited=1
      skip_reason="rate limited: the AI account was out of quota, no code was written"
      break
    fi
    if grep -qE 'OAuth session expired|could not be refreshed|Failed to authenticate|requires a newer version' "$f" 2>/dev/null; then
      rate_limited=1
      skip_reason="agent account unusable (K-007): the AI never saw this request"
      break
    fi
  done

  if [ "$rate_limited" = "1" ]; then
    result="skipped"
    gate="account"
    note="$skip_reason"

  # --- the refuse tier is scored inverted ---
  elif [ "$tier" = "refuse" ]; then
    # The pass is a fast honest refusal that names the limit and writes no
    # .krate. Producing something plausible here is the worst failure in the
    # benchmark, because the person cannot tell it is wrong.
    total_asserts=1
    if grep -q '"error":"cannot-build"' "$log" 2>/dev/null && [ ! -f "$out" ]; then
      result="pass"; passed=1
      note="refused: $(grep -o '"limit":"[^"]*"' "$log" | head -1 | cut -d'"' -f4)"
    elif [ -f "$out" ]; then
      result="fail"; gate="refuse"; missing="refused==1"
      note="BUILT AN APP FOR AN IMPOSSIBLE REQUEST -- a person cannot tell this is wrong"
    else
      result="fail"; gate="refuse"; missing="refused==1"
      note="did not build, but did not refuse either: $(head -c 140 "$dir/create.err" | tr '\t\n' '  ')"
    fi

  # --- gate 1: authored ---
  elif [ "$create_exit" != "0" ] || [ ! -f "$out" ]; then
    result="fail"
    gate="authored"
    if grep -q '"error":"cannot-build"' "$log" 2>/dev/null; then
      gate="false-refusal"
      note="REFUSED A BUILDABLE REQUEST: $(grep -o '"reason":"[^"]*"' "$log" | head -1 | cut -d'"' -f4)"
    else
      note="$(grep -m1 -iE 'error|failed|did not' "$dir/create.err" "$log" 2>/dev/null | cut -c1-160 | tr '\t\n' '  ')"
      [ -z "$note" ] && note="create exited $create_exit with no .krate"
    fi

  else
    # --- gate 2: imports only krate:* ---
    # create already enforces this, so a failure here means the .krate on disk
    # disagrees with what create checked, which is worth knowing loudly.
    if ! "$krate_bin" run "$out" --dump-caps > "$dir/caps.log" 2>&1; then
      result="fail"; gate="imports"
      note="$(head -c 160 "$dir/caps.log" | tr '\t\n' '  ')"
    else
      # --- gates 3-6: one self-exercise run ---
      #
      # `-- quick` is the app's self-exercise argument. The app drives itself
      # through its own interactions and prints its resulting state, which is
      # what the asserts read. --shoot forces the headless window path and
      # proves a frame was actually painted.
      #
      # Gate 4 (resize) and gate 5 (click) are observed from this same run
      # rather than from separate launches: the runtime has no scripted-input
      # injection path today, so the app itself is what exercises them, and it
      # reports the result the same way it reports everything else. When that
      # injection path lands this block is where it plugs in, and the corpus
      # does not change.
      shot="$dir/frame.png"
      "$krate_bin" run "$out" --auto-grant --shoot "$shot" -- quick \
        > "$dir/run.log" 2>"$dir/run.err"
      run_exit=$?

      # gate 6: stays open. A non-zero exit that is not the clean
      # close-requested code means the app fell over rather than finishing.
      if [ "$run_exit" != "0" ] && [ "$run_exit" != "2" ]; then
        result="fail"; gate="stays"
        note="app exited $run_exit: $(head -c 140 "$dir/run.err" | tr '\t\n' '  ')"
      elif [ ! -s "$shot" ]; then
        result="fail"; gate="stays"
        note="painted no frame: the window never drew anything"
      else
        # gate 3: does what was asked.
        miss=""
        old_ifs="$IFS"; IFS=';'
        for a in $asserts; do
          [ -z "$a" ] && continue
          total_asserts=$(( total_asserts + 1 ))
          if check_assert "$a" "$dir/run.log"; then
            passed=$(( passed + 1 ))
          else
            miss="${miss:+$miss,}$a"
          fi
        done
        IFS="$old_ifs"

        # Gates 4-6 are evidenced from this same run rather than from asserts.
        #
        # An earlier draft of this harness required every app to print
        # `ready:1` and `quick:done`. That was wrong and is worth recording:
        # nothing in the authoring pack teaches those keys, so every app would
        # have failed them, and the benchmark would have been measuring an
        # invention of the harness rather than the product. A benchmark whose
        # bar is not something the product was ever told to meet is not a
        # measurement of anything.
        #
        # So the evidence for these three gates is behavioural, not a printed
        # key, and it was already collected above:
        #   stays   the app exited 0 or 2, and painted a frame -- checked above
        #   click   the `quick` path IS the app driving its own controls; an
        #           app whose click handling is broken cannot report changed
        #           state, so gate 3's asserts carry this
        #   resize  NOT INDEPENDENTLY CHECKED TODAY. There is no scripted-input
        #           path in the runtime, so nothing can tell an app the window
        #           changed size and observe what it does. K-003 is the known
        #           defect. This is the one gate in the stated pass bar that
        #           this harness does not yet enforce, and saying so is better
        #           than a green tick that checks nothing.

        if [ "$passed" = "$total_asserts" ]; then
          result="pass"; gate="-"; note="-"
        else
          result="fail"; gate="does"; missing="$miss"
          note="$passed/$total_asserts observable properties held"
        fi
      fi
    fi
  fi

  [ -z "$missing" ] && missing="-"
  clean_request="$(printf '%s' "$request" | tr '\t' ' ')"
  clean_note="$(printf '%s' "$note" | tr '\t' ' ')"
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$id" "$tier" "$clean_request" "$result" "$gate" \
    "$seconds" "$krate_bytes" "$passed" "$total_asserts" "$missing" "$clean_note" \
    >> "$results"

  echo "     -> $result ${seconds}s gate=$gate asserts=$passed/$total_asserts"

  if [ "$rate_limited" = "1" ]; then
    echo
    echo "Stopping: $skip_reason"
    echo "Nothing is lost -- this run is resumable, so fix the account and"
    echo "re-run; it will pick up from request $id. Every remaining request"
    echo "would otherwise fail in seconds for a reason that is not the product."
    break
  fi

  # Drop build artifacts, keep the source. Every app dir compiles its own copy
  # of the SDK into a private target/ of about a gigabyte, and a corpus run is
  # forty of them. That fills a disk long before the run finishes, and a full
  # disk fails requests for a reason that has nothing to do with the product.
  if [ "${KEEP_BUILD_DIRS:-0}" != "1" ] && [ -n "$app_dir" ]; then
    rm -rf "$app_dir/target"
  fi
done

echo
echo "attempted $attempted request(s) this run"
print_summary
