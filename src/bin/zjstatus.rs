use zellij_tile::prelude::*;

use chrono::Local;
use std::{collections::BTreeMap, sync::Arc};
use uuid::Uuid;

use zjstatus::{
    config::{self, ModuleConfig, UpdateEventMask, ZellijState},
    frames, pipe,
    widgets::{
        command::{CommandResult, CommandWidget},
        datetime::DateTimeWidget,
        mode::ModeWidget,
        notification::NotificationWidget,
        pipe::PipeWidget,
        session::SessionWidget,
        swap_layout::SwapLayoutWidget,
        tabs::TabsWidget,
        widget::Widget,
    },
};

#[derive(Default)]
struct State {
    pending_events: Vec<Event>,
    got_permissions: bool,
    state: ZellijState,
    userspace_configuration: BTreeMap<String, String>,
    module_config: config::ModuleConfig,
    widget_map: BTreeMap<String, Arc<dyn Widget>>,
    err: Option<anyhow::Error>,
    plugin_url: Option<String>,
    prev_tab_count: usize,
    statuses_loaded: bool,
}

#[cfg(not(test))]
register_plugin!(State);

#[cfg(feature = "tracing")]
fn init_tracing() {
    use std::fs::OpenOptions;
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

    let file = OpenOptions::new().create(true).append(true).open("/host/.zjstatus.log");
    let file = match file {
        Ok(file) => file,
        Err(error) => panic!("Error: {:?}", error),
    };
    let debug_log = tracing_subscriber::fmt::layer().with_writer(Arc::new(file));

    tracing_subscriber::registry().with(debug_log).init();

    tracing::info!("tracing initialized");
}

impl ZellijPlugin for State {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        #[cfg(feature = "tracing")]
        init_tracing();

        // we need the ReadApplicationState permission to receive the ModeUpdate and TabUpdate
        // events
        // we need the RunCommands permission to run "cargo test" in a floating window
        request_permission(&[
            PermissionType::ReadApplicationState,
            PermissionType::ChangeApplicationState,
            PermissionType::RunCommands,
            PermissionType::MessageAndLaunchOtherPlugins,
        ]);

        subscribe(&[
            EventType::Mouse,
            EventType::ModeUpdate,
            EventType::PaneUpdate,
            EventType::PermissionRequestResult,
            EventType::TabUpdate,
            EventType::SessionUpdate,
            EventType::RunCommandResult,
        ]);

        self.module_config = match ModuleConfig::new(&configuration) {
            Ok(mc) => mc,
            Err(e) => {
                self.err = Some(e);
                return;
            }
        };
        self.widget_map = register_widgets(&configuration);
        self.userspace_configuration = configuration;
        self.pending_events = Vec::new();
        self.got_permissions = false;
        let uid = Uuid::new_v4();

        self.state = ZellijState {
            cols: 0,
            command_results: BTreeMap::new(),
            pipe_results: BTreeMap::new(),
            mode: ModeInfo::default(),
            panes: PaneManifest::default(),
            plugin_uuid: uid.to_string(),
            tabs: Vec::new(),
            sessions: Vec::new(),
            start_time: Local::now(),
            cache_mask: 0,
            incoming_notification: None,
            tab_statuses: BTreeMap::new(),
        };
    }

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

    #[tracing::instrument(skip_all, fields(event_type))]
    fn update(&mut self, event: Event) -> bool {
        if let Event::PermissionRequestResult(PermissionStatus::Granted) = event {
            self.got_permissions = true;

            while !self.pending_events.is_empty() {
                tracing::debug!("processing cached event");
                let ev = self.pending_events.pop();

                self.handle_event(ev.unwrap());
            }
        }

        if !self.got_permissions {
            tracing::debug!("caching event");
            self.pending_events.push(event);

            return false;
        }

        self.handle_event(event)
    }

    #[tracing::instrument(skip_all)]
    fn render(&mut self, _rows: usize, cols: usize) {
        if !self.got_permissions {
            return;
        }

        if let Some(err) = &self.err {
            println!("Error: {:?}", err);

            return;
        }

        self.state.cols = cols;

        tracing::debug!("{:?}", self.state.mode.session_name);

        let output = self
            .module_config
            .render_bar(self.state.clone(), self.widget_map.clone());

        print!("{}", output);
    }
}

impl State {
    fn sibling_plugin_ids(&self) -> Vec<u32> {
        let Some(my_url) = &self.plugin_url else {
            return Vec::new();
        };
        let my_id = get_plugin_ids().plugin_id;
        let mut ids = Vec::new();
        for (_tab_idx, pane_list) in &self.state.panes.panes {
            for pane in pane_list {
                if pane.is_plugin && pane.id != my_id {
                    if let Some(ref url) = pane.plugin_url {
                        if url == my_url {
                            ids.push(pane.id);
                        }
                    }
                }
            }
        }
        ids
    }

    fn send_to_siblings(&self, payload: &str) {
        let siblings = self.sibling_plugin_ids();
        tracing::debug!(siblings = ?siblings, "send_to_siblings");
        for id in siblings {
            pipe_message_to_plugin(
                MessageToPlugin::new("zjstatus")
                    .with_destination_plugin_id(id)
                    .with_payload(payload),
            );
        }
    }

    fn request_statuses_from_siblings(&self) {
        if self.plugin_url.is_none() {
            return;
        }
        self.send_to_siblings("zjstatus::status_request::_");
    }

    fn broadcast_statuses(&self) {
        if self.plugin_url.is_none() {
            return;
        }

        if self.state.tab_statuses.is_empty() {
            return;
        }

        let json = pipe::serialize_tab_statuses(&self.state.tab_statuses);
        let payload = format!("zjstatus::status_sync::{}", json);
        self.send_to_siblings(&payload);
    }

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

    fn load_statuses(&mut self) {
        let Some(path) = self.statuses_path() else {
            return;
        };
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => return,
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

    fn handle_event(&mut self, event: Event) -> bool {
        let mut should_render = false;
        match event {
            Event::Mouse(mouse_info) => {
                tracing::Span::current().record("event_type", "Event::Mouse");
                tracing::debug!(mouse = ?mouse_info);

                self.module_config.handle_mouse_action(
                    self.state.clone(),
                    mouse_info,
                    self.widget_map.clone(),
                );
            }
            Event::ModeUpdate(mode_info) => {
                tracing::Span::current().record("event_type", "Event::ModeUpdate");
                tracing::debug!(mode = ?mode_info.mode);
                tracing::debug!(mode = ?mode_info.session_name);

                self.state.mode = mode_info;
                self.state.cache_mask = UpdateEventMask::Mode as u8;

                should_render = true;
            }
            Event::PaneUpdate(pane_info) => {
                tracing::Span::current().record("event_type", "Event::PaneUpdate");
                tracing::debug!(pane_count = ?pane_info.panes.len());

                frames::hide_frames_conditionally(
                    &frames::FrameConfig::new(
                        self.module_config.hide_frame_for_single_pane,
                        self.module_config.hide_frame_except_for_search,
                        self.module_config.hide_frame_except_for_fullscreen,
                        self.module_config.hide_frame_except_for_scroll,
                    ),
                    &self.state.tabs,
                    &pane_info,
                    &self.state.mode,
                    get_plugin_ids(),
                    false,
                );

                self.state.panes = pane_info;

                if self.plugin_url.is_none() {
                    let my_plugin_id = get_plugin_ids().plugin_id;
                    for (_tab_idx, pane_list) in &self.state.panes.panes {
                        if let Some(pane) =
                            pane_list.iter().find(|p| p.is_plugin && p.id == my_plugin_id)
                        {
                            self.plugin_url = pane.plugin_url.clone();
                            tracing::debug!(plugin_url = ?self.plugin_url, "discovered own plugin_url");
                            self.request_statuses_from_siblings();
                            break;
                        }
                    }
                }

                self.state.cache_mask = UpdateEventMask::Tab as u8;

                should_render = true;
            }
            Event::PermissionRequestResult(result) => {
                tracing::Span::current().record("event_type", "Event::PermissionRequestResult");
                tracing::debug!(result = ?result);
                set_selectable(false);
            }
            Event::RunCommandResult(exit_code, stdout, stderr, context) => {
                tracing::Span::current().record("event_type", "Event::RunCommandResult");
                tracing::debug!(
                    exit_code = ?exit_code,
                    stdout = ?String::from_utf8(stdout.clone()),
                    stderr = ?String::from_utf8(stderr.clone()),
                    context = ?context
                );

                self.state.cache_mask = UpdateEventMask::Command as u8;

                if let Some(name) = context.get("name") {
                    let stdout = match String::from_utf8(stdout) {
                        Ok(s) => s,
                        Err(_) => "".to_owned(),
                    };

                    let stderr = match String::from_utf8(stderr) {
                        Ok(s) => s,
                        Err(_) => "".to_owned(),
                    };

                    self.state.command_results.insert(
                        name.to_owned(),
                        CommandResult {
                            exit_code,
                            stdout,
                            stderr,
                            context,
                        },
                    );
                }
            }
            Event::SessionUpdate(session_info, _) => {
                tracing::Span::current().record("event_type", "Event::SessionUpdate");

                let current_session = session_info.iter().find(|s| s.is_current_session);

                if let Some(current_session) = current_session {
                    frames::hide_frames_conditionally(
                        &frames::FrameConfig::new(
                            self.module_config.hide_frame_for_single_pane,
                            self.module_config.hide_frame_except_for_search,
                            self.module_config.hide_frame_except_for_fullscreen,
                            self.module_config.hide_frame_except_for_scroll,
                        ),
                        &current_session.tabs,
                        &current_session.panes,
                        &self.state.mode,
                        get_plugin_ids(),
                        false,
                    );
                }

                self.state.sessions = session_info;
                self.state.cache_mask = UpdateEventMask::Session as u8;

                should_render = true;
            }
            Event::TabUpdate(tab_info) => {
                tracing::Span::current().record("event_type", "Event::TabUpdate");

                tracing::debug!(tab_count = ?tab_info.len());

                self.state.cache_mask = UpdateEventMask::Tab as u8;
                self.state.tabs = tab_info;

                let valid_positions: std::collections::BTreeSet<usize> =
                    self.state.tabs.iter().map(|t| t.position).collect();
                self.state
                    .tab_statuses
                    .retain(|pos, _| valid_positions.contains(pos));

                // Broadcast statuses to new instances when tab count grows
                if self.state.tabs.len() > self.prev_tab_count
                    && !self.state.tab_statuses.is_empty()
                {
                    self.broadcast_statuses();
                }
                self.prev_tab_count = self.state.tabs.len();

                should_render = true;
            }
            _ => (),
        };
        should_render
    }
}

fn register_widgets(configuration: &BTreeMap<String, String>) -> BTreeMap<String, Arc<dyn Widget>> {
    let mut widget_map = BTreeMap::<String, Arc<dyn Widget>>::new();

    widget_map.insert(
        "command".to_owned(),
        Arc::new(CommandWidget::new(configuration)),
    );
    widget_map.insert(
        "datetime".to_owned(),
        Arc::new(DateTimeWidget::new(configuration)),
    );
    widget_map.insert("pipe".to_owned(), Arc::new(PipeWidget::new(configuration)));
    widget_map.insert(
        "swap_layout".to_owned(),
        Arc::new(SwapLayoutWidget::new(configuration)),
    );
    widget_map.insert("mode".to_owned(), Arc::new(ModeWidget::new(configuration)));
    widget_map.insert(
        "session".to_owned(),
        Arc::new(SessionWidget::new(configuration)),
    );
    widget_map.insert("tabs".to_owned(), Arc::new(TabsWidget::new(configuration)));
    widget_map.insert(
        "notifications".to_owned(),
        Arc::new(NotificationWidget::new(configuration)),
    );

    tracing::debug!("registered widgets: {:?}", widget_map.keys());

    widget_map
}
