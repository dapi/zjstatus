# Tab Status Persistence Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Persist `tab_statuses` to disk so they survive Zellij session restarts.

**Architecture:** Write `tab_statuses` as JSON to `/host/.cache/zjstatus/{session_name}.json` after every mutation. Load on first `ModeUpdate` when `session_name` becomes available. Reuse existing `serialize_tab_statuses`/`deserialize_tab_statuses`.

**Tech Stack:** Rust, `std::fs`, existing manual JSON serialization in `pipe.rs`.

**Design doc:** `docs/plans/2026-03-03-tab-status-persistence-design.md`

---

### Task 1: Make `deserialize_tab_statuses` public

**Files:**
- Modify: `src/pipe.rs:61`

**Step 1: Change visibility**

```rust
pub fn deserialize_tab_statuses(json: &str) -> Option<BTreeMap<usize, String>> {
```

**Step 2: Verify build**

Run: `cargo nextest run --lib`
Expected: all 44 tests pass (no behavior change).

**Step 3: Commit**

```
refactor: make deserialize_tab_statuses public for persistence
```

---

### Task 2: Add `save_statuses` and `load_statuses` methods

**Files:**
- Modify: `src/bin/zjstatus.rs:172-224` (`impl State` block)

**Step 1: Add `statuses_path` helper**

Add to `impl State` (after `broadcast_statuses`):

```rust
fn statuses_path(&self) -> Option<std::path::PathBuf> {
    let session_name = self.state.mode.session_name.as_ref()?;
    if session_name.is_empty() {
        return None;
    }
    Some(
        std::path::PathBuf::from("/host/.cache/zjstatus")
            .join(format!("{}.json", session_name)),
    )
}
```

**Step 2: Add `save_statuses`**

```rust
fn save_statuses(&self) {
    let Some(path) = self.statuses_path() else {
        return;
    };
    let json = pipe::serialize_tab_statuses(&self.state.tab_statuses);
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            tracing::warn!(error = %e, "failed to create zjstatus cache dir");
            return;
        }
    }
    if let Err(e) = std::fs::write(&path, &json) {
        tracing::warn!(error = %e, path = %path.display(), "failed to save tab_statuses");
    }
}
```

**Step 3: Add `load_statuses`**

```rust
fn load_statuses(&mut self) {
    let Some(path) = self.statuses_path() else {
        return;
    };
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return, // file doesn't exist yet — normal on first run
    };
    if let Some(loaded) = pipe::deserialize_tab_statuses(&content) {
        if !loaded.is_empty() {
            self.state.tab_statuses = loaded;
            tracing::debug!(path = %path.display(), "loaded tab_statuses from disk");
        }
    } else {
        tracing::warn!(path = %path.display(), "failed to parse tab_statuses file");
    }
}
```

**Step 4: Verify build**

Run: `cargo build --target wasm32-wasip1 --release`
Expected: compiles (methods not yet called).

**Step 5: Commit**

```
feat: add save_statuses and load_statuses methods
```

---

### Task 3: Wire up save on mutation and load on ModeUpdate

**Files:**
- Modify: `src/bin/zjstatus.rs:110-123` (`pipe()` method)
- Modify: `src/bin/zjstatus.rs:239-248` (`ModeUpdate` handler)
- Modify: `src/bin/zjstatus.rs:23-34` (`State` struct — add `statuses_loaded` flag)

**Step 1: Add `statuses_loaded` flag to State**

```rust
#[derive(Default)]
struct State {
    // ...existing fields...
    prev_tab_count: usize,
    statuses_loaded: bool,
}
```

**Step 2: Wire up load in ModeUpdate handler**

In `Event::ModeUpdate`, after `self.state.mode = mode_info;`:

```rust
Event::ModeUpdate(mode_info) => {
    tracing::Span::current().record("event_type", "Event::ModeUpdate");
    tracing::debug!(mode = ?mode_info.mode);
    tracing::debug!(mode = ?mode_info.session_name);

    self.state.mode = mode_info;
    self.state.cache_mask = UpdateEventMask::Mode as u8;

    if !self.statuses_loaded {
        self.statuses_loaded = true;
        self.load_statuses();
        if !self.state.tab_statuses.is_empty() {
            self.broadcast_statuses();
        }
    }

    should_render = true;
}
```

**Step 3: Wire up save in `pipe()` method**

After broadcast logic, save if statuses changed:

```rust
fn pipe(&mut self, pipe_message: PipeMessage) -> bool {
    let mut should_render = false;

    if let Some(input) = pipe_message.payload {
        let (render, broadcast) = pipe::parse_protocol(&mut self.state, &input);
        should_render = render;

        if broadcast {
            self.broadcast_statuses();
        }

        if render {
            self.save_statuses();
        }
    }

    should_render
}
```

Note: we save on `render` (not just `broadcast`) because `status_sync` sets `render=true, broadcast=false` and we want to persist the synced data too.

**Step 4: Build and clippy**

Run: `cargo build --target wasm32-wasip1 --release && cargo clippy --all-features --lib`
Expected: compiles, no warnings.

**Step 5: Commit**

```
feat: wire up tab_statuses persistence on mutation and session load
```

---

### Task 4: Manual integration test

**Step 1: Build and install**

Run: `make install`

**Step 2: Grant permissions if needed**

Run: `make grant-permissions` (then press `y`)

**Step 3: Test persistence**

1. Open zellij session: `zellij -l ai-default -s test-persist`
2. Set status: `zellij-tab-status 🤖`
3. Verify status shows on tab
4. Kill session: `zellij kill-session test-persist`
5. Restart: `zellij -l ai-default -s test-persist`
6. Verify status 🤖 still shows on tab

**Step 4: Verify file exists**

Run: `cat /tmp/.cache/zjstatus/test-persist.json`
Expected: `{"0":"🤖"}` (or similar)

**Step 5: Clean up**

Run: `zellij kill-session test-persist`
