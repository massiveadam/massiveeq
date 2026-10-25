mod client;
mod editing;
mod editor_tasks;
mod glyph;
mod graph;
mod knob;
mod live_client;
mod measurements;
mod update;

use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use massiveeq_core::{
    COMPARISON_BYPASS_ID, ChannelSelection, ComparisonSet, DeviceInfo, Filter, FilterKind,
    ImpulseLayout, MAX_COMPARISON_PROFILES, MAX_FILTERS_PER_CHANNEL, MAX_TILT_DB_PER_OCTAVE,
    ProfileAnalysis, ProfileDocument, ProfileInfo, analysis::ResponsePoint,
    analyze_profile_preview, parse_text, serialize_profile, sort_profiles,
};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
    time::{Duration, Instant},
};

use client::Client;

struct Model {
    tasks: editor_tasks::EditorTasks,
    client: Client,
    profiles: RefCell<Vec<ProfileInfo>>,
    devices: RefCell<Vec<DeviceInfo>>,
    comparisons: RefCell<HashMap<String, ComparisonSet>>,
    current_id: RefCell<Option<String>>,
    document: Rc<RefCell<Option<ProfileDocument>>>,
    analysis: Rc<RefCell<Option<ProfileAnalysis>>>,
    selected_filter: Rc<Cell<Option<usize>>>,
    bands: RefCell<Option<BandUi>>,
    listening: RefCell<Option<ListeningUi>>,
    graph_tone: RefCell<Option<GraphToneUi>>,
    curves: RefCell<massiveeq_core::Curves>,
    curve_context: RefCell<Option<gtk::Label>>,
    curves_ui: RefCell<Option<Rc<CurvesUi>>>,
    history: RefCell<editing::History>,
    reference: RefCell<Option<editing::Snapshot>>,
    restoring_history: Cell<bool>,
    syncing_text: Cell<bool>,
    last_submitted: RefCell<Option<String>>,
    bank_names_dirty: Cell<bool>,
    unsaved: Cell<bool>,
    hear_reference: Cell<bool>,
    /// Output gain held while the tone generator plays, taken when it
    /// started, so a band change is heard as itself rather than also moving
    /// the whole output's level through matching and safety.
    tone_hold: Cell<Option<f64>>,
    live_worker: live_client::LiveClient,
    live_revision: Cell<u64>,
    pending_live: RefCell<Option<gtk::glib::SourceId>>,
    graph_dragging: Cell<bool>,
    manual_trim: Cell<f64>,
    sample_rate: Cell<f64>,
    loading: Cell<bool>,
    syncing_device: Cell<bool>,
    pending_device_bypass: RefCell<Option<PendingDeviceBypass>>,
    syncing_engine: Cell<bool>,
    /// Engine switch is off, so no output is being filtered.
    engine_off: Cell<bool>,
    /// Debounced profile save; focus changes wait for it so edits land on the right profile.
    pending_save: Rc<RefCell<Option<gtk::glib::SourceId>>>,
    /// Output (storage key) the editor should jump to once no save is pending.
    pending_focus: RefCell<Option<String>>,
    /// Set while the device list is rebuilt so restoring the selection keeps profile focus.
    restoring_device: Cell<bool>,
    live: RefCell<Option<LiveIndicators>>,
    /// The impulse response's own curve per ear (service analysis minus the
    /// EQ), so live previews while dragging include it.
    ir_curve: RefCell<Option<[Vec<ResponsePoint>; 2]>>,
    overlay: Rc<RefCell<graph::Overlay>>,
    /// Last click, key press, or scroll anywhere in the window.
    last_input: Cell<Option<std::time::Instant>>,
    /// Set while the bank is rebuilt in place, so re-selecting the open profile
    /// does not reload it into the editor.
    reordering_profiles: Cell<bool>,
}

impl Model {
    fn stop_editor(&self) {
        self.tasks.close();
        if let Some(timer) = self.pending_save.borrow_mut().take() {
            timer.remove();
        }
        if let Some(timer) = self.pending_live.borrow_mut().take() {
            timer.remove();
        }
        self.live_worker.shutdown();
    }
}

/// Widgets that show which profile is actually playing.
#[derive(Clone)]
struct LiveIndicators {
    profile_status: gtk::Label,
    output_status: gtk::Label,
    profile_list: gtk::ListBox,
    device_drop: gtk::DropDown,
    /// Turns light while the open profile is playing.
    profile_tile: gtk::Box,
    /// Open menus hold off focus changes from newly connected outputs.
    menus: Vec<gtk::MenuButton>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PendingDeviceBypass {
    device_key: String,
    bypassed: bool,
}

#[derive(Clone)]
struct ConvolutionUi {
    file_name: gtk::Label,
    file_path: gtk::Label,
    channel: gtk::DropDown,
    choose: gtk::Button,
    remove: gtk::Button,
    syncing: Rc<Cell<bool>>,
    /// Layout of the open profile's impulse response, as reported by the service.
    layout: Rc<Cell<Option<ImpulseLayout>>>,
    /// Mark the editor tab for the visible page.
    sync_tabs: Rc<dyn Fn()>,
}

struct ComparisonUi {
    menu: gtk::MenuButton,
    stack: gtk::Stack,
    candidates: gtk::Box,
    checks: Rc<RefCell<Vec<(String, gtk::ToggleButton)>>>,
    listening: gtk::Box,
    save: gtk::Button,
    edit: gtk::Button,
    delete: gtk::Button,
    status: gtk::Label,
    device_state: gtk::Label,
    assign: gtk::Button,
    bypass: gtk::ToggleButton,
}

fn main() {
    let app = adw::Application::builder()
        .application_id("org.massiveeq.MassiveEQ")
        .build();
    app.connect_activate(build_ui);
    app.run();
}

fn build_ui(app: &adw::Application) {
    if let Some(window) = app.active_window() {
        window.present();
        return;
    }
    install_css();
    adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark);
    let client = match Client::connect() {
        Ok(client) => client,
        Err(error) => {
            show_startup_error(app, &error.to_string());
            return;
        }
    };
    let profiles = match client.profiles() {
        Ok(profiles) => profiles,
        Err(error) => {
            show_startup_error(app, &format!("Could not load profiles: {error}"));
            return;
        }
    };
    let devices = match client.devices() {
        Ok(devices) => devices,
        Err(error) => {
            show_startup_error(app, &format!("Could not load audio outputs: {error}"));
            return;
        }
    };
    let comparisons = match client.comparisons() {
        Ok(comparisons) => comparisons,
        Err(error) => {
            show_startup_error(app, &format!("Could not load comparison banks: {error}"));
            return;
        }
    };
    let engine_status = match client.status() {
        Ok(status) => status,
        Err(error) => {
            show_startup_error(app, &format!("Could not read audio engine status: {error}"));
            return;
        }
    };
    let active_sample_rate = engine_status
        .pointer("/engine/active/0/sample_rate")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(48_000) as f64;
    let model = Rc::new(Model {
        tasks: editor_tasks::EditorTasks::default(),
        profiles: RefCell::new(profiles),
        devices: RefCell::new(devices),
        comparisons: RefCell::new(comparisons),
        client: client.clone(),
        current_id: RefCell::new(None),
        document: Rc::new(RefCell::new(None)),
        analysis: Rc::new(RefCell::new(None)),
        selected_filter: Rc::new(Cell::new(None)),
        bands: RefCell::new(None),
        listening: RefCell::new(None),
        graph_tone: RefCell::new(None),
        curves: RefCell::new(massiveeq_core::Curves::default()),
        curve_context: RefCell::new(None),
        curves_ui: RefCell::new(None),
        history: RefCell::new(editing::History::default()),
        reference: RefCell::new(None),
        restoring_history: Cell::new(false),
        syncing_text: Cell::new(false),
        last_submitted: RefCell::new(None),
        bank_names_dirty: Cell::new(false),
        unsaved: Cell::new(false),
        hear_reference: Cell::new(false),
        tone_hold: Cell::new(None),
        live_worker: live_client::LiveClient::new(client.clone()),
        live_revision: Cell::new(0),
        pending_live: RefCell::new(None),
        graph_dragging: Cell::new(false),
        manual_trim: Cell::new(0.0),
        sample_rate: Cell::new(active_sample_rate),
        loading: Cell::new(false),
        syncing_device: Cell::new(false),
        pending_device_bypass: RefCell::new(None),
        syncing_engine: Cell::new(false),
        engine_off: Cell::new(false),
        pending_save: Rc::new(RefCell::new(None)),
        pending_focus: RefCell::new(None),
        restoring_device: Cell::new(false),
        live: RefCell::new(None),
        ir_curve: RefCell::new(None),
        overlay: Rc::new(RefCell::new(graph::Overlay::default())),
        last_input: Cell::new(None),
        reordering_profiles: Cell::new(false),
    });

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("MassiveEQ")
        .default_width(1280)
        .default_height(860)
        .build();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_css_class("instrument-root");
    let header = adw::HeaderBar::new();
    header.add_css_class("instrument-header");
    let brand = gtk::Label::new(Some("MASSIVE/EQ"));
    brand.add_css_class("brand");
    header.pack_start(&brand);
    // What plays where, and the engine's numbers, as one status strip.
    let header_status = gtk::Label::new(None);
    header_status.add_css_class("header-status");
    header_status.set_ellipsize(gtk::pango::EllipsizeMode::End);
    header.set_title_widget(Some(&header_status));

    let profile_popover = gtk::Popover::new();
    let profile_popover_box = gtk::Box::new(gtk::Orientation::Vertical, 10);
    profile_popover_box.set_margin_top(12);
    profile_popover_box.set_margin_bottom(12);
    profile_popover_box.set_margin_start(12);
    profile_popover_box.set_margin_end(12);
    profile_popover_box.set_size_request(320, -1);
    let profile_list = gtk::ListBox::new();
    profile_list.add_css_class("profile-bank");
    profile_list.set_selection_mode(gtk::SelectionMode::Single);
    let profile_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .min_content_height(110)
        .max_content_height(330)
        .child(&profile_list)
        .build();

    let profile_buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    profile_buttons.set_homogeneous(true);
    let add_button = gtk::Button::from_icon_name("list-add-symbolic");
    add_button.set_tooltip_text(Some("New profile"));
    let import_button = gtk::Button::from_icon_name("document-open-symbolic");
    import_button.set_tooltip_text(Some("Import .txt profile"));
    let duplicate_button = gtk::Button::from_icon_name("edit-copy-symbolic");
    duplicate_button.set_tooltip_text(Some("Duplicate profile"));
    let export_button = gtk::Button::from_icon_name("document-save-as-symbolic");
    export_button.set_tooltip_text(Some("Export profile"));
    let delete_button = gtk::Button::from_icon_name("user-trash-symbolic");
    delete_button.set_tooltip_text(Some("Delete profile"));
    profile_buttons.append(&add_button);
    profile_buttons.append(&import_button);
    profile_buttons.append(&duplicate_button);
    profile_buttons.append(&export_button);
    profile_buttons.append(&delete_button);
    profile_popover.set_child(Some(&profile_popover_box));

    let profile_menu = gtk::MenuButton::new();
    let profile_menu_label = gtk::Label::new(Some("Choose profile"));
    profile_menu_label.set_xalign(0.0);
    profile_menu_label.set_hexpand(true);
    profile_menu_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    profile_menu.set_child(Some(&profile_menu_label));
    profile_menu.set_always_show_arrow(true);
    profile_menu.set_popover(Some(&profile_popover));
    // Open on the list: focusing the name entry would rename on the first keypress.
    profile_popover.connect_show({
        let list = profile_list.clone();
        move |_| {
            let list = list.clone();
            gtk::glib::idle_add_local_once(move || {
                if let Some(row) = list.selected_row() {
                    row.grab_focus();
                }
            });
        }
    });
    profile_menu.add_css_class("preset-menu");
    profile_menu.update_property(&[gtk::accessible::Property::Label("Profiles")]);
    let header_add_profile = gtk::Button::from_icon_name("list-add-symbolic");
    header_add_profile.add_css_class("profile-add");
    header_add_profile.set_valign(gtk::Align::Center);
    header_add_profile.set_tooltip_text(Some("New profile"));
    let global_bypass = gtk::ToggleButton::new();
    global_bypass.set_icon_name("system-shutdown-symbolic");
    global_bypass.add_css_class("power");
    global_bypass.set_valign(gtk::Align::Center);
    global_bypass.set_active(
        !model
            .client
            .status()
            .ok()
            .and_then(|value| value.get("global_bypass").and_then(|value| value.as_bool()))
            .unwrap_or(false),
    );
    model.engine_off.set(!global_bypass.is_active());
    global_bypass.set_tooltip_text(Some(
        "Master DSP engine. Off is true dry audio at 0 dB with no EQ or level correction.",
    ));
    global_bypass.update_property(&[
        gtk::accessible::Property::Label("Engine enabled"),
        gtk::accessible::Property::Description(
            "Master DSP switch. Turning it off removes EQ and all gain correction.",
        ),
    ]);
    header.pack_end(&global_bypass);
    let global_bypass_label = gtk::Label::new(Some("ENGINE"));
    global_bypass_label.add_css_class("header-data");
    update::add_update_button(&header, &window);
    toolbar.add_top_bar(&header);

    let profile_status = gtk::Label::new(None);
    profile_status.add_css_class("tile-subtitle");
    profile_status.set_ellipsize(gtk::pango::EllipsizeMode::End);
    profile_status.set_xalign(0.0);
    let name_entry = gtk::Entry::builder()
        .placeholder_text("Profile name")
        .hexpand(true)
        .build();
    name_entry.add_css_class("preset-name");
    let output_status = gtk::Label::new(None);
    output_status.add_css_class("tile-subtitle");
    output_status.set_ellipsize(gtk::pango::EllipsizeMode::End);
    output_status.set_xalign(0.0);
    let device_strings = gtk::StringList::new(
        &model
            .devices
            .borrow()
            .iter()
            .map(|device| device.description.as_str())
            .collect::<Vec<_>>(),
    );
    let device_drop = gtk::DropDown::new(Some(device_strings.clone()), gtk::Expression::NONE);
    device_drop.set_size_request(1, -1);
    device_drop.add_css_class("output-dropdown");
    device_drop.set_factory(Some(&ellipsized_string_factory(24)));
    device_drop.set_list_factory(Some(&ellipsized_string_factory(48)));
    let device_state = gtk::Label::new(Some("Not applied"));
    device_state.set_xalign(0.0);
    device_state.add_css_class("dim-label");
    device_state.add_css_class("device-state");
    device_state.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let assign_button = gtk::Button::with_label("Apply to output");
    assign_button.add_css_class("assign-button");
    // The plugin bypass: lit while this output's filters run.
    let device_bypass = gtk::ToggleButton::new();
    device_bypass.add_css_class("footer-toggle");
    device_bypass.set_valign(gtk::Align::Center);
    {
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        content.append(&gtk::Image::from_icon_name("system-shutdown-symbolic"));
        content.append(&gtk::Label::new(Some("FILTERS")));
        device_bypass.set_child(Some(&content));
    }
    device_bypass.set_active(
        model
            .devices
            .borrow()
            .first()
            .is_some_and(|device| !device.bypassed),
    );
    device_bypass.set_tooltip_text(Some(
        "Turn filters on or off. Off retains the active profile's perceived gain correction for fair listening comparisons.",
    ));
    device_bypass.update_property(&[
        gtk::accessible::Property::Label("Filters enabled for selected output"),
        gtk::accessible::Property::Description(
            "Turning filters off retains level matching for a fair comparison.",
        ),
    ]);
    let comparison_popover = gtk::Popover::new();
    let comparison_box = gtk::Box::new(gtk::Orientation::Vertical, 10);
    comparison_box.set_margin_top(14);
    comparison_box.set_margin_bottom(14);
    comparison_box.set_margin_start(14);
    comparison_box.set_margin_end(14);
    comparison_box.set_size_request(310, 390);
    let comparison_title = gtk::Label::new(Some("PROFILE COMPARISON"));
    comparison_title.set_xalign(0.0);
    comparison_title.add_css_class("panel-title");
    comparison_box.append(&comparison_title);

    let comparison_stack = gtk::Stack::new();
    comparison_stack.set_transition_type(gtk::StackTransitionType::SlideLeftRight);
    comparison_stack.set_transition_duration(160);
    comparison_stack.set_vexpand(true);

    let comparison_setup = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let comparison_help = gtk::Label::new(Some("Choose 2–9 profiles to compare."));
    comparison_help.set_xalign(0.0);
    comparison_help.add_css_class("level-code");
    comparison_setup.append(&comparison_help);
    let comparison_candidates = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let comparison_scroll = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&comparison_candidates)
        .build();
    comparison_setup.append(&comparison_scroll);
    let comparison_save = gtk::Button::with_label("START COMPARISON");
    comparison_save.add_css_class("suggested-action");
    comparison_setup.append(&comparison_save);
    comparison_stack.add_named(&comparison_setup, Some("setup"));

    let comparison_listen = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let listening_help = gtk::Label::new(Some("Tap a profile to hear it immediately."));
    listening_help.set_xalign(0.0);
    listening_help.add_css_class("level-code");
    comparison_listen.append(&listening_help);
    let comparison_listening_buttons = gtk::Box::new(gtk::Orientation::Vertical, 3);
    let comparison_listening_scroll = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&comparison_listening_buttons)
        .build();
    comparison_listen.append(&comparison_listening_scroll);
    let comparison_actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    comparison_actions.set_homogeneous(true);
    let comparison_edit = gtk::Button::with_label("EDIT SET");
    let comparison_delete = gtk::Button::with_label("END COMPARE");
    comparison_actions.append(&comparison_edit);
    comparison_actions.append(&comparison_delete);
    comparison_listen.append(&comparison_actions);
    comparison_stack.add_named(&comparison_listen, Some("listen"));
    comparison_box.append(&comparison_stack);

    let comparison_status = gtk::Label::new(Some("No comparison bank"));
    comparison_status.set_xalign(0.0);
    comparison_status.set_ellipsize(gtk::pango::EllipsizeMode::End);
    comparison_status.add_css_class("level-code");
    comparison_box.append(&comparison_status);
    comparison_popover.set_child(Some(&comparison_box));
    let comparison_menu = gtk::MenuButton::new();
    comparison_menu.set_label("A/B");
    comparison_menu.set_popover(Some(&comparison_popover));
    comparison_menu.add_css_class("bar-menu");
    comparison_menu.set_tooltip_text(Some(
        "Hear the starting sound, or compare profiles at a shared BS.1770 K-weighted level",
    ));
    let comparison_ui = Rc::new(ComparisonUi {
        menu: comparison_menu,
        stack: comparison_stack,
        candidates: comparison_candidates,
        checks: Rc::new(RefCell::new(Vec::new())),
        listening: comparison_listening_buttons,
        save: comparison_save,
        edit: comparison_edit,
        delete: comparison_delete,
        status: comparison_status,
        device_state: device_state.clone(),
        assign: assign_button.clone(),
        bypass: device_bypass.clone(),
    });
    let curves_ui = build_curves_menu();
    *model.curves_ui.borrow_mut() = Some(curves_ui.clone());

    // Preset bar: step through the profile bank, or open it.
    let previous_profile = gtk::Button::from_icon_name("go-previous-symbolic");
    previous_profile.set_tooltip_text(Some("Previous profile"));
    previous_profile.update_property(&[gtk::accessible::Property::Label("Previous profile")]);
    let next_profile = gtk::Button::from_icon_name("go-next-symbolic");
    next_profile.set_tooltip_text(Some("Next profile"));
    next_profile.update_property(&[gtk::accessible::Property::Label("Next profile")]);
    let preset = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    preset.add_css_class("linked");
    preset.add_css_class("preset");
    preset.set_valign(gtk::Align::Center);
    preset.append(&previous_profile);
    preset.append(&profile_menu);
    preset.append(&next_profile);
    header.pack_start(&preset);
    for (button, step) in [(&previous_profile, -1_i32), (&next_profile, 1)] {
        let list = profile_list.clone();
        button.connect_clicked(move |_| {
            let count = {
                let mut count = 0;
                while list.row_at_index(count).is_some() {
                    count += 1;
                }
                count
            };
            if count == 0 {
                return;
            }
            let current = list.selected_row().map_or(0, |row| row.index());
            let next = (current + step).rem_euclid(count);
            if let Some(row) = list.row_at_index(next) {
                list.select_row(Some(&row));
            }
        });
    }
    let history_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    history_box.set_valign(gtk::Align::Center);
    header.pack_start(&history_box);
    comparison_ui.menu.set_valign(gtk::Align::Center);
    header.pack_start(&comparison_ui.menu);
    header.pack_end(&assign_button);
    assign_button.set_valign(gtk::Align::Center);
    header.pack_end(&device_drop);
    device_drop.set_valign(gtk::Align::Center);
    profile_popover_box.append(&name_entry);
    profile_popover_box.append(&profile_status);
    profile_popover_box.append(&profile_scroll);
    profile_buttons.add_css_class("bank-actions");
    profile_popover_box.append(&profile_buttons);

    // The graph is the instrument; the selected band's panel floats on it.
    let graph = graph::response_graph(
        model.analysis.clone(),
        model.document.clone(),
        model.selected_filter.clone(),
        model.overlay.clone(),
    );
    graph.set_focusable(true);
    graph.set_focus_on_click(true);
    graph.set_accessible_role(gtk::AccessibleRole::Group);
    graph.add_css_class("response-graph");
    graph.update_property(&[
        gtk::accessible::Property::Label("Frequency response graph"),
        gtk::accessible::Property::Description(
            "Logarithmic 20 hertz to 20 kilohertz response from minus 10 to plus 10 decibels. Double-click empty graph space to add a filter. Up and Down change the selected filter's gain by 0.1 decibels, or 1 with Shift; Left and Right change its frequency, or its Q with Shift.",
        ),
    ]);
    graph.set_vexpand(true);
    // With filters or the engine off, dim the graph: what it shows is not what plays.
    let dim_graph = {
        let graph = graph.clone();
        let device_bypass = device_bypass.clone();
        let global_bypass = global_bypass.clone();
        move || {
            if device_bypass.is_active() && global_bypass.is_active() {
                graph.remove_css_class("bypassed");
            } else {
                graph.add_css_class("bypassed");
            }
        }
    };
    dim_graph();
    let dim_graph = Rc::new(dim_graph);
    for button in [&device_bypass, &global_bypass] {
        let dim_graph = dim_graph.clone();
        button.connect_active_notify(move |_| dim_graph());
    }
    let graph_overlay = gtk::Overlay::new();
    graph_overlay.add_css_class("graph-stage");
    graph_overlay.set_child(Some(&graph));
    let analysis_label = gtk::Label::new(Some("AUTO PREAMP —"));
    analysis_label.add_css_class("readouts");
    analysis_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    analysis_label.set_halign(gtk::Align::End);
    analysis_label.set_valign(gtk::Align::Start);
    analysis_label.set_margin_end(30);
    analysis_label.set_margin_top(34);
    analysis_label.set_can_target(false);
    graph_overlay.add_overlay(&analysis_label);
    let status = gtk::Label::new(Some("Ready"));
    status.set_xalign(0.0);
    status.set_wrap(true);
    status.add_css_class("console-status");
    let curve_context = gtk::Label::new(None);
    curve_context.set_wrap(true);
    curve_context.set_xalign(0.0);
    curve_context.add_css_class("level-code");
    curve_context.set_visible(false);
    *model.curve_context.borrow_mut() = Some(curve_context.clone());
    let notes = gtk::Box::new(gtk::Orientation::Vertical, 2);
    notes.set_halign(gtk::Align::Start);
    notes.set_valign(gtk::Align::End);
    notes.set_margin_start(66);
    notes.set_margin_bottom(44);
    notes.set_can_target(false);
    // The header already shows the route; status shows only when it has news.
    device_state.set_visible(false);
    notes.append(&device_state);
    status.connect_label_notify(|label| {
        let text = label.text();
        label.set_visible(!text.is_empty() && text != "Ready");
    });
    status.set_visible(false);
    notes.append(&status);
    notes.append(&curve_context);
    graph_overlay.add_overlay(&notes);
    header_status.set_markup(&header_markup(&engine_status, &model));
    header_status.set_tooltip_text(Some(&engine_tooltip(&engine_status)));

    let output = knob::LabeledKnob::new(
        knob::Knob::new(-24.0, 24.0, 0.0, knob::Taper::Linear, true),
        "Output",
        |value| signed_db(value, 1),
    );
    output.knob.set_tooltip_text(Some(
        "Fine-tune the perceptually matched preamp; safety attenuation still prevents clipping",
    ));
    let trim = output.knob.clone();
    let (tone_page, tone_ui) = build_tone_page();
    let (graph_listen, graph_tone) = build_graph_tone(&tone_ui, &model.tasks);
    let listen_play = graph_tone.play.clone();
    *model.graph_tone.borrow_mut() = Some(graph_tone);

    let text_view = gtk::TextView::new();
    text_view.set_monospace(true);
    text_view.set_wrap_mode(gtk::WrapMode::None);
    text_view.buffer().set_enable_undo(true);
    let text_scroll = gtk::ScrolledWindow::builder()
        .min_content_height(420)
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .child(&text_view)
        .build();
    text_scroll.add_css_class("code-editor");
    let main_stack = gtk::Stack::new();
    main_stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    main_stack.set_transition_duration(120);
    main_stack.add_named(&graph_overlay, Some("graph"));
    let text_page = gtk::Box::new(gtk::Orientation::Vertical, 0);
    text_page.add_css_class("text-page");
    text_scroll.set_vexpand(true);
    text_page.append(&text_scroll);
    main_stack.add_named(&text_page, Some("text"));

    // Band chips select bands; the panel on the graph edits them.
    // Chips wrap onto a second row when they outgrow the footer; past two
    // rows the strip scrolls rather than growing the footer, unless the
    // footer is dragged taller, which shows more rows.
    let filter_list = gtk::FlowBox::new();
    filter_list.set_orientation(gtk::Orientation::Horizontal);
    filter_list.set_min_children_per_line(1);
    filter_list.set_max_children_per_line(64);
    filter_list.set_selection_mode(gtk::SelectionMode::None);
    filter_list.set_column_spacing(6);
    filter_list.set_row_spacing(BAND_CHIP_SPACING as u32);
    filter_list.set_valign(gtk::Align::Start);
    filter_list.set_halign(gtk::Align::Start);
    filter_list.add_css_class("band-chips");
    let chips_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .propagate_natural_height(true)
        .min_content_height(BAND_CHIP_HEIGHT)
        .max_content_height(2 * BAND_CHIP_HEIGHT + BAND_CHIP_SPACING)
        .propagate_natural_width(true)
        // Not from the chips: the add button follows the last chip.
        .hexpand(false)
        .vexpand(true)
        .child(&filter_list)
        .build();
    let add_filter_button = gtk::Button::from_icon_name("list-add-symbolic");
    add_filter_button.add_css_class("band-chip");
    add_filter_button.add_css_class("add");
    add_filter_button.set_valign(gtk::Align::Center);
    add_filter_button.set_tooltip_text(Some("Add band"));
    add_filter_button.update_property(&[gtk::accessible::Property::Label("Add band")]);
    build_band_editor(
        &model,
        &text_view.buffer(),
        &graph,
        &filter_list,
        &graph_overlay,
        &add_filter_button,
    );

    let convolution_page = gtk::Box::new(gtk::Orientation::Vertical, 12);
    convolution_page.set_size_request(380, -1);
    let convolution_card = gtk::Box::new(gtk::Orientation::Vertical, 10);
    convolution_card.add_css_class("convolution-card");
    let convolution_heading = gtk::Label::new(Some("IMPULSE RESPONSE"));
    convolution_heading.set_xalign(0.0);
    convolution_heading.add_css_class("section-label");
    convolution_card.append(&convolution_heading);
    let convolution_file_name = gtk::Label::new(Some("No impulse response selected"));
    convolution_file_name.set_xalign(0.0);
    convolution_file_name.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    convolution_file_name.add_css_class("convolution-name");
    convolution_card.append(&convolution_file_name);
    let convolution_file_path = gtk::Label::new(Some("WAV, FLAC, AIFF, or OGG · up to 10 seconds"));
    convolution_file_path.set_xalign(0.0);
    convolution_file_path.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    convolution_file_path.add_css_class("level-code");
    convolution_card.append(&convolution_file_path);
    let convolution_options = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let convolution_channel_label = gtk::Label::new(Some("OUTPUT CHANNELS"));
    convolution_channel_label.add_css_class("section-label");
    let convolution_channels = gtk::StringList::new(&["ALL", "L", "R"]);
    let convolution_channel = gtk::DropDown::new(Some(convolution_channels), gtk::Expression::NONE);
    convolution_channel.set_selected(0);
    convolution_channel.set_hexpand(true);
    convolution_options.append(&convolution_channel_label);
    convolution_options.append(&convolution_channel);
    convolution_card.append(&convolution_options);
    let convolution_actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let choose_convolution = gtk::Button::with_label("CHOOSE IMPULSE RESPONSE");
    choose_convolution.add_css_class("suggested-action");
    choose_convolution.set_hexpand(true);
    let remove_convolution = gtk::Button::with_label("REMOVE");
    remove_convolution.add_css_class("destructive-action");
    remove_convolution.set_sensitive(false);
    convolution_actions.append(&choose_convolution);
    convolution_actions.append(&remove_convolution);
    convolution_card.append(&convolution_actions);
    let convolution_note = gtk::Label::new(Some(
        "Parametric filters and GraphicEQ apply on top of the impulse response, per ear.",
    ));
    convolution_note.set_xalign(0.0);
    convolution_note.set_wrap(true);
    convolution_note.add_css_class("level-code");
    convolution_card.append(&convolution_note);
    convolution_page.append(&convolution_card);
    let ir_popover = gtk::Popover::new();
    ir_popover.set_child(Some(&convolution_page));
    let ir_menu = gtk::MenuButton::new();
    ir_menu.set_label("IR");
    ir_menu.set_popover(Some(&ir_popover));
    ir_menu.add_css_class("footer-menu");
    ir_menu.set_tooltip_text(Some("Impulse response"));
    let text_toggle = gtk::ToggleButton::with_label("TEXT");
    text_toggle.add_css_class("footer-flat");
    text_toggle.set_tooltip_text(Some("Edit the profile as Equalizer APO text (Ctrl+E)"));
    text_toggle.connect_toggled({
        let stack = main_stack.clone();
        move |button| {
            stack.set_visible_child_name(if button.is_active() { "text" } else { "graph" });
        }
    });
    curves_ui.menu.add_css_class("footer-menu");
    let reset_filters_button = gtk::Button::with_label("Flatten all bands");
    reset_filters_button.add_css_class("popover-action");
    reset_filters_button.set_tooltip_text(Some(
        "Flatten every parametric band to 0 dB without changing its frequency or Q",
    ));
    profile_popover_box.append(&reset_filters_button);
    // The IR button lights while the open profile has an impulse response.
    let sync_tabs: Rc<dyn Fn()> = Rc::new({
        let model = Rc::downgrade(&model);
        let ir_menu = ir_menu.clone();
        move || {
            let has_ir = model.upgrade().is_some_and(|model| {
                model
                    .document
                    .borrow()
                    .as_ref()
                    .is_some_and(|document| !document.convolutions.is_empty())
            });
            if has_ir {
                ir_menu.add_css_class("on");
            } else {
                ir_menu.remove_css_class("on");
            }
        }
    });
    let convolution_ui = ConvolutionUi {
        file_name: convolution_file_name,
        file_path: convolution_file_path,
        channel: convolution_channel,
        choose: choose_convolution,
        remove: remove_convolution,
        syncing: Rc::new(Cell::new(false)),
        layout: Rc::new(Cell::new(None)),
        sync_tabs: sync_tabs.clone(),
    };

    // Listening: the toggle plays a tone at the graph's tone line; its menu
    // holds the sweep, signal, and one-click corrections.
    let listen_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
    listen_box.set_size_request(400, -1);
    listen_box.append(&graph_listen);
    // TONE GEN in the footer plays and stops; the page's own button would repeat it.
    tone_ui.play.set_visible(false);
    listen_box.append(&tone_page);
    let listen_popover = gtk::Popover::new();
    listen_popover.set_child(Some(&listen_box));
    let listen_menu = gtk::MenuButton::new();
    listen_menu.set_icon_name("pan-up-symbolic");
    listen_menu.set_popover(Some(&listen_popover));
    listen_menu.set_tooltip_text(Some("Sweep, signal, and corrections"));
    listen_menu.update_property(&[gtk::accessible::Property::Label("Sweep and signal")]);
    listen_play.add_css_class("footer-toggle");
    let listen_group = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    listen_group.add_css_class("linked");
    listen_group.add_css_class("listen-group");
    listen_group.set_valign(gtk::Align::Center);
    listen_group.append(&listen_play);
    listen_group.append(&listen_menu);

    // Footer, as on a plugin: bypass and listening, the bands, the macro
    // knobs, and the secondary editors.
    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    footer.add_css_class("footer");
    let footer_start = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    footer_start.set_valign(gtk::Align::Start);
    footer_start.append(&device_bypass);
    footer_start.append(&listen_group);
    footer.append(&footer_start);
    let bands_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    bands_row.set_hexpand(true);
    bands_row.append(&chips_scroll);
    footer.append(&bands_row);
    let footer_end = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    footer_end.set_valign(gtk::Align::Start);
    let macro_knobs = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    footer_end.append(&macro_knobs);
    let editors = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    editors.set_valign(gtk::Align::Center);
    editors.append(&text_toggle);
    editors.append(&ir_menu);
    editors.append(&curves_ui.menu);
    footer_end.append(&editors);
    footer.append(&footer_end);
    // The line above the footer drags: up for more rows of band chips, down
    // to give the graph the room back. The footer never shrinks below its
    // own controls.
    let footer_split = gtk::Paned::builder()
        .orientation(gtk::Orientation::Vertical)
        .start_child(&main_stack)
        .resize_start_child(true)
        .shrink_start_child(false)
        .end_child(&footer)
        .resize_end_child(false)
        .shrink_end_child(false)
        .build();
    footer_split.add_css_class("footer-split");
    toolbar.set_content(Some(&footer_split));
    window.set_content(Some(&toolbar));

    // The status strip gives way first; narrow windows stack the footer.
    let crowded = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MaxWidth,
        1180.0,
        adw::LengthUnit::Px,
    ));
    let narrow = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MaxWidth,
        820.0,
        adw::LengthUnit::Px,
    ));
    for breakpoint in [&crowded, &narrow] {
        breakpoint.add_setter(&header_status, "visible", Some(&false.to_value()));
    }
    narrow.add_setter(
        &footer,
        "orientation",
        Some(&gtk::Orientation::Vertical.to_value()),
    );
    narrow.add_setter(&brand, "visible", Some(&false.to_value()));
    narrow.add_setter(&history_box, "visible", Some(&false.to_value()));
    narrow.add_setter(&previous_profile, "visible", Some(&false.to_value()));
    narrow.add_setter(&next_profile, "visible", Some(&false.to_value()));
    // The output picker moves from the crowded header to the footer.
    narrow.connect_apply({
        let header = header.clone();
        let footer_start = footer_start.clone();
        let device_drop = device_drop.clone();
        move |_| {
            header.remove(&device_drop);
            footer_start.append(&device_drop);
        }
    });
    narrow.connect_unapply({
        let header = header.clone();
        let footer_start = footer_start.clone();
        let device_drop = device_drop.clone();
        move |_| {
            footer_start.remove(&device_drop);
            // Packed last at the end, it returns to the left of Unassign.
            header.pack_end(&device_drop);
        }
    });
    window.add_breakpoint(crowded);
    window.add_breakpoint(narrow);
    profile_list.connect_row_activated({
        let popover = profile_popover.clone();
        move |_, _| popover.popdown()
    });

    let listening_controls = build_listening_controls(
        &window,
        &model,
        &text_view.buffer(),
        &filter_list,
        &graph,
        &trim,
        &status,
    );
    history_box.append(&listening_controls.undo);
    history_box.append(&listening_controls.redo);
    // A/B: the starting sound first, then the profile comparison bank.
    if let Some(comparison_box) = comparison_ui.stack.parent().and_downcast::<gtk::Box>() {
        let reference_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        reference_row.set_homogeneous(true);
        reference_row.append(&listening_controls.reference);
        reference_row.append(&listening_controls.capture);
        comparison_box.prepend(&gtk::Separator::new(gtk::Orientation::Horizontal));
        comparison_box.prepend(&reference_row);
        let reference_title = gtk::Label::new(Some("STARTING SOUND"));
        reference_title.set_xalign(0.0);
        reference_title.add_css_class("panel-title");
        comparison_box.prepend(&reference_title);
    }
    macro_knobs.append(&listening_controls.bass.root);
    macro_knobs.append(&listening_controls.tilt.root);
    macro_knobs.append(&output.root);
    let completion_timer = gtk::glib::timeout_add_local(std::time::Duration::from_millis(20), {
        let model = model.clone();
        let graph = graph.clone();
        let analysis_label = analysis_label.clone();
        let status = status.clone();
        move || {
            for result in model.live_worker.completions() {
                if result.id.is_empty() {
                    if let Some(listener) = model.graph_tone.borrow().as_ref() {
                        listener.tone.play.set_active(false);
                    }
                    if let Some(error) = result.error {
                        status.set_text(&error);
                    }
                    continue;
                }
                let current = model.current_id.borrow().as_deref() == Some(&result.id)
                    && result.revision == model.live_revision.get();
                if current {
                    if result.saved.is_some() {
                        model.unsaved.set(false);
                    }
                    if result.saved.is_some() && model.bank_names_dirty.replace(false) {
                        resort_profiles(&model);
                    }
                    if let Some(error) = result.error {
                        status.set_text(&error);
                    } else if result.saved.is_some() {
                        status.set_text(if model.tone_hold.get().is_some() {
                            "Saved and applied · level held for the tone"
                        } else {
                            "Saved and applied · comparison level matched"
                        });
                    }
                    if let Some(analysis) = result.analysis {
                        store_service_analysis(&model, analysis, &analysis_label);
                        graph.queue_draw();
                    }
                }
            }
            gtk::glib::ControlFlow::Continue
        }
    });
    model.tasks.track(completion_timer);
    let preview_timer = gtk::glib::timeout_add_seconds_local(2, {
        let model = model.clone();
        move || {
            if model.current_id.borrow().is_some() {
                let save = model.unsaved.get()
                    && model.pending_save.borrow().is_none()
                    && !model.graph_dragging.get();
                submit_live(&model, save);
            }
            gtk::glib::ControlFlow::Continue
        }
    });
    model.tasks.track(preview_timer);
    window.connect_close_request({
        let model = model.clone();
        let status = status.clone();
        move |_| {
            if model.tasks.is_closed() {
                return gtk::glib::Propagation::Proceed;
            }
            if let Some(timer) = model.pending_save.borrow_mut().take() {
                timer.remove();
            }
            if let Some(timer) = model.pending_live.borrow_mut().take() {
                timer.remove();
            }
            submit_live(&model, true);
            model.live_worker.stop_tone();
            if !model.live_worker.flush() {
                status.set_text("Changes are still waiting to save. Try closing again when the audio service is ready.");
                return gtk::glib::Propagation::Stop;
            }
            let mut save_error = None;
            for result in model.live_worker.completions() {
                if model.current_id.borrow().as_deref() == Some(&result.id)
                    && result.revision == model.live_revision.get()
                {
                    if result.saved.is_some() {
                        model.unsaved.set(false);
                    }
                    save_error = result.error;
                }
            }
            if model.unsaved.get() {
                status.set_text(&format!(
                    "Changes could not be saved. {}",
                    save_error.as_deref().unwrap_or("Correct the profile before closing.")
                ));
                return gtk::glib::Propagation::Stop;
            }
            if let Some(id) = model.current_id.borrow().as_ref() {
                let _ = model.client.end_preview(id);
            }
            model.stop_editor();
            gtk::glib::Propagation::Proceed
        }
    });
    window.connect_destroy({
        let model = model.clone();
        move |_| model.stop_editor()
    });

    *model.live.borrow_mut() = Some(LiveIndicators {
        profile_status,
        output_status,
        profile_list: profile_list.clone(),
        device_drop: device_drop.clone(),
        profile_tile: preset.clone(),
        menus: vec![
            profile_menu.clone(),
            listen_menu.clone(),
            ir_menu.clone(),
            curves_ui.menu.clone(),
            comparison_ui.menu.clone(),
        ],
    });
    // Seen in the capture phase, before any widget can consume the event.
    let input_watch = gtk::EventControllerLegacy::new();
    input_watch.set_propagation_phase(gtk::PropagationPhase::Capture);
    input_watch.connect_event({
        let model = model.clone();
        move |_, event| {
            if matches!(
                event.event_type(),
                gtk::gdk::EventType::ButtonPress
                    | gtk::gdk::EventType::KeyPress
                    | gtk::gdk::EventType::Scroll
                    | gtk::gdk::EventType::TouchBegin
            ) {
                model.last_input.set(Some(std::time::Instant::now()));
            }
            gtk::glib::Propagation::Proceed
        }
    });
    window.add_controller(input_watch);
    repopulate_profiles(&profile_list, &model);
    wire_actions(
        &window,
        &model,
        &profile_list,
        &filter_list,
        &text_view,
        &name_entry,
        &analysis_label,
        &graph,
        &status,
        &profile_menu,
        &profile_popover,
        &device_drop,
        &device_state,
        &add_button,
        &header_add_profile,
        &add_filter_button,
        &reset_filters_button,
        &convolution_ui,
        &import_button,
        &duplicate_button,
        &export_button,
        &delete_button,
        &assign_button,
        &device_bypass,
        &global_bypass,
        &comparison_ui,
        &trim,
    );
    wire_curves(&window, &model, &curves_ui, &graph, &status);
    wire_tone(
        &window,
        &model,
        &tone_ui,
        &device_drop,
        &filter_list,
        &text_view,
        &analysis_label,
        &graph,
        &status,
    );
    // The text editor covers the graph, so its tone line stops with it.
    main_stack.connect_visible_child_name_notify({
        let tone = tone_ui.clone();
        move |stack| {
            if stack.visible_child_name().as_deref() != Some("graph") {
                tone.play.set_active(false);
            }
        }
    });
    let poll_timer = gtk::glib::timeout_add_local(std::time::Duration::from_secs(2), {
        let model = model.clone();
        let strings = device_strings.clone();
        let drop = device_drop.clone();
        let state = device_state.clone();
        let assign = assign_button.clone();
        let bypass = device_bypass.clone();
        let engine = global_bypass.clone();
        let engine_readout = header_status.clone();
        let comparison_ui = comparison_ui.clone();
        let profile_list = profile_list.clone();
        move || {
            let Some(poll) = model.live_worker.take_poll() else {
                return gtk::glib::ControlFlow::Continue;
            };
            let poll = match poll {
                Ok(poll) => poll,
                Err(error) => {
                    engine_readout.set_text("Audio service unavailable");
                    engine_readout.set_tooltip_text(Some(&error));
                    engine.set_sensitive(false);
                    return gtk::glib::ControlFlow::Continue;
                }
            };
            {
                let devices = poll.devices;
                if let Some(index) = device_focus_target(&model.devices.borrow(), &devices) {
                    *model.pending_focus.borrow_mut() = Some(devices[index].key.as_storage_key());
                }
                let (selected_key, old_identity) = {
                    let current = model.devices.borrow();
                    (
                        current
                            .get(drop.selected() as usize)
                            .map(|device| device.key.as_storage_key()),
                        current
                            .iter()
                            .map(|device| (device.key.as_storage_key(), device.description.clone()))
                            .collect::<Vec<_>>(),
                    )
                };
                let new_identity = devices
                    .iter()
                    .map(|device| (device.key.as_storage_key(), device.description.clone()))
                    .collect::<Vec<_>>();
                let list_changed = old_identity != new_identity;
                *model.devices.borrow_mut() = devices;
                if list_changed {
                    let names = new_identity
                        .iter()
                        .map(|(_, description)| description.as_str())
                        .collect::<Vec<_>>();
                    strings.splice(0, strings.n_items(), &names);
                    if let Some(key) = selected_key
                        && let Some(index) = model
                            .devices
                            .borrow()
                            .iter()
                            .position(|device| device.key.as_storage_key() == key)
                    {
                        model.restoring_device.set(true);
                        drop.set_selected(index as u32);
                        model.restoring_device.set(false);
                    }
                }
                *model.comparisons.borrow_mut() = poll.comparisons;
                apply_pending_focus(&model, &drop, &profile_list, true);
                update_device_controls(&model, &drop, &state, &assign, &bypass);
                // The candidate rows are an unsaved draft while this popover
                // is open. Rebuilding them on the health poll loses clicks
                // and can destroy the widget currently handling input.
                if !comparison_ui.menu.is_active() {
                    refresh_comparison_ui(&model, &drop, &comparison_ui);
                }
            }
            let service_status = poll.status;
            if let Some(engine_off) = service_status
                .get("global_bypass")
                .and_then(serde_json::Value::as_bool)
            {
                engine_readout.set_markup(&header_markup(&service_status, &model));
                engine_readout.set_tooltip_text(Some(&engine_tooltip(&service_status)));
                engine.set_sensitive(true);
                model.syncing_engine.set(true);
                engine.set_active(!engine_off);
                model.syncing_engine.set(false);
                if model.engine_off.replace(engine_off) != engine_off {
                    update_live_indicators(&model);
                }
            } else {
                engine_readout
                    .set_markup("<span foreground='#ff7a1a'>AUDIO SERVICE UNAVAILABLE</span>");
                engine.set_sensitive(false);
            }
            gtk::glib::ControlFlow::Continue
        }
    });
    model.tasks.track(poll_timer);
    // Open on the output desktop audio is using, with its profile in the editor.
    let preferred = preferred_device_index(&model.devices.borrow());
    if let Some(index) = preferred {
        device_drop.set_selected(index as u32);
    }
    focus_device_profile(&model, device_drop.selected(), &profile_list);
    if profile_list.selected_row().is_none()
        && let Some(row) = profile_list.row_at_index(0)
    {
        profile_list.select_row(Some(&row));
    }
    update_device_controls(
        &model,
        &device_drop,
        &device_state,
        &assign_button,
        &device_bypass,
    );
    refresh_comparison_ui(&model, &device_drop, &comparison_ui);
    install_comparison_shortcuts(
        &window,
        &model,
        &device_drop,
        &device_bypass,
        &comparison_ui,
    );
    install_editor_shortcut(&window, &text_toggle);
    window.present();
    graph.grab_focus();
}

#[allow(clippy::too_many_arguments)]
fn wire_actions(
    window: &adw::ApplicationWindow,
    model: &Rc<Model>,
    profile_list: &gtk::ListBox,
    filter_list: &gtk::FlowBox,
    text_view: &gtk::TextView,
    name_entry: &gtk::Entry,
    analysis_label: &gtk::Label,
    graph: &gtk::DrawingArea,
    status: &gtk::Label,
    profile_menu: &gtk::MenuButton,
    profile_popover: &gtk::Popover,
    device_drop: &gtk::DropDown,
    device_state: &gtk::Label,
    add: &gtk::Button,
    header_add_profile: &gtk::Button,
    add_filter: &gtk::Button,
    reset_filters: &gtk::Button,
    convolution_ui: &ConvolutionUi,
    import: &gtk::Button,
    duplicate: &gtk::Button,
    export: &gtk::Button,
    delete: &gtk::Button,
    assign: &gtk::Button,
    device_bypass: &gtk::ToggleButton,
    global_bypass: &gtk::ToggleButton,
    comparison_ui: &Rc<ComparisonUi>,
    trim: &knob::Knob,
) {
    let buffer = text_view.buffer();
    add_filter.connect_clicked({
        let model = model.clone();
        let buffer = buffer.clone();
        let list = filter_list.clone();
        let graph = graph.clone();
        let status = status.clone();
        let convolution_ui = convolution_ui.clone();
        move |_| {
            let (text, snapshot) = {
                let mut document = model.document.borrow_mut();
                let Some(document) = document.as_mut() else {
                    return;
                };
                if !can_add_filter(document, ChannelSelection::All) {
                    status.set_text("This profile already has 64 bands on one channel");
                    return;
                }
                document.graphic_eqs.clear();
                document.filters.push(Filter {
                    enabled: true,
                    kind: FilterKind::Peaking,
                    frequency: 1000.0,
                    gain_db: 0.0,
                    q: 1.0,
                    channels: ChannelSelection::All,
                });
                model.selected_filter.set(Some(document.filters.len() - 1));
                (serialize_profile(document), document.clone())
            };
            set_editor_text(&model, &buffer, &text);
            rebuild_filter_list(&list, &snapshot, &model, &buffer, &graph);
            update_convolution_ui(&convolution_ui, &snapshot);
            graph.queue_draw();
        }
    });
    reset_filters.connect_clicked({
        let model = model.clone();
        let buffer = buffer.clone();
        let list = filter_list.clone();
        let graph = graph.clone();
        let convolution_ui = convolution_ui.clone();
        move |_| {
            let (text, snapshot) = {
                let mut document = model.document.borrow_mut();
                let Some(document) = document.as_mut() else {
                    return;
                };
                document.graphic_eqs.clear();
                for filter in &mut document.filters {
                    filter.gain_db = 0.0;
                }
                (serialize_profile(document), document.clone())
            };
            set_editor_text(&model, &buffer, &text);
            rebuild_filter_list(&list, &snapshot, &model, &buffer, &graph);
            update_convolution_ui(&convolution_ui, &snapshot);
            graph.queue_draw();
        }
    });
    convolution_ui.choose.connect_clicked({
        let model = model.clone();
        let buffer = buffer.clone();
        let list = filter_list.clone();
        let graph = graph.clone();
        let analysis_label = analysis_label.clone();
        let status = status.clone();
        let window = window.clone();
        let ui = convolution_ui.clone();
        move |_| {
            let filter = gtk::FileFilter::new();
            filter.set_name(Some("Impulse responses"));
            for pattern in [
                "*.wav", "*.WAV", "*.flac", "*.FLAC", "*.aif", "*.aiff", "*.ogg",
            ] {
                filter.add_pattern(pattern);
            }
            let chooser = gtk::FileDialog::builder()
                .title("Choose impulse response")
                .accept_label("Use impulse response")
                .default_filter(&filter)
                .build();
            let model = model.clone();
            let buffer = buffer.clone();
            let list = list.clone();
            let graph = graph.clone();
            let analysis_label = analysis_label.clone();
            let status = status.clone();
            let ui = ui.clone();
            chooser.open(Some(&window), gtk::gio::Cancellable::NONE, move |result| {
                let Ok(file) = result else {
                    return;
                };
                let Some(path) = file.path() else {
                    return;
                };
                let Some(id) = model.current_id.borrow().clone() else {
                    return;
                };
                let channel = match ui.channel.selected() {
                    1 => "L",
                    2 => "R",
                    _ => "ALL",
                };
                match model
                    .client
                    .set_convolution(&id, &path.display().to_string(), channel)
                {
                    Ok(profile) => {
                        if let Some(existing) = model
                            .profiles
                            .borrow_mut()
                            .iter_mut()
                            .find(|item| item.id == id)
                        {
                            *existing = profile.clone();
                        }
                        let document = parse_text(&profile.name, &profile.text);
                        ui.layout.set(profile.impulse_layout);
                        model.loading.set(true);
                        set_editor_text(&model, &buffer, &profile.text);
                        model.loading.set(false);
                        *model.document.borrow_mut() = Some(document.clone());
                        let initial = editing::Snapshot {
                            text: serialize_profile(&document),
                            trim: model.manual_trim.get(),
                        };
                        model.history.borrow_mut().reset(initial.clone());
                        *model.reference.borrow_mut() = Some(initial);
                        model.hear_reference.set(false);
                        sync_listening(&model);
                        model.selected_filter.set(None);
                        rebuild_filter_list(&list, &document, &model, &buffer, &graph);
                        update_convolution_ui(&ui, &document);
                        if let Ok(analysis) = model.client.analyze(&id) {
                            store_service_analysis(&model, analysis, &analysis_label);
                        }
                        graph.queue_draw();
                        status.set_text("Convolution active");
                    }
                    Err(error) => status.set_text(&error.to_string()),
                }
            });
        }
    });
    convolution_ui.channel.connect_selected_notify({
        let model = model.clone();
        let buffer = buffer.clone();
        let graph = graph.clone();
        let ui = convolution_ui.clone();
        move |drop| {
            if ui.syncing.get() {
                return;
            }
            let text = {
                let mut document = model.document.borrow_mut();
                let Some(document) = document.as_mut() else {
                    return;
                };
                let Some(convolution) = document.convolutions.first_mut() else {
                    return;
                };
                convolution.channels = match drop.selected() {
                    1 => ChannelSelection::Left,
                    2 => ChannelSelection::Right,
                    _ => ChannelSelection::All,
                };
                serialize_profile(document)
            };
            set_editor_text(&model, &buffer, &text);
            graph.queue_draw();
        }
    });
    convolution_ui.remove.connect_clicked({
        let model = model.clone();
        let buffer = buffer.clone();
        let list = filter_list.clone();
        let graph = graph.clone();
        let ui = convolution_ui.clone();
        move |_| {
            let (text, snapshot) = {
                let mut document = model.document.borrow_mut();
                let Some(document) = document.as_mut() else {
                    return;
                };
                document.convolutions.clear();
                (serialize_profile(document), document.clone())
            };
            set_editor_text(&model, &buffer, &text);
            rebuild_filter_list(&list, &snapshot, &model, &buffer, &graph);
            update_convolution_ui(&ui, &snapshot);
            graph.queue_draw();
        }
    });
    let reload = {
        let model = model.clone();
        let filter_list = filter_list.clone();
        let buffer = buffer.clone();
        let name_entry = name_entry.clone();
        let analysis_label = analysis_label.clone();
        let graph = graph.clone();
        let trim = trim.clone();
        let profile_menu = profile_menu.clone();
        let device_drop = device_drop.clone();
        let device_state = device_state.clone();
        let assign = assign.clone();
        let device_bypass = device_bypass.clone();
        let convolution_ui = convolution_ui.clone();
        move |index: usize| {
            load_profile(
                index,
                &model,
                &filter_list,
                &buffer,
                &name_entry,
                &analysis_label,
                &graph,
                &trim,
                &profile_menu,
                &convolution_ui,
            );
            update_device_controls(&model, &device_drop, &device_state, &assign, &device_bypass);
        }
    };
    let reordering = model.clone();
    profile_list.connect_row_selected(move |_, row| {
        if reordering.reordering_profiles.get() {
            return;
        }
        if let Some(row) = row {
            reload(row.index() as usize);
        }
    });
    profile_list.connect_row_activated({
        let profile_popover = profile_popover.clone();
        move |_, _| profile_popover.popdown()
    });

    buffer.connect_changed({
        let model = model.clone();
        let buffer = buffer.clone();
        let text_view = text_view.clone();
        let graph = graph.clone();
        let analysis_label = analysis_label.clone();
        let list = filter_list.clone();
        let name = name_entry.clone();
        let convolution_ui = convolution_ui.clone();
        move |_| {
            if model.loading.get() || model.syncing_text.get() {
                return;
            }
            let text = buffer
                .text(&buffer.start_iter(), &buffer.end_iter(), false)
                .to_string();
            let document = parse_text(name.text(), &text);
            if !document.is_activatable() {
                text_view.add_css_class("invalid-draft");
                return;
            }
            text_view.remove_css_class("invalid-draft");
            *model.document.borrow_mut() = Some(document.clone());
            let preview = preview_analysis(&model, &document);
            set_analysis_label(&analysis_label, &preview);
            *model.analysis.borrow_mut() = Some(preview);
            remember_edit(&model);
            schedule_live(&model);
            schedule_save(&model);
            rebuild_filter_list(&list, &document, &model, &buffer, &graph);
            update_convolution_ui(&convolution_ui, &document);
            graph.queue_draw();
        }
    });
    name_entry.connect_changed({
        let model = model.clone();
        let menu = profile_menu.clone();
        move |name| {
            if model.loading.get() {
                return;
            }
            model.unsaved.set(true);
            model
                .live_revision
                .set(model.live_revision.get().wrapping_add(1));
            set_profile_menu_text(&menu, &name.text());
            if let Some(document) = model.document.borrow_mut().as_mut() {
                document.name = name.text().to_string();
            }
            schedule_save(&model);
        }
    });

    for button in [add, header_add_profile] {
        button.connect_clicked({
            let model = model.clone();
            let list = profile_list.clone();
            let status = status.clone();
            move |_| match model.client.create("Untitled Profile") {
                Ok(created) => {
                    refresh_profiles(&model, &list);
                    let index = model
                        .profiles
                        .borrow()
                        .iter()
                        .position(|profile| profile.id == created.id);
                    if let Some(row) = index.and_then(|index| list.row_at_index(index as i32)) {
                        list.select_row(Some(&row));
                    }
                    status.set_text("New profile created");
                }
                Err(error) => status.set_text(&error.to_string()),
            }
        });
    }
    delete.connect_clicked({
        let model = model.clone();
        let list = profile_list.clone();
        let status = status.clone();
        move |_| {
            let current_id = model.current_id.borrow().clone();
            if let Some(id) = current_id {
                match model.client.delete(&id) {
                    Ok(()) => {
                        refresh_profiles(&model, &list);
                        if let Some(row) = list.row_at_index(0) {
                            list.select_row(Some(&row));
                        }
                        status.set_text("Profile deleted");
                    }
                    Err(error) => status.set_text(&error.to_string()),
                }
            }
        }
    });
    import.connect_clicked({
        let model = model.clone();
        let list = profile_list.clone();
        let window = window.clone();
        let status = status.clone();
        move |_| {
            let filter = gtk::FileFilter::new();
            filter.set_name(Some("Equalizer text profiles"));
            filter.add_pattern("*.txt");
            let chooser = gtk::FileDialog::builder()
                .title("Import EQ profile")
                .accept_label("Import")
                .default_filter(&filter)
                .build();
            let model = model.clone();
            let list = list.clone();
            let status = status.clone();
            chooser.open(Some(&window), gtk::gio::Cancellable::NONE, move |result| {
                if let Ok(file) = result
                    && let Some(path) = file.path()
                {
                    match model.client.import(&path.display().to_string()) {
                        Ok(imported) => {
                            refresh_profiles(&model, &list);
                            let index = model
                                .profiles
                                .borrow()
                                .iter()
                                .position(|profile| profile.id == imported.id);
                            if let Some(index) = index
                                && let Some(row) = list.row_at_index(index as i32)
                            {
                                list.select_row(Some(&row));
                            }
                            status.set_text("Profile imported");
                        }
                        Err(error) => status.set_text(&error.to_string()),
                    }
                }
            });
        }
    });
    duplicate.connect_clicked({
        let model = model.clone();
        let list = profile_list.clone();
        let status = status.clone();
        move |_| {
            let Some(id) = model.current_id.borrow().clone() else {
                return;
            };
            let Some(source) = model
                .profiles
                .borrow()
                .iter()
                .find(|profile| profile.id == id)
                .cloned()
            else {
                return;
            };
            match model.client.create(&format!("{} Copy", source.name)) {
                Ok(created) => match model.client.put(
                    &created.id,
                    &created.name,
                    &source.text,
                    source.manual_trim_db,
                ) {
                    Ok(_) => {
                        refresh_profiles(&model, &list);
                        let index = model
                            .profiles
                            .borrow()
                            .iter()
                            .position(|profile| profile.id == created.id);
                        if let Some(index) = index
                            && let Some(row) = list.row_at_index(index as i32)
                        {
                            list.select_row(Some(&row));
                        }
                        status.set_text("Profile duplicated");
                    }
                    Err(error) => {
                        let _ = model.client.delete(&created.id);
                        status.set_text(&error.to_string());
                    }
                },
                Err(error) => status.set_text(&error.to_string()),
            }
        }
    });
    export.connect_clicked({
        let model = model.clone();
        let window = window.clone();
        let status = status.clone();
        move |_| {
            let Some(id) = model.current_id.borrow().clone() else {
                return;
            };
            let Some(profile) = model
                .profiles
                .borrow()
                .iter()
                .find(|profile| profile.id == id)
                .cloned()
            else {
                return;
            };
            let dialog = gtk::FileDialog::builder()
                .title("Export EQ profile")
                .accept_label("Export")
                .initial_name(format!("{}.txt", profile.name.replace('/', "-")))
                .build();
            let model = model.clone();
            let status = status.clone();
            dialog.save(Some(&window), gtk::gio::Cancellable::NONE, move |result| {
                if let Ok(file) = result
                    && let Some(path) = file.path()
                {
                    match model
                        .client
                        .export(&profile.id, &path.display().to_string())
                    {
                        Ok(()) => status.set_text("Profile exported"),
                        Err(error) => status.set_text(&error.to_string()),
                    }
                }
            });
        }
    });
    assign.connect_clicked({
        let model = model.clone();
        let drop = device_drop.clone();
        let status = status.clone();
        let state = device_state.clone();
        let assign = assign.clone();
        let bypass = device_bypass.clone();
        let comparison_ui = comparison_ui.clone();
        move |_| {
            let index = drop.selected() as usize;
            let Some(device) = model.devices.borrow().get(index).cloned() else {
                return;
            };
            let Some(selected_profile) = model.current_id.borrow().clone() else {
                return;
            };
            let storage_key = device.key.as_storage_key();
            let engine_has_error = model.client.status().ok().is_some_and(|status| {
                status
                    .pointer("/engine/errors")
                    .and_then(|errors| errors.get(&storage_key))
                    .is_some()
            });
            let comparison_active = model
                .comparisons
                .borrow()
                .get(&storage_key)
                .is_some_and(|comparison| comparison.enabled);
            let unassigning = !engine_has_error
                && !comparison_active
                && device.assigned_profile.as_deref() == Some(&selected_profile);
            let profile = if unassigning { "" } else { &selected_profile };
            match model.client.assign(&storage_key, profile) {
                Ok(()) => {
                    let message = if unassigning {
                        format!("{} is now unassigned", device.description)
                    } else {
                        format!("Active on {}", device.description)
                    };
                    status.set_text(&message);
                    if let Ok(devices) = model.client.devices() {
                        *model.devices.borrow_mut() = devices;
                    }
                    if let Ok(comparisons) = model.client.comparisons() {
                        *model.comparisons.borrow_mut() = comparisons;
                    }
                    refresh_comparison_ui(&model, &drop, &comparison_ui);
                    update_device_controls(&model, &drop, &state, &assign, &bypass);
                }
                Err(error) => status.set_text(&error.to_string()),
            }
        }
    });
    device_drop.connect_selected_notify({
        let model = model.clone();
        let state = device_state.clone();
        let assign = assign.clone();
        let bypass = device_bypass.clone();
        let comparison_ui = comparison_ui.clone();
        let profile_list = profile_list.clone();
        move |drop| {
            if !model.restoring_device.get()
                && let Some(device) = model.devices.borrow().get(drop.selected() as usize)
            {
                *model.pending_focus.borrow_mut() = Some(device.key.as_storage_key());
            }
            // A restore comes from the poll or a focus change, which handle focus themselves.
            if !model.restoring_device.get() {
                apply_pending_focus(&model, drop, &profile_list, false);
            }
            update_device_controls(&model, drop, &state, &assign, &bypass);
            refresh_comparison_ui(&model, drop, &comparison_ui);
        }
    });
    comparison_ui.menu.connect_active_notify({
        let model = model.clone();
        let drop = device_drop.clone();
        let ui = comparison_ui.clone();
        move |menu| {
            if menu.is_active() {
                if let Ok(comparisons) = model.client.comparisons() {
                    *model.comparisons.borrow_mut() = comparisons;
                }
                refresh_comparison_ui(&model, &drop, &ui);
            }
        }
    });
    comparison_ui.edit.connect_clicked({
        let ui = comparison_ui.clone();
        move |_| {
            ui.stack.set_visible_child_name("setup");
            ui.status.set_text("Choose the profiles in this comparison");
        }
    });
    comparison_ui.save.connect_clicked({
        let model = model.clone();
        let drop = device_drop.clone();
        let ui = comparison_ui.clone();
        let state = device_state.clone();
        let assign = assign.clone();
        let bypass = device_bypass.clone();
        move |_| {
            let Some(device) = model
                .devices
                .borrow()
                .get(drop.selected() as usize)
                .cloned()
            else {
                return;
            };
            let selected = ui
                .checks
                .borrow()
                .iter()
                .filter(|(_, check)| check.is_active())
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>();
            match model
                .client
                .configure_comparison(&device.key.as_storage_key(), &selected)
            {
                Ok(comparison) => {
                    model
                        .comparisons
                        .borrow_mut()
                        .insert(device.key.as_storage_key(), comparison);
                    if let Ok(devices) = model.client.devices() {
                        *model.devices.borrow_mut() = devices;
                    }
                    refresh_comparison_ui(&model, &drop, &ui);
                    update_device_controls(&model, &drop, &state, &assign, &bypass);
                }
                Err(error) => ui.status.set_text(&error.to_string()),
            }
        }
    });
    comparison_ui.delete.connect_clicked({
        let model = model.clone();
        let drop = device_drop.clone();
        let ui = comparison_ui.clone();
        let state = device_state.clone();
        let assign = assign.clone();
        let bypass = device_bypass.clone();
        move |_| {
            let Some(device) = model
                .devices
                .borrow()
                .get(drop.selected() as usize)
                .cloned()
            else {
                return;
            };
            match model.client.delete_comparison(&device.key.as_storage_key()) {
                Ok(()) => {
                    model
                        .comparisons
                        .borrow_mut()
                        .remove(&device.key.as_storage_key());
                    refresh_comparison_ui(&model, &drop, &ui);
                    update_device_controls(&model, &drop, &state, &assign, &bypass);
                }
                Err(error) => ui.status.set_text(&error.to_string()),
            }
        }
    });
    device_bypass.connect_active_notify({
        let model = model.clone();
        let drop = device_drop.clone();
        let state = device_state.clone();
        let assign = assign.clone();
        let status = status.clone();
        move |switch| {
            if model.syncing_device.get() || model.pending_device_bypass.borrow().is_some() {
                return;
            }
            let device = {
                let devices = model.devices.borrow();
                devices.get(drop.selected() as usize).cloned()
            };
            if let Some(device) = device {
                let device_key = device.key.as_storage_key();
                let bypassed = !switch.is_active();
                *model.pending_device_bypass.borrow_mut() = Some(PendingDeviceBypass {
                    device_key: device_key.clone(),
                    bypassed,
                });
                state.set_text(if bypassed {
                    "Turning filters off…"
                } else {
                    "Turning filters on…"
                });
                switch.set_sensitive(false);

                // Let GTK paint the thumb at its requested position before
                // making the synchronous service call. The pending state also
                // prevents the health poll from repainting the old value while
                // this transition is being committed.
                gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(40), {
                    let model = model.clone();
                    let drop = drop.clone();
                    let state = state.clone();
                    let assign = assign.clone();
                    let status = status.clone();
                    let switch = switch.clone();
                    move || {
                        if model.tasks.is_closed() {
                            return;
                        }
                        match model.client.set_device_bypass(&device_key, bypassed) {
                            Ok(()) => {
                                if let Ok(devices) = model.client.devices() {
                                    *model.devices.borrow_mut() = devices;
                                }
                            }
                            Err(error) => status.set_text(&error.to_string()),
                        }
                        *model.pending_device_bypass.borrow_mut() = None;
                        update_device_controls(&model, &drop, &state, &assign, &switch);
                    }
                });
            }
        }
    });
    global_bypass.connect_active_notify({
        let model = model.clone();
        let status = status.clone();
        move |switch| {
            if model.syncing_engine.get() {
                return;
            }
            if let Err(error) = model.client.set_global_bypass(!switch.is_active()) {
                status.set_text(&error.to_string());
                model.syncing_engine.set(true);
                switch.set_active(!switch.is_active());
                model.syncing_engine.set(false);
            } else {
                model.engine_off.set(!switch.is_active());
                update_live_indicators(&model);
            }
        }
    });
    trim.connect_value_changed({
        let model = model.clone();
        let graph = graph.clone();
        let analysis_label = analysis_label.clone();
        move |spin| {
            if model.loading.get() {
                return;
            }
            model.manual_trim.set(spin.value());
            remember_edit(&model);
            if let Some(document) = model.document.borrow().as_ref() {
                let preview = preview_analysis(&model, document);
                set_analysis_label(&analysis_label, &preview);
                *model.analysis.borrow_mut() = Some(preview);
            }
            schedule_live(&model);
            schedule_save(&model);
            graph.queue_draw();
        }
    });

    let tone_drag = Rc::new(Cell::new(None::<f64>));
    let drag_state: Rc<RefCell<Option<(usize, f64, f64)>>> = Rc::new(RefCell::new(None));
    // Where and when empty graph space was last pressed: a second press there
    // soon after is a double-click, which adds a band.
    let empty_press = Rc::new(Cell::new(None::<(Instant, f64, f64)>));
    // The band last pressed and when: pressing it again soon deletes it.
    let point_press = Rc::new(Cell::new(None::<(Instant, usize)>));
    let gesture = gtk::GestureDrag::new();
    gesture.connect_drag_begin({
        let empty_press = empty_press.clone();
        let point_press = point_press.clone();
        let buffer = buffer.clone();
        let list = filter_list.clone();
        let tone_drag = tone_drag.clone();
        let model = model.clone();
        let state = drag_state.clone();
        let graph = graph.clone();
        let status = status.clone();
        let analysis_label = analysis_label.clone();
        move |_, x, y| {
            graph.grab_focus();
            model.graph_dragging.set(true);
            tone_drag.set(None);
            if let Some(listener) = model
                .graph_tone
                .borrow()
                .as_ref()
                .filter(|ui| ui.cursor.is_active())
            {
                if let Some((frequency, _)) = graph::values_at_position(
                    x,
                    graph.height() as f64 / 2.0,
                    graph.width() as f64,
                    graph.height() as f64,
                ) {
                    listener.sweep.set_active(false);
                    listener.tone.frequency.set_value(tone_position(frequency));
                    tone_drag.set(Some(tone_position(frequency)));
                }
                return;
            }
            model.history.borrow_mut().checkpoint();
            let Some(document) = model.document.borrow().as_ref().cloned() else {
                return;
            };
            let width = graph.width().max(1) as f64;
            let height = graph.height().max(1) as f64;
            let closest = document
                .filters
                .iter()
                .enumerate()
                .filter(|(_, filter)| filter.enabled)
                .min_by(|(_, a), (_, b)| {
                    let (ax, ay) =
                        graph::filter_point_position(a.frequency, a.gain_db, width, height);
                    let (bx, by) =
                        graph::filter_point_position(b.frequency, b.gain_db, width, height);
                    let da = ((ax - x).powi(2) + (ay - y).powi(2)).sqrt();
                    let db = ((bx - x).powi(2) + (by - y).powi(2)).sqrt();
                    da.total_cmp(&db)
                });
            let double_click =
                Duration::from_millis(gtk::Settings::default().map_or(400, |settings| {
                    settings.gtk_double_click_time().max(1) as u64
                }));
            let now = Instant::now();
            if let Some((index, filter)) = closest {
                let (point_x, point_y) =
                    graph::filter_point_position(filter.frequency, filter.gain_db, width, height);
                if ((point_x - x).powi(2) + (point_y - y).powi(2)).sqrt() <= 34.0 {
                    // Double-clicking a band deletes it, as double-clicking
                    // empty space adds one.
                    let again = point_press.take().is_some_and(|(at, last)| {
                        last == index && now.duration_since(at) <= double_click
                    });
                    if again {
                        if remove_band(&model, &buffer, &graph, &list, index) {
                            model.selected_filter.set(None);
                            sync_band_ui(&model);
                            status.set_text(&format!(
                                "Deleted band {} · Ctrl+Z restores it",
                                index + 1
                            ));
                        }
                        return;
                    }
                    point_press.set(Some((now, index)));
                    *state.borrow_mut() = Some((index, filter.frequency, filter.gain_db));
                    model.selected_filter.set(Some(index));
                    sync_band_ui(&model);
                    graph.queue_draw();
                    return;
                }
            }

            point_press.set(None);
            let is_double = empty_press.take().is_some_and(|(at, px, py)| {
                now.duration_since(at) <= double_click && (px - x).hypot(py - y) <= 8.0
            });
            if !is_double {
                // A single click on empty space deselects, as in plugin EQs.
                empty_press.set(Some((now, x, y)));
                if model.selected_filter.replace(None).is_some() {
                    sync_band_ui(&model);
                    graph.queue_draw();
                }
                return;
            }
            let Some((frequency, gain_db)) = graph::values_at_position(x, y, width, height) else {
                return;
            };
            let (index, preview) = {
                let mut document = model.document.borrow_mut();
                let Some(document) = document.as_mut() else {
                    return;
                };
                if !can_add_filter(document, ChannelSelection::All) {
                    status.set_text("This profile already has 64 bands on one channel");
                    return;
                }
                document.graphic_eqs.clear();
                document.filters.push(Filter {
                    enabled: true,
                    kind: FilterKind::Peaking,
                    frequency,
                    gain_db,
                    q: 1.0,
                    channels: ChannelSelection::All,
                });
                let index = document.filters.len() - 1;
                let preview = preview_analysis(&model, document);
                (index, preview)
            };
            *state.borrow_mut() = Some((index, frequency, gain_db));
            model.selected_filter.set(Some(index));
            set_analysis_label(&analysis_label, &preview);
            *model.analysis.borrow_mut() = Some(preview);
            status.set_text(&format!(
                "Added peak filter at {}",
                format_band_frequency(frequency)
            ));
            graph.queue_draw();
        }
    });
    gesture.connect_drag_update({
        let tone_drag = tone_drag.clone();
        let model = model.clone();
        let state = drag_state.clone();
        let graph = graph.clone();
        let analysis_label = analysis_label.clone();
        move |_, dx, dy| {
            if let Some(position) = tone_drag.get() {
                if let Some(ui) = model.graph_tone.borrow().as_ref() {
                    ui.tone.frequency.set_value(
                        (position + dx / (graph.width() as f64 - 82.0).max(1.0)).clamp(0.0, 1.0),
                    );
                }
                return;
            }
            let Some((index, start_frequency, start_gain)) = *state.borrow() else {
                return;
            };
            let width = graph.width().max(1) as f64;
            let height = graph.height().max(1) as f64;
            let preview = {
                let mut document = model.document.borrow_mut();
                let Some(document) = document.as_mut() else {
                    return;
                };
                document.graphic_eqs.clear();
                let Some(filter) = document.filters.get_mut(index) else {
                    return;
                };
                filter.frequency =
                    (start_frequency * (1000.0_f64).powf(dx / width)).clamp(20.0, 20_000.0);
                filter.gain_db = graph::gain_after_drag(start_gain, dy, height);
                preview_analysis(&model, document)
            };
            set_analysis_label(&analysis_label, &preview);
            *model.analysis.borrow_mut() = Some(preview);
            sync_band_ui(&model);
            sync_listening(&model);
            schedule_live(&model);
            graph.queue_draw();
        }
    });
    gesture.connect_drag_end({
        let state = drag_state.clone();
        let model = model.clone();
        let buffer = buffer.clone();
        let list = filter_list.clone();
        let graph = graph.clone();
        let convolution_ui = convolution_ui.clone();
        move |_, _, _| {
            model.graph_dragging.set(false);
            // The band panel held its side during the drag; place it afresh.
            if let Some(overlay) = graph.parent() {
                overlay.queue_allocate();
            }
            let snapshot = model.document.borrow().clone();
            if state.borrow_mut().take().is_some()
                && let Some(snapshot) = snapshot
            {
                rebuild_filter_list(&list, &snapshot, &model, &buffer, &graph);
                update_convolution_ui(&convolution_ui, &snapshot);
                set_editor_text(&model, &buffer, &serialize_profile(&snapshot));
            }
        }
    });
    graph.add_controller(gesture);

    // Right-click a band to delete it.
    let secondary = gtk::GestureClick::new();
    secondary.set_button(gtk::gdk::BUTTON_SECONDARY);
    secondary.connect_pressed({
        let model = model.clone();
        let buffer = buffer.clone();
        let list = filter_list.clone();
        let graph = graph.clone();
        let status = status.clone();
        move |click, _, x, y| {
            let hit = model.document.borrow().as_ref().and_then(|document| {
                band_at(document, x, y, graph.width() as f64, graph.height() as f64)
            });
            let Some(index) = hit else {
                return;
            };
            click.set_state(gtk::EventSequenceState::Claimed);
            let was_selected = model.selected_filter.get() == Some(index);
            if remove_band(&model, &buffer, &graph, &list, index) {
                if was_selected {
                    model.selected_filter.set(None);
                    sync_band_ui(&model);
                }
                status.set_text(&format!("Deleted band {} · Ctrl+Z restores it", index + 1));
            }
        }
    });
    graph.add_controller(secondary);

    let keyboard = gtk::EventControllerKey::new();
    // The last nudge key, when it was pressed, and how many repeats in a row:
    // holding a key speeds it up.
    let held = Rc::new(Cell::new(None::<(gtk::gdk::Key, Instant, u32)>));
    keyboard.connect_key_pressed({
        let held = held.clone();
        let list = filter_list.clone();
        let model = model.clone();
        let buffer = buffer.clone();
        let graph = graph.clone();
        let status = status.clone();
        move |_, key, _, modifiers| {
            if modifiers
                .intersects(gtk::gdk::ModifierType::CONTROL_MASK | gtk::gdk::ModifierType::ALT_MASK)
            {
                return gtk::glib::Propagation::Proceed;
            }
            if matches!(
                key,
                gtk::gdk::Key::Delete | gtk::gdk::Key::KP_Delete | gtk::gdk::Key::BackSpace
            ) {
                let Some(index) = model.selected_filter.get() else {
                    return gtk::glib::Propagation::Proceed;
                };
                if remove_band(&model, &buffer, &graph, &list, index) {
                    status.set_text(&format!("Deleted band {} · Ctrl+Z restores it", index + 1));
                }
                return gtk::glib::Propagation::Stop;
            }
            let direction = match key {
                gtk::gdk::Key::Up => graph::NudgeDirection::Up,
                gtk::gdk::Key::Down => graph::NudgeDirection::Down,
                gtk::gdk::Key::Left => graph::NudgeDirection::Left,
                gtk::gdk::Key::Right => graph::NudgeDirection::Right,
                _ => return gtk::glib::Propagation::Proceed,
            };
            let Some(index) = model.selected_filter.get() else {
                return gtk::glib::Propagation::Proceed;
            };
            let shifted = modifiers.contains(gtk::gdk::ModifierType::SHIFT_MASK);
            let now = Instant::now();
            let count = match held.get() {
                Some((last, at, count))
                    if last == key && now.duration_since(at) < Duration::from_millis(250) =>
                {
                    count + 1
                }
                _ => 0,
            };
            held.set(Some((key, now, count)));
            // Fine steps speed up after about a third and a full second of
            // auto-repeat; Shift steps are already coarse.
            let repeat = match (shifted, count) {
                (true, _) | (false, 0..10) => 1.0,
                (false, 10..30) => 2.0,
                (false, _) => 5.0,
            };
            update_filter(&model, &buffer, &graph, index, |filter| {
                graph::nudge_filter(filter, direction, shifted, repeat);
            });
            if let Some(filter) = model
                .document
                .borrow()
                .as_ref()
                .and_then(|document| document.filters.get(index))
            {
                status.set_text(&format!(
                    "{} · {:+.1} dB · Q {:.3}",
                    format_band_frequency(filter.frequency),
                    filter.gain_db,
                    filter.q
                ));
            }
            gtk::glib::Propagation::Stop
        }
    });
    keyboard.connect_key_released(move |_, _, _, _| held.set(None));
    graph.add_controller(keyboard);

    // Scroll over the selected band (or anywhere, with it selected and the
    // pointer near) to change its Q, as in plugin EQs.
    let pointer = Rc::new(Cell::new((0.0_f64, 0.0_f64)));
    let motion = gtk::EventControllerMotion::new();
    motion.connect_motion({
        let pointer = pointer.clone();
        move |_, x, y| pointer.set((x, y))
    });
    graph.add_controller(motion);
    let scroll = gtk::EventControllerScroll::new(
        gtk::EventControllerScrollFlags::VERTICAL | gtk::EventControllerScrollFlags::DISCRETE,
    );
    scroll.connect_scroll({
        let model = model.clone();
        let buffer = buffer.clone();
        let graph = graph.clone();
        move |_, _, dy| {
            let Some(index) = model.selected_filter.get() else {
                return gtk::glib::Propagation::Proceed;
            };
            let near = model
                .document
                .borrow()
                .as_ref()
                .and_then(|document| document.filters.get(index))
                .is_some_and(|filter| {
                    let (x, y) = graph::filter_point_position(
                        filter.frequency,
                        filter.gain_db,
                        graph.width() as f64,
                        graph.height() as f64,
                    );
                    let (px, py) = pointer.get();
                    ((px - x).powi(2) + (py - y).powi(2)).sqrt() <= 60.0
                });
            if !near {
                return gtk::glib::Propagation::Proceed;
            }
            update_filter(&model, &buffer, &graph, index, |filter| {
                filter.q = (filter.q * 1.12_f64.powf(-dy)).clamp(0.05, 50.0);
            });
            gtk::glib::Propagation::Stop
        }
    });
    graph.add_controller(scroll);
    let escape = gtk::EventControllerKey::new();
    escape.connect_key_pressed({
        let model = model.clone();
        let graph = graph.clone();
        move |_, key, _, _| {
            if key != gtk::gdk::Key::Escape || model.selected_filter.get().is_none() {
                return gtk::glib::Propagation::Proceed;
            }
            model.selected_filter.set(None);
            sync_band_ui(&model);
            graph.queue_draw();
            gtk::glib::Propagation::Stop
        }
    });
    graph.add_controller(escape);
}

fn set_profile_menu_text(menu: &gtk::MenuButton, text: &str) {
    if let Some(label) = menu
        .child()
        .and_then(|child| child.downcast::<gtk::Label>().ok())
    {
        label.set_text(text);
    } else {
        menu.set_label(text);
    }
}

#[allow(clippy::too_many_arguments)]
fn load_profile(
    index: usize,
    model: &Rc<Model>,
    filter_list: &gtk::FlowBox,
    buffer: &gtk::TextBuffer,
    name: &gtk::Entry,
    analysis_label: &gtk::Label,
    graph: &gtk::DrawingArea,
    trim: &knob::Knob,
    profile_menu: &gtk::MenuButton,
    convolution_ui: &ConvolutionUi,
) {
    let Some(profile) = model.profiles.borrow().get(index).cloned() else {
        return;
    };
    if model.pending_save.borrow().is_some() {
        if let Some(timer) = model.pending_save.borrow_mut().take() {
            timer.remove();
        }
        submit_live(model, true);
    }
    if let Some(timer) = model.pending_live.borrow_mut().take() {
        timer.remove();
    }
    // A tone left playing holds the next profile's own match, not this one's.
    model.tone_hold.set(None);
    if let Some(id) = model.current_id.borrow().as_ref() {
        model.live_worker.end(id);
    }
    model.loading.set(true);
    *model.current_id.borrow_mut() = Some(profile.id.clone());
    model.selected_filter.set(None);
    set_profile_menu_text(profile_menu, &profile.name);
    name.set_text(&profile.name);
    set_editor_text(model, buffer, &profile.text);
    model.manual_trim.set(profile.manual_trim_db);
    trim.set_value(profile.manual_trim_db);
    let document = parse_text(&profile.name, &profile.text);
    let initial = editing::Snapshot {
        text: serialize_profile(&document),
        trim: profile.manual_trim_db,
    };
    model.history.borrow_mut().reset(initial);
    model.unsaved.set(false);
    *model.reference.borrow_mut() = model.history.borrow().baseline().cloned();
    model.hear_reference.set(false);
    *model.document.borrow_mut() = Some(document.clone());
    sync_listening(model);
    rebuild_filter_list(filter_list, &document, model, buffer, graph);
    convolution_ui.layout.set(profile.impulse_layout);
    update_convolution_ui(convolution_ui, &document);
    show_profile_page(convolution_ui, &document);
    let curves = model.client.curves(&profile.id).unwrap_or_default();
    update_curve_context(model, curves);
    if let Ok(analysis) = model.client.analyze(&profile.id) {
        store_service_analysis(model, analysis, analysis_label);
        graph.queue_draw();
    }
    model.loading.set(false);
}

fn show_curve_details(
    window: &adw::ApplicationWindow,
    model: &Rc<Model>,
    kind: massiveeq_core::CurveKind,
    status: &gtk::Label,
) {
    use massiveeq_core::{
        CurveCompensation as Compensation, CurveMetadata, CurveTransducer as Transducer,
    };
    let Some(id) = model.current_id.borrow().clone() else {
        return;
    };
    let curves = model.curves.borrow().clone();
    let (exists, metadata) = match kind {
        massiveeq_core::CurveKind::Measurement => {
            (curves.measurement.is_some(), curves.measurement_metadata)
        }
        massiveeq_core::CurveKind::Target => (curves.target.is_some(), curves.target_metadata),
    };
    if !exists {
        status.set_text("Load this curve before adding its measurement details");
        return;
    }
    let metadata = metadata.unwrap_or_default();
    let dialog = adw::Window::builder()
        .transient_for(window)
        .modal(true)
        .title("Curve details")
        .default_width(430)
        .default_height(620)
        .build();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.set_margin_top(18);
    content.set_margin_bottom(18);
    content.set_margin_start(18);
    content.set_margin_end(18);
    let heading = gtk::Label::new(Some(match kind {
        massiveeq_core::CurveKind::Measurement => "Measurement context",
        massiveeq_core::CurveKind::Target => "Target context",
    }));
    heading.set_xalign(0.0);
    heading.add_css_class("panel-title");
    content.append(&heading);
    let entry = |label: &str, value: &str, placeholder: &str| {
        let field = gtk::Box::new(gtk::Orientation::Vertical, 4);
        let title = gtk::Label::new(Some(label));
        title.set_xalign(0.0);
        title.add_css_class("level-code");
        field.append(&title);
        let input = gtk::Entry::new();
        input.set_text(value);
        input.set_placeholder_text(Some(placeholder));
        input.update_property(&[gtk::accessible::Property::Label(label)]);
        field.append(&input);
        content.append(&field);
        input
    };
    let source = entry(
        "Source",
        &metadata.source,
        "Author, publication, or source URL",
    );
    source.set_max_length(2048);
    // Known fixtures by name, so mismatches are found however they were
    // typed; anything else goes in the text field.
    let fixture_title = gtk::Label::new(Some("Measurement fixture"));
    fixture_title.set_xalign(0.0);
    fixture_title.add_css_class("level-code");
    content.append(&fixture_title);
    let mut fixture_options = vec!["Unknown fixture"];
    fixture_options.extend(massiveeq_core::FIXTURES.iter().map(|(_, label)| *label));
    fixture_options.push("Other fixture…");
    let other_fixture = fixture_options.len() as u32 - 1;
    let fixture_choice = gtk::DropDown::from_strings(&fixture_options);
    fixture_choice.update_property(&[gtk::accessible::Property::Label("Measurement fixture")]);
    let known = massiveeq_core::fixture_key(&metadata.fixture).and_then(|key| {
        massiveeq_core::FIXTURES
            .iter()
            .position(|(candidate, _)| *candidate == key)
    });
    fixture_choice.set_selected(match known {
        Some(index) => index as u32 + 1,
        None if metadata.fixture.trim().is_empty() => 0,
        None => other_fixture,
    });
    content.append(&fixture_choice);
    let fixture = gtk::Entry::new();
    fixture.set_max_length(256);
    fixture.set_placeholder_text(Some("Fixture name, as the source gives it"));
    fixture.update_property(&[gtk::accessible::Property::Label("Other fixture name")]);
    if known.is_none() {
        fixture.set_text(&metadata.fixture);
    }
    fixture.set_visible(fixture_choice.selected() == other_fixture);
    content.append(&fixture);
    fixture_choice.connect_selected_notify({
        let fixture = fixture.clone();
        move |choice| fixture.set_visible(choice.selected() == other_fixture)
    });
    let fixture_name = move || match fixture_choice.selected() {
        0 => String::new(),
        index if index == other_fixture => fixture.text().trim().to_owned(),
        index => massiveeq_core::FIXTURES[index as usize - 1].1.to_owned(),
    };
    let compensation = gtk::DropDown::from_strings(&[
        "Unknown compensation",
        "Raw response",
        "Diffuse-field compensated",
        "Other compensation",
    ]);
    compensation.set_selected(match metadata.compensation {
        Compensation::Unknown => 0,
        Compensation::Raw => 1,
        Compensation::DiffuseField => 2,
        Compensation::Other => 3,
    });
    compensation.update_property(&[gtk::accessible::Property::Label("Curve compensation")]);
    content.append(&compensation);
    let baseline = entry(
        "Compensation baseline",
        &metadata.baseline,
        "Name of the fixture-specific baseline",
    );
    baseline.set_max_length(256);
    let transducer = gtk::DropDown::from_strings(&[
        "Unknown headphone type",
        "Over-ear headphones",
        "In-ear headphones",
        "Speakers",
    ]);
    transducer.set_selected(match metadata.transducer {
        Transducer::Unknown => 0,
        Transducer::OverEar => 1,
        Transducer::InEar => 2,
        Transducer::Speaker => 3,
    });
    transducer.update_property(&[gtk::accessible::Property::Label("Curve headphone type")]);
    content.append(&transducer);
    let note = gtk::Label::new(Some(
        "These details describe the supplied data. Matching declarations do not verify a measurement. Target ranges are references, not pass/fail tests; the EQ curve is a prediction.",
    ));
    note.set_wrap(true);
    note.set_xalign(0.0);
    note.add_css_class("level-code");
    content.append(&note);
    let link = gtk::LinkButton::with_label(
        "https://headphones.com/blogs/features/what-headphone-sound-profiles-do-people-like",
        "Headphones.com research and context",
    );
    content.append(&link);
    let graphworks = gtk::LinkButton::with_label(
        "https://graph.headphones.com",
        "GraphWorks measurements and preference bounds",
    );
    content.append(&graphworks);
    let error = gtk::Label::new(None);
    error.set_wrap(true);
    error.set_xalign(0.0);
    content.append(&error);
    let save = gtk::Button::with_label("Save curve details");
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let cancel = gtk::Button::with_label("Cancel");
    cancel.connect_clicked({
        let dialog = dialog.downgrade();
        move |_| {
            if let Some(dialog) = dialog.upgrade() {
                dialog.close();
            }
        }
    });
    save.set_hexpand(true);
    actions.append(&cancel);
    actions.append(&save);
    content.append(&actions);
    save.connect_clicked({
        let model = model.clone();
        let dialog = dialog.downgrade();
        let error = error.clone();
        move |_| {
            let metadata = CurveMetadata {
                source: source.text().into(),
                fixture: fixture_name(),
                baseline: baseline.text().into(),
                compensation: match compensation.selected() {
                    1 => Compensation::Raw,
                    2 => Compensation::DiffuseField,
                    3 => Compensation::Other,
                    _ => Compensation::Unknown,
                },
                transducer: match transducer.selected() {
                    1 => Transducer::OverEar,
                    2 => Transducer::InEar,
                    3 => Transducer::Speaker,
                    _ => Transducer::Unknown,
                },
            };
            match model.client.set_curve_metadata(&id, kind, &metadata) {
                Ok(curves) => {
                    update_curve_context(&model, curves);
                    if let Some(dialog) = dialog.upgrade() {
                        dialog.close();
                    }
                }
                Err(e) => error.set_text(&e.to_string()),
            }
        }
    });
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&content)
        .build();
    dialog.set_content(Some(&scroll));
    dialog.present();
}

fn update_curve_context(model: &Model, curves: massiveeq_core::Curves) {
    let full = curves.context_note();
    if let Some(label) = model.curve_context.borrow().as_ref() {
        // Most curves carry no context at all; only a real conflict is worth a line.
        label.set_visible(full.contains("contexts differ"));
        let fixtures = curves
            .measurement_metadata
            .as_ref()
            .zip(curves.target_metadata.as_ref())
            .map(|(measurement, target)| (measurement.fixture.trim(), target.fixture.trim()))
            .filter(|(measurement, target)| !measurement.is_empty() && !target.is_empty());
        let fixture_note =
            fixtures
                .filter(|_| full.contains("do not transfer"))
                .map(|(measurement, target)| {
                    let what = if curves
                        .target
                        .as_deref()
                        .is_some_and(massiveeq_core::is_range)
                    {
                        "bounds are"
                    } else {
                        "target is"
                    };
                    format!(
                        "Measured on {measurement}, but the {what} made for {target}; \
                     they do not transfer to another fixture's ears."
                    )
                });
        label.set_text(&match fixture_note {
            Some(note) => note,
            None if full.contains("contexts differ") => {
                "Measurement and target contexts differ. See curve details.".to_owned()
            }
            None if full.contains("matching user-entered") => {
                "Curves share declared context. EQ response is predicted.".to_owned()
            }
            None => "Curve context is incomplete. See curve details before comparing targets."
                .to_owned(),
        });
        label.set_tooltip_text(Some(&full));
    }
    let field = curves.measurement.as_ref().and_then(|measurement| {
        let takes = std::iter::once(measurement.as_slice())
            .chain(curves.placements.iter().map(Vec::as_slice))
            .collect::<Vec<_>>();
        massiveeq_core::measurement_field(&takes)
    });
    if let Some(ui) = model.curves_ui.borrow().as_ref() {
        let count = curves.placements.len();
        ui.add_placement.set_sensitive(
            curves.measurement.is_some()
                && curves.measurement_set.is_none()
                && count < massiveeq_core::MAX_PLACEMENTS,
        );
        ui.clear_placements.set_visible(count > 0);
        ui.clear_placements.set_label(&format!(
            "CLEAR {count} PLACEMENT{}",
            if count == 1 { "" } else { "S" }
        ));
    }
    let mut overlay = model.overlay.borrow_mut();
    // With placements, the curve to correct is the typical one: their median.
    overlay.measurement = match &field {
        Some(field) => Some(field.median.clone()),
        None => curves.measurement.clone(),
    };
    overlay.target = curves.target.clone();
    overlay.measurement_channels = curves.measurement_channels.clone();
    overlay.field = field;
    *model.curves.borrow_mut() = curves;
}

/// The graph header's menu for measurement and target curves.
struct CurvesUi {
    menu: gtk::MenuButton,
    load_measurement: gtk::Button,
    /// Another measurement of the same headphone after putting it on again.
    add_placement: gtk::Button,
    clear_placements: gtk::Button,
    load_target: gtk::Button,
    clear_measurement: gtk::Button,
    clear_target: gtk::Button,
    measurement_details: gtk::Button,
    target_details: gtk::Button,
}

fn build_curves_menu() -> Rc<CurvesUi> {
    let menu = gtk::MenuButton::new();
    menu.set_label("CURVES");
    menu.add_css_class("comparison-menu");
    menu.set_tooltip_text(Some(
        "Show a headphone measurement and a target behind the EQ",
    ));
    let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
    content.set_margin_top(8);
    content.set_margin_bottom(8);
    content.set_margin_start(8);
    content.set_margin_end(8);
    let note = gtk::Label::new(Some(
        "Frequency and dB per line, as AutoEQ, Squiglink,\nand REW export. Three columns make a range,\nsuch as preference bounds. Curves align at\n630 Hz ± 1 octave, as on GraphWorks.",
    ));
    note.set_xalign(0.0);
    note.add_css_class("level-code");
    content.append(&note);
    let button = |label: &str| {
        let button = gtk::Button::with_label(label);
        content.append(&button);
        button
    };
    let load_measurement = button("Measurements…");
    let add_placement = button("ADD PLACEMENT…");
    add_placement.set_sensitive(false);
    add_placement.set_tooltip_text(Some(
        "Add another measurement of this headphone after putting it on again. \
         The graph shows how much its response moves between placements.",
    ));
    let load_target = button("LOAD TARGET OR BOUNDS…");
    let clear_measurement = button("CLEAR MEASUREMENT");
    let clear_target = button("CLEAR TARGET");
    let clear_placements = button("CLEAR PLACEMENTS");
    clear_placements.set_visible(false);
    let measurement_details = button("Measurement details…");
    let target_details = button("Target details…");
    let popover = gtk::Popover::new();
    popover.set_child(Some(&content));
    menu.set_popover(Some(&popover));
    Rc::new(CurvesUi {
        menu,
        load_measurement,
        add_placement,
        clear_placements,
        load_target,
        clear_measurement,
        clear_target,
        measurement_details,
        target_details,
    })
}

fn wire_curves(
    window: &adw::ApplicationWindow,
    model: &Rc<Model>,
    ui: &Rc<CurvesUi>,
    graph: &gtk::DrawingArea,
    status: &gtk::Label,
) {
    ui.load_measurement.connect_clicked({
        let window = window.clone();
        let model = model.clone();
        let menu = ui.menu.clone();
        let graph = graph.clone();
        let status = status.clone();
        move |_| {
            menu.popdown();
            let Some(id) = model.current_id.borrow().clone() else {
                return;
            };
            let curves = model.curves.borrow().clone();
            let applied = Rc::new({
                let model = model.clone();
                let graph = graph.clone();
                let status = status.clone();
                let id = id.clone();
                move |curves| {
                    if model.current_id.borrow().as_deref() == Some(id.as_str()) {
                        update_curve_context(&model, curves);
                        graph.queue_draw();
                    }
                    status.set_text("Measurements saved; original takes retained");
                }
            });
            measurements::show(&window, model.client.clone(), id, &curves, applied);
        }
    });
    for (button, kind) in [
        (
            &ui.measurement_details,
            massiveeq_core::CurveKind::Measurement,
        ),
        (&ui.target_details, massiveeq_core::CurveKind::Target),
    ] {
        button.connect_clicked({
            let window = window.clone();
            let model = model.clone();
            let menu = ui.menu.clone();
            let status = status.clone();
            move |_| {
                menu.popdown();
                show_curve_details(&window, &model, kind, &status);
            }
        });
    }
    // Save (or with an empty path, clear) a curve and show the result.
    let apply = Rc::new({
        let model = model.clone();
        let graph = graph.clone();
        let status = status.clone();
        move |kind: &str, path: &str| {
            let Some(id) = model.current_id.borrow().clone() else {
                return;
            };
            match model.client.set_curve(&id, kind, path) {
                Ok(curves) => {
                    update_curve_context(&model, curves);
                    status.set_text(match (kind, path.is_empty()) {
                        ("placement", true) => "Placements cleared",
                        ("placement", false) => "Placement added",
                        (_, true) => "Curve cleared",
                        (_, false) => "Curve loaded",
                    });
                }
                Err(error) => status.set_text(&error.to_string()),
            }
            graph.queue_draw();
        }
    });
    for (button, kind, title) in [
        (&ui.load_target, "target", "Choose target curve or bounds"),
        (
            &ui.add_placement,
            "placement",
            "Choose another measurement of this headphone",
        ),
    ] {
        button.connect_clicked({
            let window = window.clone();
            let apply = apply.clone();
            let menu = ui.menu.clone();
            move |_| {
                menu.popdown();
                let filter = gtk::FileFilter::new();
                filter.set_name(Some("Frequency response"));
                for pattern in ["*.txt", "*.csv", "*.TXT", "*.CSV"] {
                    filter.add_pattern(pattern);
                }
                let chooser = gtk::FileDialog::builder()
                    .title(title)
                    .accept_label("Show curve")
                    .default_filter(&filter)
                    .build();
                let apply = apply.clone();
                chooser.open(Some(&window), gtk::gio::Cancellable::NONE, move |result| {
                    if let Some(path) = result.ok().and_then(|file| file.path()) {
                        apply(kind, &path.display().to_string());
                    }
                });
            }
        });
    }
    for (button, kind) in [
        (&ui.clear_measurement, "measurement"),
        (&ui.clear_target, "target"),
        (&ui.clear_placements, "placement"),
    ] {
        let apply = apply.clone();
        let menu = ui.menu.clone();
        button.connect_clicked(move |_| {
            menu.popdown();
            apply(kind, "");
        });
    }
}

/// A row of linked toggle buttons choosing one option.
struct Segmented {
    root: gtk::Box,
    buttons: Vec<gtk::ToggleButton>,
}

impl Segmented {
    fn new(options: &[&str], selected: usize) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.add_css_class("linked");
        root.add_css_class("segmented");
        let buttons = options
            .iter()
            .map(|option| gtk::ToggleButton::with_label(option))
            .collect::<Vec<_>>();
        for button in &buttons {
            if let Some(first) = buttons.first().filter(|first| *first != button) {
                button.set_group(Some(first));
            }
            root.append(button);
        }
        buttons[selected].set_active(true);
        Self { root, buttons }
    }

    fn selected(&self) -> u32 {
        self.buttons
            .iter()
            .position(gtk::ToggleButton::is_active)
            .unwrap_or_default() as u32
    }

    fn set_sensitive(&self, sensitive: bool) {
        self.root.set_sensitive(sensitive);
    }

    fn connect_changed(&self, callback: impl Fn() + 'static) {
        let callback = Rc::new(callback);
        for button in &self.buttons {
            let callback = callback.clone();
            // Each change toggles two buttons; act once, on the one turning on.
            button.connect_toggled(move |button| {
                if button.is_active() {
                    callback();
                }
            });
        }
    }
}

/// Controls of the tone finder page.
struct ToneUi {
    play: gtk::ToggleButton,
    signal: Segmented,
    band: Segmented,
    ears: Segmented,
    frequency: gtk::Scale,
    frequency_label: gtk::Label,
    level: gtk::Scale,
    level_label: gtk::Label,
    fix_caption: gtk::Label,
    fixes: Vec<(gtk::Button, f64)>,
    /// The running sweep's start in Hz and rate in octaves per second. The
    /// service plays it; the slider here only follows along.
    sweep: Cell<Option<(f64, f64, u32)>>,
    sweep_id: Cell<u32>,
}

/// Loudest tone levels in dBFS, as the service caps them: noise peaks run
/// about 10 dB above a sine of the same level, so noise stops lower.
const TONE_MAX_LEVEL_DB: f64 = -12.0;
const NOISE_MAX_LEVEL_DB: f64 = -22.0;

/// Slider position (0–1) to frequency, logarithmic over 20 Hz–20 kHz.
fn tone_frequency(position: f64) -> f64 {
    20.0 * 1000.0_f64.powf(position.clamp(0.0, 1.0))
}

fn tone_position(frequency: f64) -> f64 {
    (frequency / 20.0).log(1000.0).clamp(0.0, 1.0)
}

fn build_tone_page() -> (gtk::Box, Rc<ToneUi>) {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 12);
    page.set_valign(gtk::Align::Start);
    let card = gtk::Box::new(gtk::Orientation::Vertical, 14);
    card.add_css_class("convolution-card");

    // Transport and readouts: a round play control and two large values.
    let top = adw::WrapBox::new();
    top.set_child_spacing(10);
    top.set_line_spacing(10);
    let play = gtk::ToggleButton::new();
    play.set_icon_name("media-playback-start-symbolic");
    play.add_css_class("tone-play");
    play.set_valign(gtk::Align::Center);
    play.set_tooltip_text(Some("Play a test tone through this output's EQ"));
    play.update_property(&[gtk::accessible::Property::Label("Play tone")]);
    top.append(&play);
    let readout = |value: &str, caption: &str| {
        let tile = gtk::Box::new(gtk::Orientation::Vertical, 2);
        tile.add_css_class("tone-readout");
        let value = gtk::Label::new(Some(value));
        value.set_xalign(0.0);
        value.add_css_class("tone-value");
        let caption = gtk::Label::new(Some(caption));
        caption.set_xalign(0.0);
        caption.add_css_class("section-label");
        tile.append(&value);
        tile.append(&caption);
        (tile, value)
    };
    let (frequency_tile, frequency_label) = readout("1.00 kHz", "FREQUENCY");
    let (level_tile, level_label) = readout("−36 dBFS", "LEVEL");
    top.append(&frequency_tile);
    top.append(&level_tile);
    card.append(&top);

    let signal = Segmented::new(&["SINE", "PINK NOISE"], 0);
    let band = Segmented::new(&["FULL", "OCTAVE", "⅓ OCTAVE"], 2);
    band.set_sensitive(false);
    let ears = Segmented::new(&["LEFT", "BOTH", "RIGHT"], 1);
    let choices = adw::WrapBox::new();
    choices.set_child_spacing(10);
    choices.set_line_spacing(8);
    for segmented in [&signal, &band, &ears] {
        choices.append(&segmented.root);
    }
    card.append(&choices);

    let slider = |range: (f64, f64, f64), value: f64| {
        let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, range.0, range.1, range.2);
        scale.set_value(value);
        scale.set_hexpand(true);
        scale.set_draw_value(false);
        scale
    };
    let frequency = slider((0.0, 1.0, 0.001), tone_position(1_000.0));
    frequency.set_increments(1.0 / 120.0, 1.0 / 30.0);
    frequency.update_property(&[gtk::accessible::Property::Label("Graph tone frequency")]);
    let level = slider((-60.0, TONE_MAX_LEVEL_DB, 1.0), -36.0);
    level.update_property(&[gtk::accessible::Property::Label("Tone level")]);
    level.set_tooltip_text(Some(
        "Level before the EQ. Pink noise matches a sine's loudness at the same setting \
         and stops at −22 dBFS, where its peaks reach the sine's −12 dBFS limit.",
    ));
    card.append(&frequency);
    card.append(&level);

    // One-click corrections at the tone's frequency.
    let fix_caption = gtk::Label::new(Some("ADJUST 1.00 kHz"));
    fix_caption.set_xalign(0.0);
    fix_caption.add_css_class("section-label");
    card.append(&fix_caption);
    let fix_row = adw::WrapBox::new();
    fix_row.set_child_spacing(8);
    fix_row.set_line_spacing(8);
    let mut fixes = Vec::new();
    for (text, delta) in [
        ("−3 dB", -3.0),
        ("−1 dB", -1.0),
        ("+1 dB", 1.0),
        ("+3 dB", 3.0),
    ] {
        let button = gtk::Button::with_label(text);
        button.add_css_class("tone-fix");
        button.set_tooltip_text(Some(
            "Add a peak filter here for the selected ears, or adjust the one already here",
        ));
        fix_row.append(&button);
        fixes.push((button, delta));
    }
    card.append(&fix_row);
    page.append(&card);
    let ui = Rc::new(ToneUi {
        play,
        signal,
        band,
        ears,
        frequency,
        frequency_label,
        level,
        level_label,
        fix_caption,
        fixes,
        sweep: Cell::new(None),
        sweep_id: Cell::new(0),
    });
    (page, ui)
}

fn tone_settings(ui: &ToneUi) -> serde_json::Value {
    serde_json::json!({
        "signal": if ui.signal.selected() == 1 { "pink" } else { "sine" },
        "band": match ui.band.selected() { 0 => "full", 1 => "octave", _ => "third" },
        "ears": match ui.ears.selected() { 0 => "left", 2 => "right", _ => "both" },
        "frequency": tone_frequency(ui.frequency.value()),
        "level_db": ui.level.value(),
        "sweep": ui.sweep.get().map(|(start, rate, id)| serde_json::json!({
            "start": start,
            "octaves_per_second": rate,
            "id": id,
        })),
    })
}

/// Send the tone's settings (which also keeps it alive), or stop it.
fn push_tone(model: &Model, ui: &ToneUi, device_drop: &gtk::DropDown, status: &gtk::Label) {
    if model.tasks.is_closed() {
        return;
    }
    let frequency = tone_frequency(ui.frequency.value());
    ui.frequency_label
        .set_text(&format_band_frequency(frequency));
    ui.fix_caption
        .set_text(&format!("ADJUST {}", format_band_frequency(frequency)));
    ui.level_label
        .set_text(&format!("{:.0} dBFS", ui.level.value()).replace('-', "−"));
    ui.band.set_sensitive(ui.signal.selected() == 1);
    if !ui.play.is_active() {
        model.overlay.borrow_mut().tone = None;
        return;
    }
    model.overlay.borrow_mut().tone = Some(frequency);
    let key = model
        .devices
        .borrow()
        .get(device_drop.selected() as usize)
        .filter(|device| device.connected)
        .map(|device| device.key.as_storage_key());
    let Some(key) = key else {
        status.set_text("Connect an output to play a tone");
        ui.play.set_active(false);
        return;
    };
    model.live_worker.tone(key, tone_settings(ui));
}

/// Q of the fix filter: as wide as what the listener was hearing.
fn tone_fix_q(ui: &ToneUi) -> f64 {
    match (ui.signal.selected(), ui.band.selected()) {
        (1, 0) | (1, 1) => std::f64::consts::SQRT_2,
        _ => 4.318,
    }
}

/// Add `delta_db` at `frequency` for `channels`: adjust a peak filter already
/// within a twelfth of an octave there, or add one. Returns the filter index.
fn apply_tone_fix(
    document: &mut ProfileDocument,
    frequency: f64,
    q: f64,
    channels: ChannelSelection,
    delta_db: f64,
) -> Option<usize> {
    let existing = document.filters.iter().position(|filter| {
        filter.kind == FilterKind::Peaking
            && filter.channels == channels
            && (filter.frequency / frequency).log2().abs() <= 1.0 / 12.0
    });
    if let Some(index) = existing {
        let filter = &mut document.filters[index];
        filter.gain_db = (filter.gain_db + delta_db).clamp(-20.0, 20.0);
        filter.enabled = true;
        return Some(index);
    }
    if !can_add_filter(document, channels) {
        return None;
    }
    document.graphic_eqs.clear();
    document.filters.push(Filter {
        enabled: true,
        kind: FilterKind::Peaking,
        frequency: (frequency * 100.0).round() / 100.0,
        gain_db: delta_db,
        q,
        channels,
    });
    Some(document.filters.len() - 1)
}

#[allow(clippy::too_many_arguments)]
fn wire_tone(
    window: &adw::ApplicationWindow,
    model: &Rc<Model>,
    ui: &Rc<ToneUi>,
    device_drop: &gtk::DropDown,
    filter_list: &gtk::FlowBox,
    text_view: &gtk::TextView,
    analysis_label: &gtk::Label,
    graph: &gtk::DrawingArea,
    status: &gtk::Label,
) {
    let push = Rc::new({
        let model = model.clone();
        let ui = ui.clone();
        let device_drop = device_drop.clone();
        let status = status.clone();
        let graph = graph.clone();
        move || {
            push_tone(&model, &ui, &device_drop, &status);
            graph.queue_draw();
        }
    });
    ui.play.connect_toggled({
        let model = model.clone();
        let push = push.clone();
        let analysis_label = analysis_label.clone();
        move |play| {
            play.set_icon_name(if play.is_active() {
                "media-playback-stop-symbolic"
            } else {
                "media-playback-start-symbolic"
            });
            if !play.is_active() {
                model.live_worker.stop_tone();
            }
            push();
            // Hold the gain from now until the tone stops, then resume matching.
            model.tone_hold.set(None);
            tone_hold(&model);
            let preview = model
                .document
                .borrow()
                .as_ref()
                .map(|document| preview_analysis(&model, document));
            if let Some(preview) = preview {
                set_analysis_label(&analysis_label, &preview);
                *model.analysis.borrow_mut() = Some(preview);
            }
            schedule_live(&model);
        }
    });
    ui.signal.connect_changed({
        let ui = ui.clone();
        let push = push.clone();
        move || {
            // Lowering the limit pulls a louder setting down with it.
            let upper = if ui.signal.selected() == 1 {
                NOISE_MAX_LEVEL_DB
            } else {
                TONE_MAX_LEVEL_DB
            };
            ui.level.adjustment().set_upper(upper);
            if ui.level.value() > upper {
                ui.level.set_value(upper);
            }
            push();
        }
    });
    for segmented in [&ui.band, &ui.ears] {
        let push = push.clone();
        segmented.connect_changed(move || push());
    }
    for scale in [&ui.frequency, &ui.level] {
        let push = push.clone();
        scale.connect_value_changed(move |_| push());
    }
    {
        let push = push.clone();
        device_drop.connect_selected_notify(move |_| push());
    }
    // Renew the tone while it plays; the service stops it five seconds after
    // the last renewal, so a crashed editor cannot leave a tone running.
    let tone_timer = gtk::glib::timeout_add_seconds_local(2, {
        let ui = ui.clone();
        let push = push.clone();
        move || {
            if ui.play.is_active() {
                push();
            }
            gtk::glib::ControlFlow::Continue
        }
    });
    model.tasks.track(tone_timer);
    // The graph's listener remains available while editing bands. Text/IR
    // navigation is wired at the stacks, and closing still stops playback.
    window.connect_close_request({
        let model = model.clone();
        move |_| {
            model.live_worker.stop_tone();
            gtk::glib::Propagation::Proceed
        }
    });

    let buffer = text_view.buffer();
    for (button, delta) in &ui.fixes {
        let delta = *delta;
        button.connect_clicked({
            let model = model.clone();
            let ui = ui.clone();
            let buffer = buffer.clone();
            let filter_list = filter_list.clone();
            let analysis_label = analysis_label.clone();
            let graph = graph.clone();
            let status = status.clone();
            move |_| {
                let frequency = tone_frequency(ui.frequency.value());
                let channels = match ui.ears.selected() {
                    0 => ChannelSelection::Left,
                    2 => ChannelSelection::Right,
                    _ => ChannelSelection::All,
                };
                let changed = {
                    let mut document = model.document.borrow_mut();
                    let Some(document) = document.as_mut() else {
                        return;
                    };
                    apply_tone_fix(document, frequency, tone_fix_q(&ui), channels, delta).map(
                        |index| {
                            (
                                index,
                                serialize_profile(document),
                                document.clone(),
                                preview_analysis(&model, document),
                            )
                        },
                    )
                };
                let Some((index, text, snapshot, preview)) = changed else {
                    status.set_text("This profile already has 64 bands on one channel");
                    return;
                };
                model.selected_filter.set(Some(index));
                set_editor_text(&model, &buffer, &text);
                rebuild_filter_list(&filter_list, &snapshot, &model, &buffer, &graph);
                set_analysis_label(&analysis_label, &preview);
                *model.analysis.borrow_mut() = Some(preview);
                graph.queue_draw();
                status.set_text(&format!(
                    "{} {:.0} dB at {}",
                    if delta < 0.0 { "Cut" } else { "Boosted" },
                    delta.abs(),
                    format_band_frequency(frequency)
                ));
            }
        });
    }
}

fn update_convolution_ui(ui: &ConvolutionUi, profile: &ProfileDocument) {
    ui.syncing.set(true);
    if let Some(convolution) = profile.convolutions.first() {
        let file_name = convolution
            .path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("Impulse response");
        ui.file_name.set_text(file_name);
        let layout = ui.layout.get();
        ui.file_path.set_text(&match layout {
            Some(layout) if layout.is_matrix() => {
                format!("{} · {}", convolution.path.display(), layout.label())
            }
            _ => convolution.path.display().to_string(),
        });
        ui.channel.set_selected(match convolution.channels {
            ChannelSelection::All => 0,
            ChannelSelection::Left => 1,
            ChannelSelection::Right => 2,
        });
        // A multichannel IR already says which input reaches which ear.
        let matrix = layout.is_some_and(ImpulseLayout::is_matrix);
        ui.channel.set_sensitive(!matrix);
        ui.channel
            .set_tooltip_text(matrix.then_some("This impulse response feeds both ears"));
        ui.remove.set_sensitive(true);
    } else {
        ui.file_name.set_text("No impulse response selected");
        ui.file_path.set_text(
            "WAV, FLAC, AIFF, or OGG · up to 10 seconds · mono or stereo (true-stereo and 7.1 BRIRs also work)",
        );
        ui.channel.set_selected(0);
        ui.channel.set_sensitive(true);
        ui.channel.set_tooltip_text(None);
        ui.remove.set_sensitive(false);
    }
    ui.syncing.set(false);
}

/// Open a freshly loaded profile on the page that holds its processing: the
/// impulse response when that is all it has, otherwise the filters.
fn show_profile_page(ui: &ConvolutionUi, profile: &ProfileDocument) {
    let _ = profile;
    (ui.sync_tabs)();
}

struct GraphToneUi {
    tone: Rc<ToneUi>,
    /// Plays the tone at the graph's tone line; lives in the footer.
    play: gtk::ToggleButton,
    cursor: gtk::ToggleButton,
    sweep: gtk::ToggleButton,
}

/// Seconds a sweep takes across the whole range, from the speed menu.
fn speed_seconds(speed: &gtk::DropDown) -> f64 {
    match speed.selected() {
        0 => 15.0,
        2 => 60.0,
        _ => 30.0,
    }
}

/// Hand a sweep from slider position `start` to the service, which plays it.
/// Each sweep gets a new id, so starting the same one again restarts it.
fn start_sweep(tone: &ToneUi, start: f64, full_seconds: f64) {
    let id = tone.sweep_id.get().wrapping_add(1);
    tone.sweep_id.set(id);
    tone.sweep
        .set(Some((tone_frequency(start), sweep_rate(full_seconds), id)));
}

/// Octaves per second for a sweep across 20 Hz–20 kHz in `full_seconds`.
fn sweep_rate(full_seconds: f64) -> f64 {
    1000.0_f64.log2() / full_seconds.max(1.0)
}

fn sweep_position(start: f64, elapsed_seconds: f64, full_seconds: f64) -> f64 {
    (start + elapsed_seconds.max(0.0) / full_seconds.max(1.0)).clamp(0.0, 1.0)
}

fn build_graph_tone(
    tone: &Rc<ToneUi>,
    tasks: &editor_tasks::EditorTasks,
) -> (adw::WrapBox, GraphToneUi) {
    let row = adw::WrapBox::builder()
        .child_spacing(8)
        .line_spacing(8)
        .build();
    row.add_css_class("graph-listen");
    let play = gtk::ToggleButton::new();
    {
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        content.append(&gtk::Image::from_icon_name("audio-volume-high-symbolic"));
        content.append(&gtk::Label::new(Some("TONE GEN")));
        play.set_child(Some(&content));
    }
    play.set_tooltip_text(Some(
        "Play a test tone at the graph's tone line. The output level holds while it plays, \
         so a band change sounds like exactly that change. Pause music first.",
    ));
    play.update_property(&[gtk::accessible::Property::Label("Tone generator")]);
    let sweep = gtk::ToggleButton::with_label("Sweep");
    sweep.set_tooltip_text(Some(
        "Move a sine tone upward through the graph. Turn Sweep off to hold the current frequency.",
    ));
    let cursor = gtk::ToggleButton::with_label("Tone cursor");
    cursor.set_tooltip_text(Some(
        "Drag anywhere on the graph to move the tone frequency. Turn off to edit EQ points.",
    ));
    let speed = gtk::DropDown::from_strings(&["15 s", "30 s", "60 s"]);
    speed.set_selected(1);
    speed.update_property(&[gtk::accessible::Property::Label("Full sweep duration")]);
    let frequency = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.001);
    frequency.set_draw_value(false);
    frequency.set_size_request(120, -1);
    frequency.set_value(tone.frequency.value());
    frequency.update_property(&[gtk::accessible::Property::Label(
        "Graph tone frequency slider",
    )]);
    let readout = gtk::Label::new(Some("1.00 kHz"));
    readout.add_css_class("level-code");
    let ears = gtk::DropDown::from_strings(&["Both ears", "Left ear", "Right ear"]);
    ears.update_property(&[gtk::accessible::Property::Label("Graph tone ears")]);
    // The tone page below carries frequency, ears, and the corrections.
    for widget in [
        sweep.upcast_ref::<gtk::Widget>(),
        speed.upcast_ref(),
        cursor.upcast_ref(),
    ] {
        row.append(widget);
    }
    play.connect_toggled({
        let tone = tone.clone();
        move |button| tone.play.set_active(button.is_active())
    });
    tone.play.connect_toggled({
        let play = play.clone();
        let sweep = sweep.clone();
        move |button| {
            play.set_active(button.is_active());
            if !button.is_active() {
                sweep.set_active(false);
            }
        }
    });
    frequency.connect_value_changed({
        let tone = tone.clone();
        move |scale| tone.frequency.set_value(scale.value())
    });
    tone.frequency.connect_value_changed({
        let frequency = frequency.clone();
        let readout = readout.clone();
        move |scale| {
            frequency.set_value(scale.value());
            readout.set_text(&format_band_frequency(tone_frequency(scale.value())));
        }
    });
    ears.connect_selected_notify({
        let tone = tone.clone();
        move |drop| {
            tone.ears.buttons[match drop.selected() {
                1 => 0,
                2 => 2,
                _ => 1,
            }]
            .set_active(true);
        }
    });
    for (index, button) in tone.ears.buttons.iter().enumerate() {
        let ears = ears.clone();
        button.connect_toggled(move |b| {
            if b.is_active() {
                ears.set_selected(match index {
                    0 => 1,
                    2 => 2,
                    _ => 0,
                });
            }
        });
    }
    let clock = Rc::new(RefCell::new(None::<(std::time::Instant, f64)>));
    sweep.connect_toggled({
        let tone = tone.clone();
        let clock = clock.clone();
        let speed = speed.clone();
        move |button| {
            if button.is_active() {
                tone.signal.buttons[0].set_active(true);
                if tone.frequency.value() >= 0.999 {
                    tone.frequency.set_value(0.0);
                }
                let start = tone.frequency.value();
                *clock.borrow_mut() = Some((std::time::Instant::now(), start));
                start_sweep(&tone, start, speed_seconds(&speed));
                tone.play.set_active(true);
            } else {
                *clock.borrow_mut() = None;
                tone.sweep.set(None);
            }
        }
    });
    speed.connect_selected_notify({
        let clock = clock.clone();
        let tone = tone.clone();
        move |drop| {
            let running = clock.borrow().is_some();
            if running {
                let start = tone.frequency.value();
                *clock.borrow_mut() = Some((std::time::Instant::now(), start));
                start_sweep(&tone, start, speed_seconds(drop));
            }
        }
    });
    let sweep_timer = gtk::glib::timeout_add_local(std::time::Duration::from_millis(25), {
        let tone = tone.clone();
        let sweep = sweep.clone();
        let clock = clock.clone();
        move || {
            let running = *clock.borrow();
            if let Some((start, position)) = running {
                let seconds = speed_seconds(&speed);
                let next = sweep_position(position, start.elapsed().as_secs_f64(), seconds);
                tone.frequency.set_value(next);
                if next >= 1.0 {
                    sweep.set_active(false);
                    tone.play.set_active(false);
                }
            }
            gtk::glib::ControlFlow::Continue
        }
    });
    tasks.track(sweep_timer);
    (
        row,
        GraphToneUi {
            tone: tone.clone(),
            play,
            cursor,
            sweep,
        },
    )
}

struct ListeningUi {
    undo: gtk::Button,
    redo: gtk::Button,
    reference: gtk::ToggleButton,
    bass: knob::Knob,
    tilt: knob::Knob,
    syncing: Cell<bool>,
}

/// The listening controls, for placing in the header, A/B menu, and footer.
struct ListeningWidgets {
    undo: gtk::Button,
    redo: gtk::Button,
    reference: gtk::ToggleButton,
    capture: gtk::Button,
    bass: knob::LabeledKnob,
    tilt: knob::LabeledKnob,
}

fn submit_live(model: &Rc<Model>, save: bool) {
    if model.tasks.is_closed() {
        return;
    }
    let Some(id) = model.current_id.borrow().clone() else {
        return;
    };
    let Some(document) = model.document.borrow().clone() else {
        return;
    };
    if !document.is_activatable() {
        return;
    }
    let text = serialize_profile(&document);
    let reference = model
        .reference
        .borrow()
        .clone()
        .filter(|snapshot| {
            parse_text(&document.name, &snapshot.text).convolutions == document.convolutions
        })
        .map(|snapshot| live_client::Reference {
            text: snapshot.text,
            trim: snapshot.trim,
        });
    let hear_reference = reference.is_some() && model.hear_reference.get();
    let hold_gain_db = tone_hold(model);
    let key = serde_json::to_string(&(
        &id,
        &document.name,
        &text,
        model.manual_trim.get(),
        &reference,
        hear_reference,
        hold_gain_db,
    ))
    .unwrap_or_default();
    let revision = if model.last_submitted.borrow().as_ref() != Some(&key) {
        *model.last_submitted.borrow_mut() = Some(key);
        let next = model.live_revision.get().wrapping_add(1);
        model.live_revision.set(next);
        next
    } else {
        model.live_revision.get()
    };
    // The local bank keeps the latest draft even if the user switches away
    // and back before a background save completes.
    if let Some(profile) = model.profiles.borrow_mut().iter_mut().find(|p| p.id == id) {
        profile.text = text.clone();
        profile.manual_trim_db = model.manual_trim.get();
        if profile.name != document.name {
            model.bank_names_dirty.set(true);
        }
        profile.name = document.name.clone();
    }
    model.live_worker.submit(live_client::Draft {
        revision,
        id,
        name: document.name,
        text,
        trim: model.manual_trim.get(),
        reference,
        hear_reference,
        hold_gain_db,
        save,
    });
}

/// The gain to hold while the tone generator plays: taken from the open
/// profile's analysis the first time it is asked for after the tone starts.
fn tone_hold(model: &Model) -> Option<f64> {
    let playing = model
        .graph_tone
        .borrow()
        .as_ref()
        .is_some_and(|ui| ui.tone.play.is_active());
    if !playing {
        model.tone_hold.set(None);
    } else if model.tone_hold.get().is_none() {
        model.tone_hold.set(
            model
                .analysis
                .borrow()
                .as_ref()
                .map(|analysis| analysis.effective_gain_db),
        );
    }
    LEVEL_HELD.with(|held| held.set(model.tone_hold.get().is_some()));
    model.tone_hold.get()
}

/// How far below full scale the loudest tone plays (the service's limit is
/// -12 dBFS), so how far a held gain may exceed music's clipping safety.
const TONE_HEADROOM_DB: f64 = 11.0;

/// Shows the service's held gain in an analysis, as `hold_output_gain` does.
fn apply_tone_hold(model: &Model, analysis: &mut ProfileAnalysis) {
    let Some(held) = model.tone_hold.get() else {
        return;
    };
    let peak = analysis.left.peak_db.max(analysis.right.peak_db);
    let ceiling = -1.0 - peak + TONE_HEADROOM_DB;
    analysis.effective_gain_db = held.min(ceiling);
    analysis.safety_attenuation_db =
        analysis.effective_gain_db - analysis.match_gain_db - analysis.manual_trim_db;
    analysis.headroom_limited = held > ceiling + 0.005;
}

fn schedule_live(model: &Rc<Model>) {
    if model.tasks.is_closed() {
        return;
    }
    model
        .live_revision
        .set(model.live_revision.get().wrapping_add(1));
    if model.pending_live.borrow().is_some() {
        return;
    }
    let model = model.clone();
    let callback = model.clone();
    *model.pending_live.borrow_mut() = Some(gtk::glib::timeout_add_local_once(
        std::time::Duration::from_millis(35),
        move || {
            callback.pending_live.borrow_mut().take();
            submit_live(&callback, false);
        },
    ));
}

fn schedule_save(model: &Rc<Model>) {
    if model.tasks.is_closed() {
        return;
    }
    if let Some(timer) = model.pending_save.borrow_mut().take() {
        timer.remove();
    }
    let model = model.clone();
    let callback = model.clone();
    *model.pending_save.borrow_mut() = Some(gtk::glib::timeout_add_local_once(
        std::time::Duration::from_millis(300),
        move || {
            callback.pending_save.borrow_mut().take();
            callback.history.borrow_mut().checkpoint();
            submit_live(&callback, true);
        },
    ));
}

fn set_editor_text(model: &Rc<Model>, buffer: &gtk::TextBuffer, text: &str) {
    model.syncing_text.set(true);
    buffer.set_text(text);
    model.syncing_text.set(false);
    if model.loading.get() {
        return;
    }
    let name = model
        .document
        .borrow()
        .as_ref()
        .map(|d| d.name.clone())
        .unwrap_or_default();
    let document = parse_text(name, text);
    if !document.is_activatable() {
        return;
    }
    *model.document.borrow_mut() = Some(document.clone());
    *model.analysis.borrow_mut() = Some(preview_analysis(model, &document));
    remember_edit(model);
    schedule_live(model);
    schedule_save(model);
}

fn remember_edit(model: &Rc<Model>) {
    model.unsaved.set(true);
    if !model.restoring_history.get()
        && let Some(document) = model.document.borrow().as_ref()
    {
        model.history.borrow_mut().record(editing::Snapshot {
            text: serialize_profile(document),
            trim: model.manual_trim.get(),
        });
    }
    sync_listening(model);
}

fn sync_listening(model: &Model) {
    let ui = model.listening.borrow();
    let Some(ui) = ui.as_ref() else {
        return;
    };
    ui.syncing.set(true);
    ui.undo.set_sensitive(model.history.borrow().can_undo());
    ui.redo.set_sensitive(model.history.borrow().can_redo());
    let compatible = model.document.borrow().as_ref().is_some_and(|document| {
        model.reference.borrow().as_ref().is_some_and(|snapshot| {
            parse_text(&document.name, &snapshot.text).convolutions == document.convolutions
        })
    });
    ui.reference.set_sensitive(compatible);
    if !compatible {
        model.hear_reference.set(false);
    }
    ui.reference.set_active(model.hear_reference.get());
    if let Some(document) = model.document.borrow().as_ref() {
        ui.bass.set_value(editing::bass_db(document));
        ui.tilt.set_value(editing::tilt_db_per_octave(document));
    }
    ui.syncing.set(false);
}

fn build_listening_controls(
    window: &adw::ApplicationWindow,
    model: &Rc<Model>,
    buffer: &gtk::TextBuffer,
    list: &gtk::FlowBox,
    graph: &gtk::DrawingArea,
    trim: &knob::Knob,
    status: &gtk::Label,
) -> ListeningWidgets {
    let undo = gtk::Button::from_icon_name("edit-undo-symbolic");
    let redo = gtk::Button::from_icon_name("edit-redo-symbolic");
    for (button, name) in [(&undo, "Undo"), (&redo, "Redo")] {
        button.add_css_class("bar-icon");
        button.set_tooltip_text(Some(name));
        button.update_property(&[gtk::accessible::Property::Label(name)]);
    }
    let reference = gtk::ToggleButton::with_label("Hear starting sound");
    reference.set_tooltip_text(Some(
        "Compare your captured starting sound and current EQ at the same estimated perceived level",
    ));
    let capture = gtk::Button::with_label("Capture start");
    capture.set_tooltip_text(Some(
        "Use the current EQ as the new starting sound for comparison",
    ));
    let bass = knob::LabeledKnob::new(
        knob::Knob::new(-12.0, 12.0, 0.0, knob::Taper::Linear, true),
        "Bass",
        |value| signed_db(value, 1),
    );
    bass.knob.set_tooltip_text(Some(
        "Broad low shelf at 105 Hz. This adjustment remains editable as a normal EQ band.",
    ));
    let tilt = knob::LabeledKnob::new(
        knob::Knob::new(
            -MAX_TILT_DB_PER_OCTAVE,
            MAX_TILT_DB_PER_OCTAVE,
            0.0,
            knob::Taper::Linear,
            true,
        ),
        "Tilt",
        |value| signed_db(value, 2),
    );
    tilt.knob.set_tooltip_text(Some(
        "Slope in dB per octave across the whole range, pivoting at 1 kHz. \
         Positive is brighter; negative is warmer. −0.5 is about +3 dB at 20 Hz and −2 dB at 20 kHz.",
    ));
    *model.listening.borrow_mut() = Some(ListeningUi {
        undo: undo.clone(),
        redo: redo.clone(),
        reference: reference.clone(),
        bass: bass.knob.clone(),
        tilt: tilt.knob.clone(),
        syncing: Cell::new(false),
    });
    for (button, backwards) in [(&undo, true), (&redo, false)] {
        button.connect_clicked({
            let model = model.clone();
            let buffer = buffer.clone();
            let list = list.clone();
            let graph = graph.clone();
            let trim = trim.clone();
            move |_| {
                let snapshot = {
                    let mut history = model.history.borrow_mut();
                    if backwards {
                        history.undo()
                    } else {
                        history.redo()
                    }
                };
                let Some(snapshot) = snapshot else {
                    return;
                };
                model.restoring_history.set(true);
                model.hear_reference.set(false);
                model.manual_trim.set(snapshot.trim);
                model.loading.set(true);
                trim.set_value(snapshot.trim);
                model.loading.set(false);
                let name = model
                    .document
                    .borrow()
                    .as_ref()
                    .map(|d| d.name.clone())
                    .unwrap_or_default();
                *model.document.borrow_mut() = Some(parse_text(&name, &snapshot.text));
                set_editor_text(&model, &buffer, &snapshot.text);
                model.restoring_history.set(false);
                if let Some(document) = model.document.borrow().as_ref().cloned() {
                    rebuild_filter_list(&list, &document, &model, &buffer, &graph);
                }
                sync_listening(&model);
                graph.queue_draw();
            }
        });
    }
    reference.connect_toggled({
        let model = model.clone();
        move |button| {
            if model
                .listening
                .borrow()
                .as_ref()
                .is_some_and(|ui| ui.syncing.get())
            {
                return;
            }
            model.hear_reference.set(button.is_active());
            submit_live(&model, false);
        }
    });
    capture.connect_clicked({
        let model = model.clone();
        let status = status.clone();
        move |_| {
            if let Some(document) = model.document.borrow().as_ref() {
                *model.reference.borrow_mut() = Some(editing::Snapshot {
                    text: serialize_profile(document),
                    trim: model.manual_trim.get(),
                });
            }
            model.hear_reference.set(false);
            sync_listening(&model);
            submit_live(&model, false);
            status.set_text("Starting sound captured; comparisons are level matched");
        }
    });
    for (spin, is_bass) in [(&bass.knob, true), (&tilt.knob, false)] {
        spin.connect_value_changed({
            let model = model.clone();
            let buffer = buffer.clone();
            let graph = graph.clone();
            let list = list.clone();
            let status = status.clone();
            move |spin| {
                if model.loading.get()
                    || model
                        .listening
                        .borrow()
                        .as_ref()
                        .is_some_and(|ui| ui.syncing.get())
                {
                    return;
                }
                let result = {
                    let mut document = model.document.borrow_mut();
                    let Some(document) = document.as_mut() else {
                        return;
                    };
                    (if is_bass {
                        editing::set_bass(document, spin.value())
                    } else {
                        editing::set_tilt(document, spin.value())
                    })
                    .map(|()| document.clone())
                };
                match result {
                    Ok(document) => {
                        model.hear_reference.set(false);
                        set_editor_text(&model, &buffer, &serialize_profile(&document));
                        rebuild_filter_list(&list, &document, &model, &buffer, &graph);
                        graph.queue_draw();
                    }
                    Err(error) => {
                        status.set_text(&error);
                        sync_listening(&model);
                    }
                }
            }
        });
    }
    let shortcuts = gtk::ShortcutController::new();
    shortcuts.set_scope(gtk::ShortcutScope::Managed);
    for (trigger, button) in [
        ("<Control>z", undo.clone()),
        ("<Control><Shift>z", redo.clone()),
    ] {
        shortcuts.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string(trigger),
            Some(gtk::CallbackAction::new(move |widget, _| {
                // Text entries keep their native text-editing undo behavior.
                if widget.root().and_then(|r| r.focus()).is_some_and(|f| {
                    f.is::<gtk::TextView>() || f.is::<gtk::Text>() || f.is::<gtk::Entry>()
                }) {
                    return gtk::glib::Propagation::Proceed;
                }
                button.emit_clicked();
                gtk::glib::Propagation::Stop
            })),
        ));
    }
    window.add_controller(shortcuts);
    ListeningWidgets {
        undo,
        redo,
        reference,
        capture,
        bass,
        tilt,
    }
}

/// A band's chip in the footer: its symbol and frequency; selects the band.
struct BandTile {
    select: gtk::ToggleButton,
    glyph: gtk::DrawingArea,
    /// The band number, coloured by ear as its node on the graph is.
    number: gtk::Label,
    label: gtk::Label,
}

/// The selected band's panel, floating beside its node on the graph.
struct BandUi {
    root: gtk::Box,
    overlay: gtk::Overlay,
    kind: Segmented,
    channels: Segmented,
    frequency: knob::Knob,
    gain: knob::Knob,
    q: knob::Knob,
    enabled: gtk::ToggleButton,
    tiles: Vec<BandTile>,
    /// Add band, kept as the last item among the chips so it wraps with them.
    add: gtk::Button,
    syncing: Rc<Cell<bool>>,
}

/// The band types the editor offers, as Squiglink does. Imported profiles may
/// still hold other types; they play and show their symbol until changed.
const BAND_KINDS: [FilterKind; 3] = [
    FilterKind::LowShelf,
    FilterKind::Peaking,
    FilterKind::HighShelf,
];
/// Chip height and the gap between chip rows, matching the CSS.
const BAND_CHIP_HEIGHT: i32 = 30;
const BAND_CHIP_SPACING: i32 = 6;
const BAND_KIND_LABELS: [&str; 3] = ["LS", "PK", "HS"];
const BAND_KIND_NAMES: [&str; 3] = ["Low shelf", "Peak", "High shelf"];

fn kind_name(kind: FilterKind) -> &'static str {
    match kind {
        FilterKind::Peaking => "Peak",
        FilterKind::LowShelf => "Low shelf",
        FilterKind::HighShelf => "High shelf",
        FilterKind::LowPass => "Low pass",
        FilterKind::HighPass => "High pass",
        FilterKind::BandPass => "Band pass",
        FilterKind::Notch => "Notch",
        FilterKind::AllPass => "All pass",
    }
}

/// Where the band panel sits relative to its node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PanelSide {
    Below,
    Above,
    Right,
    Left,
    /// At the top or bottom edge of the graph, clear of lines around the node.
    Top,
    Bottom,
}

/// Gap between the node and the panel, in px.
const PANEL_GAP: f64 = 20.0;
/// Keep the panel this far inside the graph, in px.
const PANEL_MARGIN: f64 = 8.0;

/// Top-left corner of the panel on `side` of `node`, kept inside the graph.
fn panel_origin(
    side: PanelSide,
    node: (f64, f64),
    size: (f64, f64),
    area: (f64, f64),
) -> (f64, f64) {
    let (x, y) = node;
    let (panel_width, panel_height) = size;
    let (left, top) = match side {
        PanelSide::Below => (x - panel_width / 2.0, y + PANEL_GAP),
        PanelSide::Above => (x - panel_width / 2.0, y - PANEL_GAP - panel_height),
        PanelSide::Right => (x + PANEL_GAP, y - panel_height / 2.0),
        PanelSide::Left => (x - PANEL_GAP - panel_width, y - panel_height / 2.0),
        // Below the legend and the gain readout.
        PanelSide::Top => (x - panel_width / 2.0, 64.0),
        PanelSide::Bottom => (x - panel_width / 2.0, area.1),
    };
    (
        left.clamp(
            PANEL_MARGIN,
            (area.0 - panel_width - PANEL_MARGIN).max(PANEL_MARGIN),
        ),
        top.clamp(
            PANEL_MARGIN,
            (area.1 - panel_height - PANEL_MARGIN).max(PANEL_MARGIN),
        ),
    )
}

/// The side whose panel hides the least of the graph's lines. Lines near the
/// node's frequency count most, a panel over the node itself is ruled out,
/// and `previous` wins unless another side is clearly better.
fn best_panel_side(
    node: (f64, f64),
    size: (f64, f64),
    area: (f64, f64),
    lines: &[(f64, f64)],
    previous: Option<PanelSide>,
) -> PanelSide {
    let cost = |side: PanelSide| {
        let (left, top) = panel_origin(side, node, size, area);
        let (right, bottom) = (left + size.0, top + size.1);
        let inside = |(x, y): (f64, f64), pad: f64| {
            x >= left - pad && x <= right + pad && y >= top - pad && y <= bottom + pad
        };
        if inside(node, 12.0) {
            return f64::INFINITY;
        }
        let hidden = lines
            .iter()
            .filter(|point| inside(**point, 0.0))
            .map(|(x, _)| {
                // Within about a panel's width of the node, a line is the
                // band's own region.
                let distance = (x - node.0) / 180.0;
                1.0 + 4.0 * (-distance * distance).exp()
            })
            .sum::<f64>();
        // The order breaks ties: next to the node before the edges.
        hidden
            + match side {
                PanelSide::Below => 0.0,
                PanelSide::Above => 0.5,
                PanelSide::Right => 1.0,
                PanelSide::Left => 1.5,
                PanelSide::Top => 3.0,
                PanelSide::Bottom => 3.5,
            }
    };
    let sides = [
        PanelSide::Below,
        PanelSide::Above,
        PanelSide::Right,
        PanelSide::Left,
        PanelSide::Top,
        PanelSide::Bottom,
    ];
    let best = sides
        .into_iter()
        .min_by(|a, b| cost(*a).total_cmp(&cost(*b)))
        .unwrap_or(PanelSide::Below);
    match previous {
        Some(previous) if cost(previous).is_finite() && cost(previous) <= cost(best) + 8.0 => {
            previous
        }
        _ => best,
    }
}

/// Build the selected band's floating panel and place it on `overlay`, which
/// holds the graph. It follows the band's node as it moves.
fn build_band_editor(
    model: &Rc<Model>,
    buffer: &gtk::TextBuffer,
    graph: &gtk::DrawingArea,
    list: &gtk::FlowBox,
    overlay: &gtk::Overlay,
    add: &gtk::Button,
) {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
    root.add_css_class("band-panel");
    root.set_halign(gtk::Align::Start);
    root.set_valign(gtk::Align::Start);
    let top = adw::WrapBox::builder()
        .child_spacing(8)
        .line_spacing(8)
        .build();
    let kind = Segmented::new(&BAND_KIND_LABELS, 1);
    kind.root.add_css_class("kind-picker");
    for ((button, name), filter_kind) in kind.buttons.iter().zip(BAND_KIND_NAMES).zip(BAND_KINDS) {
        button.set_child(Some(&glyph::widget(filter_kind, 26, 16)));
        button.set_tooltip_text(Some(name));
        // The text label `with_label` pointed at is gone; name the glyph button directly.
        button.reset_relation(gtk::AccessibleRelation::LabelledBy);
        button.update_property(&[gtk::accessible::Property::Label(name)]);
    }
    top.append(&kind.root);
    let channels = Segmented::new(&["L", "L+R", "R"], 1);
    channels.root.add_css_class("ear-picker");
    for (button, name) in channels
        .buttons
        .iter()
        .zip(["Left ear", "Both ears", "Right ear"])
    {
        button.set_tooltip_text(Some(name));
        button.reset_relation(gtk::AccessibleRelation::LabelledBy);
        button.update_property(&[gtk::accessible::Property::Label(name)]);
    }
    top.append(&channels.root);
    let enabled = gtk::ToggleButton::new();
    enabled.set_icon_name("system-shutdown-symbolic");
    enabled.add_css_class("band-power");
    enabled.set_tooltip_text(Some("Band on or off"));
    enabled.update_property(&[gtk::accessible::Property::Label("Selected band enabled")]);
    top.append(&enabled);
    let remove = gtk::Button::from_icon_name("window-close-symbolic");
    remove.add_css_class("band-remove");
    remove.set_tooltip_text(Some("Delete band"));
    remove.update_property(&[gtk::accessible::Property::Label("Delete selected band")]);
    top.append(&remove);
    root.append(&top);

    let knobs = gtk::Box::new(gtk::Orientation::Horizontal, 18);
    knobs.set_halign(gtk::Align::Center);
    let frequency = knob::LabeledKnob::new(
        knob::Knob::new(20.0, 20_000.0, 1_000.0, knob::Taper::Log, false),
        "Freq",
        format_band_frequency,
    );
    let gain = knob::LabeledKnob::new(
        knob::Knob::new(-24.0, 24.0, 0.0, knob::Taper::Linear, true),
        "Gain",
        |value| format!("{} dB", signed_db(value, 1)),
    );
    let q = knob::LabeledKnob::new(
        knob::Knob::new(0.1, 20.0, 1.0, knob::Taper::Log, false),
        "Q",
        |value| format!("{value:.2}"),
    );
    for (control, label) in [(&frequency, "Frequency"), (&gain, "Gain"), (&q, "Q")] {
        control.knob.set_size_request(48, 48);
        control
            .knob
            .update_property(&[gtk::accessible::Property::Label(&format!(
                "Selected band {label}"
            ))]);
        knobs.append(&control.root);
    }
    root.append(&knobs);
    root.set_visible(false);
    overlay.add_overlay(&root);
    // Clicks on the panel leave keyboard focus on the graph, so the arrow
    // keys keep editing the selected band.
    for button in kind.buttons.iter().chain(&channels.buttons) {
        button.set_focus_on_click(false);
    }
    enabled.set_focus_on_click(false);
    remove.set_focus_on_click(false);

    // The panel follows its node, but holds still while the pointer is on it
    // or one of its knobs is dragged: a panel moving under the pointer would
    // carry a knob away mid-drag and send the mouse wheel to the graph.
    let hovered = Rc::new(Cell::new(false));
    let dragging = Rc::new(Cell::new(false));
    let placed = Rc::new(Cell::new(None::<(usize, gtk::gdk::Rectangle)>));
    let motion = gtk::EventControllerMotion::new();
    motion.connect_enter({
        let hovered = hovered.clone();
        move |_, _, _| hovered.set(true)
    });
    motion.connect_leave({
        let hovered = hovered.clone();
        let overlay = overlay.clone();
        move |_| {
            hovered.set(false);
            overlay.queue_allocate();
        }
    });
    root.add_controller(motion);
    for control in [&frequency, &gain, &q] {
        let dragging = dragging.clone();
        let overlay = overlay.clone();
        control.knob.connect_dragging(move |active| {
            dragging.set(active);
            if !active {
                overlay.queue_allocate();
            }
        });
    }
    // Sit beside the node wherever the panel hides the least of the curves,
    // above all near the band's own frequency, which is what is being tuned.
    let side = Rc::new(Cell::new(PanelSide::Below));
    overlay.connect_get_child_position({
        let model = Rc::downgrade(model);
        let graph = graph.clone();
        let panel = root.clone();
        move |_, child| {
            if child != panel.upcast_ref::<gtk::Widget>() {
                return None;
            }
            let model = model.upgrade()?;
            let index = model.selected_filter.get()?;
            if (hovered.get() || dragging.get())
                && let Some((placed_index, rectangle)) = placed.get()
                && placed_index == index
            {
                return Some(rectangle);
            }
            let (frequency, gain_db) = model
                .document
                .borrow()
                .as_ref()?
                .filters
                .get(index)
                .map(|filter| (filter.frequency, filter.gain_db))?;
            let (width, height) = (graph.width() as f64, graph.height() as f64);
            let node = graph::filter_point_position(frequency, gain_db, width, height);
            // Narrow graphs get a narrower panel; its top row wraps to fit.
            let (minimum, natural, _, _) = panel.measure(gtk::Orientation::Horizontal, -1);
            let panel_width = (natural as f64).min(width - 16.0).max(minimum as f64);
            let (_, panel_height, _, _) =
                panel.measure(gtk::Orientation::Vertical, panel_width as i32);
            let size = (panel_width, panel_height as f64);
            let lines = graph::line_points(
                model.analysis.borrow().as_ref(),
                &model.overlay.borrow(),
                width,
                height,
            );
            let previous = (placed
                .get()
                .is_some_and(|(placed_index, _)| placed_index == index))
            .then(|| side.get());
            // Moving the node keeps the panel on its side rather than
            // letting it hop around under the pointer.
            let chosen = match previous {
                Some(previous) if model.graph_dragging.get() => previous,
                _ => best_panel_side(node, size, (width, height), &lines, previous),
            };
            side.set(chosen);
            let (left, top) = panel_origin(chosen, node, size, (width, height));
            let rectangle =
                gtk::gdk::Rectangle::new(left as i32, top as i32, panel_width as i32, panel_height);
            placed.set(Some((index, rectangle)));
            Some(rectangle)
        }
    });
    let syncing = Rc::new(Cell::new(false));
    for (button, filter_kind) in kind.buttons.iter().zip(BAND_KINDS) {
        button.connect_toggled({
            let model = model.clone();
            let buffer = buffer.clone();
            let graph = graph.clone();
            let syncing = syncing.clone();
            move |button| {
                if !syncing.get()
                    && button.is_active()
                    && let Some(index) = model.selected_filter.get()
                {
                    update_filter(&model, &buffer, &graph, index, |filter| {
                        filter.kind = filter_kind
                    });
                }
            }
        });
    }
    for (button, selection) in channels.buttons.iter().zip([
        ChannelSelection::Left,
        ChannelSelection::All,
        ChannelSelection::Right,
    ]) {
        button.connect_toggled({
            let model = model.clone();
            let buffer = buffer.clone();
            let graph = graph.clone();
            let syncing = syncing.clone();
            move |button| {
                if !syncing.get()
                    && button.is_active()
                    && let Some(index) = model.selected_filter.get()
                {
                    update_filter(&model, &buffer, &graph, index, |filter| {
                        filter.channels = selection
                    });
                }
            }
        });
    }
    for (control, parameter) in [(&frequency, 0), (&gain, 1), (&q, 2)] {
        control.knob.connect_value_changed({
            let model = model.clone();
            let buffer = buffer.clone();
            let graph = graph.clone();
            let syncing = syncing.clone();
            move |knob| {
                if !syncing.get()
                    && let Some(index) = model.selected_filter.get()
                {
                    update_filter(&model, &buffer, &graph, index, |filter| match parameter {
                        0 => filter.frequency = knob.value(),
                        1 => filter.gain_db = knob.value(),
                        _ => filter.q = knob.value(),
                    });
                }
            }
        });
    }
    enabled.connect_active_notify({
        let model = model.clone();
        let buffer = buffer.clone();
        let graph = graph.clone();
        let syncing = syncing.clone();
        move |button| {
            if !syncing.get()
                && let Some(index) = model.selected_filter.get()
            {
                update_filter(&model, &buffer, &graph, index, |filter| {
                    filter.enabled = button.is_active()
                });
            }
        }
    });
    remove.connect_clicked({
        let model = model.clone();
        let buffer = buffer.clone();
        let graph = graph.clone();
        let list = list.clone();
        move |_| {
            if let Some(index) = model.selected_filter.get() {
                remove_band(&model, &buffer, &graph, &list, index);
            }
        }
    });
    *model.bands.borrow_mut() = Some(BandUi {
        root,
        overlay: overlay.clone(),
        kind,
        channels,
        frequency: frequency.knob,
        gain: gain.knob,
        q: q.knob,
        enabled,
        tiles: Vec::new(),
        add: add.clone(),
        syncing,
    });
}

/// The enabled band whose node is under (x, y) on the graph, if any.
fn band_at(document: &ProfileDocument, x: f64, y: f64, width: f64, height: f64) -> Option<usize> {
    let width = width.max(1.0);
    let height = height.max(1.0);
    document
        .filters
        .iter()
        .enumerate()
        .filter(|(_, filter)| filter.enabled)
        .map(|(index, filter)| {
            let (px, py) =
                graph::filter_point_position(filter.frequency, filter.gain_db, width, height);
            (index, (px - x).hypot(py - y))
        })
        .filter(|(_, distance)| *distance <= 16.0)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(index, _)| index)
}

/// Delete one band as its own undo step. The next band (or the last) stays
/// selected only if this one was, so deleting from the graph or a chip
/// does not open another band's panel. Returns whether a band was removed.
fn remove_band(
    model: &Rc<Model>,
    buffer: &gtk::TextBuffer,
    graph: &gtk::DrawingArea,
    list: &gtk::FlowBox,
    index: usize,
) -> bool {
    model.history.borrow_mut().checkpoint();
    let snapshot = {
        let mut document = model.document.borrow_mut();
        let Some(document) = document.as_mut() else {
            return false;
        };
        if index >= document.filters.len() {
            return false;
        }
        document.filters.remove(index);
        let selected = model.selected_filter.get();
        model.selected_filter.set(match selected {
            _ if document.filters.is_empty() => None,
            Some(current) if current == index => Some(index.min(document.filters.len() - 1)),
            Some(current) if current > index => Some(current - 1),
            other => other,
        });
        document.clone()
    };
    set_editor_text(model, buffer, &serialize_profile(&snapshot));
    *model.analysis.borrow_mut() = Some(preview_analysis(model, &snapshot));
    rebuild_filter_list(list, &snapshot, model, buffer, graph);
    graph.queue_draw();
    true
}

/// Refresh values in place, keeping keyboard focus and an in-progress knob drag.
fn sync_band_ui(model: &Model) {
    let document = model.document.borrow();
    let Some(profile) = document.as_ref() else {
        return;
    };
    let ui = model.bands.borrow();
    let Some(ui) = ui.as_ref() else {
        return;
    };
    ui.syncing.set(true);
    let selected = model.selected_filter.get();
    for (index, (tile, filter)) in ui.tiles.iter().zip(&profile.filters).enumerate() {
        tile.select.set_active(selected == Some(index));
        if filter.enabled {
            tile.select.remove_css_class("disabled-band");
        } else {
            tile.select.add_css_class("disabled-band");
        }
        glyph::set_kind(&tile.glyph, filter.kind);
        tile.number.set_text(&(index + 1).to_string());
        for (class, channels) in [
            ("left", ChannelSelection::Left),
            ("right", ChannelSelection::Right),
        ] {
            if filter.channels == channels {
                tile.number.add_css_class(class);
            } else {
                tile.number.remove_css_class(class);
            }
        }
        tile.label
            .set_text(&compact_band_frequency(filter.frequency));
        tile.select.set_tooltip_text(Some(&format!(
            "Band {} · {} · {} · {} dB · Q {:.2}{}\nRight-click to delete",
            index + 1,
            kind_name(filter.kind),
            format_band_frequency(filter.frequency),
            signed_db(filter.gain_db, 1),
            filter.q,
            if filter.enabled { "" } else { " · off" }
        )));
    }
    let filter = selected.and_then(|index| profile.filters.get(index));
    ui.root.set_visible(filter.is_some());
    if let Some(filter) = filter {
        // A type outside the three (from an imported profile) lights none.
        match BAND_KINDS.iter().position(|kind| *kind == filter.kind) {
            Some(kind) => ui.kind.buttons[kind].set_active(true),
            None => {
                for button in &ui.kind.buttons {
                    button.set_active(false);
                }
            }
        }
        ui.kind.root.set_tooltip_text(
            (!BAND_KINDS.contains(&filter.kind))
                .then(|| format!("{} · choose a type to change it", kind_name(filter.kind)))
                .as_deref(),
        );
        ui.channels.buttons[match filter.channels {
            ChannelSelection::Left => 0,
            ChannelSelection::All => 1,
            ChannelSelection::Right => 2,
        }]
        .set_active(true);
        ui.frequency.set_value(filter.frequency);
        ui.gain.set_value(filter.gain_db);
        ui.q.set_value(filter.q);
        ui.enabled.set_active(filter.enabled);
        // Gain and Q mean nothing for every type; dim what does not apply.
        let has_gain = matches!(
            filter.kind,
            FilterKind::Peaking | FilterKind::LowShelf | FilterKind::HighShelf
        );
        ui.gain.set_sensitive(has_gain);
        if filter.enabled {
            ui.root.remove_css_class("disabled-band");
        } else {
            ui.root.add_css_class("disabled-band");
        }
    }
    ui.overlay.queue_allocate();
    ui.syncing.set(false);
}

fn compact_band_frequency(frequency: f64) -> String {
    if frequency >= 1000.0 {
        format!("{:.1}k", frequency / 1000.0)
    } else {
        format!("{frequency:.0}")
    }
}

fn rebuild_filter_list(
    list: &gtk::FlowBox,
    profile: &ProfileDocument,
    model: &Rc<Model>,
    buffer: &gtk::TextBuffer,
    graph: &gtk::DrawingArea,
) {
    if profile.filters.is_empty() {
        model.selected_filter.set(None);
    } else if model
        .selected_filter
        .get()
        .is_some_and(|i| i >= profile.filters.len())
    {
        model.selected_filter.set(Some(profile.filters.len() - 1));
    }
    let add = model.bands.borrow().as_ref().map(|ui| ui.add.clone());
    // Autosaves and graph edits need value updates, not new chips.
    let count = profile.filters.len() as i32;
    let reuse = model
        .bands
        .borrow()
        .as_ref()
        .is_some_and(|ui| ui.tiles.len() == profile.filters.len())
        && !profile.filters.is_empty()
        && profile.graphic_eqs.is_empty()
        && list.child_at_index(count).and_then(|child| child.child())
            == add.as_ref().map(|add| add.clone().upcast())
        && list.child_at_index(count + 1).is_none();
    if reuse {
        sync_band_ui(model);
        return;
    }
    list.remove_all();
    let syncing = model
        .bands
        .borrow()
        .as_ref()
        .map(|ui| ui.syncing.clone())
        .unwrap_or_default();
    let mut tiles = Vec::new();
    for (index, filter) in profile.filters.iter().enumerate() {
        let select = gtk::ToggleButton::new();
        select.add_css_class("band-chip");
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let symbol = glyph::widget(filter.kind, 20, 12);
        let number = gtk::Label::new(None);
        number.add_css_class("band-number");
        let label = gtk::Label::new(None);
        content.append(&symbol);
        content.append(&number);
        content.append(&label);
        select.set_child(Some(&content));
        select.update_property(&[gtk::accessible::Property::Label(&format!(
            "Select band {}",
            index + 1
        ))]);
        select.connect_toggled({
            let model = model.clone();
            let graph = graph.clone();
            let syncing = syncing.clone();
            move |button| {
                if syncing.get() {
                    return;
                }
                // A second click on the selected chip closes its panel.
                let selected = model.selected_filter.get() == Some(index);
                if button.is_active() || selected {
                    model
                        .selected_filter
                        .set((button.is_active()).then_some(index));
                    sync_band_ui(&model);
                    graph.queue_draw();
                    // Arrow keys edit the band just selected.
                    graph.grab_focus();
                }
            }
        });
        // Right-click a chip to delete its band. Deferred, since deleting
        // rebuilds the chips, this one included.
        let secondary = gtk::GestureClick::new();
        secondary.set_button(gtk::gdk::BUTTON_SECONDARY);
        secondary.connect_pressed({
            let model = model.clone();
            let buffer = buffer.clone();
            let graph = graph.clone();
            let list = list.clone();
            move |click, _, _, _| {
                click.set_state(gtk::EventSequenceState::Claimed);
                let (model, buffer, graph, list) =
                    (model.clone(), buffer.clone(), graph.clone(), list.clone());
                gtk::glib::idle_add_local_once(move || {
                    let was_selected = model.selected_filter.get() == Some(index);
                    if remove_band(&model, &buffer, &graph, &list, index) && was_selected {
                        model.selected_filter.set(None);
                        sync_band_ui(&model);
                    }
                });
            }
        });
        select.add_controller(secondary);
        list.insert(&select, -1);
        tiles.push(BandTile {
            select,
            glyph: symbol,
            number,
            label,
        });
    }
    if let Some(ui) = model.bands.borrow_mut().as_mut() {
        ui.tiles = tiles;
    }
    for graphic in &profile.graphic_eqs {
        let chip = gtk::Label::new(Some(&format!(
            "GraphicEQ · {} points",
            graphic.points.len()
        )));
        chip.add_css_class("band-chip");
        list.insert(&chip, -1);
    }
    if profile.filters.is_empty() && profile.graphic_eqs.is_empty() {
        let empty = gtk::Label::new(Some("Double-click the graph to add a band"));
        empty.add_css_class("level-code");
        list.insert(&empty, -1);
    }
    if let Some(add) = add {
        list.insert(&add, -1);
    }
    sync_band_ui(model);
}

fn update_filter(
    model: &Rc<Model>,
    buffer: &gtk::TextBuffer,
    graph: &gtk::DrawingArea,
    index: usize,
    update: impl FnOnce(&mut Filter),
) {
    let (text, preview) = {
        let mut document = model.document.borrow_mut();
        let Some(document) = document.as_mut() else {
            return;
        };
        document.graphic_eqs.clear();
        let Some(filter) = document.filters.get_mut(index) else {
            return;
        };
        update(filter);
        (
            serialize_profile(document),
            preview_analysis(model, document),
        )
    };
    *model.analysis.borrow_mut() = Some(preview);
    sync_band_ui(model);
    graph.queue_draw();
    set_editor_text(model, buffer, &text);
}

fn can_add_filter(document: &ProfileDocument, channels: ChannelSelection) -> bool {
    let left_bands = document
        .filters
        .iter()
        .filter(|filter| {
            matches!(
                filter.channels,
                ChannelSelection::All | ChannelSelection::Left
            )
        })
        .count();
    let right_bands = document
        .filters
        .iter()
        .filter(|filter| {
            matches!(
                filter.channels,
                ChannelSelection::All | ChannelSelection::Right
            )
        })
        .count();
    (!matches!(channels, ChannelSelection::All | ChannelSelection::Left)
        || left_bands < MAX_FILTERS_PER_CHANNEL)
        && (!matches!(channels, ChannelSelection::All | ChannelSelection::Right)
            || right_bands < MAX_FILTERS_PER_CHANNEL)
}

fn ellipsized_string_factory(max_chars: i32) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(move |_, object| {
        let Some(item) = object.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let label = gtk::Label::new(None);
        label.set_xalign(0.0);
        label.set_hexpand(true);
        label.set_width_chars(1);
        label.set_max_width_chars(max_chars);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        item.set_child(Some(&label));
    });
    factory.connect_bind(|_, object| {
        let Some(item) = object.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(label) = item.child().and_downcast::<gtk::Label>() else {
            return;
        };
        let Some(value) = item.item().and_downcast::<gtk::StringObject>() else {
            return;
        };
        label.set_text(&value.string());
        label.set_tooltip_text(Some(&value.string()));
    });
    factory
}

fn format_band_frequency(frequency: f64) -> String {
    if frequency >= 1000.0 {
        format!("{:.2} kHz", frequency / 1000.0)
    } else {
        format!("{frequency:.0} Hz")
    }
}

fn comparison_candidate_name(model: &Model, profile_id: &str) -> String {
    if profile_id == COMPARISON_BYPASS_ID {
        return "OFF · LEVEL MATCHED".into();
    }
    model
        .profiles
        .borrow()
        .iter()
        .find(|profile| profile.id == profile_id)
        .map(|profile| profile.name.clone())
        .unwrap_or_else(|| "Missing profile".into())
}

fn refresh_comparison_ui(model: &Rc<Model>, drop: &gtk::DropDown, ui: &Rc<ComparisonUi>) {
    while let Some(child) = ui.candidates.first_child() {
        ui.candidates.remove(&child);
    }
    while let Some(child) = ui.listening.first_child() {
        ui.listening.remove(&child);
    }
    ui.checks.borrow_mut().clear();
    let Some(device) = model
        .devices
        .borrow()
        .get(drop.selected() as usize)
        .cloned()
    else {
        ui.menu.set_sensitive(false);
        ui.menu.set_label("A/B");
        ui.stack.set_visible_child_name("setup");
        ui.status.set_text("No output selected");
        return;
    };
    ui.menu.set_sensitive(matches!(device.channels, 1 | 2));
    let key = device.key.as_storage_key();
    let comparison = model.comparisons.borrow().get(&key).cloned();

    let mut candidates = Vec::with_capacity(model.profiles.borrow().len() + 1);
    candidates.push((
        COMPARISON_BYPASS_ID.to_owned(),
        "OFF · level matched".to_owned(),
    ));
    candidates.extend(
        model
            .profiles
            .borrow()
            .iter()
            .map(|profile| (profile.id.clone(), profile.name.clone())),
    );
    for (profile_id, name) in candidates {
        let check = gtk::ToggleButton::new();
        check.set_hexpand(true);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let label = gtk::Label::new(Some(&name));
        label.set_xalign(0.0);
        label.set_hexpand(true);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let selected_mark = gtk::Label::new(None);
        selected_mark.add_css_class("comparison-mark");
        row.append(&label);
        row.append(&selected_mark);
        check.set_child(Some(&row));
        check.update_property(&[
            gtk::accessible::Property::Label(&format!("Include {name} in comparison")),
            gtk::accessible::Property::Description(
                "Select or remove this profile from the comparison bank draft.",
            ),
        ]);
        let selected = comparison
            .as_ref()
            .is_some_and(|comparison| comparison.profile_ids.contains(&profile_id));
        check.set_active(selected);
        selected_mark.set_text(if selected { "SELECTED" } else { "ADD" });
        check.add_css_class("comparison-candidate");
        ui.candidates.append(&check);
        ui.checks.borrow_mut().push((profile_id, check.clone()));
        check.connect_toggled({
            // These rows are rebuilt when the popover is reopened. Capturing
            // a strong Rc would form checks -> button -> closure -> checks and
            // leak every discarded candidate row.
            let checks = Rc::downgrade(&ui.checks);
            let status = ui.status.clone();
            let save = ui.save.clone();
            let selected_mark = selected_mark.clone();
            move |button| {
                let Some(checks) = checks.upgrade() else {
                    return;
                };
                let selected = checks
                    .borrow()
                    .iter()
                    .filter(|(_, candidate)| candidate.is_active())
                    .count();
                if selected > MAX_COMPARISON_PROFILES && button.is_active() {
                    button.set_active(false);
                    status.set_text(&format!(
                        "Choose no more than {MAX_COMPARISON_PROFILES} candidates"
                    ));
                    return;
                }
                selected_mark.set_text(if button.is_active() {
                    "SELECTED"
                } else {
                    "ADD"
                });
                save.set_sensitive((2..=MAX_COMPARISON_PROFILES).contains(&selected));
                status.set_text(&if selected < 2 {
                    format!("{selected} selected · choose at least two")
                } else {
                    format!("{selected} selected · ready to save")
                });
            }
        });
    }

    let selected_ids = comparison
        .as_ref()
        .map(|comparison| comparison.profile_ids.clone())
        .unwrap_or_default();
    ui.save
        .set_sensitive((2..=MAX_COMPARISON_PROFILES).contains(&selected_ids.len()));
    if let Some(comparison) = &comparison {
        ui.save.set_label("UPDATE COMPARISON");
        ui.delete.set_sensitive(true);
        ui.stack.set_visible_child_name("listen");

        for (index, profile_id) in comparison.profile_ids.iter().enumerate() {
            let name = comparison_candidate_name(model, profile_id);
            let button = gtk::Button::new();
            button.set_hexpand(true);
            button.add_css_class("comparison-listen");
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let label = gtk::Label::new(Some(&name));
            label.set_xalign(0.0);
            label.set_hexpand(true);
            label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            let state_text = if comparison.enabled && profile_id == &comparison.active_profile_id {
                "PLAYING".to_owned()
            } else {
                format!("ALT+{}", index + 1)
            };
            let state = gtk::Label::new(Some(&state_text));
            state.add_css_class("comparison-mark");
            row.append(&label);
            row.append(&state);
            button.set_child(Some(&row));
            if comparison.enabled && profile_id == &comparison.active_profile_id {
                button.add_css_class("active");
            }
            button.update_property(&[
                gtk::accessible::Property::Label(&format!("Listen to {name}")),
                gtk::accessible::Property::Description(
                    if comparison.enabled && profile_id == &comparison.active_profile_id {
                        "This is the profile currently playing."
                    } else {
                        "Switch immediately to this level-matched profile."
                    },
                ),
            ]);
            button.connect_clicked({
                let model = model.clone();
                let drop = drop.clone();
                let ui = Rc::downgrade(ui);
                let key = key.clone();
                let profile_id = profile_id.clone();
                move |_| match model.client.select_comparison_profile(&key, &profile_id) {
                    Ok(()) => {
                        if let Ok(comparisons) = model.client.comparisons() {
                            *model.comparisons.borrow_mut() = comparisons;
                        }
                        if let Ok(devices) = model.client.devices() {
                            *model.devices.borrow_mut() = devices;
                        }
                        let model = model.clone();
                        let drop = drop.clone();
                        if let Some(ui) = ui.upgrade() {
                            gtk::glib::idle_add_local_once(move || {
                                refresh_comparison_ui(&model, &drop, &ui);
                                update_device_controls(
                                    &model,
                                    &drop,
                                    &ui.device_state,
                                    &ui.assign,
                                    &ui.bypass,
                                );
                            });
                        }
                    }
                    Err(error) => {
                        if let Some(ui) = ui.upgrade() {
                            ui.status.set_text(&error.to_string());
                        }
                    }
                }
            });
            ui.listening.append(&button);
        }

        let active_name = comparison_candidate_name(model, &comparison.active_profile_id);
        if comparison.enabled {
            let active_position = comparison
                .profile_ids
                .iter()
                .position(|id| id == &comparison.active_profile_id)
                .unwrap_or_default()
                + 1;
            ui.menu.set_label(&format!(
                "A/B · {active_position}/{}",
                comparison.profile_ids.len()
            ));
            ui.menu.set_tooltip_text(Some(&format!(
                "BS.1770 K-weighted comparison · {active_name}"
            )));
            ui.status
                .set_text(&format!("LEVEL MATCHED · {active_name}"));
        } else {
            ui.menu.set_label("A/B · PAUSED");
            ui.menu
                .set_tooltip_text(Some("This output's comparison bank is paused"));
            ui.status.set_text("PAUSED · choose a profile to resume");
        }
    } else {
        ui.save.set_label("START COMPARISON");
        ui.delete.set_sensitive(false);
        ui.stack.set_visible_child_name("setup");
        ui.menu.set_label("A/B");
        ui.menu.set_tooltip_text(Some(
            "Compare profiles at a shared BS.1770 K-weighted pink-noise level",
        ));
        ui.status.set_text("Choose at least two candidates");
    }
}

fn install_comparison_shortcuts(
    window: &adw::ApplicationWindow,
    model: &Rc<Model>,
    device_drop: &gtk::DropDown,
    device_bypass: &gtk::ToggleButton,
    comparison_ui: &Rc<ComparisonUi>,
) {
    let controller = gtk::EventControllerKey::new();
    controller.connect_key_pressed({
        let model = model.clone();
        let drop = device_drop.clone();
        let bypass = device_bypass.clone();
        let ui = comparison_ui.clone();
        move |_, key, keycode, modifiers| {
            let character = key.to_unicode().map(|value| value.to_ascii_lowercase());
            if modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK)
                && character == Some('b')
                && bypass.is_sensitive()
            {
                bypass.set_active(!bypass.is_active());
                return gtk::glib::Propagation::Stop;
            }
            if !modifiers.contains(gtk::gdk::ModifierType::ALT_MASK) {
                return gtk::glib::Propagation::Proceed;
            }
            let Some(index) = comparison_slot(key, keycode) else {
                return gtk::glib::Propagation::Proceed;
            };
            let Some(device) = model
                .devices
                .borrow()
                .get(drop.selected() as usize)
                .cloned()
            else {
                return gtk::glib::Propagation::Proceed;
            };
            let key = device.key.as_storage_key();
            let Some(comparison) = model.comparisons.borrow().get(&key).cloned() else {
                return gtk::glib::Propagation::Proceed;
            };
            if !comparison.enabled {
                return gtk::glib::Propagation::Proceed;
            }
            let Some(profile_id) = comparison.profile_ids.get(index).cloned() else {
                return gtk::glib::Propagation::Proceed;
            };
            match model.client.select_comparison_profile(&key, &profile_id) {
                Ok(()) => {
                    if let Some(comparison) = model.comparisons.borrow_mut().get_mut(&key) {
                        comparison.active_profile_id = profile_id;
                    }
                    refresh_comparison_ui(&model, &drop, &ui);
                }
                Err(error) => ui.status.set_text(&error.to_string()),
            }
            gtk::glib::Propagation::Stop
        }
    });
    // Run before focused widgets, which would otherwise see the keys first.
    controller.set_propagation_phase(gtk::PropagationPhase::Capture);
    window.add_controller(controller);
}

/// Bank slot for Alt+1…9 from the digit row or keypad. Layouts such as AZERTY
/// put symbols on the unshifted digit row, so the physical key also counts.
fn comparison_slot(key: gtk::gdk::Key, keycode: u32) -> Option<usize> {
    // XKB keycodes of the 1…9 keys on the digit row (evdev 2…10, plus 8).
    const DIGIT_ROW: std::ops::RangeInclusive<u32> = 10..=18;
    key.to_unicode()
        .and_then(|value| value.to_digit(10))
        .filter(|value| (1..=9).contains(value))
        .map(|value| value as usize - 1)
        .or_else(|| {
            DIGIT_ROW
                .contains(&keycode)
                .then(|| (keycode - DIGIT_ROW.start()) as usize)
        })
}

fn install_editor_shortcut(window: &adw::ApplicationWindow, text_toggle: &gtk::ToggleButton) {
    let controller = gtk::EventControllerKey::new();
    controller.connect_key_pressed({
        let text_toggle = text_toggle.clone();
        move |_, key, _, modifiers| {
            let character = key.to_unicode().map(|value| value.to_ascii_lowercase());
            if !modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK) || character != Some('e') {
                return gtk::glib::Propagation::Proceed;
            }
            text_toggle.set_active(!text_toggle.is_active());
            gtk::glib::Propagation::Stop
        }
    });
    window.add_controller(controller);
}

fn update_device_controls(
    model: &Rc<Model>,
    drop: &gtk::DropDown,
    state: &gtk::Label,
    assign: &gtk::Button,
    bypass: &gtk::ToggleButton,
) {
    let Some(device) = model
        .devices
        .borrow()
        .get(drop.selected() as usize)
        .cloned()
    else {
        state.set_text("No output connected");
        assign.set_sensitive(false);
        bypass.set_sensitive(false);
        update_live_indicators(model);
        return;
    };

    let device_key = device.key.as_storage_key();
    let pending_bypass = model.pending_device_bypass.borrow();
    let pending_bypassed = pending_bypass
        .as_ref()
        .filter(|pending| pending.device_key == device_key);
    let displayed_bypassed =
        displayed_device_bypass(&device_key, device.bypassed, pending_bypass.as_ref());
    model.syncing_device.set(true);
    let filters_active = !displayed_bypassed;
    if bypass.is_active() != filters_active {
        bypass.set_active(filters_active);
    }
    model.syncing_device.set(false);
    drop.set_tooltip_text(Some(&device.description));

    let assigned_name = device.assigned_profile.as_ref().and_then(|id| {
        model
            .profiles
            .borrow()
            .iter()
            .find(|profile| &profile.id == id)
            .map(|profile| profile.name.clone())
    });
    let selected_matches = device.assigned_profile.as_ref() == model.current_id.borrow().as_ref();
    let engine_error = model.client.status().ok().and_then(|status| {
        status
            .pointer("/engine/errors")
            .and_then(|errors| errors.get(device.key.as_storage_key()))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    });
    let comparison = model
        .comparisons
        .borrow()
        .get(&device.key.as_storage_key())
        .filter(|comparison| comparison.enabled)
        .cloned();
    let has_ab_source = comparison.is_some() || assigned_name.is_some();

    if let Some(error) = engine_error {
        state.set_text(&format!("Last valid audio retained · {error}"));
        assign.set_label("Retry profile");
        assign.set_sensitive(model.current_id.borrow().is_some());
    } else if device.channels > 2 {
        state.set_text("Unsupported surround output · bypassed");
        assign.set_label("Unavailable");
        assign.set_sensitive(false);
    } else if let Some(comparison) = comparison {
        let active_name = comparison_candidate_name(model, &comparison.active_profile_id);
        state.set_text(&if displayed_bypassed {
            format!("Filters off · level matched to {active_name}")
        } else if comparison.active_profile_id == COMPARISON_BYPASS_ID {
            format!("Comparing · {active_name}")
        } else {
            format!("Comparing · {active_name} · level matched")
        });
        assign.set_label("Set selected as normal profile");
        assign.set_sensitive(model.current_id.borrow().is_some());
    } else if displayed_bypassed {
        state.set_text(&match assigned_name {
            Some(name) => format!("Filters off · level matched to {name}"),
            None => "Not applied · filters unavailable".to_owned(),
        });
        assign.set_label(if selected_matches {
            "Unassign from output"
        } else {
            "Apply selected profile"
        });
        assign.set_sensitive(model.current_id.borrow().is_some());
    } else if let Some(name) = assigned_name {
        let surround = device.assigned_profile.as_ref().is_some_and(|id| {
            model.profiles.borrow().iter().any(|profile| {
                &profile.id == id && profile.impulse_layout == Some(ImpulseLayout::Surround71)
            })
        });
        state.set_text(&if surround {
            format!("Active · {name} · 7.1 surround")
        } else {
            format!("Active · {name}")
        });
        assign.set_label(if selected_matches {
            "Unassign from output"
        } else {
            "Replace with selected"
        });
        assign.set_sensitive(model.current_id.borrow().is_some());
    } else {
        state.set_text("Not applied · output is unchanged");
        assign.set_label("Apply selected profile");
        assign.set_sensitive(model.current_id.borrow().is_some());
    }

    bypass.set_sensitive(
        pending_bypassed.is_none() && has_ab_source && matches!(device.channels, 1 | 2),
    );
    // The accent is for actions that put a profile on the output. The bar
    // shows a short verb; the full action is its name and tooltip.
    let action = assign
        .label()
        .map(|label| label.to_string())
        .unwrap_or_default();
    let (short, primary) = match action.as_str() {
        "Apply selected profile" => ("Apply", true),
        "Replace with selected" => ("Replace", true),
        "Retry profile" => ("Retry", true),
        "Unassign from output" => ("Unassign", false),
        "Set selected as normal profile" => ("Make default", false),
        other => (other, false),
    };
    if primary {
        assign.add_css_class("primary");
    } else {
        assign.remove_css_class("primary");
    }
    assign.set_tooltip_text(Some(&action));
    assign.update_property(&[gtk::accessible::Property::Label(&action)]);
    let short = short.to_owned();
    assign.set_label(&short);
    std::mem::drop(pending_bypass);
    update_live_indicators(model);
}

/// Output to show at startup: the system default, else the first connected
/// output with a profile, else the first connected output.
fn preferred_device_index(devices: &[DeviceInfo]) -> Option<usize> {
    devices
        .iter()
        .position(|device| device.connected && device.is_default)
        .or_else(|| {
            devices
                .iter()
                .position(|device| device.connected && device.assigned_profile.is_some())
        })
        .or_else(|| devices.iter().position(|device| device.connected))
}

/// Output the editor should jump to after a device poll: a newly connected
/// output (the default one if several arrive), or a new system default.
/// Outputs are compared as hardware, so a Bluetooth headset switching between
/// its call and music routes does not count as a new arrival.
fn device_focus_target(old: &[DeviceInfo], new: &[DeviceInfo]) -> Option<usize> {
    let was_connected = |device: &DeviceInfo| {
        old.iter()
            .any(|previous| previous.connected && previous.same_hardware(device))
    };
    let arrived = new
        .iter()
        .enumerate()
        .filter(|(_, device)| device.connected && !was_connected(device))
        .collect::<Vec<_>>();
    if let Some((index, _)) = arrived
        .iter()
        .find(|(_, device)| device.is_default)
        .or(arrived.first())
    {
        return Some(*index);
    }
    let old_default = old
        .iter()
        .find(|device| device.connected && device.is_default);
    new.iter().position(|device| {
        device.connected
            && device.is_default
            && !old_default.is_some_and(|previous| previous.same_hardware(device))
    })
}

/// How long the editor waits after the last click, key press, or scroll before
/// a newly connected output may take it over.
const FOCUS_IDLE: std::time::Duration = std::time::Duration::from_secs(3);

/// The user is in the middle of something the editor should not pull away from.
fn user_is_busy(model: &Model) -> bool {
    if model.graph_dragging.get() {
        return true;
    }
    model
        .last_input
        .get()
        .is_some_and(|input| input.elapsed() < FOCUS_IDLE)
        || model
            .live
            .borrow()
            .as_ref()
            .is_some_and(|live| live.menus.iter().any(gtk::MenuButton::is_active))
}

/// Select the pending output and open the profile it plays. Always waits for
/// a pending save so an edit is not written to the wrong profile; focus that
/// comes from a device change also waits until the user is idle.
fn apply_pending_focus(
    model: &Rc<Model>,
    drop: &gtk::DropDown,
    profile_list: &gtk::ListBox,
    wait_for_idle: bool,
) {
    if model.pending_save.borrow().is_some() || (wait_for_idle && user_is_busy(model)) {
        return;
    }
    let Some(key) = model.pending_focus.borrow_mut().take() else {
        return;
    };
    let index = model
        .devices
        .borrow()
        .iter()
        .position(|device| device.key.as_storage_key() == key);
    let Some(index) = index else {
        return;
    };
    if drop.selected() != index as u32 {
        model.restoring_device.set(true);
        drop.set_selected(index as u32);
        model.restoring_device.set(false);
    }
    focus_device_profile(model, index as u32, profile_list);
}

fn focus_device_profile(model: &Rc<Model>, device_index: u32, profile_list: &gtk::ListBox) {
    let target = model
        .devices
        .borrow()
        .get(device_index as usize)
        .and_then(|device| {
            device
                .effective_profile_id(&model.comparisons.borrow())
                .map(str::to_owned)
        });
    let Some(target) = target else {
        return;
    };
    if model.current_id.borrow().as_deref() == Some(target.as_str()) {
        return;
    }
    let position = model
        .profiles
        .borrow()
        .iter()
        .position(|profile| profile.id == target);
    if let Some(row) = position.and_then(|position| profile_list.row_at_index(position as i32)) {
        profile_list.select_row(Some(&row));
    }
}

/// Mark where the edited profile is playing, the profiles in use in the bank,
/// and whether the selected output is the system default.
fn update_live_indicators(model: &Rc<Model>) {
    let Some(live) = model.live.borrow().clone() else {
        return;
    };
    let devices = model.devices.borrow();
    let comparisons = model.comparisons.borrow();
    let pending_bypass = model.pending_device_bypass.borrow();
    let engine_off = model.engine_off.get();
    // Connected outputs set to this profile, split by whether it is audible.
    let users_of = |profile_id: &str| {
        let mut playing = Vec::new();
        let mut paused = Vec::new();
        for device in devices.iter().filter(|device| {
            device.connected && device.effective_profile_id(&comparisons) == Some(profile_id)
        }) {
            let bypassed = displayed_device_bypass(
                &device.key.as_storage_key(),
                device.bypassed,
                pending_bypass.as_ref(),
            );
            if !engine_off && device.is_filtering(&comparisons, bypassed) {
                playing.push(device.description.as_str());
            } else {
                paused.push(device.description.as_str());
            }
        }
        (playing, paused)
    };
    let disconnected_owners_of = |profile_id: &str| {
        devices
            .iter()
            .filter(|device| {
                !device.connected && device.assigned_profile.as_deref() == Some(profile_id)
            })
            .map(|device| device.description.as_str())
            .collect::<Vec<_>>()
    };
    let paused_reason = if engine_off {
        "engine off"
    } else {
        "filters off"
    };

    let current = model.current_id.borrow().clone();
    let (playing_on, paused_on) = current.as_deref().map(users_of).unwrap_or_default();
    let status = &live.profile_status;
    let (text, playing) = match (playing_on.as_slice(), paused_on.as_slice()) {
        ([], []) => ("Not playing".to_owned(), false),
        ([], paused) => (format!("{} · {paused_reason}", paused.join(", ")), false),
        ([only], _) => (format!("Playing on {only}"), true),
        (outputs, _) => (format!("Playing on {} outputs", outputs.len()), true),
    };
    status.set_text(&text);
    status.set_tooltip_text(Some(&match (playing_on.as_slice(), paused_on.as_slice()) {
        ([], []) => "This profile is not set on any connected output".to_owned(),
        ([], paused) => format!(
            "Set on {}, but not being applied: {paused_reason}",
            paused.join(", ")
        ),
        (outputs, _) => format!("Playing on {}", outputs.join(", ")),
    }));
    if playing {
        live.profile_tile.add_css_class("live");
    } else {
        live.profile_tile.remove_css_class("live");
    }
    status.set_visible(current.is_some());

    let selected = devices.get(live.device_drop.selected() as usize);
    live.output_status.set_text(match selected {
        Some(device) if !device.connected => "Not connected",
        Some(device) if device.is_default => "System output",
        Some(_) => "Available",
        None => "No outputs",
    });

    let profiles = model.profiles.borrow();
    for (index, profile) in profiles.iter().enumerate() {
        let (outputs, paused) = users_of(&profile.id);
        if let Some(row) = live
            .profile_list
            .row_at_index(index as i32)
            .and_downcast::<adw::ActionRow>()
        {
            if outputs.is_empty() {
                row.remove_css_class("playing");
            } else {
                row.add_css_class("playing");
            }
            let resting = disconnected_owners_of(&profile.id);
            row.set_subtitle(&if !outputs.is_empty() {
                format!("Playing on {}", outputs.join(", "))
            } else if !paused.is_empty() {
                format!("{} · {paused_reason}", paused.join(", "))
            } else if !resting.is_empty() {
                format!("{} · not connected", resting.join(", "))
            } else if profile.activatable {
                "Ready".to_owned()
            } else {
                "Needs attention".to_owned()
            });
        }
    }
}

fn displayed_device_bypass(
    device_key: &str,
    service_bypassed: bool,
    pending: Option<&PendingDeviceBypass>,
) -> bool {
    pending
        .filter(|pending| pending.device_key == device_key)
        .map(|pending| pending.bypassed)
        .unwrap_or(service_bypassed)
}

/// Whether playing audio reaches the graph at its own rate.
#[derive(Debug, PartialEq)]
enum SourceRate {
    Native,
    Resampled {
        source: u32,
        graph: u32,
        /// PipeWire would be allowed to switch to the source's rate.
        allowed: bool,
        /// Only one rate is allowed, so PipeWire can never switch.
        fixed: bool,
    },
}

fn source_rate(status: &serde_json::Value) -> Option<SourceRate> {
    let graph = status.pointer("/graph/rate")?.as_u64()? as u32;
    let streams = status.pointer("/graph/streams")?.as_array()?;
    let allowed = status
        .pointer("/graph/allowed_rates")
        .and_then(serde_json::Value::as_array)
        .map(|rates| {
            rates
                .iter()
                .filter_map(serde_json::Value::as_u64)
                .map(|rate| rate as u32)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let rates = streams
        .iter()
        .filter_map(|stream| stream.get("rate")?.as_u64())
        .map(|rate| rate as u32)
        .collect::<Vec<_>>();
    if rates.is_empty() {
        return None;
    }
    match rates.iter().find(|rate| **rate != graph) {
        None => Some(SourceRate::Native),
        Some(&source) => Some(SourceRate::Resampled {
            source,
            graph,
            allowed: allowed.contains(&source),
            fixed: allowed.len() <= 1,
        }),
    }
}

fn engine_tooltip(status: &serde_json::Value) -> String {
    let khz = |rate: u32| format!("{} kHz", rate as f64 / 1000.0);
    let rate = match source_rate(status) {
        Some(SourceRate::Native) => {
            "Playing audio reaches MassiveEQ at its own sample rate, with no resampling.\n\n"
                .to_owned()
        }
        Some(SourceRate::Resampled {
            source,
            graph,
            fixed: true,
            ..
        }) => format!(
            "PipeWire is converting {} audio to {} because that is the only rate it may run \
             at. To play each source at its own rate, create \
             ~/.config/pipewire/pipewire.conf.d/rates.conf containing\n\n\
             context.properties = {{ default.clock.allowed-rates = [ 44100 48000 88200 96000 \
             176400 192000 ] }}\n\nthen restart PipeWire. MassiveEQ follows the new rate \
             automatically.\n\n",
            khz(source),
            khz(graph)
        ),
        Some(SourceRate::Resampled {
            source,
            graph,
            allowed: true,
            ..
        }) => format!(
            "PipeWire is converting {} audio to {}. It runs at one rate at a time, so \
             another playing stream or the output's supported rates are holding it at {}.\n\n",
            khz(source),
            khz(graph),
            khz(graph)
        ),
        Some(SourceRate::Resampled { source, graph, .. }) => format!(
            "PipeWire is converting {} audio to {}: {} is not in PipeWire's \
             default.clock.allowed-rates.\n\n",
            khz(source),
            khz(graph),
            khz(source)
        ),
        None => String::new(),
    };
    format!(
        "{rate}Double-click the graph to add a band, drag points to edit, or use arrows on the selected point"
    )
}

fn install_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(
        r#"
        @define-color accent_bg_color #ff7a1a;
        @define-color accent_color #ff8c33;
        @define-color accent_fg_color #1a0d03;

        window,
        .instrument-root {
            background-color: #0b0b0c;
            color: #ecebe6;
            font-size: 13px;
        }

        .brand,
        .header-status,
        .panel-title,
        .section-label,
        .readouts,
        .console-status,
        .level-code,
        .knob-value,
        .knob-caption {
            font-family: monospace;
        }

        /* Header: the preset bar */
        .instrument-header {
            background-color: #0b0b0c;
            border-bottom: 1px solid #222225;
            box-shadow: none;
            min-height: 50px;
        }

        .brand {
            font-weight: 800;
            letter-spacing: 1px;
            margin: 0 10px 0 4px;
        }

        .header-status {
            color: #8b8b90;
            font-size: 11px;
            letter-spacing: 0.4px;
        }

        .preset button,
        .preset menubutton > button {
            background-color: #1b1b1d;
            border: none;
            box-shadow: none;
            min-height: 32px;
        }

        .preset > button {
            color: #8b8b90;
            padding: 0 8px;
        }

        .preset-menu > button {
            font-family: monospace;
            font-weight: 700;
            min-width: 140px;
            padding: 0 12px;
        }

        .preset.live .preset-menu > button {
            color: #ff8c33;
        }

        button.bar-icon {
            background: transparent;
            border: none;
            box-shadow: none;
            color: #8b8b90;
            min-height: 32px;
            min-width: 32px;
            padding: 0;
        }

        button.bar-icon:hover {
            color: #ecebe6;
        }

        .bar-menu > button {
            background-color: #1b1b1d;
            border: none;
            border-radius: 10px;
            box-shadow: none;
            font-family: monospace;
            font-size: 11px;
            font-weight: 700;
            min-height: 32px;
            padding: 0 12px;
            margin-left: 6px;
        }

        dropdown.output-dropdown > button {
            background-color: #1b1b1d;
            border: none;
            border-radius: 10px;
            box-shadow: none;
            font-family: monospace;
            font-size: 11.5px;
            min-height: 32px;
            padding: 0 10px;
        }

        button.assign-button {
            background-color: #1b1b1d;
            border: none;
            border-radius: 10px;
            box-shadow: none;
            color: #ecebe6;
            font-family: monospace;
            font-size: 11px;
            font-weight: 700;
            min-height: 32px;
            padding: 0 12px;
        }

        button.assign-button.primary {
            background-color: #ff7a1a;
            color: #1a0d03;
        }

        button.power {
            background-color: #242426;
            border: none;
            border-radius: 999px;
            box-shadow: none;
            color: #8b8b90;
            min-height: 32px;
            min-width: 32px;
            padding: 0;
            margin-left: 4px;
        }

        button.power:checked {
            background-color: #ff7a1a;
            color: #fff;
        }

        .update-button {
            background-color: transparent;
            border: 1px solid rgba(255, 122, 26, 0.6);
            border-radius: 10px;
            color: #ff8c33;
            font-family: monospace;
            font-size: 9px;
            font-weight: 800;
            padding: 5px 10px;
        }

        /* Preset popover */
        entry.preset-name {
            font-family: monospace;
            background: transparent;
            border: none;
            border-bottom: 1px solid #2a2a2d;
            border-radius: 0;
            box-shadow: none;
            font-size: 14px;
            font-weight: 700;
            padding: 2px 0;
        }

        entry.preset-name:focus-within {
            border-bottom-color: #ff7a1a;
            box-shadow: none;
        }

        .tile-subtitle {
            color: #8b8b90;
            font-size: 12px;
        }

        list.profile-bank,
        list.profile-bank > row {
            background: transparent;
        }

        list.profile-bank > row {
            border-radius: 6px;
            margin-bottom: 2px;
            font-family: monospace;
            font-size: 12px;
        }

        .graph-stage .response-graph.bypassed {
            opacity: 0.4;
        }

        list.profile-bank > row:hover {
            background-color: #1f1f22;
        }

        list.profile-bank > row:selected {
            background-color: #2a2a2d;
        }

        list.profile-bank > row.playing .subtitle {
            color: #ff8c33;
            opacity: 1;
        }

        .bank-actions button {
            background-color: #1f1f22;
            border: none;
            border-radius: 10px;
            box-shadow: none;
            min-height: 32px;
        }

        button.popover-action {
            background: transparent;
            border: none;
            box-shadow: none;
            color: #8b8b90;
            font-family: monospace;
            font-size: 11px;
        }

        button.popover-action:hover {
            color: #ecebe6;
        }

        /* The graph stage */
        .graph-stage {
            padding: 6px 10px 0 4px;
        }

        .readouts {
            background-color: rgba(11, 11, 12, 0.82);
            border-radius: 8px;
            color: #8b8b90;
            font-size: 10.5px;
            padding: 3px 8px;
        }

        .device-state,
        .console-status {
            color: #6e6e73;
            font-family: monospace;
            font-size: 10.5px;
        }

        /* The selected band's floating panel */
        .band-panel {
            background-color: rgba(24, 24, 27, 0.97);
            border: 1px solid #3a3a3e;
            border-radius: 16px;
            box-shadow: 0 14px 34px rgba(0, 0, 0, 0.55);
            padding: 10px 12px 8px 12px;
        }

        .band-panel.disabled-band knob {
            opacity: 0.45;
        }

        .kind-picker button,
        .ear-picker button {
            background-color: #242427;
            border: none;
            box-shadow: none;
            color: #8b8b90;
            min-height: 26px;
            padding: 0 6px;
        }

        .kind-picker button:checked,
        .ear-picker button:checked {
            background-color: #3a3a3e;
            color: #ecebe6;
        }

        .ear-picker button {
            font-family: monospace;
            font-size: 10px;
            font-weight: 700;
        }

        button.band-power,
        button.band-remove {
            background-color: #242427;
            border: none;
            border-radius: 8px;
            box-shadow: none;
            color: #8b8b90;
            min-height: 26px;
            min-width: 26px;
            padding: 0;
        }

        button.band-power:checked {
            background-color: #ff7a1a;
            color: #1a0d03;
        }

        button.band-remove:hover {
            color: #ecebe6;
        }

        knob {
            min-width: 40px;
            min-height: 40px;
        }

        .knob-value {
            font-size: 11.5px;
            font-weight: 600;
        }

        .knob-caption {
            color: #56565b;
            font-size: 9px;
            letter-spacing: 0.6px;
        }

        /* Footer: bypass and listening, bands, macros, editors */
        .footer {
            background-color: #0b0b0c;
            padding: 8px 14px;
        }

        paned.footer-split > separator {
            background-color: #222225;
            min-height: 1px;
        }

        paned.footer-split > separator:hover {
            background-color: #ff7a1a;
        }

        button.footer-toggle,
        .listen-group button {
            background-color: #1b1b1d;
            border: none;
            box-shadow: none;
            color: #ecebe6;
            font-family: monospace;
            font-size: 11px;
            font-weight: 700;
            min-height: 34px;
            padding: 0 12px;
        }

        button.footer-toggle {
            border-radius: 10px;
        }

        .listen-group button {
            border-radius: 0;
        }

        .listen-group button:first-child {
            border-radius: 10px 0 0 10px;
        }

        .listen-group menubutton > button {
            border-radius: 0 10px 10px 0;
            border-left: 1px solid #0b0b0c;
            padding: 0 8px;
        }

        button.footer-toggle:checked {
            background-color: #ff7a1a;
            color: #1a0d03;
        }

        .band-chips,
        .band-chips > flowboxchild {
            background: transparent;
            padding: 0;
        }

        button.band-chip,
        label.band-chip {
            background-color: #1b1b1d;
            border: none;
            border-radius: 9px;
            box-shadow: none;
            color: #8b8b90;
            font-family: monospace;
            font-size: 10.5px;
            min-height: 30px;
            padding: 0 9px;
        }

        button.band-chip:checked {
            background-color: #ecebe6;
            color: #111;
        }

        /* Band numbers take their node's ear colour. */
        label.band-number {
            color: #b8bdbd;
            font-weight: 700;
        }

        label.band-number.left {
            color: #54adf0;
        }

        label.band-number.right {
            color: #ed4f2e;
        }

        button.band-chip:checked label.band-number {
            color: #111;
        }

        button.band-chip:checked label.band-number.left {
            color: #1c6fae;
        }

        button.band-chip:checked label.band-number.right {
            color: #c23718;
        }

        button.band-chip.disabled-band {
            opacity: 0.45;
        }

        button.band-chip.add {
            background: transparent;
            border: 1px dashed #3a3a3e;
            padding: 0 6px;
        }

        .footer-menu > button,
        button.footer-flat {
            background: transparent;
            border: none;
            border-radius: 10px;
            box-shadow: none;
            color: #8b8b90;
            font-family: monospace;
            font-size: 11px;
            font-weight: 700;
            min-height: 32px;
            padding: 0 10px;
        }

        .footer-menu > button:hover,
        button.footer-flat:hover {
            color: #ecebe6;
        }

        button.footer-flat:checked,
        .footer-menu.on > button {
            color: #ff8c33;
        }

        .text-page {
            padding: 12px 16px;
        }

        .code-editor {
            background-color: #121213;
            border: 1px solid #2a2a2d;
            border-radius: 14px;
        }

        textview {
            background-color: #121213;
            color: #e7e7e2;
            font-family: monospace;
            padding: 14px;
        }

        textview.invalid-draft {
            color: #ff9b88;
        }

        /* Shared */
        .panel-title {
            font-size: 11px;
            font-weight: 700;
            letter-spacing: 0.7px;
        }

        .section-label {
            font-size: 9px;
            font-weight: 700;
            letter-spacing: 0.6px;
            opacity: 0.46;
        }

        .level-code {
            color: #8b8b90;
            font-size: 10px;
        }

        .comparison-candidate,
        .comparison-listen {
            background-color: #1b1b1d;
            border: 1px solid #2a2a2d;
            border-radius: 10px;
            box-shadow: none;
            font-family: monospace;
            padding: 6px 8px;
        }

        .comparison-candidate:checked {
            background-color: rgba(255, 122, 26, 0.10);
            border-color: rgba(255, 122, 26, 0.72);
        }

        .comparison-listen.active {
            background-color: #ff7a1a;
            border-color: #ff8c33;
            color: #1a0d03;
        }

        .comparison-listen.active .comparison-mark {
            color: #1a0d03;
        }

        .comparison-mark {
            color: #ff8c33;
            font-family: monospace;
            font-size: 9px;
            font-weight: 700;
        }

        entry,
        spinbutton,
        dropdown > button {
            background-color: #1b1b1d;
            border: 1px solid #2a2a2d;
            border-radius: 10px;
            box-shadow: none;
            color: #ecebe6;
        }

        entry:focus,
        spinbutton:focus-within,
        dropdown > button:focus {
            border-color: #ff7a1a;
            box-shadow: 0 0 0 1px rgba(255, 122, 26, 0.4);
        }

        button {
            border-radius: 10px;
        }

        button.suggested-action {
            background-color: #ff7a1a;
            border-color: #ff8c33;
            color: #1a0d03;
            font-family: monospace;
            font-weight: 700;
        }

        button.suggested-action:disabled {
            background-color: #242426;
            border-color: #2a2a2d;
            color: #8b8b90;
        }

        switch:checked {
            background-color: #ff7a1a;
        }

        separator {
            background-color: #222225;
        }

        .convolution-card {
            background-color: transparent;
            padding: 6px;
        }

        .convolution-name {
            font-family: monospace;
            font-size: 15px;
            font-weight: 600;
        }

        .tone-play {
            background-color: #242426;
            border: none;
            border-radius: 999px;
            min-height: 56px;
            min-width: 56px;
            padding: 0;
        }

        .tone-play:checked {
            background-color: #ff7a1a;
            color: #1a0d03;
        }

        .tone-readout {
            background-color: #1f1f22;
            border-radius: 14px;
            min-width: 130px;
            padding: 8px 14px;
        }

        .tone-value {
            font-family: monospace;
            font-size: 20px;
            font-weight: 600;
        }

        .segmented button {
            background-color: #1b1b1d;
            border: 1px solid #2a2a2d;
            box-shadow: none;
            color: #8b8b90;
            font-family: monospace;
            font-size: 10px;
            font-weight: 700;
            padding: 6px 10px;
        }

        .segmented button:checked {
            background-color: #2b2b2e;
            border-color: #3a3a3e;
            color: #ecebe6;
        }

        .tone-fix {
            background-color: #1f1f22;
            border: none;
            border-radius: 12px;
            font-family: monospace;
            font-weight: 700;
            min-width: 70px;
            padding: 8px 12px;
        }

        .graph-listen button,
        .graph-listen dropdown > button {
            font-family: monospace;
            font-size: 11px;
        }

        popover > contents {
            background-color: #151516;
            border: 1px solid #2a2a2d;
            border-radius: 16px;
            box-shadow: 0 16px 38px rgba(0, 0, 0, 0.5);
            padding: 10px;
        }

        scale trough {
            background-color: #242426;
            border-radius: 3px;
            min-height: 6px;
        }

        scale highlight {
            background-color: #ff7a1a;
            border-radius: 3px;
        }

        scale slider {
            background-color: #ecebe6;
            border-radius: 999px;
            box-shadow: 0 2px 6px rgba(0, 0, 0, 0.5);
            min-height: 18px;
            min-width: 18px;
        }

        scrollbar slider {
            background-color: #3a3a3e;
            border-radius: 999px;
            min-height: 4px;
            min-width: 4px;
        }
        "#,
    );
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

/// Keep the service's analysis of the open profile, and remember what its
/// impulse response alone contributes for later live previews.
/// Keep the service's exact analysis (with any tone hold) and show it.
fn store_service_analysis(model: &Model, mut analysis: ProfileAnalysis, label: &gtk::Label) {
    apply_tone_hold(model, &mut analysis);
    set_analysis_label(label, &analysis);
    let ir_curve = model
        .document
        .borrow()
        .as_ref()
        .filter(|document| !document.convolutions.is_empty())
        .map(|document| {
            let eq = analyze_profile_preview(document, model.sample_rate.get(), 0.0);
            let without = |full: &[ResponsePoint], eq: &[ResponsePoint]| {
                full.iter()
                    .map(|point| ResponsePoint {
                        frequency: point.frequency,
                        gain_db: point.gain_db - interpolate_response(eq, point.frequency),
                    })
                    .collect::<Vec<_>>()
            };
            [
                without(&analysis.left.response, &eq.left.response),
                without(&analysis.right.response, &eq.right.response),
            ]
        });
    *model.ir_curve.borrow_mut() = ir_curve;
    *model.analysis.borrow_mut() = Some(analysis);
}

/// Instant local analysis of an edited document. The local analyser only
/// knows EQ, so with an impulse response its stored curve is added and the
/// gain readouts stay at the service's last exact values.
fn preview_analysis(model: &Model, document: &ProfileDocument) -> ProfileAnalysis {
    let mut preview =
        analyze_profile_preview(document, model.sample_rate.get(), model.manual_trim.get());
    if document.convolutions.is_empty() {
        apply_tone_hold(model, &mut preview);
        return preview;
    }
    if let Some(ir) = model.ir_curve.borrow().as_ref() {
        for (channel, curve) in [&mut preview.left, &mut preview.right].into_iter().zip(ir) {
            for point in &mut channel.response {
                point.gain_db += interpolate_response(curve, point.frequency);
            }
        }
    }
    if let Some(service) = model.analysis.borrow().as_ref() {
        preview.match_gain_db = service.match_gain_db;
        preview.safety_attenuation_db = service.safety_attenuation_db;
        preview.effective_gain_db = service.effective_gain_db;
        preview.headroom_limited = service.headroom_limited;
        preview.left.preamp_db = service.left.preamp_db;
        preview.right.preamp_db = service.right.preamp_db;
    }
    preview
}

/// Gain of a log-frequency response at `frequency`, linearly interpolated.
fn interpolate_response(points: &[ResponsePoint], frequency: f64) -> f64 {
    let Some(first) = points.first() else {
        return 0.0;
    };
    if frequency <= first.frequency {
        return first.gain_db;
    }
    for pair in points.windows(2) {
        if frequency <= pair[1].frequency {
            let span = (pair[1].frequency / pair[0].frequency).ln();
            let amount = if span > 0.0 {
                (frequency / pair[0].frequency).ln() / span
            } else {
                0.0
            };
            return pair[0].gain_db + amount * (pair[1].gain_db - pair[0].gain_db);
        }
    }
    points.last().map_or(0.0, |point| point.gain_db)
}

thread_local! {
    /// The preamp value on the profile tile; set with every analysis.
    static PREAMP_READOUT: RefCell<Option<gtk::Label>> = const { RefCell::new(None) };
    /// The tone generator is holding the gain; the readout says so.
    static LEVEL_HELD: Cell<bool> = const { Cell::new(false) };
}

const ACCENT: &str = "#ff7a1a";

/// A signed decibel value with a typographic minus.
fn signed_db(value: f64, digits: usize) -> String {
    format!("{value:+.digits$}").replace('-', "−")
}

fn set_analysis_label(label: &gtk::Label, analysis: &ProfileAnalysis) {
    let left_preamp = analysis.left.preamp_db + analysis.effective_gain_db;
    let right_preamp = analysis.right.preamp_db + analysis.effective_gain_db;
    let value = |text: String| format!("<span foreground='{ACCENT}'>{text}</span>");
    let preamp = if (left_preamp - right_preamp).abs() < 0.01 {
        value(signed_db(left_preamp, 2))
    } else {
        format!(
            "L {}  R {}",
            value(signed_db(left_preamp, 2)),
            value(signed_db(right_preamp, 2))
        )
    };
    let levels = if LEVEL_HELD.with(Cell::get) {
        "LEVEL HELD FOR TONE GEN".to_owned()
    } else {
        format!(
            "MATCH {}     SAFETY {}",
            value(signed_db(analysis.match_gain_db, 2)),
            value(signed_db(analysis.safety_attenuation_db, 2))
        )
    };
    label.set_markup(&format!(
        "PREAMP {preamp}     {levels}{}",
        if analysis.headroom_limited {
            "     <span foreground='#ecebe6'>HEADROOM LIMITED</span>"
        } else {
            ""
        }
    ));
    label.set_tooltip_text(Some(&label.text()));
    PREAMP_READOUT.with(|readout| {
        if let Some(readout) = readout.borrow().as_ref() {
            readout.set_text(&format!(
                "{} dB",
                signed_db(left_preamp.min(right_preamp), 1)
            ));
        }
    });
}

/// The header's status strip: what plays on the system output, then the
/// engine's rate, latency, and load.
fn header_markup(status: &serde_json::Value, model: &Model) -> String {
    let escape = |text: &str| gtk::glib::markup_escape_text(text).to_string();
    let accent = |text: &str| format!("<span foreground='{ACCENT}'>{}</span>", escape(text));
    let devices = model.devices.borrow();
    let comparisons = model.comparisons.borrow();
    let route = devices
        .iter()
        .find(|device| device.connected && device.is_default)
        .map(|device| {
            let profile = (!model.engine_off.get())
                .then(|| device.playing_profile_id(&comparisons))
                .flatten()
                .and_then(|id| {
                    model
                        .profiles
                        .borrow()
                        .iter()
                        .find(|profile| profile.id == id)
                        .map(|profile| profile.name.clone())
                });
            match profile {
                Some(profile) => format!(
                    "{}  ▸  {}",
                    accent(&profile.to_uppercase()),
                    escape(&device.description.to_uppercase())
                ),
                None => format!("DIRECT  ▸  {}", escape(&device.description.to_uppercase())),
            }
        })
        .unwrap_or_else(|| "NO OUTPUT".to_owned());
    let mut parts = vec![route];
    if let Some(active) = status.pointer("/engine/active/0") {
        let number = |key: &str| active.get(key).and_then(serde_json::Value::as_f64);
        if let Some(rate) = number("sample_rate") {
            parts.push(format!("{} kHz", accent(&format!("{:.1}", rate / 1000.0))));
        }
    } else if model.engine_off.get() {
        parts.push("ENGINE OFF".to_owned());
    }
    if let Some(SourceRate::Resampled { source, .. }) = source_rate(status) {
        parts.push(format!(
            "SOURCE {} kHz RESAMPLED",
            accent(&format!("{:.1}", source as f64 / 1000.0))
        ));
    }
    parts.join("  <span foreground='#3a3a3e'>│</span>  ")
}

fn refresh_profiles(model: &Rc<Model>, list: &gtk::ListBox) {
    if let Ok(profiles) = model.client.profiles() {
        *model.profiles.borrow_mut() = profiles;
        repopulate_profiles(list, model);
    }
}
/// Re-sort the bank after a rename, keeping the open profile selected
/// without reloading it into the editor.
fn resort_profiles(model: &Rc<Model>) {
    let Some(list) = model
        .live
        .borrow()
        .as_ref()
        .map(|live| live.profile_list.clone())
    else {
        return;
    };
    sort_profiles(&mut model.profiles.borrow_mut());
    let current = model.current_id.borrow().clone();
    let position = model
        .profiles
        .borrow()
        .iter()
        .position(|profile| Some(&profile.id) == current.as_ref());
    model.reordering_profiles.set(true);
    repopulate_profiles(&list, model);
    if let Some(row) = position.and_then(|position| list.row_at_index(position as i32)) {
        list.select_row(Some(&row));
    }
    model.reordering_profiles.set(false);
}

fn repopulate_profiles(list: &gtk::ListBox, model: &Rc<Model>) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
    for profile in model.profiles.borrow().iter() {
        let row = adw::ActionRow::builder()
            .title(&profile.name)
            .subtitle(if profile.activatable {
                "Ready"
            } else {
                "Needs attention"
            })
            .build();
        list.append(&row);
    }
    update_live_indicators(model);
}

fn show_startup_error(app: &adw::Application, message: &str) {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("MassiveEQ")
        .default_width(520)
        .default_height(220)
        .build();
    let status = adw::StatusPage::builder()
        .title("Could not start MassiveEQ")
        .description(message)
        .icon_name("dialog-error-symbolic")
        .build();
    window.set_content(Some(&status));
    window.present();
}

#[cfg(test)]
mod tests {
    use super::sweep_position;
    use super::{
        ChannelSelection, DeviceInfo, Filter, FilterKind, PendingDeviceBypass, ProfileDocument,
        SourceRate, apply_tone_fix, can_add_filter, device_focus_target, displayed_device_bypass,
        engine_tooltip, preferred_device_index, source_rate, tone_frequency, tone_position,
    };
    use super::{PanelSide, best_panel_side, panel_origin};

    #[test]
    fn comparison_slots_follow_the_digit_keys_on_any_layout() {
        use super::gtk::gdk::Key;
        assert_eq!(super::comparison_slot(Key::_1, 10), Some(0));
        assert_eq!(super::comparison_slot(Key::KP_9, 89), Some(8));
        // AZERTY: the unshifted 1 and 9 keys type & and ç.
        assert_eq!(super::comparison_slot(Key::ampersand, 10), Some(0));
        assert_eq!(super::comparison_slot(Key::ccedilla, 18), Some(8));
        assert_eq!(super::comparison_slot(Key::_0, 19), None);
        assert_eq!(super::comparison_slot(Key::a, 38), None);
    }

    #[test]
    fn band_panel_moves_off_the_lines_near_its_node() {
        let area = (1200.0, 700.0);
        let size = (440.0, 190.0);
        let node = (600.0, 300.0);
        // A flat line just below the node: the default spot would cover it.
        let below = (0..120)
            .map(|step| (step as f64 * 10.0, 400.0))
            .collect::<Vec<_>>();
        assert_eq!(
            best_panel_side(node, size, area, &below, None),
            PanelSide::Above
        );
        // Lines above and below too: a side placement covers less of them
        // near the node.
        let both = below
            .iter()
            .map(|(x, _)| (*x, 200.0))
            .chain(below.iter().copied())
            .collect::<Vec<_>>();
        let side = best_panel_side(node, size, area, &both, None);
        assert!(
            matches!(side, PanelSide::Left | PanelSide::Right),
            "{side:?}"
        );
        // With nothing in the way the panel stays below, and a previous side
        // holds when it is no worse.
        assert_eq!(
            best_panel_side(node, size, area, &[], None),
            PanelSide::Below
        );
        assert_eq!(
            best_panel_side(node, size, area, &[], Some(PanelSide::Left)),
            PanelSide::Left
        );
    }

    #[test]
    fn band_panel_never_covers_its_node() {
        let area = (1200.0, 700.0);
        let size = (440.0, 190.0);
        for node in [(70.0, 60.0), (1150.0, 650.0), (600.0, 690.0), (40.0, 350.0)] {
            let side = best_panel_side(node, size, area, &[], None);
            let (left, top) = panel_origin(side, node, size, area);
            let covered = node.0 >= left - 12.0
                && node.0 <= left + size.0 + 12.0
                && node.1 >= top - 12.0
                && node.1 <= top + size.1 + 12.0;
            assert!(!covered, "{node:?} {side:?}");
        }
    }
    use massiveeq_core::DeviceKey;

    fn output(id: &str, connected: bool, is_default: bool, profile: Option<&str>) -> DeviceInfo {
        DeviceInfo {
            key: DeviceKey {
                backend: "alsa".into(),
                stable_id: id.into(),
                route: "playback".into(),
            },
            node_name: id.into(),
            description: id.into(),
            channels: 2,
            connected,
            assigned_profile: profile.map(str::to_owned),
            bypassed: false,
            is_default,
        }
    }

    #[test]
    fn sweep_moves_in_log_frequency_and_stops_at_the_end() {
        assert_eq!(sweep_position(0.25, 0.0, 30.0), 0.25);
        assert_eq!(sweep_position(0.25, 7.5, 30.0), 0.5);
        assert_eq!(sweep_position(0.9, 10.0, 15.0), 1.0);
        assert_eq!(sweep_position(0.25, -1.0, 30.0), 0.25);
    }

    #[test]
    fn tone_fix_adjusts_a_nearby_filter_or_adds_one() {
        let mut document = ProfileDocument::empty("tone");
        assert_eq!(
            apply_tone_fix(&mut document, 3_000.0, 4.318, ChannelSelection::All, -3.0),
            Some(0)
        );
        // A twentieth of an octave away is the same filter; it deepens.
        assert_eq!(
            apply_tone_fix(&mut document, 3_105.0, 4.318, ChannelSelection::All, -1.0),
            Some(0)
        );
        assert_eq!(document.filters.len(), 1);
        assert_eq!(document.filters[0].gain_db, -4.0);
        // The other ear, or a different frequency, gets its own filter.
        assert_eq!(
            apply_tone_fix(&mut document, 3_000.0, 4.318, ChannelSelection::Left, 1.0),
            Some(1)
        );
        assert_eq!(
            apply_tone_fix(&mut document, 6_000.0, 4.318, ChannelSelection::All, 1.0),
            Some(2)
        );
        assert!((tone_frequency(tone_position(440.0)) - 440.0).abs() < 1e-9);
    }

    #[test]
    fn source_rate_tells_native_from_resampled_playback() {
        let status = |allowed: &[u32], streams: &[u32]| {
            serde_json::json!({ "graph": {
                "rate": 48_000,
                "allowed_rates": allowed,
                "streams": streams.iter().map(|rate| serde_json::json!({"name": "x", "rate": rate})).collect::<Vec<_>>(),
            }})
        };
        assert_eq!(source_rate(&status(&[48_000], &[])), None);
        assert_eq!(
            source_rate(&status(&[48_000], &[48_000])),
            Some(SourceRate::Native)
        );
        assert_eq!(
            source_rate(&status(&[48_000], &[44_100])),
            Some(SourceRate::Resampled {
                source: 44_100,
                graph: 48_000,
                allowed: false,
                fixed: true
            })
        );
        assert!(matches!(
            source_rate(&status(&[44_100, 48_000], &[48_000, 44_100])),
            Some(SourceRate::Resampled {
                allowed: true,
                fixed: false,
                ..
            })
        ));
        assert!(engine_tooltip(&status(&[48_000], &[44_100])).contains("allowed-rates"));
    }

    #[test]
    fn startup_prefers_the_system_default_output() {
        let devices = [
            output("speakers", true, false, Some("flat")),
            output("headphones", true, true, None),
        ];
        assert_eq!(preferred_device_index(&devices), Some(1));
    }

    #[test]
    fn startup_falls_back_to_an_output_with_a_profile() {
        let devices = [
            output("hdmi", true, false, None),
            output("speakers", true, false, Some("flat")),
            output("old", false, false, Some("flat")),
        ];
        assert_eq!(preferred_device_index(&devices), Some(1));
    }

    #[test]
    fn newly_connected_output_takes_focus() {
        let old = [
            output("speakers", true, true, None),
            output("buds", false, false, None),
        ];
        let new = [
            output("buds", true, false, None),
            output("speakers", true, true, None),
        ];
        assert_eq!(device_focus_target(&old, &new), Some(0));
    }

    #[test]
    fn default_output_change_takes_focus() {
        let old = [
            output("buds", true, false, None),
            output("speakers", true, true, None),
        ];
        let new = [
            output("buds", true, true, None),
            output("speakers", true, false, None),
        ];
        assert_eq!(device_focus_target(&old, &new), Some(0));
    }

    #[test]
    fn unchanged_outputs_keep_focus() {
        let old = [
            output("buds", true, true, None),
            output("speakers", false, false, None),
        ];
        assert_eq!(device_focus_target(&old, &old.clone()), None);
        let fewer = [output("buds", true, true, None)];
        assert_eq!(device_focus_target(&old, &fewer), None);
    }

    #[test]
    fn bluetooth_route_change_is_not_a_new_arrival() {
        let route = |device: &DeviceInfo, route: &str| {
            let mut device = device.clone();
            device.key.route = route.into();
            device
        };
        let speakers = output("speakers", true, true, None);
        let buds = output("buds", true, false, None);
        // The headset connected on its call route; focus already moved to it.
        let old = [route(&buds, "headset-head-unit"), speakers.clone()];
        // It switches to music and the call route is remembered, disconnected.
        let mut call = route(&buds, "headset-head-unit");
        call.connected = false;
        let new = [route(&buds, "a2dp-sink"), speakers.clone(), call];
        assert_eq!(device_focus_target(&old, &new), None);
        // A new system default on the same headset does not take focus again.
        let mut old_default = old.clone();
        old_default[0].is_default = true;
        old_default[1].is_default = false;
        let mut new_default = new.clone();
        new_default[0].is_default = true;
        new_default[1].is_default = false;
        assert_eq!(device_focus_target(&old_default, &new_default), None);
    }

    #[test]
    fn pending_filter_toggle_wins_over_stale_service_poll() {
        let pending = PendingDeviceBypass {
            device_key: "selected-output".into(),
            bypassed: true,
        };

        assert!(displayed_device_bypass(
            "selected-output",
            false,
            Some(&pending)
        ));
    }

    #[test]
    fn pending_filter_toggle_does_not_leak_to_another_output() {
        let pending = PendingDeviceBypass {
            device_key: "other-output".into(),
            bypassed: true,
        };

        assert!(!displayed_device_bypass(
            "selected-output",
            false,
            Some(&pending)
        ));
    }

    #[test]
    fn graph_add_respects_the_per_channel_band_limit() {
        let mut document = ProfileDocument::empty("limit");
        document.filters = (0..64)
            .map(|_| Filter {
                enabled: true,
                kind: FilterKind::Peaking,
                frequency: 1_000.0,
                gain_db: 0.0,
                q: 1.0,
                channels: ChannelSelection::Left,
            })
            .collect();

        assert!(!can_add_filter(&document, ChannelSelection::All));
        assert!(!can_add_filter(&document, ChannelSelection::Left));
        assert!(can_add_filter(&document, ChannelSelection::Right));
    }
}
