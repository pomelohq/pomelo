#!/bin/sh
# Stands in for pom on the remote: prints each argument it got, NUL-terminated.
for arg in "$@"; do printf '%s\0' "$arg"; done
