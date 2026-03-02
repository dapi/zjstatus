# Tab Status Placeholder

zjstatus supports a `{status}` placeholder in tab templates, allowing you to display emoji indicators on tabs via pipe commands. This is useful for showing the state of running processes (e.g., AI agent working, build status, task completion).

## Configuration

Add `{status}` to your tab templates in the layout file:

```kdl
tab_active   "#[fg=#89B4FA,bold] {status}{name} "
tab_normal   "#[fg=#6C7086] {status}{name} "
```

When a status is set, it replaces `{status}` with the emoji string. When no status is set, `{status}` is replaced with an empty string.

**Tip:** Place `{status}` before `{name}` without a space, and include the space in the emoji string itself (e.g., `"🤖 "`). This avoids a leading space when no status is set.

## Pipe Commands

### Set status

```bash
zellij pipe --name zjstatus -- "zjstatus::set_status::${ZELLIJ_PANE_ID}::🤖"
```

Sets the emoji status on the tab containing the specified pane. The `pane_id` is automatically resolved to its parent tab.

### Clear status

```bash
zellij pipe --name zjstatus -- "zjstatus::clear_status::${ZELLIJ_PANE_ID}"
```

Removes the status from the tab containing the specified pane.

## CLI Helper

The [`zellij-tab-status`](../scripts/zellij-tab-status) script provides a convenient interface:

```bash
zellij-tab-status 🤖        # set emoji status on current tab
zellij-tab-status "🤖 "     # set emoji with trailing space
zellij-tab-status --clear   # remove status from current tab
```

The script uses `$ZELLIJ_PANE_ID` from the environment, so it works automatically inside any Zellij pane.

## Behavior

- Status is a property of a **tab**, not a pane. Multiple panes in the same tab share one status.
- Last write wins: setting status from any pane in a tab overwrites the previous value.
- Statuses are stored in memory only. They are lost when zjstatus is reloaded or Zellij restarts.
- When a tab is closed, its status is automatically cleaned up.
- Empty emoji in `set_status` is treated as `clear_status`.
- `clear_status` on a tab without status is a no-op (idempotent).

## Example

Full tab configuration with status support:

```kdl
pane size=1 borderless=true {
    plugin location="file:path/to/zjstatus.wasm" {
        // ...
        tab_active              "#[fg=#89B4FA,bold] {status}{name} "
        tab_active_fullscreen   "#[fg=#89B4FA,bold] {status}{name} 󰊓 "
        tab_active_sync         "#[fg=#89B4FA,bold] {status}{name} 󰓦 "
        tab_normal              "#[fg=#6C7086] {status}{name} "
        tab_normal_fullscreen   "#[fg=#6C7086] {status}{name} 󰊓 "
        tab_normal_sync         "#[fg=#6C7086] {status}{name} 󰓦 "
        // ...
    }
}
```

Result with status set (`zellij-tab-status "🤖 "`):
```
🤖 my-tab
```

Result without status:
```
my-tab
```
