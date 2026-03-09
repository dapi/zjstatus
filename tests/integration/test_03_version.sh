#!/usr/bin/env bash
#
# Version query tests — zjstatus::version pipe command.
# Sourced by docker-test-runner.sh (helpers.sh already loaded).
#
# NOTE: cli_pipe_output doesn't return data in headless mode (script PTY).
# Query response correctness is covered by unit tests in pipe.rs.
# Here we verify: no crash, no hang, pipe exits cleanly.
#

# --- test_version_no_hang ---
echo "  [test_version_no_hang] zjstatus::version::_ completes without hanging"
if timeout 5 zellij pipe --plugin "file:$PLUGIN_WASM" --name "ver-$$" -- "zjstatus::version::_" < /dev/null 2>/dev/null; then
    echo "  PASS: version pipe completed (exit 0)"
    ((PASS++)) || true
else
    echo "  FAIL: version pipe timed out or failed"
    ((FAIL++)) || true
fi

# --- test_version_no_crash ---
echo "  [test_version_no_crash] session alive after version query"
assert_session_alive "version: session alive"

# --- test_version_plugin_still_responds ---
echo "  [test_version_still_responds] plugin responds after version query"
assert_pipe_responds "zjstatus::notify::after-version" "version: plugin still responds"
