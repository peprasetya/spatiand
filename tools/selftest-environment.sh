#!/usr/bin/env bash
# The environment picker test; see selftest-scratch.sh.
exec "$(dirname "$0")/selftest-scratch.sh" --selftest-environment "$@"
