# Tab Status Persistence Design

**Date:** 2026-03-03
**Status:** Approved

## Problem

`tab_statuses` are stored in memory and lost when a Zellij session is restarted. Users who set tab statuses via `zellij pipe` lose them on session restart/reattach.

## Approach: File-based per-session persistence

### Storage

File: `/host/.cache/zjstatus/{session_name}.json`
Format: `{"0":"🤖","1":"✅"}` — same as existing `serialize_tab_statuses`/`deserialize_tab_statuses`.

### Write trigger

After every `tab_statuses` mutation (set_status, clear_status, status_sync) — call `save_statuses()`. File is tiny (tens of bytes), overhead minimal. Every instance writes on change — idempotent since data is identical after sync.

### Read trigger

On first `ModeUpdate` event (when `session_name` becomes available). Loaded statuses merge into `tab_statuses` and broadcast to siblings.

### Error handling

If file doesn't exist, is unreadable, or unparseable — ignore, start with empty map. Log via `tracing::warn!`.

### Cleanup

No cleanup. Files are tiny, session names are unique.

## Changes

**`src/bin/zjstatus.rs`:**
- Add `save_statuses()` method — serialize + write to file
- Add `load_statuses()` method — read file + deserialize
- Call `load_statuses()` on first `ModeUpdate` (when session_name available)
- Call `save_statuses()` in `pipe()` when `tab_statuses` changed

**`src/pipe.rs`:**
- No changes — serialization helpers already exist
