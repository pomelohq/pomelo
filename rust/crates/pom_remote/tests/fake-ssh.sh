#!/bin/sh
# Stands in for ssh in the port forwarding tests: records each call's argv, answers `-O check` as FAKE_MASTER
# says and listens on the forward's port for `-N`. Checked in rather than written by the tests, since a file
# written while another test thread spawns can fail to run (ETXTBSY).
for arg in "$@"; do printf '%s ' "$arg"; done >> "$FAKE_SSH_LOG"
printf '\n' >> "$FAKE_SSH_LOG"
case "$1" in
  -O)
    if [ "$2" = check ]; then exit "${FAKE_MASTER:-0}"; fi
    exit 0 ;;
  -N)
    while [ $# -gt 0 ]; do
      if [ "$1" = -L ]; then spec=$2; fi
      shift
    done
    port=${spec#127.0.0.1:}
    port=${port%%:*}
    exec nc -lk 127.0.0.1 "$port" ;;
esac
exit 1
