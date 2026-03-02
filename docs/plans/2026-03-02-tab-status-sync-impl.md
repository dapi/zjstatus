# Tab Status Sync Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Synchronize `tab_statuses` between zjstatus plugin instances within a Zellij session so tab statuses don't disappear when switching tabs.

**Architecture:** When any instance updates `tab_statuses`, it broadcasts the full map to all sibling instances via `pipe_message_to_plugin`. New instances receive statuses when `TabUpdate` triggers a broadcast. Cycle prevention: broadcasts only on CLI/Keybind source, not on Plugin source.

**Tech Stack:** Rust, zellij-tile 0.43.1, `pipe_message_to_plugin` API, manual JSON serialization (no serde).

**Design doc:** `docs/plans/2026-03-02-tab-status-sync-design.md`

---

### Task 1: Change `process_line` return type to `(bool, bool)`

**Files:**
- Modify: `src/pipe.rs:29-43` (`parse_protocol`)
- Modify: `src/pipe.rs:55-133` (`process_line`)
- Modify: `src/pipe.rs:171-327` (all tests)

**Step 1: Write failing test for new return type**

Add to `src/pipe.rs` test module:

```rust
#[test]
fn test_set_status_returns_should_broadcast_true() {
    let mut state = make_state_with_panes();
    let (should_render, should_broadcast) = process_line(&mut state, "zjstatus::set_status::10::🤖");
    assert!(should_render);
    assert!(should_broadcast, "set_status from CLI should trigger broadcast");
}

#[test]
fn test_rerun_returns_should_broadcast_false() {
    let mut state = make_state_with_panes();
    state.command_results.insert("test_cmd".to_string(), crate::widgets::command::CommandResult {
        exit_code: Some(0),
        stdout: String::new(),
        stderr: String::new(),
        context: BTreeMap::new(),
    });
    let (should_render, should_broadcast) = process_line(&mut state, "zjstatus::rerun::test_cmd");
    assert!(should_render);
    assert!(!should_broadcast, "rerun should not trigger broadcast");
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo nextest run --lib test_set_status_returns_should_broadcast`
Expected: compile error — `process_line` returns `bool`, not tuple.

**Step 3: Change `process_line` to return `(bool, bool)`**

In `src/pipe.rs`, change `process_line` signature and body:

```rust
fn process_line(state: &mut ZellijState, line: &str) -> (bool, bool) {
    // ...existing parsing...
    let mut should_render = false;
    let mut should_broadcast = false;

    match parts[1] {
        "rerun" => { rerun_command(state, parts[2]); should_render = true; }
        "notify" => { notify(state, parts[2]); should_render = true; }
        "pipe" => {
            if parts.len() < 4 { return (false, false); }
            pipe(state, parts[2], parts[3]);
            should_render = true;
        }
        "set_status" => {
            // ...existing logic...
            if let Some(tab_idx) = resolve_tab_index(&state.panes, pane_id) {
                // ...existing insert/remove...
                should_render = true;
                should_broadcast = true;
            }
        }
        "clear_status" => {
            // ...existing logic...
            if let Some(tab_idx) = resolve_tab_index(&state.panes, pane_id) {
                state.tab_statuses.remove(&tab_idx);
                should_render = true;
                should_broadcast = true;
            }
        }
        _ => { tracing::debug!("unknown zjstatus command: {}", parts[1]); }
    }

    (should_render, should_broadcast)
}
```

Update `parse_protocol` to aggregate both bools:

```rust
pub fn parse_protocol(state: &mut ZellijState, input: &str) -> (bool, bool) {
    let lines = input.split('\n').collect::<Vec<&str>>();
    let mut should_render = false;
    let mut should_broadcast = false;

    for line in lines {
        let (line_render, line_broadcast) = process_line(state, line);
        if line_render { should_render = true; }
        if line_broadcast { should_broadcast = true; }
    }

    (should_render, should_broadcast)
}
```

Fix all existing tests to destructure `(bool, bool)` instead of `bool`. The test assertions on `should_render` remain the same; add `_` for `should_broadcast` in tests that don't care.

**Step 4: Run all tests**

Run: `cargo nextest run --lib`
Expected: all pass.

**Step 5: Commit**

```
feat: change process_line return to (should_render, should_broadcast)
```

---

### Task 2: Add `status_sync` command with JSON serialization

**Files:**
- Modify: `src/pipe.rs:55-133` (`process_line` — add `status_sync` arm)
- Add serialization/deserialization helper functions to `src/pipe.rs`

**Step 1: Write failing tests**

```rust
#[test]
fn test_status_sync_replaces_local_statuses() {
    let mut state = make_state_with_panes();
    state.tab_statuses.insert(0, "old".to_string());

    let (should_render, should_broadcast) =
        process_line(&mut state, r#"zjstatus::status_sync::{"0":"🤖","1":"✅"}"#);

    assert!(should_render);
    assert!(!should_broadcast, "status_sync must NOT trigger broadcast (cycle prevention)");
    assert_eq!(state.tab_statuses.get(&0), Some(&"🤖".to_string()));
    assert_eq!(state.tab_statuses.get(&1), Some(&"✅".to_string()));
}

#[test]
fn test_status_sync_clears_missing_keys() {
    let mut state = make_state_with_panes();
    state.tab_statuses.insert(0, "🤖".to_string());
    state.tab_statuses.insert(1, "✅".to_string());

    let (should_render, _) =
        process_line(&mut state, r#"zjstatus::status_sync::{"1":"✅"}"#);

    assert!(should_render);
    assert!(state.tab_statuses.get(&0).is_none(), "key 0 should be removed");
    assert_eq!(state.tab_statuses.get(&1), Some(&"✅".to_string()));
}

#[test]
fn test_status_sync_empty_map() {
    let mut state = make_state_with_panes();
    state.tab_statuses.insert(0, "🤖".to_string());

    let (should_render, _) = process_line(&mut state, "zjstatus::status_sync::{}");

    assert!(should_render);
    assert!(state.tab_statuses.is_empty());
}

#[test]
fn test_status_sync_invalid_json() {
    let mut state = make_state_with_panes();
    state.tab_statuses.insert(0, "🤖".to_string());

    let (should_render, _) = process_line(&mut state, "zjstatus::status_sync::not_json");

    assert!(!should_render, "invalid JSON should not render");
    assert_eq!(state.tab_statuses.get(&0), Some(&"🤖".to_string()), "original state preserved");
}

#[test]
fn test_serialize_tab_statuses() {
    let mut map = BTreeMap::new();
    map.insert(0_usize, "🤖".to_string());
    map.insert(1_usize, "✅".to_string());
    let json = serialize_tab_statuses(&map);
    // BTreeMap is ordered, so output is deterministic
    assert_eq!(json, r#"{"0":"🤖","1":"✅"}"#);
}

#[test]
fn test_serialize_empty_map() {
    let map = BTreeMap::new();
    assert_eq!(serialize_tab_statuses(&map), "{}");
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo nextest run --lib test_status_sync`
Expected: compile error — `status_sync` arm and `serialize_tab_statuses` don't exist.

**Step 3: Implement serialization helpers and `status_sync` command**

Add to `src/pipe.rs` (public, needed by zjstatus.rs for broadcast):

```rust
pub fn serialize_tab_statuses(map: &BTreeMap<usize, String>) -> String {
    let entries: Vec<String> = map
        .iter()
        .map(|(k, v)| {
            let escaped = v.replace('\\', "\\\\").replace('"', "\\\"");
            format!("\"{}\":\"{}\"", k, escaped)
        })
        .collect();
    format!("{{{}}}", entries.join(","))
}

fn deserialize_tab_statuses(json: &str) -> Option<BTreeMap<usize, String>> {
    let json = json.trim();
    if !json.starts_with('{') || !json.ends_with('}') {
        return None;
    }
    let inner = &json[1..json.len() - 1];
    if inner.trim().is_empty() {
        return Some(BTreeMap::new());
    }

    let mut map = BTreeMap::new();
    // Simple parser: split on "," between entries, parse "key":"value"
    for part in inner.split(',') {
        let part = part.trim();
        let kv: Vec<&str> = part.splitn(2, ':').collect();
        if kv.len() != 2 {
            return None;
        }
        let key = kv[0].trim().trim_matches('"');
        let value = kv[1].trim().trim_matches('"');
        let idx = key.parse::<usize>().ok()?;
        map.insert(idx, value.to_string());
    }
    Some(map)
}
```

Add `status_sync` arm in `process_line`:

```rust
"status_sync" => {
    if parts.len() < 3 {
        return (false, false);
    }
    if let Some(new_statuses) = deserialize_tab_statuses(parts[2]) {
        state.tab_statuses = new_statuses;
        should_render = true;
        // should_broadcast stays false — cycle prevention
    } else {
        tracing::warn!("status_sync: invalid JSON: {}", parts[2]);
    }
}
```

**Step 4: Run all tests**

Run: `cargo nextest run --lib`
Expected: all pass.

**Step 5: Commit**

```
feat: add status_sync command with JSON serialization
```

---

### Task 3: Extract plugin_url from PaneManifest

**Files:**
- Modify: `src/bin/zjstatus.rs:24-32` (State struct — add `plugin_url`)
- Modify: `src/bin/zjstatus.rs:202-224` (PaneUpdate handler)

**Step 1: Add `plugin_url` field to State**

In `src/bin/zjstatus.rs`, add to `State` struct:

```rust
struct State {
    // ...existing fields...
    plugin_url: Option<String>,
}
```

**Step 2: Extract plugin_url on PaneUpdate**

In the `Event::PaneUpdate` handler, after `self.state.panes = pane_info;`, add:

```rust
if self.plugin_url.is_none() {
    let my_plugin_id = get_plugin_ids().plugin_id;
    for (_tab_idx, pane_list) in &self.state.panes.panes {
        if let Some(pane) = pane_list.iter().find(|p| p.is_plugin && p.id == my_plugin_id) {
            self.plugin_url = pane.plugin_url.clone();
            tracing::debug!(plugin_url = ?self.plugin_url, "discovered own plugin_url");
            break;
        }
    }
}
```

**Step 3: Build and verify**

Run: `cargo build --target wasm32-wasip1 --release`
Expected: compiles without errors.

**Step 4: Commit**

```
feat: extract and cache plugin_url from PaneManifest
```

---

### Task 4: Implement `broadcast_statuses` and wire it up

**Files:**
- Modify: `src/bin/zjstatus.rs:24-32` (State — add `prev_tab_count`)
- Modify: `src/bin/zjstatus.rs:107-129` (`pipe()` method)
- Modify: `src/bin/zjstatus.rs:289-303` (`TabUpdate` handler)
- Add `broadcast_statuses()` method to `State`

**Step 1: Add `prev_tab_count` to State**

```rust
struct State {
    // ...existing fields...
    plugin_url: Option<String>,
    prev_tab_count: usize,
}
```

**Step 2: Implement `broadcast_statuses`**

Add method to `impl State`:

```rust
fn broadcast_statuses(&self) {
    let Some(url) = &self.plugin_url else {
        tracing::debug!("broadcast_statuses: plugin_url not yet discovered, skipping");
        return;
    };

    if self.state.tab_statuses.is_empty() {
        return;
    }

    let json = pipe::serialize_tab_statuses(&self.state.tab_statuses);
    let payload = format!("zjstatus::status_sync::{}", json);

    pipe_message_to_plugin(
        MessageToPlugin::new("zjstatus")
            .with_plugin_url(url)
            .with_message_payload(&payload),
    );

    tracing::debug!(payload = %payload, "broadcast tab_statuses to siblings");
}
```

**Step 3: Wire up `pipe()` method**

Change `pipe()` to use `(should_render, should_broadcast)`:

```rust
fn pipe(&mut self, pipe_message: PipeMessage) -> bool {
    let mut should_render = false;

    if let Some(input) = pipe_message.payload {
        let (render, broadcast) = pipe::parse_protocol(&mut self.state, &input);
        should_render = render;

        if broadcast {
            self.broadcast_statuses();
        }
    }

    should_render
}
```

**Step 4: Wire up TabUpdate handler**

In `Event::TabUpdate`, after existing code:

```rust
Event::TabUpdate(tab_info) => {
    // ...existing code...
    self.state.tabs = tab_info;

    // Broadcast statuses to new instances when tab count grows
    if self.state.tabs.len() > self.prev_tab_count && !self.state.tab_statuses.is_empty() {
        self.broadcast_statuses();
    }
    self.prev_tab_count = self.state.tabs.len();

    // ...existing cleanup and should_render...
}
```

**Step 5: Build and clippy**

Run: `cargo build --target wasm32-wasip1 --release && cargo clippy --all-features --lib`
Expected: compiles, no clippy warnings.

**Step 6: Commit**

```
feat: broadcast tab_statuses to sibling zjstatus instances
```

---

### Task 5: Update the desync test to document expected behavior

**Files:**
- Modify: `src/pipe.rs` — test `test_new_instance_missing_statuses_from_earlier_pipes`

**Step 1: Add sync test alongside existing one**

Keep the original desync test as documentation of the root cause. Add new test:

```rust
#[test]
fn test_status_sync_resolves_instance_desync() {
    // Instance 0 receives set_status
    let mut instance_0 = make_state_with_panes();
    let (_, should_broadcast) = process_line(&mut instance_0, "zjstatus::set_status::10::🤖");
    assert!(should_broadcast);

    // Simulate broadcast: serialize instance_0's statuses
    let sync_payload = serialize_tab_statuses(&instance_0.tab_statuses);
    let sync_line = format!("zjstatus::status_sync::{}", sync_payload);

    // Instance 1 (new tab) receives the sync
    let mut instance_1 = make_state_with_panes();
    assert!(instance_1.tab_statuses.is_empty(), "starts empty");

    let (should_render, should_broadcast) = process_line(&mut instance_1, &sync_line);
    assert!(should_render);
    assert!(!should_broadcast, "sync must not re-broadcast");

    // Now both instances agree
    assert_eq!(instance_0.tab_statuses, instance_1.tab_statuses);
    assert_eq!(instance_1.tab_statuses.get(&0), Some(&"🤖".to_string()));
}
```

**Step 2: Run all tests**

Run: `cargo nextest run --lib`
Expected: all pass.

**Step 3: Commit**

```
test: add sync resolution test for tab_statuses desync
```

---

### Task 6: Manual integration test

**Step 1: Build and install plugin**

Run: `make install` (or `cargo build --target wasm32-wasip1 --release && cp target/wasm32-wasip1/release/zjstatus.wasm ~/.config/zellij/plugins/`)

**Step 2: Test in live session**

1. Open zellij session with zjstatus layout
2. Set status on tab 0: `zellij-tab-status 🤖`
3. Create new tab
4. Click on new tab — verify tab 0 still shows 🤖
5. Click back on tab 0 — verify status is still there
6. Clear status: `zellij-tab-status --clear`
7. Switch tabs — verify status cleared on all

**Step 3: Final commit if any fixes needed**
