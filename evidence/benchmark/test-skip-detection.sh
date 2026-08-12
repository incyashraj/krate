#!/usr/bin/env bash
# Does the detection classify each real failure text as skipped?
check() {
  local f="$1" label="$2"
  if grep -q '"status":"rejected"' "$f" 2>/dev/null; then echo "  $label -> skipped (quota)"; return; fi
  if grep -qE 'OAuth session expired|could not be refreshed|Failed to authenticate|requires a newer version' "$f" 2>/dev/null; then echo "  $label -> skipped (auth)"; return; fi
  if grep -qE 'Connection closed mid-response|API Error: Connection|connection reset by peer' "$f" 2>/dev/null; then echo "  $label -> skipped (network)"; return; fi
  if grep -qE 'timed out after|KRATE_AUTHOR_TIMEOUT_SECS|Raise the budget' "$f" 2>/dev/null; then echo "  $label -> skipped (budget)"; return; fi
  echo "  $label -> FAIL (scored as a bad app)"
}
printf 'API Error: Connection closed mid-response. The response above may be incomplete.\n' > /tmp/f.net
printf 'the AI agent did not finish within 15 minutes and was stopped.\n  2. Raise the budget: set KRATE_AUTHOR_TIMEOUT_SECS to more seconds.\n' > /tmp/f.budget
printf '{"status":"rejected"}\n' > /tmp/f.quota
printf 'OAuth session expired\n' > /tmp/f.auth
printf 'error: the app did not paint a frame\n' > /tmp/f.real
check /tmp/f.net    "real API disconnect  "
check /tmp/f.budget "real timeout message "
check /tmp/f.quota  "quota rejection      "
check /tmp/f.auth   "auth failure         "
check /tmp/f.real   "a genuine app failure"
