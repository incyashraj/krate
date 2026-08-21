#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"

output=${1:-/dev/stdout}
{
  printf 'captured_utc\t%s\n' "$(ts_utc)"
  printf 'hostname\t%s\n' "$(/bin/hostname)"
  printf 'git_commit\t%s\n' "$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || printf unknown)"
  printf 'macos_version\t%s\n' "$(/usr/bin/sw_vers -productVersion)"
  printf 'macos_build\t%s\n' "$(/usr/bin/sw_vers -buildVersion)"
  printf 'kernel\t%s\n' "$(/usr/bin/uname -a)"
  printf 'host_arch\t%s\n' "$(/usr/bin/uname -m)"
  printf 'cpu\t%s\n' "$(/usr/sbin/sysctl -n machdep.cpu.brand_string 2>/dev/null || printf unknown)"
  printf 'memory_bytes\t%s\n' "$(/usr/sbin/sysctl -n hw.memsize)"
  printf 'logical_cpus\t%s\n' "$(/usr/sbin/sysctl -n hw.logicalcpu)"
  printf 'physical_cpus\t%s\n' "$(/usr/sbin/sysctl -n hw.physicalcpu)"
  printf 'low_power_mode\t%s\n' "$(/usr/bin/pmset -g custom | /usr/bin/awk '/lowpowermode/{print $2; exit}' || true)"
  printf 'power_source\t%s\n' "$(/usr/bin/pmset -g batt | /usr/bin/head -1 | /usr/bin/tr -d "'" || true)"
  printf 'display_summary\t%s\n' "$(/usr/sbin/system_profiler SPDisplaysDataType 2>/dev/null | /usr/bin/awk '/Resolution:|UI Looks like:|Main Display:|Refresh Rate:/{gsub(/^[[:space:]]+/, ""); printf "%s; ", $0}' || true)"
} >"$output"
