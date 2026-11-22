#!/usr/bin/env bash
# The machine a run used, as key<TAB>value. audit.py compares the fields
# that must not change (hw_model, chip, memory_bytes, host_arch,
# macos_version, macos_build) between the prepared manifest, the start
# and the end of a run.
set -euo pipefail
source "$(dirname "$0")/lib.sh"

output=${1:-/dev/stdout}
{
  printf 'captured_utc\t%s\n' "$(ts_utc)"
  printf 'hw_model\t%s\n' "$(/usr/sbin/sysctl -n hw.model 2>/dev/null || printf unknown)"
  printf 'chip\t%s\n' "$(/usr/sbin/sysctl -n machdep.cpu.brand_string 2>/dev/null || printf unknown)"
  printf 'memory_bytes\t%s\n' "$(/usr/sbin/sysctl -n hw.memsize)"
  printf 'logical_cpus\t%s\n' "$(/usr/sbin/sysctl -n hw.logicalcpu)"
  printf 'host_arch\t%s\n' "$(/usr/bin/uname -m)"
  printf 'macos_version\t%s\n' "$(/usr/bin/sw_vers -productVersion)"
  printf 'macos_build\t%s\n' "$(/usr/bin/sw_vers -buildVersion)"
  printf 'kernel\t%s\n' "$(/usr/bin/uname -rv | /usr/bin/tr '\t' ' ')"
  printf 'uptime\t%s\n' "$(/usr/bin/uptime | /usr/bin/sed 's/^ *//' | /usr/bin/tr '\t' ' ')"
  printf 'low_power_mode\t%s\n' "$(/usr/bin/pmset -g 2>/dev/null | /usr/bin/awk '/lowpowermode/{print $2; exit}')"
  printf 'power_source\t%s\n' "$(/usr/bin/pmset -g batt 2>/dev/null | /usr/bin/head -1 | /usr/bin/sed "s/.*'\(.*\)'.*/\1/")"
  printf 'battery\t%s\n' "$(/usr/bin/pmset -g batt 2>/dev/null | /usr/bin/sed -n 2p | /usr/bin/tr '\t' ' ' | /usr/bin/sed 's/^ *//')"
  printf 'appearance\t%s\n' "$(/usr/bin/defaults read -g AppleInterfaceStyle 2>/dev/null || printf Light)"
  printf 'displays\t%s\n' "$(/usr/sbin/system_profiler SPDisplaysDataType 2>/dev/null | /usr/bin/awk '/Resolution:|UI Looks like:|Refresh Rate:|Main Display:/{gsub(/^[[:space:]]+/, ""); printf "%s; ", $0}')"
  printf 'free_disk_kib\t%s\n' "$(/bin/df -k / | /usr/bin/awk 'NR==2{print $4}')"
  printf 'krate_no_usage\t%s\n' "${KRATE_NO_USAGE:+set}"
  printf 'synthetic\tno\n'
} >"$output"
