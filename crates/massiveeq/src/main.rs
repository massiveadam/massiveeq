mod client;
mod editing;
mod graph;
mod live_client;
mod update;

use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use massiveeq_core::{
    COMPARISON_BYPASS_ID, ChannelSelection, ComparisonSet, DeviceInfo, Filter, FilterKind,
    ImpulseLayout, MAX_COMPARISON_PROFILES, MAX_FILTERS_PER_CHANNEL, ProfileAnalysis,
    ProfileDocument, ProfileInfo, analysis::ResponsePoint, analyze_profile_preview, parse_text,
    serialize_profile, sort_profiles,
};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
};

use client::Client;

struct Model {
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
    history: RefCell<editing::History>,
    reference: RefCell<Option<editing::Snapshot>>,
    restoring_history: Cell<bool>,
    syncing_text: Cell<bool>,
    last_submitted: RefCell<Option<String>>,
    bank_names_dirty: Cell<bool>,
    unsaved: Cell<bool>,
    hear_reference: Cell<bool>,
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
    stack: adw::ViewStack,
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
        history: RefCell::new(editing::History::default()),
        reference: RefCell::new(None),
        restoring_history: Cell::new(false),
        syncing_text: Cell::new(false),
        last_submitted: RefCell::new(None),
        bank_names_dirty: Cell::new(false),
        unsaved: Cell::new(false),
        hear_reference: Cell::new(false),
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
    let rail_menu = gtk::MenuButton::new();
    rail_menu.set_icon_name("sidebar-show-symbolic");
    rail_menu.set_tooltip_text(Some("Profiles and tools"));
    rail_menu.update_property(&[gtk::accessible::Property::Label("Profiles and tools")]);
    rail_menu.set_visible(false);
    let rail_popover = gtk::Popover::new();
    let rail_popup_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .max_content_height(640)
        .build();
    rail_popover.set_child(Some(&rail_popup_scroll));
    rail_menu.set_popover(Some(&rail_popover));
    header.pack_start(&rail_menu);
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
    profile_popover_box.set_size_request(330, 360);
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
    profile_menu.set_margin_start(10);
    profile_menu.set_margin_top(3);
    profile_menu.set_margin_bottom(3);
    profile_menu.set_popover(Some(&profile_popover));
    profile_menu.add_css_class("profile-menu");
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

    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.set_margin_top(16);
    content.set_margin_bottom(22);
    content.set_margin_start(18);
    content.set_margin_end(18);

    let profile_status = gtk::Label::new(None);
    profile_status.add_css_class("tile-subtitle");
    profile_status.set_ellipsize(gtk::pango::EllipsizeMode::End);
    profile_status.set_xalign(0.0);
    let name_entry = gtk::Entry::builder()
        .placeholder_text("Profile name")
        .hexpand(true)
        .build();
    name_entry.add_css_class("tile-title");
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
    device_drop.set_hexpand(true);
    device_drop.set_size_request(1, -1);
    device_drop.add_css_class("tile-dropdown");
    device_drop.set_factory(Some(&ellipsized_string_factory(32)));
    device_drop.set_list_factory(Some(&ellipsized_string_factory(48)));
    let device_state = gtk::Label::new(Some("Not applied"));
    device_state.set_xalign(0.0);
    device_state.add_css_class("dim-label");
    device_state.add_css_class("device-state");
    device_state.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let assign_button = gtk::Button::with_label("Apply to output");
    assign_button.add_css_class("tile-action");
    assign_button.set_hexpand(true);
    // The round accent control: lit while this output's filters run.
    let device_bypass = gtk::ToggleButton::new();
    device_bypass.add_css_class("filters-dot");
    device_bypass.set_valign(gtk::Align::Center);
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
    comparison_menu.set_label("COMPARE");
    comparison_menu.set_popover(Some(&comparison_popover));
    comparison_menu.add_css_class("comparison-menu");
    comparison_menu.set_tooltip_text(Some(
        "Compare profiles at a shared BS.1770 K-weighted pink-noise level",
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

    // Left rail: the open profile and its output as tiles, the profile bank,
    // and the tools.
    let rail = gtk::Box::new(gtk::Orientation::Vertical, 10);
    rail.add_css_class("rail");
    rail.set_size_request(292, -1);

    let profile_tile = gtk::Box::new(gtk::Orientation::Vertical, 12);
    profile_tile.add_css_class("rail-tile");
    let profile_head = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    profile_head.append(&tile_icon("audio-headphones-symbolic"));
    let profile_text = gtk::Box::new(gtk::Orientation::Vertical, 0);
    profile_text.set_hexpand(true);
    profile_text.set_valign(gtk::Align::Center);
    profile_text.append(&name_entry);
    profile_text.append(&profile_status);
    profile_head.append(&profile_text);
    profile_tile.append(&profile_head);
    let profile_controls = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let preamp_tile = gtk::Box::new(gtk::Orientation::Vertical, 0);
    preamp_tile.add_css_class("tile-value");
    preamp_tile.set_hexpand(true);
    preamp_tile.set_valign(gtk::Align::Center);
    let preamp_value = gtk::Label::new(Some("—"));
    preamp_value.add_css_class("tile-value-number");
    let preamp_caption = gtk::Label::new(Some("Preamp"));
    preamp_caption.add_css_class("tile-value-caption");
    preamp_tile.append(&preamp_value);
    preamp_tile.append(&preamp_caption);
    PREAMP_READOUT.with(|readout| *readout.borrow_mut() = Some(preamp_value));
    profile_controls.append(&preamp_tile);
    profile_controls.append(&device_bypass);
    profile_tile.append(&profile_controls);
    rail.append(&profile_tile);

    let output_tile = gtk::Box::new(gtk::Orientation::Vertical, 10);
    output_tile.add_css_class("rail-tile");
    let output_head = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    output_head.append(&tile_icon("audio-speakers-symbolic"));
    let output_text = gtk::Box::new(gtk::Orientation::Vertical, 0);
    output_text.set_hexpand(true);
    output_text.set_valign(gtk::Align::Center);
    output_text.set_size_request(1, -1);
    output_text.append(&device_drop);
    output_text.append(&output_status);
    output_head.append(&output_text);
    output_tile.append(&output_head);
    output_tile.append(&device_state);
    output_tile.append(&assign_button);
    rail.append(&output_tile);

    rail.append(&rail_label("PROFILES"));
    rail.append(&profile_scroll);
    profile_buttons.add_css_class("bank-actions");
    rail.append(&profile_buttons);

    rail.append(&rail_label("TOOLS"));
    let tools = gtk::Grid::new();
    tools.set_column_spacing(8);
    tools.set_row_spacing(8);
    tools.set_column_homogeneous(true);
    comparison_ui.menu.add_css_class("tool-tile");
    tools.attach(&comparison_ui.menu, 0, 0, 2, 1);
    let tone_tool = gtk::Button::with_label("Tone");
    tone_tool.add_css_class("tool-tile");
    tone_tool.add_css_class("round");
    tone_tool.set_halign(gtk::Align::Center);
    tone_tool.set_valign(gtk::Align::Center);
    tools.attach(&tone_tool, 2, 0, 1, 1);
    let ir_tool = gtk::Button::with_label("IR");
    ir_tool.add_css_class("tool-tile");
    ir_tool.add_css_class("round");
    ir_tool.set_halign(gtk::Align::Center);
    ir_tool.set_valign(gtk::Align::Center);
    ir_tool.set_tooltip_text(Some("Impulse response"));
    tools.attach(&ir_tool, 0, 1, 1, 1);
    curves_ui.menu.add_css_class("tool-tile");
    tools.attach(&curves_ui.menu, 1, 1, 2, 1);
    rail.append(&tools);

    let main_column = gtk::Box::new(gtk::Orientation::Vertical, 18);
    main_column.set_hexpand(true);
    let body = gtk::Box::new(gtk::Orientation::Horizontal, 18);
    body.append(&rail);
    body.append(&main_column);

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
    graph.set_tooltip_text(Some(
        "Click an empty position to add a peak filter. Arrows edit gain and frequency; Shift+Up/Down edits Q; Shift+Left/Right moves frequency farther.",
    ));
    graph.update_property(&[
        gtk::accessible::Property::Label("Frequency response graph"),
        gtk::accessible::Property::Description(
            "Logarithmic 20 hertz to 20 kilohertz response from minus 10 to plus 10 decibels. Click empty graph space to add a filter. Use arrow keys to edit the selected filter; hold Shift for Q or larger frequency steps.",
        ),
    ]);
    let analysis_label = gtk::Label::new(Some("AUTO PREAMP —"));
    analysis_label.set_xalign(0.0);
    analysis_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    analysis_label.add_css_class("readouts");
    let level_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    level_row.add_css_class("readout-row");
    analysis_label.set_hexpand(true);
    level_row.append(&analysis_label);
    let trim = gtk::SpinButton::with_range(-24.0, 24.0, 0.1);
    trim.set_digits(1);
    trim.set_width_chars(5);
    trim.set_tooltip_text(Some(
        "Fine-tune the perceptually matched preamp; safety attenuation still prevents clipping",
    ));
    let trim_label = gtk::Label::new(Some("ADJUST"));
    trim_label.add_css_class("readout-key");
    let trim_unit = gtk::Label::new(Some("dB"));
    trim_unit.add_css_class("level-code");
    let trim_controls = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    trim_controls.set_halign(gtk::Align::End);
    trim_controls.append(&trim_label);
    trim_controls.append(&trim);
    trim_controls.append(&trim_unit);
    level_row.append(&trim_controls);
    header_status.set_markup(&header_markup(&engine_status, &model));
    header_status.set_tooltip_text(Some(&engine_tooltip(&engine_status)));
    let (tone_page, tone_ui) = build_tone_page();
    let (graph_listen, graph_tone) = build_graph_tone(&tone_ui);
    *model.graph_tone.borrow_mut() = Some(graph_tone);
    let response_body = gtk::Box::new(gtk::Orientation::Vertical, 6);
    response_body.append(&graph);
    response_body.append(&level_row);
    response_body.append(&graph_listen);
    let curve_context = gtk::Label::new(None);
    curve_context.set_wrap(true);
    curve_context.set_xalign(0.0);
    curve_context.add_css_class("level-code");
    curve_context.set_visible(false);
    response_body.append(&curve_context);
    *model.curve_context.borrow_mut() = Some(curve_context);
    main_column.append(&titled_frame(1, "response", &response_body, None));
    content.append(&body);

    let view_stack = adw::ViewStack::new();
    view_stack.set_vhomogeneous(false);
    let filter_list = gtk::FlowBox::new();
    filter_list.set_selection_mode(gtk::SelectionMode::None);
    filter_list.set_min_children_per_line(1);
    filter_list.set_max_children_per_line(64);
    filter_list.set_column_spacing(10);
    filter_list.set_row_spacing(10);
    filter_list.set_homogeneous(true);
    filter_list.set_activate_on_single_click(false);
    filter_list.set_valign(gtk::Align::Start);
    filter_list.set_halign(gtk::Align::Start);
    filter_list.add_css_class("filter-list");
    let narrow_filters = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MaxWidth,
        720.0,
        adw::LengthUnit::Px,
    ));
    // The rail sits above the editor once both no longer fit side by side.
    let narrow_route = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MaxWidth,
        880.0,
        adw::LengthUnit::Px,
    ));
    // Only the last matching breakpoint applies. Each narrow rule moves the
    // same live rail into a header popover, preserving its controls and state.
    for breakpoint in [&narrow_route, &narrow_filters] {
        breakpoint.add_setter(&rail_menu, "visible", Some(&true.to_value()));
        breakpoint.add_setter(&header_status, "visible", Some(&false.to_value()));
        breakpoint.connect_apply({
            let body = body.clone();
            let rail = rail.clone();
            let scroll = rail_popup_scroll.clone();
            move |_| {
                body.remove(&rail);
                scroll.set_child(Some(&rail));
            }
        });
        breakpoint.connect_unapply({
            let body = body.clone();
            let rail = rail.clone();
            let popover = rail_popover.clone();
            let scroll = rail_popup_scroll.clone();
            move |_| {
                popover.popdown();
                scroll.set_child(gtk::Widget::NONE);
                body.prepend(&rail);
            }
        });
    }
    narrow_filters.add_setter(
        &level_row,
        "orientation",
        Some(&gtk::Orientation::Vertical.to_value()),
    );
    narrow_filters.add_setter(&header_status, "visible", Some(&false.to_value()));
    narrow_filters.add_setter(&analysis_label, "wrap", Some(&true.to_value()));
    narrow_filters.add_setter(
        &analysis_label,
        "ellipsize",
        Some(&gtk::pango::EllipsizeMode::None.to_value()),
    );
    let parametric_stack = gtk::Stack::new();
    parametric_stack.set_vhomogeneous(false);
    parametric_stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    parametric_stack.set_transition_duration(120);
    let visual_page = gtk::Box::new(gtk::Orientation::Vertical, 14);
    visual_page.append(&filter_list);
    parametric_stack.add_named(&visual_page, Some("visual"));
    view_stack.set_valign(gtk::Align::Start);

    let convolution_page = gtk::Box::new(gtk::Orientation::Vertical, 12);
    convolution_page.set_valign(gtk::Align::Start);
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
    narrow_filters.add_setter(
        &convolution_options,
        "orientation",
        Some(&gtk::Orientation::Vertical.to_value()),
    );
    narrow_filters.add_setter(
        &convolution_actions,
        "orientation",
        Some(&gtk::Orientation::Vertical.to_value()),
    );
    let convolution_note = gtk::Label::new(Some(
        "Parametric filters and GraphicEQ apply on top of the impulse response, per ear.",
    ));
    convolution_note.set_xalign(0.0);
    convolution_note.set_wrap(true);
    convolution_note.add_css_class("level-code");
    convolution_card.append(&convolution_note);
    convolution_page.append(&convolution_card);
    view_stack.add_titled(&convolution_page, Some("convolution"), "Convolution");

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
    parametric_stack.add_named(&text_scroll, Some("text"));
    let band_editor = build_band_editor(&model, &text_view.buffer(), &graph, &filter_list);
    visual_page.append(&band_editor);
    parametric_stack.add_named(&tone_page, Some("tone"));
    view_stack.add_titled(&parametric_stack, Some("filters"), "Parametric");
    // One row of editor pages: the bands as cards or text, the impulse
    // response, and the tone finder. Two stacks sit behind it.
    let editor_switcher = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    editor_switcher.add_css_class("editor-switcher");
    let visual_mode = gtk::ToggleButton::with_label("BANDS");
    let text_mode = gtk::ToggleButton::with_label("TEXT");
    let convolution_mode = gtk::ToggleButton::with_label("IR");
    let tone_mode = gtk::ToggleButton::with_label("TONE");
    tone_mode.set_tooltip_text(Some(
        "Find peaks and dips on your own ears with a test tone",
    ));
    for (button, view, editor) in [
        (&visual_mode, "filters", Some("visual")),
        (&text_mode, "filters", Some("text")),
        (&convolution_mode, "convolution", None),
        (&tone_mode, "filters", Some("tone")),
    ] {
        if button != &visual_mode {
            button.set_group(Some(&visual_mode));
        }
        editor_switcher.append(button);
        let view_stack = view_stack.clone();
        let parametric_stack = parametric_stack.clone();
        button.connect_clicked(move |button| {
            button.set_active(true);
            if let Some(editor) = editor {
                parametric_stack.set_visible_child_name(editor);
            }
            view_stack.set_visible_child_name(view);
        });
    }
    // Mark the tab for whatever page the stacks show.
    let sync_tabs: Rc<dyn Fn()> = Rc::new({
        let view_stack = view_stack.clone();
        let parametric_stack = parametric_stack.clone();
        let tabs = [
            visual_mode.clone(),
            text_mode.clone(),
            convolution_mode.clone(),
            tone_mode.clone(),
        ];
        move || {
            let index = if view_stack.visible_child_name().as_deref() == Some("convolution") {
                2
            } else {
                match parametric_stack.visible_child_name().as_deref() {
                    Some("text") => 1,
                    Some("tone") => 3,
                    _ => 0,
                }
            };
            tabs[index].set_active(true);
        }
    });
    {
        let sync = sync_tabs.clone();
        view_stack.connect_visible_child_name_notify(move |_| sync());
        let sync = sync_tabs.clone();
        parametric_stack.connect_visible_child_name_notify(move |_| sync());
    }
    visual_mode.set_active(true);
    let reset_filters_button = gtk::Button::with_label("RESET FILTERS");
    reset_filters_button.add_css_class("reset-filters");
    reset_filters_button.set_tooltip_text(Some(
        "Flatten every parametric band to 0 dB without changing its frequency or Q",
    ));
    window.add_breakpoint(narrow_route);
    window.add_breakpoint(narrow_filters);
    let add_filter_button = gtk::Button::with_label("＋ Add band");
    add_filter_button.add_css_class("band-add");
    add_filter_button.set_halign(gtk::Align::Start);
    let convolution_ui = ConvolutionUi {
        stack: view_stack.clone(),
        file_name: convolution_file_name,
        file_path: convolution_file_path,
        channel: convolution_channel,
        choose: choose_convolution,
        remove: remove_convolution,
        syncing: Rc::new(Cell::new(false)),
        layout: Rc::new(Cell::new(None)),
        sync_tabs: sync_tabs.clone(),
    };
    view_stack.connect_visible_child_name_notify({
        let add_filter = add_filter_button.clone();
        let reset_filters = reset_filters_button.clone();
        let parametric_stack = parametric_stack.clone();
        move |stack| {
            let parametric = stack.visible_child_name().as_deref() == Some("filters");
            let visual = parametric_stack.visible_child_name().as_deref() == Some("visual");
            add_filter.set_visible(parametric && visual);
            reset_filters.set_visible(true);
            reset_filters.set_sensitive(parametric && visual);
            reset_filters.set_opacity(if parametric && visual { 1.0 } else { 0.0 });
        }
    });
    parametric_stack.connect_visible_child_name_notify({
        let view_stack = view_stack.clone();
        let add_filter = add_filter_button.clone();
        let reset_filters = reset_filters_button.clone();
        move |stack| {
            let parametric = view_stack.visible_child_name().as_deref() == Some("filters");
            let visual = stack.visible_child_name().as_deref() == Some("visual");
            add_filter.set_visible(parametric && visual);
            reset_filters.set_sensitive(parametric && visual);
            reset_filters.set_opacity(if parametric && visual { 1.0 } else { 0.0 });
        }
    });
    let bands_body = gtk::Box::new(gtk::Orientation::Vertical, 10);
    let band_actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    band_actions.append(&add_filter_button);
    reset_filters_button.set_hexpand(true);
    reset_filters_button.set_halign(gtk::Align::End);
    band_actions.append(&reset_filters_button);
    bands_body.append(&band_actions);
    bands_body.append(&view_stack);
    main_column.append(&titled_frame(
        2,
        "bands",
        &bands_body,
        Some(editor_switcher.upcast_ref()),
    ));
    // Tools that open editor pages.
    tone_tool.connect_clicked({
        let popover = rail_popover.clone();
        let view_stack = view_stack.clone();
        let parametric_stack = parametric_stack.clone();
        move |_| {
            parametric_stack.set_visible_child_name("tone");
            view_stack.set_visible_child_name("filters");
            popover.popdown();
        }
    });
    ir_tool.connect_clicked({
        let view_stack = view_stack.clone();
        let popover = rail_popover.clone();
        move |_| {
            view_stack.set_visible_child_name("convolution");
            popover.popdown();
        }
    });
    profile_list.connect_row_activated({
        let popover = rail_popover.clone();
        move |_, _| popover.popdown()
    });

    let status = gtk::Label::new(Some("Ready"));
    status.set_xalign(0.0);
    status.set_wrap(true);
    status.add_css_class("dim-label");
    status.add_css_class("console-status");
    content.append(&status);

    let clamp = adw::Clamp::builder()
        .maximum_size(1280)
        .tightening_threshold(980)
        .child(&content)
        .build();
    let page_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&clamp)
        .build();
    toolbar.set_content(Some(&page_scroll));
    window.set_content(Some(&toolbar));

    let listening_controls = build_listening_controls(
        &window,
        &model,
        &text_view.buffer(),
        &filter_list,
        &graph,
        &trim,
        &status,
    );
    response_body.append(&listening_controls);
    gtk::glib::timeout_add_local(std::time::Duration::from_millis(20), {
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
                        status.set_text("Saved and applied · comparison level matched");
                    }
                    if let Some(analysis) = result.analysis {
                        set_analysis_label(&analysis_label, &analysis);
                        store_service_analysis(&model, analysis);
                        graph.queue_draw();
                    }
                }
            }
            gtk::glib::ControlFlow::Continue
        }
    });
    gtk::glib::timeout_add_seconds_local(2, {
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
    window.connect_close_request({
        let model = model.clone();
        let status = status.clone();
        move |_| {
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
            gtk::glib::Propagation::Proceed
        }
    });

    *model.live.borrow_mut() = Some(LiveIndicators {
        profile_status,
        output_status,
        profile_list: profile_list.clone(),
        device_drop: device_drop.clone(),
        profile_tile: profile_tile.clone(),
        menus: vec![
            profile_menu.clone(),
            rail_menu.clone(),
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
    let stop_hidden_listener = Rc::new({
        let views = view_stack.clone();
        let editors = parametric_stack.clone();
        let tone = tone_ui.clone();
        let bar = graph_listen.clone();
        move || {
            let visible = views.visible_child_name().as_deref() == Some("filters")
                && matches!(
                    editors.visible_child_name().as_deref(),
                    Some("visual" | "tone")
                );
            bar.set_visible(visible);
            if !visible {
                tone.play.set_active(false);
            }
        }
    });
    view_stack.connect_visible_child_name_notify({
        let sync = stop_hidden_listener.clone();
        move |_| sync()
    });
    parametric_stack.connect_visible_child_name_notify({
        let sync = stop_hidden_listener.clone();
        move |_| sync()
    });
    gtk::glib::timeout_add_local(std::time::Duration::from_secs(2), {
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
    install_editor_shortcut(&window, &view_stack, &parametric_stack);
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
    trim: &gtk::SpinButton,
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
                            set_analysis_label(&analysis_label, &analysis);
                            store_service_analysis(&model, analysis);
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
    let gesture = gtk::GestureDrag::new();
    gesture.connect_drag_begin({
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
            if let Some((index, filter)) = closest {
                let (point_x, point_y) =
                    graph::filter_point_position(filter.frequency, filter.gain_db, width, height);
                if ((point_x - x).powi(2) + (point_y - y).powi(2)).sqrt() <= 34.0 {
                    *state.borrow_mut() = Some((index, filter.frequency, filter.gain_db));
                    model.selected_filter.set(Some(index));
                    sync_band_ui(&model);
                    graph.queue_draw();
                    return;
                }
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

    let keyboard = gtk::EventControllerKey::new();
    keyboard.connect_key_pressed({
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
            update_filter(&model, &buffer, &graph, index, |filter| {
                graph::nudge_filter(filter, direction, shifted);
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
    graph.add_controller(keyboard);
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
    trim: &gtk::SpinButton,
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
    if let Some(id) = model.current_id.borrow().as_ref() {
        model.live_worker.end(id);
    }
    model.loading.set(true);
    *model.current_id.borrow_mut() = Some(profile.id.clone());
    model.selected_filter.set(None);
    if let Some(ui) = model.bands.borrow().as_ref() {
        ui.range.set(12.0);
    }
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
        set_analysis_label(analysis_label, &analysis);
        store_service_analysis(model, analysis);
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
    let fixture = entry(
        "Measurement fixture",
        &metadata.fixture,
        "Unknown unless documented by the source",
    );
    fixture.set_max_length(256);
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
                fixture: fixture.text().into(),
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
        label.set_visible(curves.measurement.is_some() || curves.target.is_some());
        label.set_text(if full.contains("contexts differ") {
            "Measurement and target contexts differ. See curve details."
        } else if full.contains("matching user-entered") {
            "Curves share declared context. EQ response is predicted."
        } else {
            "Curve context is incomplete. See curve details before comparing targets."
        });
        label.set_tooltip_text(Some(&full));
    }
    let mut overlay = model.overlay.borrow_mut();
    overlay.measurement = curves.measurement.clone();
    overlay.target = curves.target.clone();
    *model.curves.borrow_mut() = curves;
}

/// The graph header's menu for measurement and target curves.
struct CurvesUi {
    menu: gtk::MenuButton,
    load_measurement: gtk::Button,
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
        "Frequency and dB per line, as AutoEQ, Squiglink,\nand REW export. Three columns make a range.",
    ));
    note.set_xalign(0.0);
    note.add_css_class("level-code");
    content.append(&note);
    let button = |label: &str| {
        let button = gtk::Button::with_label(label);
        content.append(&button);
        button
    };
    let load_measurement = button("LOAD MEASUREMENT…");
    let load_target = button("LOAD TARGET…");
    let clear_measurement = button("CLEAR MEASUREMENT");
    let clear_target = button("CLEAR TARGET");
    let measurement_details = button("Measurement details…");
    let target_details = button("Target details…");
    let popover = gtk::Popover::new();
    popover.set_child(Some(&content));
    menu.set_popover(Some(&popover));
    Rc::new(CurvesUi {
        menu,
        load_measurement,
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
                    status.set_text(if path.is_empty() {
                        "Curve cleared"
                    } else {
                        "Curve loaded"
                    });
                }
                Err(error) => status.set_text(&error.to_string()),
            }
            graph.queue_draw();
        }
    });
    for (button, kind, title) in [
        (
            &ui.load_measurement,
            "measurement",
            "Choose headphone measurement",
        ),
        (&ui.load_target, "target", "Choose target curve"),
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
}

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
    frequency.update_property(&[gtk::accessible::Property::Label("Tone frequency")]);
    let level = slider((-60.0, -12.0, 1.0), -36.0);
    level.update_property(&[gtk::accessible::Property::Label("Tone level")]);
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
    })
}

/// Send the tone's settings (which also keeps it alive), or stop it.
fn push_tone(model: &Model, ui: &ToneUi, device_drop: &gtk::DropDown, status: &gtk::Label) {
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
        }
    });
    for segmented in [&ui.signal, &ui.band, &ui.ears] {
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
    gtk::glib::timeout_add_seconds_local(2, {
        let ui = ui.clone();
        let push = push.clone();
        move || {
            if ui.play.is_active() {
                push();
            }
            gtk::glib::ControlFlow::Continue
        }
    });
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
    let convolution_only = !profile.convolutions.is_empty()
        && profile.filters.is_empty()
        && profile.graphic_eqs.is_empty();
    ui.stack.set_visible_child_name(if convolution_only {
        "convolution"
    } else {
        "filters"
    });
    // The stack's own notification can be missed before the window maps.
    (ui.sync_tabs)();
}

struct GraphToneUi {
    tone: Rc<ToneUi>,
    cursor: gtk::ToggleButton,
    sweep: gtk::ToggleButton,
}

fn sweep_position(start: f64, elapsed_seconds: f64, full_seconds: f64) -> f64 {
    (start + elapsed_seconds.max(0.0) / full_seconds.max(1.0)).clamp(0.0, 1.0)
}

fn build_graph_tone(tone: &Rc<ToneUi>) -> (adw::WrapBox, GraphToneUi) {
    let row = adw::WrapBox::builder()
        .child_spacing(8)
        .line_spacing(8)
        .build();
    row.add_css_class("graph-listen");
    let play = gtk::ToggleButton::with_label("Listen");
    play.update_property(&[gtk::accessible::Property::Label("Listen to graph tone")]);
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
    frequency.update_property(&[gtk::accessible::Property::Label("Graph tone frequency")]);
    let readout = gtk::Label::new(Some("1.00 kHz"));
    readout.add_css_class("level-code");
    let ears = gtk::DropDown::from_strings(&["Both ears", "Left ear", "Right ear"]);
    ears.update_property(&[gtk::accessible::Property::Label("Graph tone ears")]);
    for widget in [
        play.upcast_ref::<gtk::Widget>(),
        sweep.upcast_ref(),
        speed.upcast_ref(),
        cursor.upcast_ref(),
        frequency.upcast_ref(),
        readout.upcast_ref(),
        ears.upcast_ref(),
    ] {
        row.append(widget);
    }
    for (text, index) in [("−1 dB here", 1), ("+1 dB here", 2)] {
        let fix = gtk::Button::with_label(text);
        let target = tone.fixes[index].0.clone();
        fix.connect_clicked(move |_| target.emit_clicked());
        row.append(&fix);
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
        move |button| {
            if button.is_active() {
                tone.signal.buttons[0].set_active(true);
                if tone.frequency.value() >= 0.999 {
                    tone.frequency.set_value(0.0);
                }
                *clock.borrow_mut() = Some((std::time::Instant::now(), tone.frequency.value()));
                tone.play.set_active(true);
            } else {
                *clock.borrow_mut() = None;
            }
        }
    });
    speed.connect_selected_notify({
        let clock = clock.clone();
        let tone = tone.clone();
        move |_| {
            let running = clock.borrow().is_some();
            if running {
                *clock.borrow_mut() = Some((std::time::Instant::now(), tone.frequency.value()));
            }
        }
    });
    gtk::glib::timeout_add_local(std::time::Duration::from_millis(25), {
        let tone = tone.clone();
        let sweep = sweep.clone();
        let clock = clock.clone();
        move || {
            let running = *clock.borrow();
            if let Some((start, position)) = running {
                let seconds = match speed.selected() {
                    0 => 15.0,
                    2 => 60.0,
                    _ => 30.0,
                };
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
    (
        row,
        GraphToneUi {
            tone: tone.clone(),
            cursor,
            sweep,
        },
    )
}

struct ListeningUi {
    undo: gtk::Button,
    redo: gtk::Button,
    reference: gtk::ToggleButton,
    bass: gtk::SpinButton,
    tilt: gtk::SpinButton,
    syncing: Cell<bool>,
}

fn submit_live(model: &Rc<Model>, save: bool) {
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
    let key = serde_json::to_string(&(
        &id,
        &document.name,
        &text,
        model.manual_trim.get(),
        &reference,
        hear_reference,
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
        save,
    });
}

fn schedule_live(model: &Rc<Model>) {
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
        ui.tilt.set_value(editing::tilt_db(document));
    }
    ui.syncing.set(false);
}

fn build_listening_controls(
    window: &adw::ApplicationWindow,
    model: &Rc<Model>,
    buffer: &gtk::TextBuffer,
    list: &gtk::FlowBox,
    graph: &gtk::DrawingArea,
    trim: &gtk::SpinButton,
    status: &gtk::Label,
) -> adw::WrapBox {
    let row = adw::WrapBox::builder()
        .child_spacing(8)
        .line_spacing(8)
        .build();
    row.add_css_class("listening-controls");
    let undo = gtk::Button::with_label("Undo");
    let redo = gtk::Button::with_label("Redo");
    let reference = gtk::ToggleButton::with_label("Hear starting sound");
    reference.set_tooltip_text(Some(
        "Compare your captured starting sound and current EQ at the same estimated perceived level",
    ));
    let capture = gtk::Button::with_label("Capture start");
    capture.set_tooltip_text(Some(
        "Use the current EQ as the new starting sound for comparison",
    ));
    for widget in [
        undo.upcast_ref::<gtk::Widget>(),
        redo.upcast_ref(),
        reference.upcast_ref(),
        capture.upcast_ref(),
    ] {
        row.append(widget);
    }
    let bass = gtk::SpinButton::with_range(-12.0, 12.0, 0.5);
    let tilt = gtk::SpinButton::with_range(-12.0, 12.0, 0.5);
    for (spin, label, tooltip) in [
        (
            &bass,
            "Bass",
            "Broad low shelf at 105 Hz. This adjustment remains editable as a normal EQ band.",
        ),
        (
            &tilt,
            "Tilt",
            "Positive is brighter; negative is warmer. Balanced low/high shelves around 1 kHz.",
        ),
    ] {
        spin.set_digits(1);
        spin.set_width_chars(4);
        spin.update_property(&[gtk::accessible::Property::Label(label)]);
        spin.set_tooltip_text(Some(tooltip));
        let field = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let label = gtk::Label::new(Some(label));
        label.add_css_class("level-code");
        field.append(&label);
        field.append(spin);
        row.append(&field);
    }
    *model.listening.borrow_mut() = Some(ListeningUi {
        undo: undo.clone(),
        redo: redo.clone(),
        reference: reference.clone(),
        bass: bass.clone(),
        tilt: tilt.clone(),
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
    for (spin, is_bass) in [(&bass, true), (&tilt, false)] {
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
    for (trigger, button) in [("<Control>z", undo), ("<Control><Shift>z", redo)] {
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
    row
}

struct BandTile {
    root: gtk::Box,
    select: gtk::ToggleButton,
    gain: gtk::Label,
    frequency: gtk::Label,
    kind: gtk::Label,
    scale: gtk::Scale,
}

struct BandUi {
    root: gtk::Box,
    title: gtk::Label,
    kind: Segmented,
    channels: Segmented,
    frequency: gtk::SpinButton,
    gain: gtk::SpinButton,
    q: gtk::SpinButton,
    enabled: gtk::Switch,
    previous: gtk::Button,
    next: gtk::Button,
    tiles: Vec<BandTile>,
    range: Cell<f64>,
    syncing: Rc<Cell<bool>>,
}

const BAND_KINDS: [FilterKind; 8] = [
    FilterKind::Peaking,
    FilterKind::LowShelf,
    FilterKind::HighShelf,
    FilterKind::LowPass,
    FilterKind::HighPass,
    FilterKind::BandPass,
    FilterKind::Notch,
    FilterKind::AllPass,
];
const BAND_KIND_LABELS: [&str; 8] = ["PK", "LS", "HS", "LP", "HP", "BP", "NO", "AP"];

fn build_band_editor(
    model: &Rc<Model>,
    buffer: &gtk::TextBuffer,
    graph: &gtk::DrawingArea,
    list: &gtk::FlowBox,
) -> gtk::Box {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
    root.add_css_class("band-editor");
    let heading = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let title = gtk::Label::new(Some("Select a band"));
    title.add_css_class("band-editor-title");
    title.set_xalign(0.0);
    title.set_hexpand(true);
    heading.append(&title);
    let enabled = gtk::Switch::new();
    enabled.set_valign(gtk::Align::Center);
    enabled.update_property(&[gtk::accessible::Property::Label("Selected band enabled")]);
    heading.append(&enabled);
    let previous = gtk::Button::from_icon_name("go-previous-symbolic");
    let next = gtk::Button::from_icon_name("go-next-symbolic");
    let remove = gtk::Button::from_icon_name("user-trash-symbolic");
    for (button, label) in [
        (&previous, "Move band earlier"),
        (&next, "Move band later"),
        (&remove, "Delete selected band"),
    ] {
        button.add_css_class("flat");
        button.set_tooltip_text(Some(label));
        button.update_property(&[gtk::accessible::Property::Label(label)]);
        heading.append(button);
    }
    root.append(&heading);
    let kind = Segmented::new(&BAND_KIND_LABELS, 0);
    let options = adw::WrapBox::builder()
        .child_spacing(10)
        .line_spacing(8)
        .build();
    kind.root.add_css_class("band-kinds");
    for (button, name) in kind.buttons.iter().zip([
        "Peak",
        "Low shelf",
        "High shelf",
        "Low pass",
        "High pass",
        "Band pass",
        "Notch",
        "All pass",
    ]) {
        button.set_tooltip_text(Some(name));
        button.update_property(&[gtk::accessible::Property::Label(name)]);
    }
    let channels = Segmented::new(&["L+R", "L", "R"], 0);
    options.append(&kind.root);
    options.append(&channels.root);
    let frequency = gtk::SpinButton::with_range(20.0, 20_000.0, 1.0);
    frequency.set_digits(1);
    frequency.set_width_chars(6);
    let gain = gtk::SpinButton::with_range(-60.0, 60.0, 0.1);
    gain.set_digits(2);
    gain.set_width_chars(5);
    let q = gtk::SpinButton::with_range(0.01, 1000.0, 0.01);
    q.set_digits(3);
    q.set_width_chars(5);
    for (spin, label, unit) in [
        (&frequency, "Frequency", "Hz"),
        (&gain, "Gain", "dB"),
        (&q, "Q", "Q"),
    ] {
        spin.add_css_class("band-readout");
        spin.update_property(&[gtk::accessible::Property::Label(&format!(
            "Selected band {label}"
        ))]);
        let field = spin_field(label, unit, spin);
        field.set_hexpand(false);
        spin.set_hexpand(false);
        options.append(&field);
    }
    root.append(&options);
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
        ChannelSelection::All,
        ChannelSelection::Left,
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
    for (spin, parameter) in [(&frequency, 0), (&gain, 1), (&q, 2)] {
        spin.connect_value_changed({
            let model = model.clone();
            let buffer = buffer.clone();
            let graph = graph.clone();
            let syncing = syncing.clone();
            move |spin| {
                if !syncing.get()
                    && let Some(index) = model.selected_filter.get()
                {
                    update_filter(&model, &buffer, &graph, index, |filter| match parameter {
                        0 => filter.frequency = spin.value(),
                        1 => filter.gain_db = spin.value(),
                        _ => filter.q = spin.value(),
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
        move |switch| {
            if !syncing.get()
                && let Some(index) = model.selected_filter.get()
            {
                update_filter(&model, &buffer, &graph, index, |filter| {
                    filter.enabled = switch.is_active()
                });
            }
        }
    });
    for (button, offset) in [(&previous, -1_i32), (&next, 1), (&remove, 0)] {
        button.connect_clicked({
            let model = model.clone();
            let buffer = buffer.clone();
            let graph = graph.clone();
            let list = list.clone();
            move |_| {
                let Some(index) = model.selected_filter.get() else {
                    return;
                };
                let snapshot = {
                    let mut document = model.document.borrow_mut();
                    let Some(document) = document.as_mut() else {
                        return;
                    };
                    if index >= document.filters.len() {
                        return;
                    }
                    if offset == 0 {
                        document.filters.remove(index);
                        model.selected_filter.set(if document.filters.is_empty() {
                            None
                        } else {
                            Some(index.min(document.filters.len() - 1))
                        });
                    } else {
                        let target = index as i32 + offset;
                        if target < 0 || target as usize >= document.filters.len() {
                            return;
                        }
                        document.filters.swap(index, target as usize);
                        model.selected_filter.set(Some(target as usize));
                    }
                    document.clone()
                };
                set_editor_text(&model, &buffer, &serialize_profile(&snapshot));
                *model.analysis.borrow_mut() = Some(preview_analysis(&model, &snapshot));
                rebuild_filter_list(&list, &snapshot, &model, &buffer, &graph);
                graph.queue_draw();
            }
        });
    }
    *model.bands.borrow_mut() = Some(BandUi {
        root: root.clone(),
        title,
        kind,
        channels,
        frequency,
        gain,
        q,
        enabled,
        previous,
        next,
        tiles: Vec::new(),
        range: Cell::new(12.0),
        syncing,
    });
    root
}

/// Refresh values in place, keeping keyboard focus and an in-progress fader drag.
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
    // Normal EQ gets a useful ±12 dB travel; larger edits expand every fader
    // together so their zero points and gain distances stay comparable.
    let range = profile
        .filters
        .iter()
        .map(|f| f.gain_db.abs())
        .fold(12.0, f64::max);
    let range = ((range / 12.0).ceil() * 12.0).max(ui.range.get());
    ui.range.set(range);
    for (index, (tile, filter)) in ui.tiles.iter().zip(&profile.filters).enumerate() {
        let active = selected == Some(index);
        tile.select.set_active(active);
        if active {
            tile.root.add_css_class("selected");
        } else {
            tile.root.remove_css_class("selected");
        }
        if filter.enabled {
            tile.root.remove_css_class("disabled-band");
        } else {
            tile.root.add_css_class("disabled-band");
        }
        tile.scale.set_range(-range, range);
        tile.scale.set_value(filter.gain_db);
        tile.gain.set_text(&signed_db(filter.gain_db, 1));
        tile.frequency
            .set_text(&compact_band_frequency(filter.frequency));
        let kind = BAND_KINDS
            .iter()
            .position(|kind| *kind == filter.kind)
            .unwrap_or_default();
        let ears = match filter.channels {
            ChannelSelection::All => "L+R",
            ChannelSelection::Left => "L",
            ChannelSelection::Right => "R",
        };
        tile.kind
            .set_text(&format!("{} · {ears}", BAND_KIND_LABELS[kind]));
        tile.root.set_tooltip_text(Some(&format!(
            "Band {} · {} · Q {:.3}{}",
            index + 1,
            format_band_frequency(filter.frequency),
            filter.q,
            if filter.enabled { "" } else { " · off" }
        )));
    }
    let filter = selected.and_then(|index| profile.filters.get(index));
    ui.root.set_visible(filter.is_some());
    if let Some(filter) = filter {
        let index = selected.unwrap_or_default();
        ui.title.set_text(&format!("Band {:02}", index + 1));
        let kind = BAND_KINDS
            .iter()
            .position(|kind| *kind == filter.kind)
            .unwrap_or_default();
        ui.kind.buttons[kind].set_active(true);
        ui.channels.buttons[match filter.channels {
            ChannelSelection::All => 0,
            ChannelSelection::Left => 1,
            ChannelSelection::Right => 2,
        }]
        .set_active(true);
        ui.frequency.set_value(filter.frequency);
        ui.gain.set_value(filter.gain_db);
        ui.q.set_value(filter.q);
        ui.enabled.set_active(filter.enabled);
        ui.previous.set_sensitive(index > 0);
        ui.next.set_sensitive(index + 1 < profile.filters.len());
    }
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
    if !profile.filters.is_empty()
        && model
            .selected_filter
            .get()
            .is_none_or(|i| i >= profile.filters.len())
    {
        model.selected_filter.set(Some(0));
    } else if profile.filters.is_empty() {
        model.selected_filter.set(None);
    }
    // Autosaves and graph edits need value updates, not new slider widgets.
    let reuse = model
        .bands
        .borrow()
        .as_ref()
        .is_some_and(|ui| ui.tiles.len() == profile.filters.len())
        && !profile.filters.is_empty()
        && profile.graphic_eqs.is_empty()
        && list.child_at_index(profile.filters.len() as i32).is_none();
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
    let mut group: Option<gtk::ToggleButton> = None;
    for (index, filter) in profile.filters.iter().enumerate() {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 5);
        root.add_css_class("band-fader");
        let select = gtk::ToggleButton::with_label(&format!("{:02}", index + 1));
        select.add_css_class("fader-select");
        select.update_property(&[gtk::accessible::Property::Label(&format!(
            "Select band {}",
            index + 1
        ))]);
        if let Some(first) = &group {
            select.set_group(Some(first));
        } else {
            group = Some(select.clone());
        }
        root.append(&select);
        let gain = gtk::Label::new(None);
        gain.add_css_class("fader-gain");
        root.append(&gain);
        let scale = gtk::Scale::with_range(gtk::Orientation::Vertical, -12.0, 12.0, 0.1);
        scale.set_inverted(true);
        scale.set_draw_value(false);
        scale.set_has_origin(true);
        scale.set_size_request(-1, 124);
        scale.set_halign(gtk::Align::Center);
        scale.add_mark(0.0, gtk::PositionType::Left, None);
        scale.add_css_class("band-slider");
        scale.update_property(&[gtk::accessible::Property::Label(&format!(
            "Band {} gain",
            index + 1
        ))]);
        root.append(&scale);
        let frequency = gtk::Label::new(Some(&compact_band_frequency(filter.frequency)));
        frequency.add_css_class("fader-frequency");
        root.append(&frequency);
        let kind = gtk::Label::new(None);
        kind.add_css_class("fader-kind");
        root.append(&kind);
        select.connect_toggled({
            let model = model.clone();
            let graph = graph.clone();
            let syncing = syncing.clone();
            move |button| {
                if !syncing.get() && button.is_active() {
                    model.selected_filter.set(Some(index));
                    sync_band_ui(&model);
                    graph.queue_draw();
                }
            }
        });
        scale.connect_value_changed({
            let model = model.clone();
            let buffer = buffer.clone();
            let graph = graph.clone();
            let syncing = syncing.clone();
            move |scale| {
                if !syncing.get() {
                    model.selected_filter.set(Some(index));
                    update_filter(&model, &buffer, &graph, index, |filter| {
                        filter.gain_db = scale.value()
                    });
                }
            }
        });
        let click = gtk::GestureClick::new();
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        click.connect_pressed({
            let model = model.clone();
            let graph = graph.clone();
            move |_, _, _, _| {
                model.selected_filter.set(Some(index));
                sync_band_ui(&model);
                graph.queue_draw();
            }
        });
        root.add_controller(click);
        list.insert(&root, -1);
        tiles.push(BandTile {
            root,
            select,
            gain,
            frequency,
            kind,
            scale,
        });
    }
    if let Some(ui) = model.bands.borrow_mut().as_mut() {
        ui.tiles = tiles;
    }
    for graphic in &profile.graphic_eqs {
        let row = gtk::Label::new(Some(&format!("GraphicEQ\n{} points", graphic.points.len())));
        row.add_css_class("band-fader");
        list.insert(&row, -1);
    }
    if profile.filters.is_empty() && profile.graphic_eqs.is_empty() {
        let empty = gtk::Label::new(Some(
            "Add a band or click the response graph to start shaping your EQ.",
        ));
        empty.set_wrap(true);
        empty.set_max_width_chars(34);
        empty.add_css_class("level-code");
        list.insert(&empty, -1);
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

fn spin_field(label: &str, unit: &str, spin: &gtk::SpinButton) -> gtk::Box {
    let field = gtk::Box::new(gtk::Orientation::Vertical, 5);
    field.set_hexpand(true);
    field.set_size_request(120, -1);
    let heading = gtk::Label::new(Some(&format!("{label} · {}", unit.to_uppercase())));
    heading.set_xalign(0.0);
    heading.add_css_class("section-label");
    field.append(&heading);
    spin.set_hexpand(true);
    field.append(spin);
    field
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
        ui.menu.set_label("COMPARE");
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
                "COMPARE · {active_position}/{}",
                comparison.profile_ids.len()
            ));
            ui.menu.set_tooltip_text(Some(&format!(
                "BS.1770 K-weighted comparison · {active_name}"
            )));
            ui.status
                .set_text(&format!("LEVEL MATCHED · {active_name}"));
        } else {
            ui.menu.set_label("COMPARE · PAUSED");
            ui.menu
                .set_tooltip_text(Some("This output's comparison bank is paused"));
            ui.status.set_text("PAUSED · choose a profile to resume");
        }
    } else {
        ui.save.set_label("START COMPARISON");
        ui.delete.set_sensitive(false);
        ui.stack.set_visible_child_name("setup");
        ui.menu.set_label("COMPARE");
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
        move |_, key, _, modifiers| {
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
            let Some(index) = character
                .and_then(|value| value.to_digit(10))
                .filter(|value| (1..=9).contains(value))
                .map(|value| value as usize - 1)
            else {
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
    window.add_controller(controller);
}

fn install_editor_shortcut(
    window: &adw::ApplicationWindow,
    mode_stack: &adw::ViewStack,
    editor_stack: &gtk::Stack,
) {
    let controller = gtk::EventControllerKey::new();
    controller.connect_key_pressed({
        let mode_stack = mode_stack.clone();
        let editor_stack = editor_stack.clone();
        move |_, key, _, modifiers| {
            let character = key.to_unicode().map(|value| value.to_ascii_lowercase());
            if !modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK) || character != Some('e') {
                return gtk::glib::Propagation::Proceed;
            }
            mode_stack.set_visible_child_name("filters");
            let next = if editor_stack.visible_child_name().as_deref() == Some("text") {
                "visual"
            } else {
                "text"
            };
            editor_stack.set_visible_child_name(next);
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
    // The accent is for actions that put a profile on the output.
    if matches!(
        assign.label().as_deref(),
        Some("Apply selected profile" | "Replace with selected" | "Retry profile")
    ) {
        assign.add_css_class("primary");
    } else {
        assign.remove_css_class("primary");
    }
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
        "{rate}Click empty graph space to add a band, drag points to edit, or use arrows on the selected point"
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

        .instrument-header {
            background-color: #0b0b0c;
            border-bottom: 1px solid #232326;
            box-shadow: none;
            min-height: 52px;
        }

        .brand,
        .header-status,
        .header-data,
        .panel-title,
        .section-label,
        .rail-label,
        .frame-title,
        .readouts,
        .readout-key,
        .console-status,
        .tile-value-number,
        .level-code {
            font-family: monospace;
        }

        .brand {
            font-weight: 800;
            letter-spacing: 1px;
            margin-left: 4px;
        }

        .header-status {
            color: #8b8b90;
            font-size: 11px;
            letter-spacing: 0.4px;
        }

        .header-data {
            font-size: 9px;
            font-weight: 700;
            opacity: 0.54;
        }

        /* Engine: an orange power button while the DSP runs. */
        button.power {
            min-width: 32px;
            min-height: 32px;
            padding: 0;
            border-radius: 999px;
            background-color: #242426;
            color: #8b8b90;
            border: none;
            box-shadow: none;
        }

        button.power:checked {
            background-color: #ff7a1a;
            color: #fff;
        }

        .update-button {
            background-color: transparent;
            border: 1px solid rgba(255, 122, 26, 0.6);
            border-radius: 12px;
            color: #ff8c33;
            font-family: monospace;
            font-size: 9px;
            font-weight: 800;
            padding: 5px 10px;
        }

        .update-button:hover {
            background-color: rgba(255, 122, 26, 0.14);
        }

        /* Left rail */
        .rail-label {
            color: #56565b;
            font-size: 9.5px;
            font-weight: 700;
            letter-spacing: 0.8px;
            margin: 6px 4px 0 4px;
        }

        .rail-tile {
            background-color: #1b1b1d;
            border-radius: 22px;
            padding: 14px;
        }

        .tile-icon {
            min-width: 46px;
            min-height: 46px;
            border-radius: 999px;
            background-color: #242426;
            color: #ecebe6;
        }

        entry.tile-title,
        entry.tile-title:focus-within {
            background: transparent;
            border: none;
            box-shadow: none;
            font-size: 17px;
            font-weight: 600;
            min-height: 26px;
            padding: 0;
        }

        entry.tile-title:focus-within {
            border-bottom: 1px solid #ff7a1a;
            border-radius: 0;
        }

        .tile-subtitle {
            color: #8b8b90;
            font-size: 12.5px;
        }

        .tile-value {
            background-color: #242426;
            border-radius: 999px;
            padding: 7px 0;
        }

        .tile-value-number {
            font-size: 17px;
            font-weight: 600;
        }

        .tile-value-caption {
            color: #8b8b90;
            font-size: 11px;
        }

        /* Filters on this output: the watch's orange circle. */
        button.filters-dot {
            min-width: 54px;
            min-height: 54px;
            padding: 0;
            border-radius: 999px;
            background-color: #3a2414;
            border: none;
            box-shadow: none;
        }

        button.filters-dot:checked {
            background-color: #ff7a1a;
        }

        /* The open profile, while it plays: a lit tile. */
        .rail-tile.live {
            background-color: #f3f2ee;
            color: #121213;
        }

        .rail-tile.live .tile-icon {
            background-color: #e2e1dc;
            color: #121213;
        }

        .rail-tile.live entry.tile-title {
            color: #121213;
            caret-color: #121213;
        }

        .rail-tile.live .tile-subtitle,
        .rail-tile.live .tile-value-caption {
            color: #6b6b70;
        }

        .rail-tile.live .tile-value {
            background-color: #e6e5e0;
        }

        dropdown.tile-dropdown > button,
        dropdown.tile-dropdown > button:hover {
            background: transparent;
            border: none;
            box-shadow: none;
            font-size: 17px;
            font-weight: 600;
            min-height: 26px;
            padding: 0;
        }

        dropdown.tile-dropdown {
            min-width: 0;
        }

        .device-state {
            color: #8b8b90;
            font-family: monospace;
            font-size: 10.5px;
        }

        button.tile-action {
            background-color: #242426;
            border: none;
            border-radius: 14px;
            box-shadow: none;
            color: #ecebe6;
            font-family: monospace;
            font-size: 11px;
            font-weight: 700;
            padding: 9px 12px;
        }

        button.tile-action.primary {
            background-color: #ff7a1a;
            color: #1a0d03;
        }

        button.tile-action:disabled {
            color: #56565b;
        }

        list.profile-bank,
        list.profile-bank > row {
            background: transparent;
        }

        list.profile-bank > row {
            background-color: #1b1b1d;
            border-radius: 16px;
            margin-bottom: 6px;
            padding: 0 2px;
        }

        list.profile-bank > row:hover {
            background-color: #222225;
        }

        /* The profile open in the editor. */
        list.profile-bank > row:selected {
            background-color: #2b2b2e;
        }

        list.profile-bank > row.playing .subtitle {
            color: #ff8c33;
            opacity: 1;
        }

        .bank-actions button {
            background-color: #1b1b1d;
            border: none;
            border-radius: 14px;
            box-shadow: none;
            min-height: 34px;
        }

        .tool-tile,
        .tool-tile > button {
            min-height: 74px;
        }

        button.tool-tile,
        .tool-tile > button {
            background-color: #1b1b1d;
            border: none;
            border-radius: 20px;
            box-shadow: none;
            color: #ecebe6;
            font-size: 12.5px;
            font-weight: 500;
        }

        button.tool-tile.round {
            min-width: 74px;
            border-radius: 999px;
            padding: 0;
        }

        button.tool-tile:hover,
        .tool-tile > button:hover {
            background-color: #232326;
        }

        button.tool-tile.on,
        .tool-tile.on > button {
            background-color: #ff7a1a;
            color: #1a0d03;
        }

        /* Frames with their titles set into the rule (after btop). */
        .titled-frame {
            border: 1px solid #3a3a3e;
            border-radius: 14px;
            padding: 20px 16px 12px 16px;
        }

        .frame-title {
            background-color: #0b0b0c;
            color: #ecebe6;
            font-size: 12px;
            padding: 0 6px;
        }

        .frame-end {
            background-color: #0b0b0c;
            padding: 0 4px;
        }

        .readout-row {
            padding: 2px 4px 0 40px;
        }

        .readouts,
        .readout-key {
            color: #8b8b90;
            font-size: 11px;
        }

        .readout-key {
            font-size: 9.5px;
            letter-spacing: 0.6px;
        }

        .level-code {
            color: #8b8b90;
            font-size: 10px;
        }

        .panel-title {
            font-size: 12px;
            font-weight: 700;
            letter-spacing: 0.7px;
        }

        .section-label {
            font-size: 9px;
            font-weight: 700;
            letter-spacing: 0.6px;
            opacity: 0.46;
        }

        .comparison-menu > button {
            border-radius: 14px;
            font-family: monospace;
            font-size: 10px;
            font-weight: 700;
            padding: 7px 12px;
        }

        .comparison-candidate {
            background-color: #1b1b1d;
            border: 1px solid #2a2a2d;
            border-radius: 12px;
            padding: 6px 8px;
            font-family: monospace;
            box-shadow: none;
        }

        .comparison-candidate:checked {
            background-color: rgba(255, 122, 26, 0.10);
            border-color: rgba(255, 122, 26, 0.72);
            color: #f2f1ec;
        }

        .comparison-listen {
            background-color: #1b1b1d;
            border: 1px solid #2a2a2d;
            border-radius: 12px;
            padding: 8px 10px;
            font-family: monospace;
            box-shadow: none;
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

        /* Editor pages, set into the bands frame. */
        .editor-switcher button {
            background: transparent;
            border: 0;
            border-radius: 10px;
            box-shadow: none;
            color: #8b8b90;
            font-family: monospace;
            font-size: 10.5px;
            font-weight: 700;
            min-height: 0;
            padding: 5px 10px;
        }

        .editor-switcher button:checked {
            background-color: #242426;
            color: #ecebe6;
        }

        .code-editor {
            background-color: #121213;
            border: 1px solid #2a2a2d;
            border-radius: 18px;
        }

        textview.invalid-draft {
            color: #ff9b88;
        }

        entry,
        spinbutton,
        dropdown > button {
            background-color: #1b1b1d;
            color: #ecebe6;
            border: 1px solid #2a2a2d;
            border-radius: 12px;
            box-shadow: none;
        }

        entry:focus,
        spinbutton:focus-within,
        dropdown > button:focus {
            border-color: #ff7a1a;
            box-shadow: 0 0 0 1px rgba(255, 122, 26, 0.4);
        }

        button {
            border-radius: 12px;
        }

        button.suggested-action {
            background-color: #ff7a1a;
            color: #1a0d03;
            border-color: #ff8c33;
            font-family: monospace;
            font-weight: 700;
        }

        button.suggested-action:disabled {
            background-color: #242426;
            color: #8b8b90;
            border-color: #2a2a2d;
        }

        switch:checked {
            background-color: #ff7a1a;
        }

        switch {
            min-width: 34px;
            min-height: 18px;
            border-radius: 999px;
            padding: 2px;
        }

        switch slider {
            min-width: 14px;
            min-height: 14px;
            border-radius: 999px;
            margin: 0;
        }

        separator {
            background-color: #232326;
        }

        .filter-list,
        .filter-list > row,
        .filter-list > flowboxchild {
            background: transparent;
        }

        .filter-list > flowboxchild {
            padding: 0;
            min-width: 0;
        }

        .band-fader {
            min-width: 72px;
            background-color: #1b1b1d;
            border-radius: 18px;
            padding: 7px 7px 10px;
            color: #ecebe6;
        }

        .band-fader.selected {
            background-color: #f3f2ee;
            color: #121213;
        }

        .band-fader.disabled-band .fader-gain,
        .band-fader.disabled-band .band-slider {
            opacity: 0.35;
        }

        button.fader-select {
            background: transparent;
            box-shadow: none;
            border: none;
            min-height: 18px;
            padding: 0;
            color: #8b8b90;
            font-family: monospace;
            font-size: 9px;
        }

        .band-fader.selected button.fader-select {
            color: #6b6b70;
        }

        .fader-gain,
        .fader-frequency,
        .fader-kind,
        .band-editor-title {
            font-family: monospace;
        }

        .fader-gain { font-size: 14px; font-weight: 700; }
        .fader-frequency { font-size: 12px; }
        .fader-kind { font-size: 9px; color: #8b8b90; }
        .band-fader.selected .fader-kind { color: #6b6b70; }

        scale.band-slider { padding: 10px 20px; }
        scale.band-slider trough { min-width: 6px; background: #303033; border-radius: 3px; }
        scale.band-slider highlight { background: #ff7a1a; border-radius: 3px; }
        scale.band-slider slider {
            min-width: 20px;
            min-height: 20px;
            margin: -7px;
            background: #ecebe6;
            border: none;
            box-shadow: 0 2px 4px rgba(0, 0, 0, 0.3);
        }
        .band-fader.selected scale.band-slider trough { background: #d8d7d1; }
        .band-fader.selected scale.band-slider slider { background: #121213; }
        scale.band-slider marks indicator { background: #56565b; min-width: 5px; }

        .band-editor { border-top: 1px solid #2a2a2d; padding-top: 10px; }
        .band-editor-title { font-size: 11px; color: #8b8b90; }
        .listening-controls button,
        .graph-listen button,
        .graph-listen dropdown > button {
            font-family: monospace;
            font-size: 10px;
            min-height: 24px;
            padding: 5px 8px;
        }
        .listening-controls spinbutton { min-height: 28px; }
        .listening-controls spinbutton text { font-family: monospace; font-size: 11px; }

        .band-editor .band-kinds button { min-width: 12px; padding: 6px; font-size: 10px; }
        .band-kinds { margin-top: 12px; }
        .band-editor .segmented { margin-top: 12px; }
        .band-editor .segmented button { min-height: 24px; }
        .band-editor .band-readout { margin-top: 0; }

        .convolution-card {
            background-color: #1b1b1d;
            border: none;
            border-radius: 20px;
            padding: 18px;
        }

        .convolution-name {
            font-family: monospace;
            font-size: 17px;
            font-weight: 600;
        }

        .filter-index {
            min-width: 28px;
            min-height: 28px;
            border-radius: 999px;
            font-family: monospace;
            font-size: 9px;
            font-weight: 800;
            padding: 0 8px;
        }

        .filter-index:checked {
            background-color: #ecebe6;
            color: #121213;
        }

        .band-frequency {
            font-family: monospace;
            font-size: 16px;
            font-weight: 500;
        }

        .filter-switch {
            min-width: 36px;
            min-height: 20px;
        }

        .filter-switch slider {
            min-width: 16px;
            min-height: 16px;
            border-radius: 999px;
        }

        .band-readout text {
            font-family: monospace;
            font-size: 13px;
        }

        .band-add {
            background-color: transparent;
            color: #8b8b90;
            border: 1px solid #3a3a3e;
            border-radius: 12px;
            font-family: monospace;
            font-weight: 700;
            padding: 9px 14px;
        }

        .band-add:hover {
            color: #ecebe6;
            border-color: #56565b;
        }

        .reset-filters {
            background: transparent;
            color: #8b8b90;
            font-family: monospace;
            font-size: 10px;
            font-weight: 700;
            padding: 7px 12px;
        }

        .tone-play {
            min-width: 64px;
            min-height: 64px;
            border-radius: 999px;
            background-color: #242426;
            border: none;
            padding: 0;
        }

        .tone-play:checked {
            background-color: #ff7a1a;
            color: #1a0d03;
        }

        .tone-readout {
            background-color: #242426;
            border-radius: 18px;
            min-width: 150px;
            padding: 10px 16px;
        }

        .tone-value {
            font-family: monospace;
            font-size: 22px;
            font-weight: 600;
        }

        .segmented button {
            background-color: #1b1b1d;
            border: 1px solid #2a2a2d;
            color: #8b8b90;
            font-family: monospace;
            font-size: 10px;
            font-weight: 700;
            padding: 6px 12px;
            box-shadow: none;
        }

        .segmented button:checked {
            background-color: #2b2b2e;
            border-color: #3a3a3e;
            color: #ecebe6;
        }

        .tone-fix {
            background-color: #242426;
            border: none;
            border-radius: 14px;
            font-family: monospace;
            font-weight: 700;
            min-width: 76px;
            padding: 10px 14px;
        }

        .tone-fix:hover {
            background-color: #2b2b2e;
        }

        .console-status {
            color: #8b8b90;
            font-size: 10px;
            padding: 2px 6px;
        }

        popover > contents {
            background-color: #151516;
            border: 1px solid #2a2a2d;
            border-radius: 20px;
            box-shadow: 0 16px 38px rgba(0, 0, 0, 0.45);
        }

        list.boxed-list,
        list.boxed-list > row {
            background-color: #1b1b1d;
        }

        scale trough {
            background-color: #242426;
            min-height: 6px;
            border-radius: 3px;
        }

        scale highlight {
            background-color: #ff7a1a;
            border-radius: 3px;
        }

        scale slider {
            background-color: #ecebe6;
            min-width: 20px;
            min-height: 20px;
            border-radius: 999px;
            box-shadow: 0 2px 6px rgba(0, 0, 0, 0.5);
        }

        scrollbar slider {
            background-color: #3a3a3e;
            border-radius: 999px;
            min-width: 5px;
            min-height: 5px;
        }

        textview {
            background-color: #121213;
            color: #e7e7e2;
            font-family: monospace;
            padding: 14px;
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
fn store_service_analysis(model: &Model, analysis: ProfileAnalysis) {
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
    label.set_markup(&format!(
        "PREAMP {preamp}     MATCH {}     SAFETY {}{}",
        value(signed_db(analysis.match_gain_db, 2)),
        value(signed_db(analysis.safety_attenuation_db, 2)),
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
        if let Some(latency) = number("latency_ms") {
            parts.push(format!("{} ms", accent(&format!("{latency:.1}"))));
        }
        if let Some(cpu) = number("cpu_percent_of_deadline") {
            parts.push(format!("CPU {}", accent(&format!("{cpu:.0}%"))));
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

/// A round icon badge for the start of a tile.
fn tile_icon(name: &str) -> gtk::Box {
    let badge = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    badge.add_css_class("tile-icon");
    badge.set_hexpand(false);
    badge.set_valign(gtk::Align::Center);
    let image = gtk::Image::from_icon_name(name);
    image.set_hexpand(true);
    image.set_halign(gtk::Align::Center);
    badge.append(&image);
    badge
}

fn rail_label(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.add_css_class("rail-label");
    label
}

/// A panel outlined by a thin rule, its numbered title set into the top edge
/// (after btop), with an optional widget set into the right of that edge.
fn titled_frame(
    number: u32,
    title: &str,
    child: &impl IsA<gtk::Widget>,
    end: Option<&gtk::Widget>,
) -> gtk::Overlay {
    let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
    frame.add_css_class("titled-frame");
    frame.set_margin_top(14);
    frame.append(child);
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&frame));
    let label = gtk::Label::new(None);
    label.set_markup(&format!(
        "<span foreground='{ACCENT}' size='70%' rise='4000'>{number}</span>{}",
        gtk::glib::markup_escape_text(title)
    ));
    label.add_css_class("frame-title");
    label.set_halign(gtk::Align::Start);
    label.set_valign(gtk::Align::Start);
    label.set_margin_start(16);
    label.set_margin_top(5);
    overlay.add_overlay(&label);
    if let Some(end) = end {
        end.add_css_class("frame-end");
        end.set_halign(gtk::Align::End);
        end.set_valign(gtk::Align::Start);
        end.set_margin_end(16);
        overlay.add_overlay(end);
        overlay.set_measure_overlay(end, true);
    }
    overlay
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
