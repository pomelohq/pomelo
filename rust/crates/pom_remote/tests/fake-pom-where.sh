#!/bin/sh
# Linked into the places the launcher looks for pom: says which folder it ran from, then its arguments.
echo "$(basename "$(dirname "$0")")" "$@"
