//! The microphone workspace. Its profiles and service never use playback state.
use crate::{editing, editor_tasks::EditorTasks, graph, knob};
use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use massiveeq_core::{
    ChannelSelection, Filter, FilterKind, ProfileAnalysis, ProfileDocument,
    input::{InputLibrary, InputProfile, MAX_INPUT_FILTERS},
};
use serde::Deserialize;
use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
    sync::mpsc,
    time::{Duration, Instant},
};

const INSTALL_COMMAND: &str = "sudo pacman -S --needed carla pipewire-jack python lsp-plugins-lv2 noise-suppression-for-voice";
const RNNOISE_LATENCY_MS: f64 = 10.0;
const KINDS: [FilterKind; 5] = [
    FilterKind::Peaking,
    FilterKind::LowShelf,
    FilterKind::HighShelf,
    FilterKind::HighPass,
    FilterKind::LowPass,
];

#[derive(Clone, Debug, Deserialize)]
struct Source {
    name: String,
    description: Option<String>,
    channels: Vec<String>,
}

enum Reply {
    Sources(Result<Vec<Source>, String>),
    Doctor(serde_json::Value),
    Status {
        active: bool,
        pid: u32,
        runtime: serde_json::Value,
    },
    Action(Result<(), String>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ServiceStatus {
    Off,
    Running,
    Other,
}

#[derive(Clone, Debug, PartialEq)]
struct InputLatency {
    live: bool,
    sample_rate: Option<u32>,
    buffer_frames: Option<u32>,
    plugin_ms: f64,
    routing_ms: Option<f64>,
    added_ms: Option<f64>,
    plus_unknown: bool,
    rnnoise_enabled: bool,
    custom_rack: bool,
}

struct Widgets {
    header_status: gtk::Label,
    profiles: gtk::DropDown,
    name: gtk::Entry,
    sources: gtk::DropDown,
    engine: gtk::ToggleButton,
    status: gtk::Label,
    save_status: gtk::Label,
    dependency: gtk::Box,
    dependency_label: gtk::Label,
    graph: gtk::DrawingArea,
    graph_overlay: gtk::Overlay,
    bands: gtk::FlowBox,
    selected_box: gtk::Box,
    kind: crate::Segmented,
    band_enabled: gtk::ToggleButton,
    frequency: knob::LabeledKnob,
    gain: knob::LabeledKnob,
    q: knob::LabeledKnob,
    eq: gtk::CheckButton,
    rnnoise: gtk::CheckButton,
    gate: gtk::CheckButton,
    gate_threshold: gtk::SpinButton,
    gate_attack: gtk::SpinButton,
    gate_release: gtk::SpinButton,
    compressor: gtk::CheckButton,
    threshold: gtk::SpinButton,
    ratio: gtk::SpinButton,
    attack: gtk::SpinButton,
    release: gtk::SpinButton,
    makeup: gtk::SpinButton,
    managed: gtk::Box,
    custom: gtk::Label,
    clear_custom: gtk::Button,
    undo: gtk::Button,
    redo: gtk::Button,
    delete_profile: gtk::Button,
    add_band: gtk::Button,
}

struct State {
    library: RefCell<InputLibrary>,
    path: Option<PathBuf>,
    load_error: Option<String>,
    document: Rc<RefCell<Option<ProfileDocument>>>,
    analysis: Rc<RefCell<Option<ProfileAnalysis>>>,
    selected: Rc<Cell<Option<usize>>>,
    sources: RefCell<Vec<Source>>,
    source_names: RefCell<Vec<String>>,
    syncing: Cell<bool>,
    dragging: Cell<bool>,
    graph_epoch: Cell<u64>,
    dirty: Cell<bool>,
    pending: RefCell<Option<gtk::glib::SourceId>>,
    history: RefCell<editing::History>,
    tasks: EditorTasks,
    tx: mpsc::Sender<Reply>,
    rx: mpsc::Receiver<Reply>,
    poll_busy: Cell<bool>,
    action_busy: Cell<bool>,
    last_poll: Cell<Instant>,
    clock_rate: Cell<Option<u32>>,
    clock_quantum: Cell<Option<u32>>,
    service_status: Cell<ServiceStatus>,
    runtime_latency: RefCell<Option<InputLatency>>,
    ui: Widgets,
}

#[derive(Clone)]
pub struct InputView {
    pub root: adw::ToolbarView,
    pub header: adw::HeaderBar,
    state: Rc<State>,
}

impl InputView {
    pub fn flush(&self) -> bool {
        self.state.flush()
    }
    pub fn close(&self) {
        self.state.tasks.close();
        if let Some(timer) = self.state.pending.borrow_mut().take() {
            timer.remove();
        }
    }
}

impl State {
    fn snapshot(&self) -> editing::Snapshot {
        editing::Snapshot {
            text: serde_json::to_string(self.library.borrow().active())
                .expect("valid input profile"),
            trim: 0.0,
        }
    }

    fn edit(self: &Rc<Self>, change: impl FnOnce(&mut InputProfile)) {
        if self.syncing.get() || self.load_error.is_some() {
            return;
        }
        {
            let mut library = self.library.borrow_mut();
            let before = library.active().clone();
            change(library.active_mut());
            if let Err(error) = library.validate() {
                *library.active_mut() = before;
                self.ui.save_status.set_text(&error);
                return;
            }
        }
        if !self.dragging.get() {
            self.history.borrow_mut().checkpoint();
        }
        self.history.borrow_mut().record(self.snapshot());
        self.changed();
    }

    fn changed(self: &Rc<Self>) {
        self.sync();
        self.dirty.set(true);
        self.ui.save_status.set_text("Saving input profile…");
        if let Some(timer) = self.pending.borrow_mut().take() {
            timer.remove();
        }
        let state = Rc::downgrade(self);
        *self.pending.borrow_mut() = Some(gtk::glib::timeout_add_local_once(
            Duration::from_millis(300),
            move || {
                if let Some(state) = state.upgrade() {
                    state.pending.borrow_mut().take();
                    state.save();
                }
            },
        ));
    }

    fn save(&self) -> bool {
        if !self.dirty.get() {
            return true;
        }
        let result = self
            .path
            .as_deref()
            .ok_or_else(|| "Could not find microphone settings".to_string())
            .and_then(|path| self.library.borrow().save(path));
        match result {
            Ok(()) => {
                self.dirty.set(false);
                self.profiles();
                self.ui.save_status.set_text("Input profile saved");
                true
            }
            Err(error) => {
                self.ui
                    .save_status
                    .set_text(&format!("Could not save input profile: {error}"));
                false
            }
        }
    }

    fn flush(&self) -> bool {
        if let Some(timer) = self.pending.borrow_mut().take() {
            timer.remove();
        }
        self.save()
    }

    fn sync(self: &Rc<Self>) {
        self.syncing.set(true);
        let profile = self.library.borrow().active().clone();
        let custom = profile.advanced_project.is_some();
        self.ui
            .managed
            .set_sensitive(!custom && self.load_error.is_none());
        self.ui.managed.set_opacity(if custom { 0.4 } else { 1.0 });
        self.ui
            .custom
            .set_text(&profile.advanced_project.as_ref().map_or_else(
                || "Built-in EQ and voice processing".into(),
                |path| {
                    format!(
                        "Custom rack: {}. Built-in EQ and voice controls are inactive.",
                        path.file_name().unwrap_or_default().to_string_lossy()
                    )
                },
            ));
        self.ui.clear_custom.set_visible(custom);
        self.ui.eq.set_active(profile.eq_enabled);
        self.ui.rnnoise.set_active(profile.rnnoise.enabled);
        self.ui.gate.set_active(profile.gate.enabled);
        self.ui.gate_threshold.set_value(profile.gate.threshold_db);
        self.ui.gate_attack.set_value(profile.gate.attack_ms);
        self.ui.gate_release.set_value(profile.gate.release_ms);
        self.ui.compressor.set_active(profile.compressor.enabled);
        self.ui.threshold.set_value(profile.compressor.threshold_db);
        self.ui.ratio.set_value(profile.compressor.ratio);
        self.ui.attack.set_value(profile.compressor.attack_ms);
        self.ui.release.set_value(profile.compressor.release_ms);
        self.ui.makeup.set_value(profile.compressor.makeup_db);
        if self
            .selected
            .get()
            .is_some_and(|i| i >= profile.filters.len())
        {
            self.selected.set(None);
        }
        self.ui
            .selected_box
            .set_visible(self.selected.get().is_some());
        if let Some(filter) = self.selected.get().and_then(|i| profile.filters.get(i)) {
            self.ui.frequency.knob.set_value(filter.frequency);
            self.ui.gain.knob.set_value(filter.gain_db);
            self.ui.gain.knob.set_sensitive(!matches!(
                filter.kind,
                FilterKind::HighPass | FilterKind::LowPass
            ));
            self.ui.q.knob.set_value(filter.q);
            self.ui.band_enabled.set_active(filter.enabled);
            let kind = KINDS.iter().position(|k| *k == filter.kind).unwrap_or(0);
            self.ui.kind.buttons[kind].set_active(true);
            if filter.enabled {
                self.ui.selected_box.remove_css_class("disabled-band");
            } else {
                self.ui.selected_box.add_css_class("disabled-band");
            }
        }
        while let Some(child) = self.ui.bands.first_child() {
            self.ui.bands.remove(&child);
        }
        for (index, filter) in profile.filters.iter().enumerate() {
            let button = gtk::ToggleButton::with_label(&format!(
                "{}  {:.0} Hz",
                index + 1,
                filter.frequency
            ));
            button.add_css_class("band-chip");
            if !filter.enabled {
                button.add_css_class("disabled-band");
            }
            button.set_focus_on_click(false);
            button.set_active(self.selected.get() == Some(index));
            button.update_property(&[gtk::accessible::Property::Label(&format!(
                "Input band {}",
                index + 1
            ))]);
            let state = Rc::downgrade(self);
            button.connect_clicked(move |button| {
                if let Some(s) = state.upgrade() {
                    s.select(button.is_active().then_some(index));
                    s.ui.graph.grab_focus();
                }
            });
            self.ui.bands.insert(&button, -1);
        }
        self.ui
            .add_band
            .set_sensitive(profile.filters.len() < MAX_INPUT_FILTERS);
        let document = profile.document();
        *self.analysis.borrow_mut() = Some(massiveeq_core::analyze_profile_preview(
            &document, 48_000.0, 0.0,
        ));
        *self.document.borrow_mut() = Some(document);
        self.ui
            .graph
            .set_opacity(if profile.eq_enabled { 1.0 } else { 0.45 });
        self.ui.graph.queue_draw();
        self.ui.graph_overlay.queue_allocate();
        self.ui.undo.set_sensitive(self.history.borrow().can_undo());
        self.ui.redo.set_sensitive(self.history.borrow().can_redo());
        self.syncing.set(false);
        self.update_latency();
    }

    fn current_latency(&self) -> InputLatency {
        let library = self.library.borrow();
        let profile = library.active();
        if self.service_status.get() == ServiceStatus::Running
            && let Some(runtime) = self.runtime_latency.borrow().clone()
        {
            if runtime.rnnoise_enabled == profile.rnnoise.enabled
                && runtime.custom_rack == profile.advanced_project.is_some()
            {
                return runtime;
            }
            let mut estimated = estimate_input_latency(
                profile,
                runtime.sample_rate.or_else(|| self.clock_rate.get()),
                runtime.buffer_frames.or_else(|| self.clock_quantum.get()),
            );
            estimated.live = true;
            return estimated;
        }
        estimate_input_latency(profile, self.clock_rate.get(), self.clock_quantum.get())
    }

    fn update_latency(&self) {
        let latency = self.current_latency();
        let tooltip = input_latency_tooltip(&latency);
        self.ui
            .header_status
            .set_markup(&input_header_markup(&latency));
        self.ui.header_status.set_tooltip_text(Some(&tooltip));
        if self.load_error.is_some() || self.action_busy.get() {
            return;
        }
        let summary = input_latency_summary(&latency);
        match self.service_status.get() {
            ServiceStatus::Running => {
                self.ui.status.set_text(&format!(
                    "Input on · DSP {summary} · select MassiveEQ Microphone in your calling or recording app"
                ));
                self.ui.status.set_tooltip_text(Some(&tooltip));
            }
            ServiceStatus::Off => {
                self.ui.status.set_text(&format!(
                    "Input off · est. DSP {summary} · playback processing is independent"
                ));
                self.ui.status.set_tooltip_text(Some(&tooltip));
            }
            ServiceStatus::Other => {
                self.ui.status.set_tooltip_text(None);
            }
        }
    }

    fn select(self: &Rc<Self>, index: Option<usize>) {
        for editor in [
            &self.ui.frequency.value_editor,
            &self.ui.gain.value_editor,
            &self.ui.q.value_editor,
        ] {
            editor.set_visible_child_name("value");
        }
        if self.selected.get() != index {
            self.graph_epoch.set(self.graph_epoch.get().wrapping_add(1));
        }
        self.selected.set(index);
        self.history.borrow_mut().checkpoint();
        self.sync();
    }

    fn profiles(&self) {
        self.syncing.set(true);
        let library = self.library.borrow();
        let names = library
            .profiles
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>();
        let unchanged =
            self.ui
                .profiles
                .model()
                .and_downcast::<gtk::StringList>()
                .is_some_and(|model| {
                    model.n_items() as usize == names.len()
                        && names.iter().enumerate().all(|(index, name)| {
                            model.string(index as u32).as_deref() == Some(*name)
                        })
                });
        if !unchanged {
            self.ui
                .profiles
                .set_model(Some(&gtk::StringList::new(&names)));
        }
        let selected = library
            .profiles
            .iter()
            .position(|p| p.id == library.active_profile)
            .unwrap_or(0) as u32;
        if self.ui.profiles.selected() != selected {
            self.ui.profiles.set_selected(selected);
        }
        self.ui.name.set_text(&library.active().name);
        self.ui
            .delete_profile
            .set_sensitive(library.profiles.len() > 1);
        self.syncing.set(false);
    }

    fn source_list(&self) {
        self.syncing.set(true);
        let library = self.library.borrow();
        let profile = library.active();
        let sources = self.sources.borrow();
        let mut names = vec![String::new()];
        let mut labels = vec!["Choose a microphone".to_string()];
        for source in sources.iter() {
            names.push(source.name.clone());
            labels.push(
                source
                    .description
                    .clone()
                    .unwrap_or_else(|| source.name.clone()),
            );
        }
        if !profile.source.is_empty() && !names.contains(&profile.source) {
            names.push(profile.source.clone());
            labels.push(format!("{} (disconnected)", profile.source));
        }
        let refs = labels.iter().map(String::as_str).collect::<Vec<_>>();
        self.ui
            .sources
            .set_model(Some(&gtk::StringList::new(&refs)));
        self.ui
            .sources
            .set_selected(names.iter().position(|s| *s == profile.source).unwrap_or(0) as u32);
        *self.source_names.borrow_mut() = names;
        self.syncing.set(false);
    }

    fn refresh_sources(&self) {
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = std::process::Command::new(helper())
                .arg("sources")
                .output()
                .map_err(|e| e.to_string())
                .and_then(|o| {
                    if !o.status.success() {
                        return Err(String::from_utf8_lossy(&o.stderr).trim().to_string());
                    }
                    serde_json::from_slice::<Vec<Source>>(&o.stdout).map_err(|e| e.to_string())
                });
            let _ = tx.send(Reply::Sources(result));
            let doctor = std::process::Command::new(helper())
                .arg("doctor")
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| serde_json::from_slice(&o.stdout).ok())
                .unwrap_or(serde_json::Value::Null);
            let _ = tx.send(Reply::Doctor(doctor));
        });
    }

    fn poll(&self) {
        if self.poll_busy.replace(true) {
            return;
        }
        self.last_poll.set(Instant::now());
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let output = std::process::Command::new("systemctl")
                .args([
                    "--user",
                    "show",
                    "massiveeq-input.service",
                    "--property=ActiveState",
                    "--property=MainPID",
                ])
                .output();
            let text = output
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
                .unwrap_or_default();
            let active = text
                .lines()
                .any(|l| l == "ActiveState=active" || l == "ActiveState=activating");
            let pid = text
                .lines()
                .find_map(|l| l.strip_prefix("MainPID="))
                .and_then(|v| v.parse::<u32>().ok())
                .unwrap_or(0);
            let runtime = std::env::var_os("XDG_RUNTIME_DIR")
                .map(PathBuf::from)
                .and_then(|p| std::fs::read(p.join("massiveeq-input-status.json")).ok())
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .unwrap_or(serde_json::Value::Null);
            let _ = tx.send(Reply::Status {
                active,
                pid,
                runtime,
            });
        });
    }

    fn engine(self: &Rc<Self>, enabled: bool) {
        if self.syncing.get() || self.action_busy.get() {
            return;
        }
        if enabled
            && (self.library.borrow().active().source.is_empty() || self.load_error.is_some())
        {
            self.ui
                .status
                .set_text("Choose a microphone before starting input processing");
            self.syncing.set(true);
            self.ui.engine.set_active(false);
            self.syncing.set(false);
            return;
        }
        if enabled && !self.flush() {
            return;
        }
        if enabled && !PathBuf::from("/usr/lib/carla/libcarla_standalone2.so").is_file() {
            self.ui.dependency.set_visible(true);
            self.ui
                .status
                .set_text("Install microphone processing support, then turn Input on");
            self.syncing.set(true);
            self.ui.engine.set_active(false);
            self.syncing.set(false);
            return;
        }
        self.action_busy.set(true);
        self.ui.engine.set_sensitive(false);
        self.ui.status.set_text(if enabled {
            "Starting microphone processing…"
        } else {
            "Stopping microphone processing…"
        });
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = std::process::Command::new("systemctl")
                .args([
                    "--user",
                    if enabled { "enable" } else { "disable" },
                    "--now",
                    "massiveeq-input.service",
                ])
                .output()
                .map_err(|e| e.to_string())
                .and_then(|o| {
                    if o.status.success() {
                        Ok(())
                    } else {
                        Err(String::from_utf8_lossy(&o.stderr).trim().to_string())
                    }
                });
            let _ = tx.send(Reply::Action(result));
        });
    }

    fn replies(self: &Rc<Self>) {
        while let Ok(reply) = self.rx.try_recv() {
            match reply {
                Reply::Doctor(doctor) => {
                    let available = doctor
                        .get("available")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let noise = doctor
                        .get("rnnoise_available")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    self.ui.dependency.set_visible(!available || !noise);
                    let packages = doctor
                        .get("missing")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        })
                        .unwrap_or_default();
                    let text = if available && !noise {
                        "RNNoise support is not installed. EQ, gate, and compressor are available."
                            .to_string()
                    } else if available {
                        "Microphone processing support is installed.".to_string()
                    } else {
                        format!("Install microphone support: {packages}")
                    };
                    self.ui.dependency_label.set_text(&text);
                    if let Some(rate) = doctor
                        .get("clock_rate")
                        .and_then(|v| v.as_u64())
                        .and_then(|v| u32::try_from(v).ok())
                        .filter(|v| *v > 0)
                    {
                        self.clock_rate.set(Some(rate));
                    }
                    if let Some(quantum) = doctor
                        .get("clock_quantum")
                        .and_then(|v| v.as_u64())
                        .and_then(|v| u32::try_from(v).ok())
                        .filter(|v| *v > 0)
                    {
                        self.clock_quantum.set(Some(quantum));
                    }
                    self.update_latency();
                }
                Reply::Sources(Ok(sources)) => {
                    *self.sources.borrow_mut() = sources;
                    self.source_list();
                }
                Reply::Sources(Err(error)) => {
                    self.service_status.set(ServiceStatus::Other);
                    self.ui
                        .status
                        .set_text(&format!("Could not list microphones: {error}"));
                }
                Reply::Action(result) => {
                    self.action_busy.set(false);
                    self.ui.engine.set_sensitive(true);
                    if let Err(error) = result {
                        self.ui
                            .save_status
                            .set_text(&format!("Input service: {error}"));
                    }
                    self.poll();
                }
                Reply::Status {
                    active,
                    pid,
                    runtime,
                } => {
                    self.poll_busy.set(false);
                    if self.action_busy.get() {
                        continue;
                    }
                    self.syncing.set(true);
                    self.ui.engine.set_active(active);
                    self.syncing.set(false);
                    if self.load_error.is_some() {
                        continue;
                    }
                    let state = runtime.get("state").and_then(|v| v.as_str());
                    let error = runtime
                        .get("error")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty());
                    let current =
                        runtime.get("pid").and_then(|v| v.as_u64()) == Some(pid as u64) && pid != 0;
                    if active && current && state == Some("running") {
                        self.service_status.set(ServiceStatus::Running);
                        let parsed = {
                            let library = self.library.borrow();
                            parse_runtime_latency(
                                &runtime,
                                library.active(),
                                self.clock_rate.get(),
                                self.clock_quantum.get(),
                            )
                        };
                        *self.runtime_latency.borrow_mut() = parsed;
                        self.update_latency();
                    } else if let Some(error) = error {
                        self.service_status.set(ServiceStatus::Other);
                        *self.runtime_latency.borrow_mut() = None;
                        self.ui
                            .status
                            .set_text(&format!("Input needs attention: {error}"));
                        self.update_latency();
                    } else if active {
                        self.service_status.set(ServiceStatus::Other);
                        *self.runtime_latency.borrow_mut() = None;
                        self.ui.status.set_text("Starting microphone processing…");
                        self.update_latency();
                    } else {
                        self.service_status.set(ServiceStatus::Off);
                        *self.runtime_latency.borrow_mut() = None;
                        self.update_latency();
                    }
                }
            }
        }
        if self.last_poll.get().elapsed() >= Duration::from_secs(2) {
            self.poll();
        }
    }
}

fn parse_runtime_latency(
    runtime: &serde_json::Value,
    profile: &InputProfile,
    fallback_rate: Option<u32>,
    fallback_quantum: Option<u32>,
) -> Option<InputLatency> {
    let custom_rack = runtime.get("backend").and_then(|v| v.as_str()) == Some("custom");
    let Some(latency) = runtime.get("latency").and_then(|v| v.as_object()) else {
        let mut estimated = estimate_input_latency(profile, fallback_rate, fallback_quantum);
        estimated.live = true;
        estimated.custom_rack = custom_rack;
        estimated.plus_unknown |= custom_rack;
        return Some(estimated);
    };
    let u32_field = |key: &str| {
        latency
            .get(key)
            .and_then(|v| v.as_u64())
            .and_then(|v| u32::try_from(v).ok())
            .filter(|v| *v > 0)
    };
    let f64_field = |key: &str| {
        latency
            .get(key)
            .and_then(|v| v.as_f64())
            .filter(|v| v.is_finite() && *v >= 0.0)
    };
    let sample_rate = u32_field("sample_rate").or(fallback_rate);
    let buffer_frames = u32_field("buffer_frames").or(fallback_quantum);
    let plugin_ms = f64_field("plugin_latency_ms").unwrap_or(0.0);
    let routing_ms = f64_field("routing_latency_ms")
        .or_else(|| Some(buffer_frames? as f64 * 1000.0 / sample_rate? as f64));
    let added_ms = f64_field("added_latency_ms").or_else(|| routing_ms.map(|r| plugin_ms + r));
    let plus_unknown = latency
        .get("custom_plugins_unreported")
        .and_then(|v| v.as_bool())
        .unwrap_or(custom_rack);
    let rnnoise_enabled = !custom_rack && profile.rnnoise.enabled;
    Some(InputLatency {
        live: true,
        sample_rate,
        buffer_frames,
        plugin_ms,
        routing_ms,
        added_ms,
        plus_unknown,
        rnnoise_enabled,
        custom_rack,
    })
}

fn estimate_input_latency(
    profile: &InputProfile,
    clock_rate: Option<u32>,
    clock_quantum: Option<u32>,
) -> InputLatency {
    let custom_rack = profile.advanced_project.is_some();
    let rnnoise_enabled = !custom_rack && profile.rnnoise.enabled;
    let plugin_ms = if rnnoise_enabled {
        RNNOISE_LATENCY_MS
    } else {
        0.0
    };
    let routing_ms = match (clock_rate, clock_quantum) {
        (Some(rate), Some(quantum)) if rate > 0 && quantum > 0 => {
            Some(quantum as f64 * 1000.0 / rate as f64)
        }
        _ => None,
    };
    let added_ms = routing_ms.map(|routing| plugin_ms + routing);
    InputLatency {
        live: false,
        sample_rate: clock_rate,
        buffer_frames: clock_quantum,
        plugin_ms,
        routing_ms,
        added_ms,
        plus_unknown: custom_rack,
        rnnoise_enabled,
        custom_rack,
    }
}

fn input_latency_summary(latency: &InputLatency) -> String {
    if let Some(added_ms) = latency.added_ms {
        crate::format_rough_latency_ms(added_ms, latency.plus_unknown)
    } else if latency.custom_rack {
        "custom".to_owned()
    } else if latency.plugin_ms >= 0.5 {
        format!(
            "{} + buffer",
            crate::format_rough_latency_ms(latency.plugin_ms, false)
        )
    } else {
        "~1 buffer".to_owned()
    }
}

fn input_header_markup(latency: &InputLatency) -> String {
    let escape = |text: &str| gtk::glib::markup_escape_text(text).to_string();
    let accent = |text: &str| format!("<span foreground='#ff7a1a'>{}</span>", escape(text));
    let mut parts = vec!["MICROPHONE".to_owned()];
    if latency.live
        && let Some(rate) = latency.sample_rate
    {
        parts.push(format!(
            "{} kHz",
            accent(&format!("{:.1}", rate as f64 / 1000.0))
        ));
    }
    let summary = input_latency_summary(latency);
    if latency.live {
        parts.push(format!("DSP {}", accent(&summary)));
    } else {
        parts.push(format!("EST. DSP {}", accent(&summary)));
    }
    parts.join("  <span foreground='#3a3a3e'>│</span>  ")
}

fn input_latency_tooltip(latency: &InputLatency) -> String {
    let summary = input_latency_summary(latency);
    let mut details = Vec::new();
    if latency.custom_rack {
        if latency.plugin_ms >= 0.05 {
            details.push(format!("{:.1} ms reported plugin delay", latency.plugin_ms));
        }
    } else if latency.rnnoise_enabled {
        details.push(format!("{:.1} ms RNNoise", latency.plugin_ms));
    } else if latency.plugin_ms >= 0.05 {
        details.push(format!("{:.1} ms plugins", latency.plugin_ms));
    } else {
        details.push("0 ms EQ/gate/compressor".to_owned());
    }
    if let (Some(routing_ms), Some(frames), Some(rate)) = (
        latency.routing_ms,
        latency.buffer_frames,
        latency.sample_rate,
    ) {
        details.push(format!(
            "~{routing_ms:.1} ms PipeWire rack/loopback buffer ({frames} frames at {:.1} kHz)",
            rate as f64 / 1000.0
        ));
    } else {
        details.push("PipeWire rack/loopback buffer".to_owned());
    }
    if latency.plus_unknown {
        details.push("custom plugins may add more".to_owned());
    }
    let detail_text = details.join("; ");
    if latency.live {
        if latency.plus_unknown {
            format!("Input DSP adds at least {summary} ({detail_text}).")
        } else {
            format!("Input DSP adds roughly {summary} ({detail_text}).")
        }
    } else {
        format!("Estimated Input DSP latency when enabled: {summary} ({detail_text}).")
    }
}

fn helper() -> PathBuf {
    let installed = PathBuf::from("/usr/bin/massiveeq-input");
    let sibling = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.join("massiveeq-input")));
    if let Some(sibling) = sibling.filter(|p| p.is_file()) {
        return sibling;
    }
    if installed.is_file() {
        installed
    } else {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../packaging/massiveeq-input")
    }
}

fn spin(label: &str, low: f64, high: f64, step: f64, digits: u32) -> gtk::SpinButton {
    let spin = gtk::SpinButton::with_range(low, high, step);
    spin.set_digits(digits);
    spin.set_width_chars(6);
    spin.update_property(&[gtk::accessible::Property::Label(label)]);
    spin
}

fn field(grid: &gtk::Grid, label: &str, widget: &impl IsA<gtk::Widget>, column: i32, row: i32) {
    let caption = gtk::Label::new(Some(label));
    caption.set_xalign(0.0);
    grid.attach(&caption, column * 2, row, 1, 1);
    grid.attach(widget, column * 2 + 1, row, 1, 1);
}

pub fn build(window: &adw::ApplicationWindow) -> InputView {
    build_at(window, InputLibrary::path())
}

fn build_at(window: &adw::ApplicationWindow, path: Result<PathBuf, String>) -> InputView {
    let loaded = path
        .as_ref()
        .map_err(Clone::clone)
        .and_then(|p| InputLibrary::load(p));
    let load_error = loaded.as_ref().err().cloned();
    let library = loaded.unwrap_or_default();
    let document = Rc::new(RefCell::new(None));
    let analysis = Rc::new(RefCell::new(None));
    let selected = Rc::new(Cell::new(None));
    let graph = graph::response_graph(
        analysis.clone(),
        document.clone(),
        selected.clone(),
        Rc::new(RefCell::new(graph::Overlay::default())),
    );
    graph.set_height_request(240);
    graph.set_vexpand(true);
    graph.set_focusable(true);
    graph.update_property(&[gtk::accessible::Property::Label("Microphone EQ graph")]);
    let root = adw::ToolbarView::new();
    root.add_css_class("instrument-root");
    let header = adw::HeaderBar::new();
    header.add_css_class("instrument-header");
    let brand = gtk::Label::new(Some("MASSIVE/EQ"));
    brand.add_css_class("brand");
    header.pack_start(&brand);
    let header_status = gtk::Label::new(Some("MICROPHONE"));
    header_status.add_css_class("header-status");
    header_status.set_ellipsize(gtk::pango::EllipsizeMode::End);
    header.set_title_widget(Some(&header_status));
    let engine = gtk::ToggleButton::new();
    engine.set_icon_name("system-shutdown-symbolic");
    engine.add_css_class("power");
    engine.set_tooltip_text(Some(
        "Microphone processing. Playback has its own Engine switch.",
    ));
    engine.update_property(&[gtk::accessible::Property::Label("Input engine enabled")]);
    header.pack_end(&engine);
    root.add_top_bar(&header);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 10);
    body.set_margin_start(16);
    body.set_margin_end(16);
    body.set_margin_top(10);
    body.set_margin_bottom(16);
    let profiles = gtk::DropDown::from_strings(&[]);
    profiles.set_hexpand(true);
    profiles.update_property(&[gtk::accessible::Property::Label("Input profiles")]);
    let name = gtk::Entry::new();
    name.set_placeholder_text(Some("Input profile name"));
    name.set_hexpand(true);
    name.set_tooltip_text(Some("Rename this microphone profile; press Enter to save"));
    name.update_property(&[gtk::accessible::Property::Label("Input profile name")]);
    let new_profile = gtk::Button::from_icon_name("list-add-symbolic");
    new_profile.set_tooltip_text(Some("New input profile"));
    let duplicate = gtk::Button::from_icon_name("edit-copy-symbolic");
    duplicate.set_tooltip_text(Some("Duplicate input profile"));
    let delete_profile = gtk::Button::from_icon_name("user-trash-symbolic");
    delete_profile.set_tooltip_text(Some("Delete input profile"));
    let profile_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    profile_row.append(&gtk::Label::new(Some("Profile")));
    profile_row.append(&profiles);
    profile_row.append(&name);
    for button in [&new_profile, &duplicate, &delete_profile] {
        profile_row.append(button);
    }
    body.append(&profile_row);
    let sources = gtk::DropDown::from_strings(&["Choose a microphone"]);
    sources.set_hexpand(true);
    sources.update_property(&[gtk::accessible::Property::Label("Microphone source")]);
    let refresh = gtk::Button::from_icon_name("view-refresh-symbolic");
    refresh.set_tooltip_text(Some("Refresh microphones"));
    let source_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    source_row.append(&gtk::Label::new(Some("Microphone")));
    source_row.append(&sources);
    source_row.append(&refresh);
    body.append(&source_row);
    let status = gtk::Label::new(Some("Input off"));
    status.set_xalign(0.0);
    status.set_wrap(true);
    body.append(&status);
    let dependency = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let dependency_label =
        gtk::Label::new(Some("Microphone processing needs optional audio effects."));
    dependency_label.set_wrap(true);
    dependency_label.set_hexpand(true);
    dependency_label.set_xalign(0.0);
    let install = gtk::Button::with_label("Install support…");
    dependency.append(&dependency_label);
    dependency.append(&install);
    dependency.set_visible(!PathBuf::from("/usr/lib/carla/libcarla_standalone2.so").is_file());
    body.append(&dependency);
    let managed = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let graph_overlay = gtk::Overlay::new();
    graph_overlay.set_child(Some(&graph));
    graph_overlay.set_vexpand(true);
    managed.append(&graph_overlay);
    let eq = gtk::CheckButton::with_label("Microphone EQ");
    let undo = gtk::Button::from_icon_name("edit-undo-symbolic");
    undo.set_tooltip_text(Some("Undo input edit"));
    let redo = gtk::Button::from_icon_name("edit-redo-symbolic");
    redo.set_tooltip_text(Some("Redo input edit"));
    let add_band = gtk::Button::with_label("Add band");
    let eq_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    eq_row.append(&eq);
    eq_row.append(&undo);
    eq_row.append(&redo);
    eq_row.append(&add_band);
    managed.append(&eq_row);
    let bands = gtk::FlowBox::new();
    bands.set_selection_mode(gtk::SelectionMode::None);
    bands.set_min_children_per_line(1);
    bands.set_max_children_per_line(16);
    bands.set_halign(gtk::Align::Start);
    bands.add_css_class("band-chips");
    bands.set_column_spacing(4);
    bands.set_row_spacing(4);
    managed.append(&bands);
    let selected_box = gtk::Box::new(gtk::Orientation::Vertical, 10);
    selected_box.add_css_class("band-panel");
    selected_box.set_halign(gtk::Align::Start);
    selected_box.set_valign(gtk::Align::Start);
    let top = adw::WrapBox::builder()
        .child_spacing(8)
        .line_spacing(8)
        .build();
    let kind = crate::Segmented::new(
        &["Peak", "Low shelf", "High shelf", "High pass", "Low pass"],
        0,
    );
    kind.root.add_css_class("kind-picker");
    for (button, filter_kind) in kind.buttons.iter().zip(KINDS) {
        button.set_child(Some(&crate::glyph::widget(filter_kind, 26, 16)));
        button.set_tooltip_text(Some(crate::kind_name(filter_kind)));
        button.reset_relation(gtk::AccessibleRelation::LabelledBy);
        button.update_property(&[gtk::accessible::Property::Label(crate::kind_name(
            filter_kind,
        ))]);
        button.set_focus_on_click(false);
    }
    let band_enabled = gtk::ToggleButton::new();
    band_enabled.set_icon_name("system-shutdown-symbolic");
    band_enabled.add_css_class("band-power");
    band_enabled.set_tooltip_text(Some("Band on or off"));
    band_enabled.update_property(&[gtk::accessible::Property::Label("Input band enabled")]);
    band_enabled.set_focus_on_click(false);
    let frequency = knob::LabeledKnob::new(
        knob::Knob::new(20.0, 20_000.0, 1000.0, knob::Taper::Log, false),
        "Freq",
        |v| format!("{v:.0} Hz"),
    );
    let gain = knob::LabeledKnob::new(
        knob::Knob::new(-24.0, 24.0, 0.0, knob::Taper::Linear, true),
        "Gain",
        |v| format!("{v:+.1} dB"),
    );
    let q = knob::LabeledKnob::new(
        knob::Knob::new(0.1, 20.0, 1.0, knob::Taper::Log, false),
        "Q",
        |v| format!("{v:.2}"),
    );
    for (knob, caption) in [(&frequency, "Frequency"), (&gain, "Gain"), (&q, "Q")] {
        knob.knob
            .update_property(&[gtk::accessible::Property::Label(&format!(
                "Input band {caption}"
            ))]);
    }
    let remove = gtk::Button::from_icon_name("window-close-symbolic");
    remove.add_css_class("band-remove");
    remove.set_tooltip_text(Some("Remove input band"));
    remove.set_focus_on_click(false);
    top.append(&kind.root);
    top.append(&band_enabled);
    top.append(&remove);
    selected_box.append(&top);
    let knobs = gtk::Box::new(gtk::Orientation::Horizontal, 18);
    knobs.set_halign(gtk::Align::Center);
    for control in [&frequency, &gain, &q] {
        control.knob.set_size_request(48, 48);
        knobs.append(&control.root);
    }
    selected_box.append(&knobs);
    selected_box.set_visible(false);
    graph_overlay.add_overlay(&selected_box);
    let voice = gtk::Expander::new(Some("Voice processing"));
    let voice_box = gtk::Box::new(gtk::Orientation::Vertical, 10);
    voice_box.set_margin_top(10);
    let rnnoise = gtk::CheckButton::with_label("RNNoise noise reduction");
    voice_box.append(&rnnoise);
    let gate = gtk::CheckButton::with_label("Noise gate");
    voice_box.append(&gate);
    let gate_threshold = spin("Gate threshold dB", -80.0, 0.0, 1.0, 1);
    let gate_attack = spin("Gate attack ms", 0.1, 100.0, 1.0, 1);
    let gate_release = spin("Gate release ms", 10.0, 1000.0, 10.0, 0);
    let grid = gtk::Grid::new();
    grid.set_column_spacing(8);
    grid.set_row_spacing(6);
    field(&grid, "Threshold, dB", &gate_threshold, 0, 0);
    field(&grid, "Attack, ms", &gate_attack, 1, 0);
    field(&grid, "Release, ms", &gate_release, 0, 1);
    voice_box.append(&grid);
    let compressor = gtk::CheckButton::with_label("Compressor");
    voice_box.append(&compressor);
    let threshold = spin("Compressor threshold dB", -60.0, 0.0, 1.0, 1);
    let ratio = spin("Compressor ratio", 1.0, 20.0, 0.5, 1);
    let attack = spin("Compressor attack ms", 0.1, 100.0, 1.0, 1);
    let release = spin("Compressor release ms", 10.0, 1000.0, 10.0, 0);
    let makeup = spin("Compressor makeup dB", 0.0, 24.0, 0.5, 1);
    let grid = gtk::Grid::new();
    grid.set_column_spacing(8);
    grid.set_row_spacing(6);
    field(&grid, "Threshold, dB", &threshold, 0, 0);
    field(&grid, "Ratio", &ratio, 1, 0);
    field(&grid, "Attack, ms", &attack, 0, 1);
    field(&grid, "Release, ms", &release, 1, 1);
    field(&grid, "Makeup, dB", &makeup, 0, 2);
    voice_box.append(&grid);
    voice.set_child(Some(&voice_box));
    managed.append(&voice);
    body.append(&managed);
    let advanced = gtk::Expander::new(Some("Advanced plugins"));
    let advanced_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    advanced_box.set_margin_top(10);
    let custom = gtk::Label::new(None);
    custom.set_wrap(true);
    custom.set_xalign(0.0);
    advanced_box.append(&custom);
    let choose = gtk::Button::with_label("Use custom Carla rack…");
    let clear_custom = gtk::Button::with_label("Use built-in processing");
    advanced_box.append(&choose);
    advanced_box.append(&clear_custom);
    advanced.set_child(Some(&advanced_box));
    body.append(&advanced);
    let save_status = gtk::Label::new(None);
    save_status.set_xalign(0.0);
    save_status.set_wrap(true);
    body.append(&save_status);
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&body)
        .build();
    root.set_content(Some(&scroll));
    let (tx, rx) = mpsc::channel();
    let state = Rc::new(State {
        library: RefCell::new(library),
        path: path.ok(),
        load_error,
        document,
        analysis,
        selected,
        sources: RefCell::new(Vec::new()),
        source_names: RefCell::new(vec![String::new()]),
        syncing: Cell::new(false),
        dragging: Cell::new(false),
        graph_epoch: Cell::new(0),
        dirty: Cell::new(false),
        pending: RefCell::new(None),
        history: RefCell::new(editing::History::default()),
        tasks: EditorTasks::default(),
        tx,
        rx,
        poll_busy: Cell::new(false),
        action_busy: Cell::new(false),
        last_poll: Cell::new(Instant::now()),
        clock_rate: Cell::new(None),
        clock_quantum: Cell::new(None),
        service_status: Cell::new(ServiceStatus::Off),
        runtime_latency: RefCell::new(None),
        ui: Widgets {
            header_status,
            profiles,
            name,
            sources,
            engine,
            status,
            save_status,
            dependency,
            dependency_label,
            graph,
            graph_overlay,
            bands,
            selected_box,
            kind,
            band_enabled,
            frequency,
            gain,
            q,
            eq,
            rnnoise,
            gate,
            gate_threshold,
            gate_attack,
            gate_release,
            compressor,
            threshold,
            ratio,
            attack,
            release,
            makeup,
            managed,
            custom,
            clear_custom,
            undo,
            redo,
            delete_profile,
            add_band,
        },
    });
    crate::band_panel::attach(
        &state.ui.graph_overlay,
        &state.ui.graph,
        &state.ui.selected_box,
        &[
            &state.ui.frequency.knob,
            &state.ui.gain.knob,
            &state.ui.q.knob,
        ],
        {
            let state = Rc::downgrade(&state);
            move || {
                let state = state.upgrade()?;
                let index = state.selected.get()?;
                let library = state.library.borrow();
                let filter = library.active().filters.get(index)?;
                Some(crate::band_panel::Anchor {
                    index,
                    frequency: filter.frequency,
                    gain_db: filter.gain_db,
                    graph_dragging: state.dragging.get(),
                })
            }
        },
        {
            let state = Rc::downgrade(&state);
            move |width, height| {
                state.upgrade().map_or_else(Vec::new, |state| {
                    graph::line_points(
                        state.analysis.borrow().as_ref(),
                        &graph::Overlay::default(),
                        width,
                        height,
                    )
                })
            }
        },
    );
    state.history.borrow_mut().reset(state.snapshot());
    state.profiles();
    state.source_list();
    state.sync();
    if let Some(error) = &state.load_error {
        state.ui.status.set_text(&format!(
            "Could not load microphone settings: {error}. Existing settings were preserved."
        ));
    }
    wire(
        &state,
        window,
        &new_profile,
        &duplicate,
        &refresh,
        &remove,
        &choose,
        &install,
    );
    state.refresh_sources();
    state.poll();
    let weak = Rc::downgrade(&state);
    state.tasks.track(gtk::glib::timeout_add_local(
        Duration::from_millis(250),
        move || {
            let Some(state) = weak.upgrade() else {
                return gtk::glib::ControlFlow::Break;
            };
            state.replies();
            gtk::glib::ControlFlow::Continue
        },
    ));
    InputView {
        root,
        header,
        state,
    }
}

#[allow(clippy::too_many_arguments)]
fn wire(
    state: &Rc<State>,
    window: &adw::ApplicationWindow,
    new_profile: &gtk::Button,
    duplicate: &gtk::Button,
    refresh: &gtk::Button,
    remove: &gtk::Button,
    choose: &gtk::Button,
    install: &gtk::Button,
) {
    state.ui.profiles.connect_selected_notify({
        let s = Rc::downgrade(state);
        move |drop| {
            let Some(s) = s.upgrade() else {
                return;
            };
            if s.syncing.get() {
                return;
            }
            // Capture the user's choice before saving: save() can refresh the
            // picker. Ignore notifications that only reflect the active profile.
            let id = {
                let library = s.library.borrow();
                library
                    .profiles
                    .get(drop.selected() as usize)
                    .filter(|profile| profile.id != library.active_profile)
                    .map(|profile| profile.id.clone())
            };
            if let Some(id) = id {
                if !s.flush() {
                    s.profiles();
                    return;
                }
                s.library.borrow_mut().active_profile = id;
                s.history.borrow_mut().reset(s.snapshot());
                s.select(None);
                s.profiles();
                s.source_list();
                s.changed();
            }
        }
    });
    state.ui.name.connect_changed({
        let s = Rc::downgrade(state);
        move |entry| {
            if let Some(s) = s.upgrade() {
                let name = entry.text().trim().to_string();
                s.edit(|p| p.name = name);
            }
        }
    });
    state.ui.name.connect_activate({
        let s = Rc::downgrade(state);
        move |entry| {
            if let Some(s) = s.upgrade() {
                let name = entry.text().trim().to_string();
                s.edit(|p| p.name = name);
                s.profiles();
            }
        }
    });
    for (button, copy) in [(new_profile, false), (duplicate, true)] {
        button.connect_clicked({
            let s = Rc::downgrade(state);
            move |_| {
                if let Some(s) = s.upgrade() {
                    if !s.flush() || s.load_error.is_some() {
                        return;
                    }
                    let mut library = s.library.borrow_mut();
                    if library.profiles.len() >= 128 {
                        s.ui.save_status
                            .set_text("Input profiles are limited to 128");
                        return;
                    }
                    let fresh = InputProfile::default();
                    let mut profile = if copy {
                        library.active().clone()
                    } else {
                        fresh.clone()
                    };
                    profile.id = fresh.id;
                    profile.name = if copy {
                        let mut name = profile.name.clone();
                        while name.len() > 123 {
                            name.pop();
                        }
                        format!("{name} copy")
                    } else {
                        format!("Microphone {}", library.profiles.len() + 1)
                    };
                    if !copy {
                        profile.source = library.active().source.clone();
                        profile.channels = library.active().channels.clone();
                    }
                    library.active_profile = profile.id.clone();
                    library.profiles.push(profile);
                    drop(library);
                    s.history.borrow_mut().reset(s.snapshot());
                    s.select(None);
                    s.profiles();
                    s.source_list();
                    s.changed();
                }
            }
        });
    }
    state.ui.delete_profile.connect_clicked({
        let s = Rc::downgrade(state);
        move |_| {
            if let Some(s) = s.upgrade() {
                if !s.flush() || s.load_error.is_some() {
                    return;
                }
                let mut library = s.library.borrow_mut();
                if library.profiles.len() <= 1 {
                    return;
                }
                let id = library.active_profile.clone();
                library.profiles.retain(|p| p.id != id);
                library.active_profile = library.profiles[0].id.clone();
                drop(library);
                s.history.borrow_mut().reset(s.snapshot());
                s.select(None);
                s.profiles();
                s.source_list();
                s.changed();
            }
        }
    });
    state.ui.sources.connect_selected_notify({
        let s = Rc::downgrade(state);
        move |drop| {
            if let Some(s) = s.upgrade() {
                if s.syncing.get() {
                    return;
                }
                let source = s
                    .source_names
                    .borrow()
                    .get(drop.selected() as usize)
                    .cloned()
                    .unwrap_or_default();
                let channels = s
                    .sources
                    .borrow()
                    .iter()
                    .find(|p| p.name == source)
                    .map(|p| p.channels.clone());
                s.edit(|p| {
                    p.source = source;
                    if let Some(channels) = channels {
                        p.channels = channels;
                    }
                });
            }
        }
    });
    refresh.connect_clicked({
        let s = Rc::downgrade(state);
        move |_| {
            if let Some(s) = s.upgrade() {
                s.refresh_sources();
            }
        }
    });
    state.ui.engine.connect_toggled({
        let s = Rc::downgrade(state);
        move |button| {
            if let Some(s) = s.upgrade() {
                s.engine(button.is_active());
            }
        }
    });
    install.connect_clicked({
        let window = window.clone();
        let status = state.ui.save_status.clone();
        move |_| {
            let dialog = adw::AlertDialog::new(Some("Install microphone processing"), Some("Install the optional host and effects once. Your terminal will ask for administrator authentication."));
            dialog.add_responses(&[("cancel", "Cancel"), ("copy", "Copy command"), ("install", "Open terminal")]);
            dialog.set_default_response(Some("install"));
            dialog.set_close_response("cancel");
            let status = status.clone();
            dialog.connect_response(None, move |_, response| {
                if response == "copy" && let Some(display) = gtk::gdk::Display::default() {
                    display.clipboard().set_text(INSTALL_COMMAND);
                    status.set_text("Install command copied. Paste it into your terminal.");
                }
                if response == "install" && !crate::update::install_input_support() {
                    status.set_text("Could not open a terminal. Use Install support → Copy command.");
                }
            });
            dialog.present(Some(&window));
        }
    });
    for (button, redo) in [(&state.ui.undo, false), (&state.ui.redo, true)] {
        button.connect_clicked({
            let s = Rc::downgrade(state);
            move |_| {
                if let Some(s) = s.upgrade() {
                    let snapshot = if redo {
                        s.history.borrow_mut().redo()
                    } else {
                        s.history.borrow_mut().undo()
                    };
                    if let Some(snapshot) = snapshot
                        && let Ok(profile) = serde_json::from_str::<InputProfile>(&snapshot.text)
                    {
                        *s.library.borrow_mut().active_mut() = profile;
                        s.select(None);
                        s.profiles();
                        s.source_list();
                        s.changed();
                    }
                }
            }
        });
    }
    state.ui.add_band.connect_clicked({
        let s = Rc::downgrade(state);
        move |_| {
            if let Some(s) = s.upgrade() {
                add_band(&s, 1000.0, 0.0);
            }
        }
    });
    remove.connect_clicked({
        let s = Rc::downgrade(state);
        move |_| {
            if let Some(s) = s.upgrade()
                && let Some(index) = s.selected.get()
            {
                s.select(None);
                s.edit(|p| {
                    p.filters.remove(index);
                });
            }
        }
    });
    for (control, which) in [
        (&state.ui.frequency, 0),
        (&state.ui.gain, 1),
        (&state.ui.q, 2),
    ] {
        control.knob.connect_value_changed({
            let s = Rc::downgrade(state);
            move |knob| {
                if let Some(s) = s.upgrade()
                    && let Some(index) = s.selected.get()
                {
                    s.edit(|p| {
                        let f = &mut p.filters[index];
                        match which {
                            0 => f.frequency = knob.value(),
                            1 => f.gain_db = knob.value(),
                            _ => f.set_q(knob.value()),
                        }
                    });
                }
            }
        });
        control.knob.connect_dragging({
            let s = Rc::downgrade(state);
            move |active| {
                if let Some(s) = s.upgrade() {
                    s.dragging.set(active);
                    s.history.borrow_mut().checkpoint();
                }
            }
        });
    }
    for (button, kind) in state.ui.kind.buttons.iter().zip(KINDS) {
        button.connect_toggled({
            let s = Rc::downgrade(state);
            move |button| {
                if button.is_active()
                    && let Some(s) = s.upgrade()
                    && let Some(index) = s.selected.get()
                {
                    s.edit(|p| p.filters[index].kind = kind);
                }
            }
        });
    }
    state.ui.band_enabled.connect_toggled({
        let s = Rc::downgrade(state);
        move |button| {
            if let Some(s) = s.upgrade()
                && let Some(index) = s.selected.get()
            {
                s.edit(|p| p.filters[index].enabled = button.is_active());
            }
        }
    });
    for (button, which) in [
        (&state.ui.eq, 0),
        (&state.ui.rnnoise, 1),
        (&state.ui.gate, 2),
        (&state.ui.compressor, 3),
    ] {
        button.connect_toggled({
            let s = Rc::downgrade(state);
            move |button| {
                if let Some(s) = s.upgrade() {
                    s.edit(|p| match which {
                        0 => p.eq_enabled = button.is_active(),
                        1 => p.rnnoise.enabled = button.is_active(),
                        2 => p.gate.enabled = button.is_active(),
                        _ => p.compressor.enabled = button.is_active(),
                    });
                }
            }
        });
    }
    for (spin, which) in [
        (&state.ui.gate_threshold, 0),
        (&state.ui.gate_attack, 1),
        (&state.ui.gate_release, 2),
        (&state.ui.threshold, 3),
        (&state.ui.ratio, 4),
        (&state.ui.attack, 5),
        (&state.ui.release, 6),
        (&state.ui.makeup, 7),
    ] {
        spin.connect_value_changed({
            let s = Rc::downgrade(state);
            move |spin| {
                if let Some(s) = s.upgrade() {
                    s.edit(|p| match which {
                        0 => p.gate.threshold_db = spin.value(),
                        1 => p.gate.attack_ms = spin.value(),
                        2 => p.gate.release_ms = spin.value(),
                        3 => p.compressor.threshold_db = spin.value(),
                        4 => p.compressor.ratio = spin.value(),
                        5 => p.compressor.attack_ms = spin.value(),
                        6 => p.compressor.release_ms = spin.value(),
                        _ => p.compressor.makeup_db = spin.value(),
                    });
                }
            }
        });
    }
    choose.connect_clicked({
        let s = Rc::downgrade(state);
        let window = window.clone();
        move |_| {
            let dialog = gtk::FileDialog::builder()
                .title("Choose a custom Carla rack")
                .build();
            let s = s.clone();
            dialog.open(Some(&window), gtk::gio::Cancellable::NONE, move |result| {
                if let (Some(s), Ok(file)) = (s.upgrade(), result)
                    && let Some(path) = file.path()
                {
                    s.edit(|p| p.advanced_project = Some(path));
                }
            });
        }
    });
    state.ui.clear_custom.connect_clicked({
        let s = Rc::downgrade(state);
        move |_| {
            if let Some(s) = s.upgrade() {
                s.edit(|p| p.advanced_project = None);
            }
        }
    });
    graph_input(state);
    let shortcuts = gtk::ShortcutController::new();
    shortcuts.set_scope(gtk::ShortcutScope::Managed);
    for (trigger, button) in [
        ("<Control>z", state.ui.undo.clone()),
        ("<Control><Shift>z", state.ui.redo.clone()),
    ] {
        shortcuts.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string(trigger),
            Some(gtk::CallbackAction::new(move |widget, _| {
                if !button.is_mapped()
                    || widget
                        .root()
                        .and_then(|r| r.focus())
                        .is_some_and(|f| f.is::<gtk::Text>() || f.is::<gtk::Entry>())
                {
                    return gtk::glib::Propagation::Proceed;
                }
                button.emit_clicked();
                gtk::glib::Propagation::Stop
            })),
        ));
    }
    state.ui.managed.add_controller(shortcuts);
}

fn add_band(state: &Rc<State>, frequency: f64, gain_db: f64) {
    if state.library.borrow().active().filters.len() >= MAX_INPUT_FILTERS {
        return;
    }
    let index = state.library.borrow().active().filters.len();
    state.edit(|p| {
        p.filters.push(Filter {
            enabled: true,
            kind: FilterKind::Peaking,
            frequency,
            gain_db,
            q: 1.0,
            bandwidth: None,
            channels: ChannelSelection::All,
        })
    });
    state.select(Some(index));
}

fn graph_input(state: &Rc<State>) {
    let click = gtk::GestureClick::new();
    click.set_button(1);
    click.connect_pressed({
        let s = Rc::downgrade(state);
        move |gesture, presses, x, y| {
            if let Some(s) = s.upgrade() {
                let area = &s.ui.graph;
                area.grab_focus();
                let hit = s.document.borrow().as_ref().and_then(|doc| {
                    crate::band_at(doc, x, y, area.width() as f64, area.height() as f64)
                });
                if let Some(index) = hit {
                    s.select(Some(index));
                } else if presses == 2
                    && let Some((f, g)) =
                        graph::values_at_position(x, y, area.width() as f64, area.height() as f64)
                {
                    add_band(&s, f, g);
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                }
            }
        }
    });
    state.ui.graph.add_controller(click);
    let drag = gtk::GestureDrag::new();
    let origin = Rc::new(Cell::new(None::<(u64, usize, f64, f64)>));
    drag.connect_drag_begin({
        let s = Rc::downgrade(state);
        let origin = origin.clone();
        move |_, x, y| {
            if let Some(s) = s.upgrade() {
                let hit = s.document.borrow().as_ref().and_then(|doc| {
                    crate::band_at(
                        doc,
                        x,
                        y,
                        s.ui.graph.width() as f64,
                        s.ui.graph.height() as f64,
                    )
                });

                s.dragging.set(hit.is_some());
                s.history.borrow_mut().checkpoint();
                if let Some(index) = hit {
                    s.select(Some(index));
                }
                origin.set(hit.map(|index| (s.graph_epoch.get(), index, x, y)));
            }
        }
    });
    drag.connect_drag_update({
        let s = Rc::downgrade(state);
        let origin = origin.clone();
        move |gesture, dx, dy| {
            if let (Some(s), Some((epoch, index, x, y))) = (s.upgrade(), origin.get())
                && epoch == s.graph_epoch.get()
                && let Some((f, g)) = graph::values_at_position(
                    x + dx,
                    y + dy,
                    s.ui.graph.width() as f64,
                    s.ui.graph.height() as f64,
                )
            {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                s.edit(|p| {
                    if let Some(filter) = p.filters.get_mut(index) {
                        filter.frequency = f;
                        filter.gain_db = g;
                    }
                });
            }
        }
    });
    drag.connect_drag_end({
        let s = Rc::downgrade(state);
        let origin = origin.clone();
        move |_, _, _| {
            origin.set(None);
            if let Some(s) = s.upgrade() {
                s.dragging.set(false);
                s.history.borrow_mut().checkpoint();
            }
        }
    });
    drag.connect_cancel({
        let s = Rc::downgrade(state);
        move |_, _| {
            origin.set(None);
            if let Some(s) = s.upgrade() {
                s.dragging.set(false);
                s.history.borrow_mut().checkpoint();
            }
        }
    });
    state.ui.graph.add_controller(drag);
    let pointer = Rc::new(Cell::new((0.0_f64, 0.0_f64)));
    let motion = gtk::EventControllerMotion::new();
    motion.connect_motion({
        let pointer = pointer.clone();
        move |_, x, y| pointer.set((x, y))
    });
    state.ui.graph.add_controller(motion);
    let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    scroll.connect_scroll({
        let s = Rc::downgrade(state);
        move |_, _, dy| {
            if let Some(s) = s.upgrade()
                && let Some(index) = s.selected.get()
            {
                let near = s
                    .document
                    .borrow()
                    .as_ref()
                    .and_then(|doc| doc.filters.get(index))
                    .is_some_and(|filter| {
                        let (x, y) = graph::filter_point_position(
                            filter.frequency,
                            filter.gain_db,
                            s.ui.graph.width() as f64,
                            s.ui.graph.height() as f64,
                        );
                        let (px, py) = pointer.get();
                        (px - x).hypot(py - y) <= 60.0
                    });
                if !near {
                    return gtk::glib::Propagation::Proceed;
                }
                s.edit(|p| {
                    let q = (p.filters[index].q * 1.1_f64.powf(-dy)).clamp(0.1, 20.0);
                    p.filters[index].set_q(q)
                });
                return gtk::glib::Propagation::Stop;
            }
            gtk::glib::Propagation::Proceed
        }
    });
    state.ui.graph.add_controller(scroll);
    let keys = gtk::EventControllerKey::new();
    keys.connect_key_pressed({
        let s = Rc::downgrade(state);
        move |_, key, _, modifiers| {
            let Some(s) = s.upgrade() else {
                return gtk::glib::Propagation::Proceed;
            };
            if modifiers
                .intersects(gtk::gdk::ModifierType::CONTROL_MASK | gtk::gdk::ModifierType::ALT_MASK)
            {
                return gtk::glib::Propagation::Proceed;
            }
            let Some(index) = s.selected.get() else {
                return gtk::glib::Propagation::Proceed;
            };
            if key == gtk::gdk::Key::Escape {
                s.select(None);
                return gtk::glib::Propagation::Stop;
            }
            if matches!(key, gtk::gdk::Key::Delete | gtk::gdk::Key::BackSpace) {
                s.select(None);
                s.edit(|p| {
                    p.filters.remove(index);
                });
                return gtk::glib::Propagation::Stop;
            }
            let direction = match key {
                gtk::gdk::Key::Up => graph::NudgeDirection::Up,
                gtk::gdk::Key::Down => graph::NudgeDirection::Down,
                gtk::gdk::Key::Left => graph::NudgeDirection::Left,
                gtk::gdk::Key::Right => graph::NudgeDirection::Right,
                _ => return gtk::glib::Propagation::Proceed,
            };
            s.edit(|p| {
                let filter = &mut p.filters[index];
                graph::nudge_filter(
                    filter,
                    direction,
                    modifiers.contains(gtk::gdk::ModifierType::SHIFT_MASK),
                    1.0,
                );
                filter.q = filter.q.clamp(0.1, 20.0);
            });
            gtk::glib::Propagation::Stop
        }
    });
    state.ui.graph.add_controller(keys);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires a display; run with xvfb-run and --ignored"]
    fn switching_input_profile_preserves_the_requested_selection_during_save() {
        adw::init().expect("GTK display");
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("input.json");
        let app = adw::Application::new(
            Some("org.massiveeq.InputRegression"),
            gtk::gio::ApplicationFlags::NON_UNIQUE,
        );
        app.register(gtk::gio::Cancellable::NONE).unwrap();
        let window = adw::ApplicationWindow::new(&app);
        let view = build_at(&window, Ok(path.clone()));
        let second = InputProfile {
            name: "Second microphone".into(),
            ..Default::default()
        };
        let second_id = second.id.clone();
        view.state.library.borrow_mut().profiles.push(second);
        view.state.profiles();
        view.state.edit(|p| p.gate.threshold_db = -32.0);
        assert!(view.state.dirty.get());
        view.state.ui.profiles.set_selected(1);
        let selected = view.state.library.borrow().active_profile.clone();
        view.flush();
        view.close();
        window.close();
        let saved = InputLibrary::load(&path).unwrap();

        assert_eq!(
            selected, second_id,
            "flushing a pending edit must not reset the requested profile"
        );
        assert_eq!(saved.active_profile, second_id);
        assert_eq!(saved.profiles[0].gate.threshold_db, -32.0);
    }

    #[test]
    fn estimated_input_latency_reflects_rnnoise_quantum_and_custom_racks() {
        let mut profile = InputProfile::default();
        let plain = estimate_input_latency(&profile, Some(48_000), Some(1_024));
        assert_eq!(input_latency_summary(&plain), "~21 ms");
        assert!(input_header_markup(&plain).contains("EST. DSP"));
        assert!(input_latency_tooltip(&plain).contains("0 ms EQ/gate/compressor"));

        profile.rnnoise.enabled = true;
        let rnnoise = estimate_input_latency(&profile, Some(48_000), Some(1_024));
        assert_eq!(input_latency_summary(&rnnoise), "~31 ms");
        assert!(input_latency_tooltip(&rnnoise).contains("10.0 ms RNNoise"));

        // Without PipeWire clock info yet, still shows RNNoise + buffer cleanly.
        let no_clock = estimate_input_latency(&profile, None, None);
        assert_eq!(input_latency_summary(&no_clock), "~10 ms + buffer");

        // Custom Carla racks mark unknown plugin latency with a trailing '+'.
        profile.advanced_project = Some(PathBuf::from("/tmp/rack.carxp"));
        let custom = estimate_input_latency(&profile, Some(48_000), Some(1_024));
        assert_eq!(input_latency_summary(&custom), "~21 ms+");
        assert!(input_latency_tooltip(&custom).contains("custom plugins may add more"));
    }

    #[test]
    fn runtime_input_latency_parses_live_service_metrics() {
        let mut profile = InputProfile::default();
        profile.rnnoise.enabled = true;
        let runtime = serde_json::json!({
            "state": "running",
            "backend": "managed",
            "latency": {
                "sample_rate": 48_000,
                "buffer_frames": 256,
                "plugin_latency_frames": 480,
                "plugin_latency_ms": 10.0,
                "routing_latency_ms": 5.33,
                "added_latency_ms": 15.33,
                "custom_plugins_unreported": false
            }
        });
        let live = parse_runtime_latency(&runtime, &profile, None, None).unwrap();
        assert!(live.live);
        assert_eq!(input_latency_summary(&live), "~15 ms");
        assert!(input_header_markup(&live).contains("48.0</span> kHz"));
        assert!(input_header_markup(&live).contains("DSP <span"));
        assert!(input_latency_tooltip(&live).contains("Input DSP adds roughly ~15 ms"));
    }
}
