#!/usr/bin/env bash
# The guard: a produced .krate beats any pattern match.
sim() { # sim <krate exists> <file content> <label>
  local out=/tmp/g.krate f=/tmp/g.txt
  [ "$1" = "yes" ] && touch $out || rm -f $out
  printf '%s\n' "$2" > $f
  rate_limited=0
  if [ -f "$out" ]; then rate_limited=0
  else
    grep -qE 'Connection closed mid-response|connection reset by peer' $f 2>/dev/null && rate_limited=1
  fi
  [ $rate_limited = 1 ] && echo "  $3 -> skipped" || echo "  $3 -> scored normally"
}
sim yes '{"last_error":"connection reset by peer"}' "app built + phrase in its data "
sim no  'API Error: Connection closed mid-response'  "no app + real disconnect      "
sim yes 'nothing unusual'                            "app built, clean log          "
sim no  'nothing unusual'                            "no app, clean log             "
