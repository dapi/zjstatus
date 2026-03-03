use std::collections::BTreeMap;
use std::ops::Sub;

use chrono::{Duration, Local};

use zellij_tile::prelude::PaneManifest;

use crate::{
    config::ZellijState,
    widgets::{command::TIMESTAMP_FORMAT, notification},
};

/// Parses the line protocol and updates the state accordingly
///
/// The protocol is as follows:
///
/// zjstatus::command_name::args
///
/// It first starts with `zjstatus` as a prefix to indicate that the line is
/// used for the line protocol and zjstatus should parse it. It is followed
/// by the command name and then the arguments. The following commands are
/// available:
///
/// - `rerun` - Reruns the command with the given name (like in the config) as
///             argument. E.g. `zjstatus::rerun::command_1`
///
/// The function returns a boolean indicating whether the state has been
/// changed and the UI should be re-rendered.
#[tracing::instrument(skip(state))]
pub fn parse_protocol(state: &mut ZellijState, input: &str) -> (bool, bool) {
    tracing::debug!("parsing protocol");
    let lines = input.split('\n').collect::<Vec<&str>>();

    let mut should_render = false;
    let mut should_broadcast = false;
    for line in lines {
        let (line_renders, line_broadcasts) = process_line(state, line);

        if line_renders {
            should_render = true;
        }
        if line_broadcasts {
            should_broadcast = true;
        }
    }

    (should_render, should_broadcast)
}

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

pub fn deserialize_tab_statuses(json: &str) -> Option<BTreeMap<usize, String>> {
    let json = json.trim();
    if !json.starts_with('{') || !json.ends_with('}') {
        return None;
    }
    let inner = &json[1..json.len() - 1];
    if inner.trim().is_empty() {
        return Some(BTreeMap::new());
    }
    let mut map = BTreeMap::new();
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

fn resolve_tab_index(panes: &PaneManifest, pane_id: u32) -> Option<usize> {
    for (tab_index, pane_list) in &panes.panes {
        if pane_list.iter().any(|p| p.id == pane_id) {
            return Some(*tab_index);
        }
    }
    None
}

#[tracing::instrument(skip_all)]
fn process_line(state: &mut ZellijState, line: &str) -> (bool, bool) {
    let parts = line.split("::").collect::<Vec<&str>>();

    if parts.len() < 3 {
        return (false, false);
    }

    if parts[0] != "zjstatus" {
        return (false, false);
    }

    tracing::debug!("command: {}", parts[1]);

    let mut should_render = false;
    let mut should_broadcast = false;
    #[allow(clippy::single_match)]
    match parts[1] {
        "rerun" => {
            rerun_command(state, parts[2]);

            should_render = true;
        }
        "notify" => {
            notify(state, parts[2]);

            should_render = true;
        }
        "pipe" => {
            if parts.len() < 4 {
                return (false, false);
            }

            pipe(state, parts[2], parts[3]);

            should_render = true;
        }
        "set_status" => {
            if parts.len() < 4 {
                return (false, false);
            }
            let pane_id = match parts[2].parse::<u32>() {
                Ok(id) => id,
                Err(_) => {
                    tracing::warn!("set_status: invalid pane_id: {}", parts[2]);
                    return (false, false);
                }
            };
            let emoji = parts[3];
            if let Some(tab_idx) = resolve_tab_index(&state.panes, pane_id) {
                if emoji.is_empty() {
                    state.tab_statuses.remove(&tab_idx);
                } else {
                    state.tab_statuses.insert(tab_idx, emoji.to_string());
                }
                should_render = true;
                should_broadcast = true;
            }
        }
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
        "status_request" => {
            // New instance requesting statuses from siblings.
            // Don't render, just trigger broadcast so siblings respond with status_sync.
            should_broadcast = true;
        }
        "clear_status" => {
            if parts.len() < 3 {
                return (false, false);
            }
            let pane_id = match parts[2].parse::<u32>() {
                Ok(id) => id,
                Err(_) => {
                    tracing::warn!("clear_status: invalid pane_id: {}", parts[2]);
                    return (false, false);
                }
            };
            if let Some(tab_idx) = resolve_tab_index(&state.panes, pane_id) {
                state.tab_statuses.remove(&tab_idx);
                should_render = true;
                should_broadcast = true;
            }
        }
        _ => {
            tracing::debug!("unknown zjstatus command: {}", parts[1]);
        }
    }

    (should_render, should_broadcast)
}

fn pipe(state: &mut ZellijState, name: &str, content: &str) {
    tracing::debug!("saving pipe result {name} {content}");
    state
        .pipe_results
        .insert(name.to_owned(), content.to_owned());
}

fn notify(state: &mut ZellijState, message: &str) {
    state.incoming_notification = Some(notification::Message {
        body: message.to_string(),
        received_at: Local::now(),
    });
}

fn rerun_command(state: &mut ZellijState, command_name: &str) {
    let command_result = state.command_results.get(command_name);

    if command_result.is_none() {
        return;
    }

    let mut command_result = command_result.unwrap().clone();

    let ts = Sub::<Duration>::sub(Local::now(), Duration::try_days(1).unwrap());

    command_result.context.insert(
        "timestamp".to_string(),
        ts.format(TIMESTAMP_FORMAT).to_string(),
    );

    state.command_results.remove(command_name);
    state
        .command_results
        .insert(command_name.to_string(), command_result.clone());
}

#[cfg(test)]
mod test {
    use std::collections::HashMap;

    use zellij_tile::prelude::{PaneInfo, PaneManifest};

    use crate::config::ZellijState;

    use super::{deserialize_tab_statuses, process_line, resolve_tab_index, serialize_tab_statuses};

    fn make_state_with_panes() -> ZellijState {
        let mut panes = HashMap::new();
        panes.insert(
            0,
            vec![
                PaneInfo {
                    id: 10,
                    ..PaneInfo::default()
                },
                PaneInfo {
                    id: 11,
                    ..PaneInfo::default()
                },
            ],
        );
        panes.insert(
            1,
            vec![PaneInfo {
                id: 20,
                ..PaneInfo::default()
            }],
        );

        let mut state = ZellijState::default();
        state.panes = PaneManifest { panes };
        state
    }

    #[test]
    fn test_resolve_tab_index_found() {
        let state = make_state_with_panes();
        assert_eq!(resolve_tab_index(&state.panes, 20), Some(1));
    }

    #[test]
    fn test_resolve_tab_index_not_found() {
        let state = make_state_with_panes();
        assert_eq!(resolve_tab_index(&state.panes, 99), None);
    }

    #[test]
    fn test_set_status_valid() {
        let mut state = make_state_with_panes();
        let (result, _) = process_line(&mut state, "zjstatus::set_status::10::🤖");
        assert!(result);
        assert_eq!(state.tab_statuses.get(&0), Some(&"🤖".to_string()));
    }

    #[test]
    fn test_set_status_invalid_pane_id() {
        let mut state = make_state_with_panes();
        let (result, _) = process_line(&mut state, "zjstatus::set_status::abc::🤖");
        assert!(!result);
        assert!(state.tab_statuses.is_empty());
    }

    #[test]
    fn test_set_status_unknown_pane_id() {
        let mut state = make_state_with_panes();
        let (result, _) = process_line(&mut state, "zjstatus::set_status::99::🤖");
        assert!(!result);
        assert!(state.tab_statuses.is_empty());
    }

    #[test]
    fn test_set_status_empty_emoji_clears() {
        let mut state = make_state_with_panes();
        state.tab_statuses.insert(0, "🤖".to_string());
        let (result, _) = process_line(&mut state, "zjstatus::set_status::10::");
        assert!(result);
        assert!(state.tab_statuses.get(&0).is_none());
    }

    #[test]
    fn test_clear_status() {
        let mut state = make_state_with_panes();
        state.tab_statuses.insert(1, "✅".to_string());
        let (result, _) = process_line(&mut state, "zjstatus::clear_status::20");
        assert!(result);
        assert!(state.tab_statuses.get(&1).is_none());
    }

    #[test]
    fn test_clear_status_idempotent() {
        let mut state = make_state_with_panes();
        let (result, _) = process_line(&mut state, "zjstatus::clear_status::20");
        assert!(result);
        assert!(state.tab_statuses.is_empty());
    }

    #[test]
    fn test_set_status_too_few_parts() {
        let mut state = make_state_with_panes();
        let (result, _) = process_line(&mut state, "zjstatus::set_status::10");
        assert!(!result);
    }

    #[test]
    fn test_clear_status_invalid_pane_id() {
        let mut state = make_state_with_panes();
        state.tab_statuses.insert(0, "✅".to_string());
        let (result, _) = process_line(&mut state, "zjstatus::clear_status::abc");
        assert!(!result);
        assert_eq!(state.tab_statuses.get(&0), Some(&"✅".to_string()));
    }

    #[test]
    fn test_set_status_returns_should_broadcast_true() {
        let mut state = make_state_with_panes();
        let (should_render, should_broadcast) =
            process_line(&mut state, "zjstatus::set_status::10::🤖");
        assert!(should_render);
        assert!(should_broadcast);
    }

    #[test]
    fn test_clear_status_returns_should_broadcast_true() {
        let mut state = make_state_with_panes();
        state.tab_statuses.insert(1, "✅".to_string());
        let (should_render, should_broadcast) =
            process_line(&mut state, "zjstatus::clear_status::20");
        assert!(should_render);
        assert!(should_broadcast);
    }

    #[test]
    fn test_status_sync_replaces_local_statuses() {
        let mut state = make_state_with_panes();
        state.tab_statuses.insert(0, "old".to_string());
        let (should_render, should_broadcast) =
            process_line(&mut state, "zjstatus::status_sync::{\"0\":\"🤖\",\"1\":\"✅\"}");
        assert!(should_render);
        assert!(!should_broadcast);
        assert_eq!(state.tab_statuses.get(&0), Some(&"🤖".to_string()));
        assert_eq!(state.tab_statuses.get(&1), Some(&"✅".to_string()));
    }

    #[test]
    fn test_status_sync_clears_missing_keys() {
        let mut state = make_state_with_panes();
        state.tab_statuses.insert(0, "🤖".to_string());
        state.tab_statuses.insert(1, "✅".to_string());
        let (should_render, _) =
            process_line(&mut state, "zjstatus::status_sync::{\"1\":\"✅\"}");
        assert!(should_render);
        assert!(state.tab_statuses.get(&0).is_none());
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
        assert!(!should_render);
        assert_eq!(state.tab_statuses.get(&0), Some(&"🤖".to_string()));
    }

    #[test]
    fn test_serialize_tab_statuses() {
        let mut map = std::collections::BTreeMap::new();
        map.insert(0, "🤖".to_string());
        map.insert(1, "✅".to_string());
        assert_eq!(serialize_tab_statuses(&map), "{\"0\":\"🤖\",\"1\":\"✅\"}");
    }

    #[test]
    fn test_serialize_empty_map() {
        let map = std::collections::BTreeMap::new();
        assert_eq!(serialize_tab_statuses(&map), "{}");
    }

    /// Demonstrates the root cause of status desync between zjstatus instances.
    ///
    /// In production, each tab has its own zjstatus plugin instance with independent state.
    /// When a pipe message (set_status) is sent, only EXISTING instances receive it.
    /// A new instance (created with a new tab) starts with empty tab_statuses and
    /// never receives messages that were sent before its creation.
    ///
    /// Repro: create session → set status on tab 0 → create tab 1 → click tab 1 →
    /// tab 0's status disappears (because tab 1's zjstatus has empty tab_statuses).
    #[test]
    fn test_new_instance_missing_statuses_from_earlier_pipes() {
        // Instance 0 exists from the start, receives set_status for tab 0
        let mut instance_0 = make_state_with_panes();
        let _ = process_line(&mut instance_0, "zjstatus::set_status::10::🤖");
        assert_eq!(
            instance_0.tab_statuses.get(&0),
            Some(&"🤖".to_string()),
            "instance_0 correctly stores the status"
        );

        // Instance 1 is created later (new tab) — starts with empty state.
        // It has the same PaneManifest (Zellij sends PaneUpdate to all plugins),
        // but it never received the set_status pipe message.
        let instance_1 = make_state_with_panes();

        // BUG: instance_1 doesn't know about tab 0's status.
        // When tab 1 is active, instance_1 renders and tab 0 appears without status.
        assert!(
            instance_1.tab_statuses.is_empty(),
            "new instance has no statuses — this is the bug: \
             when tab 1 is active, instance_1 renders and tab 0's status is lost"
        );

        // Both instances should ideally agree on tab_statuses,
        // but they diverge because there's no sync mechanism.
        assert_ne!(
            instance_0.tab_statuses, instance_1.tab_statuses,
            "instances have divergent tab_statuses — root cause of the visual glitch"
        );
    }

    /// Verifies that status_sync resolves the desync between instances.
    ///
    /// When instance_0 updates tab_statuses, it broadcasts via status_sync.
    /// A new instance_1 receives the sync and now agrees with instance_0.
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

    #[test]
    fn test_status_request_triggers_broadcast_without_render() {
        let mut state = make_state_with_panes();
        state.tab_statuses.insert(0, "🤖".to_string());

        let (should_render, should_broadcast) =
            process_line(&mut state, "zjstatus::status_request::_");

        assert!(!should_render, "status_request should not render");
        assert!(should_broadcast, "status_request should trigger broadcast");
        // State unchanged
        assert_eq!(state.tab_statuses.get(&0), Some(&"🤖".to_string()));
    }

    #[test]
    fn test_status_request_on_empty_instance() {
        let mut state = make_state_with_panes();

        let (should_render, should_broadcast) =
            process_line(&mut state, "zjstatus::status_request::_");

        assert!(!should_render);
        assert!(should_broadcast);
        assert!(state.tab_statuses.is_empty());
    }

    /// Full sync flow: new instance sends status_request → sibling responds with status_sync.
    #[test]
    fn test_pull_based_sync_flow() {
        // Instance 0 has statuses
        let mut instance_0 = make_state_with_panes();
        let _ = process_line(&mut instance_0, "zjstatus::set_status::10::🤖");

        // Instance 1 starts, sends status_request
        // Instance 0 receives status_request → should_broadcast=true → broadcasts status_sync
        let (_, should_broadcast) =
            process_line(&mut instance_0, "zjstatus::status_request::_");
        assert!(should_broadcast);

        // Simulate broadcast: instance_0 serializes and sends status_sync
        let sync_payload = serialize_tab_statuses(&instance_0.tab_statuses);
        let sync_line = format!("zjstatus::status_sync::{}", sync_payload);

        // Instance 1 receives status_sync
        let mut instance_1 = make_state_with_panes();
        let (should_render, should_broadcast) = process_line(&mut instance_1, &sync_line);
        assert!(should_render);
        assert!(!should_broadcast, "sync must not re-broadcast");
        assert_eq!(instance_0.tab_statuses, instance_1.tab_statuses);
    }
}
