#!/usr/bin/env bash
check() {
  local f="$1" label="$2"
  if grep -q '"status":"rejected"' "$f" 2>/dev/null; then echo "  $label -> skipped (quota)"; return; fi
  if grep -qE 'OAuth session expired|could not be refreshed|Failed to authenticate|requires a newer version' "$f" 2>/dev/null; then echo "  $label -> skipped (auth)"; return; fi
  if grep -qE 'Connection closed mid-response|API Error: Connection|connection reset by peer' "$f" 2>/dev/null; then echo "  $label -> skipped (network)"; return; fi
  if grep -qE 'did not finish within [0-9]+ minutes and was stopped' "$f" 2>/dev/null; then echo "  $label -> skipped (budget)"; return; fi
  echo "  $label -> scored normally"
}
printf 'the AI agent did not finish within 30 minutes and was stopped.\n  2. Raise the budget: set KRATE_AUTHOR_TIMEOUT_SECS to more seconds.\n' > /tmp/g.timeout
printf 'KRATE_APP_NAME=click-counter\nKRATE_AUTHOR_TIMEOUT_SECS=1800\nKRATE_BIN=/tmp/krate\n' > /tmp/g.envdump
printf 'API Error: Connection closed mid-response.\n' > /tmp/g.net
printf 'error: the app did not paint a frame\n' > /tmp/g.real
check /tmp/g.timeout "REAL timeout          "
check /tmp/g.envdump "env dump (was false+) "
check /tmp/g.net     "real API disconnect   "
check /tmp/g.real    "genuine app failure   "
