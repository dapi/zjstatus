# Tab Status Sync Between zjstatus Instances

**Date:** 2026-03-02
**Status:** Approved

## Problem

Each tab in Zellij has its own zjstatus plugin instance with independent `tab_statuses: BTreeMap<usize, String>`. When `set_status` is sent via `zellij pipe`, only existing instances receive it. A new instance (created with a new tab) starts with an empty map, causing statuses to disappear when switching to the new tab.

Repro: create session → set status on tab 0 → create tab 1 → click tab 1 → tab 0's status disappears.

## Approach: Broadcast on Change + Broadcast on TabUpdate

### Protocol

New pipe command `status_sync`:

```
zjstatus::status_sync::{"0":"🤖","1":"✅"}
```

Full `tab_statuses` map serialized as JSON. On receive — deserialize and **replace** local map (not merge).

### Broadcast Triggers

Broadcast happens when:
1. After processing `set_status` / `clear_status` from CLI or Keybind
2. On `TabUpdate` when tab count **increased** (new instance needs statuses)

Broadcast does NOT happen:
- On receiving `status_sync` from `PipeSource::Plugin` (cycle prevention)
- On `TabUpdate` when tab count stayed same or decreased

### Plugin URL Discovery

`pipe_message_to_plugin` requires `plugin_url` to address all instances. Extract own URL from `PaneManifest` on first `PaneUpdate` by matching `get_plugin_ids().plugin_id` to pane entries. Cache in `State.plugin_url: Option<String>`.

### Changes

**`State` (zjstatus.rs):**
- `+prev_tab_count: usize` — track tab count growth
- `+plugin_url: Option<String>` — cached plugin URL from PaneManifest

**`process_line` (pipe.rs):**
- Return type: `bool` → `(bool, bool)` — `(should_render, should_broadcast)`
- New command `status_sync`: deserialize JSON → replace `tab_statuses`, return `(true, false)`
- `set_status` / `clear_status`: return `(true, true)`
- Other commands (`rerun`, `notify`, `pipe`): return `(result, false)`

**`pipe()` method (zjstatus.rs):**
- After `parse_protocol`, if `should_broadcast` → call `broadcast_statuses()`

**`handle_event` for `TabUpdate` (zjstatus.rs):**
- If `tab_info.len() > self.prev_tab_count` → call `broadcast_statuses()`
- Update `self.prev_tab_count = tab_info.len()`

**`handle_event` for `PaneUpdate` (zjstatus.rs):**
- On first call: find own pane by `get_plugin_ids().plugin_id`, save `plugin_url`

**New function `broadcast_statuses()` (zjstatus.rs):**
- Serialize `tab_statuses` to JSON
- Send via `pipe_message_to_plugin(MessageToPlugin { plugin_url, message_name: "zjstatus", message_payload: "zjstatus::status_sync::JSON" })`

## Future Work

- Persist statuses across sessions (file-based storage) — separate PR
- Refactor `tab_statuses` key from `tab.position` to stable identifier (pane_id) — task #6
