mod client;
mod graph;

use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use massiveeq_core::{
    COMPARISON_BYPASS_ID, ChannelSelection, ComparisonSet, DeviceInfo, Filter, FilterKind,
    MAX_COMPARISON_PROFILES, ProfileAnalysis, ProfileDocument, ProfileInfo,
    analyze_profile_preview, parse_text, serialize_profile,
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
    manual_trim: Cell<f64>,
    sample_rate: Cell<f64>,
    loading: Cell<bool>,
    syncing_device: Cell<bool>,
    pending_device_bypass: RefCell<Option<PendingDeviceBypass>>,
    syncing_engine: Cell<bool>,
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
    add_filter: gtk::Button,
    reset_filters: gtk::Button,
    syncing: Rc<Cell<bool>>,
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
    bypass: gtk::Switch,
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
        client,
        current_id: RefCell::new(None),
        document: Rc::new(RefCell::new(None)),
        analysis: Rc::new(RefCell::new(None)),
        selected_filter: Rc::new(Cell::new(None)),
        manual_trim: Cell::new(0.0),
        sample_rate: Cell::new(active_sample_rate),
        loading: Cell::new(false),
        syncing_device: Cell::new(false),
        pending_device_bypass: RefCell::new(None),
        syncing_engine: Cell::new(false),
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
    let title = adw::WindowTitle::new("MASSIVE / EQ", "");
    title.add_css_class("brand-title");
    header.set_title_widget(Some(&title));

    let profile_popover = gtk::Popover::new();
    let profile_popover_box = gtk::Box::new(gtk::Orientation::Vertical, 10);
    profile_popover_box.set_margin_top(12);
    profile_popover_box.set_margin_bottom(12);
    profile_popover_box.set_margin_start(12);
    profile_popover_box.set_margin_end(12);
    profile_popover_box.set_size_request(330, 360);
    let profile_heading = gtk::Label::new(Some("PROFILE BANK"));
    profile_heading.set_xalign(0.0);
    profile_heading.add_css_class("section-label");
    profile_popover_box.append(&profile_heading);
    let profile_list = gtk::ListBox::new();
    profile_list.add_css_class("boxed-list");
    profile_list.set_selection_mode(gtk::SelectionMode::Single);
    let profile_scroll = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&profile_list)
        .build();
    profile_popover_box.append(&profile_scroll);

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
    profile_popover_box.append(&profile_buttons);
    profile_popover.set_child(Some(&profile_popover_box));

    let profile_menu = gtk::MenuButton::new();
    profile_menu.set_label("Choose profile");
    profile_menu.set_popover(Some(&profile_popover));
    profile_menu.add_css_class("profile-menu");
    header.pack_start(&profile_menu);
    let header_add_profile = gtk::Button::with_label("+ NEW PROFILE");
    header_add_profile.add_css_class("profile-add");
    header_add_profile.set_tooltip_text(Some("Create and select a new profile"));
    header.pack_start(&header_add_profile);

    let global_bypass = gtk::Switch::new();
    global_bypass.set_valign(gtk::Align::Center);
    global_bypass.set_active(
        !model
            .client
            .status()
            .ok()
            .and_then(|value| value.get("global_bypass").and_then(|value| value.as_bool()))
            .unwrap_or(false),
    );
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
    header.pack_end(&global_bypass_label);
    toolbar.add_top_bar(&header);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.set_margin_top(16);
    content.set_margin_bottom(22);
    content.set_margin_start(18);
    content.set_margin_end(18);

    let dashboard = gtk::Box::new(gtk::Orientation::Vertical, 12);

    let controls_card = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    controls_card.add_css_class("control-card");
    let route_heading = gtk::Label::new(Some("SIGNAL ROUTE"));
    route_heading.set_xalign(0.0);
    route_heading.add_css_class("panel-title");
    route_heading.set_valign(gtk::Align::Center);
    controls_card.append(&route_heading);
    let route_rule = gtk::Separator::new(gtk::Orientation::Vertical);
    controls_card.append(&route_rule);
    let identity_row = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    identity_row.set_hexpand(true);
    let profile_column = gtk::Box::new(gtk::Orientation::Vertical, 5);
    profile_column.set_hexpand(true);
    let profile_name_label = gtk::Label::new(Some("PROFILE NAME"));
    profile_name_label.set_xalign(0.0);
    profile_name_label.add_css_class("section-label");
    profile_column.append(&profile_name_label);
    let name_entry = gtk::Entry::builder()
        .placeholder_text("Profile name")
        .hexpand(true)
        .build();
    name_entry.add_css_class("profile-name");
    profile_column.append(&name_entry);
    identity_row.append(&profile_column);

    let device_column = gtk::Box::new(gtk::Orientation::Vertical, 5);
    device_column.set_hexpand(true);
    let device_heading = gtk::Label::new(Some("OUTPUT DEVICE"));
    device_heading.set_xalign(0.0);
    device_heading.add_css_class("section-label");
    device_column.append(&device_heading);
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
    device_drop.add_css_class("device-dropdown");
    device_drop.set_factory(Some(&ellipsized_string_factory(32)));
    device_drop.set_list_factory(Some(&ellipsized_string_factory(48)));
    device_column.set_size_request(1, -1);
    device_column.append(&device_drop);
    let device_state = gtk::Label::new(Some("Not applied"));
    device_state.set_xalign(0.0);
    device_state.add_css_class("dim-label");
    device_state.add_css_class("device-state");
    device_state.set_ellipsize(gtk::pango::EllipsizeMode::End);
    device_column.append(&device_state);
    identity_row.append(&device_column);
    controls_card.append(&identity_row);

    let device_actions = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    device_actions.set_valign(gtk::Align::Center);
    device_actions.set_halign(gtk::Align::Fill);
    let assign_button = gtk::Button::with_label("Apply to output");
    assign_button.add_css_class("suggested-action");
    assign_button.set_hexpand(true);
    let device_bypass = gtk::Switch::new();
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
    let bypass_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    bypass_row.set_halign(gtk::Align::Center);
    bypass_row.append(&gtk::Label::new(Some("FILTERS")));
    bypass_row.append(&device_bypass);
    device_actions.append(&bypass_row);
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
    device_actions.append(&comparison_ui.menu);
    device_actions.append(&assign_button);
    controls_card.append(&device_actions);
    dashboard.append(&controls_card);

    let graph_card = gtk::Box::new(gtk::Orientation::Vertical, 8);
    graph_card.add_css_class("graph-card");
    graph_card.set_hexpand(true);
    let graph_header = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let graph_title = gtk::Label::new(Some("FREQUENCY RESPONSE"));
    graph_title.set_xalign(0.0);
    graph_title.add_css_class("panel-title");
    graph_title.set_hexpand(true);
    graph_header.append(&graph_title);
    let graph_hint = gtk::Label::new(Some(&engine_summary(&engine_status)));
    graph_hint.add_css_class("level-code");
    graph_hint.set_tooltip_text(Some("Drag response points to edit parametric filters"));
    graph_header.append(&graph_hint);
    graph_card.append(&graph_header);
    let graph = graph::response_graph(
        model.analysis.clone(),
        model.document.clone(),
        model.selected_filter.clone(),
    );
    graph.update_property(&[
        gtk::accessible::Property::Label("Frequency response graph"),
        gtk::accessible::Property::Description(
            "Logarithmic 20 hertz to 20 kilohertz response from minus 10 to plus 10 decibels. Use the filter controls below for keyboard editing.",
        ),
    ]);
    let analysis_label = gtk::Label::new(Some("AUTO PREAMP —"));
    analysis_label.set_xalign(0.0);
    analysis_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    analysis_label.add_css_class("level-code");
    let level_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    level_row.add_css_class("level-readout");
    analysis_label.set_hexpand(true);
    level_row.append(&analysis_label);
    let trim = gtk::SpinButton::with_range(-24.0, 24.0, 0.1);
    trim.set_digits(1);
    trim.set_width_chars(5);
    trim.set_tooltip_text(Some(
        "Fine-tune the perceptually matched preamp; safety attenuation still prevents clipping",
    ));
    let trim_label = gtk::Label::new(Some("ADJUST"));
    trim_label.add_css_class("section-label");
    let trim_unit = gtk::Label::new(Some("dB"));
    trim_unit.add_css_class("level-code");
    let trim_controls = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    trim_controls.set_halign(gtk::Align::End);
    trim_controls.append(&trim_label);
    trim_controls.append(&trim);
    trim_controls.append(&trim_unit);
    level_row.append(&trim_controls);
    graph_card.append(&level_row);
    graph_card.append(&graph);
    dashboard.append(&graph_card);
    content.append(&dashboard);

    let view_stack = adw::ViewStack::new();
    let filter_list = gtk::FlowBox::new();
    filter_list.set_selection_mode(gtk::SelectionMode::None);
    filter_list.set_min_children_per_line(2);
    filter_list.set_max_children_per_line(2);
    filter_list.set_column_spacing(12);
    filter_list.set_row_spacing(12);
    filter_list.set_homogeneous(true);
    filter_list.set_activate_on_single_click(false);
    filter_list.set_valign(gtk::Align::Start);
    filter_list.add_css_class("filter-list");
    let narrow_filters = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MaxWidth,
        720.0,
        adw::LengthUnit::Px,
    ));
    narrow_filters.add_setter(
        &filter_list,
        "min-children-per-line",
        Some(&1_u32.to_value()),
    );
    let narrow_route = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MaxWidth,
        1040.0,
        adw::LengthUnit::Px,
    ));
    narrow_route.add_setter(
        &controls_card,
        "orientation",
        Some(&gtk::Orientation::Vertical.to_value()),
    );
    narrow_route.add_setter(
        &route_rule,
        "orientation",
        Some(&gtk::Orientation::Horizontal.to_value()),
    );
    narrow_filters.add_setter(
        &identity_row,
        "orientation",
        Some(&gtk::Orientation::Vertical.to_value()),
    );
    narrow_filters.add_setter(
        &device_actions,
        "orientation",
        Some(&gtk::Orientation::Vertical.to_value()),
    );
    narrow_filters.add_setter(
        &graph_header,
        "orientation",
        Some(&gtk::Orientation::Vertical.to_value()),
    );
    narrow_filters.add_setter(
        &level_row,
        "orientation",
        Some(&gtk::Orientation::Vertical.to_value()),
    );
    narrow_filters.add_setter(&title, "visible", Some(&false.to_value()));
    narrow_filters.add_setter(&global_bypass_label, "visible", Some(&false.to_value()));
    let parametric_stack = gtk::Stack::new();
    parametric_stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    parametric_stack.add_named(&filter_list, Some("visual"));
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
        "Convolution replaces the parametric chain for this profile.",
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
    view_stack.add_titled(&parametric_stack, Some("filters"), "Parametric");
    let switcher = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    switcher.set_halign(gtk::Align::Center);
    switcher.add_css_class("mode-switcher");
    let parametric_mode = gtk::ToggleButton::with_label("PARAMETRIC");
    parametric_mode.set_active(true);
    let convolution_mode = gtk::ToggleButton::with_label("CONVOLUTION");
    convolution_mode.set_group(Some(&parametric_mode));
    parametric_mode.connect_clicked({
        let stack = view_stack.clone();
        move |button| {
            button.set_active(true);
            stack.set_visible_child_name("filters");
        }
    });
    convolution_mode.connect_clicked({
        let stack = view_stack.clone();
        move |button| {
            button.set_active(true);
            stack.set_visible_child_name("convolution");
        }
    });
    view_stack.connect_visible_child_name_notify({
        let parametric_mode = parametric_mode.clone();
        let convolution_mode = convolution_mode.clone();
        move |stack| {
            let convolution = stack.visible_child_name().as_deref() == Some("convolution");
            parametric_mode.set_active(!convolution);
            convolution_mode.set_active(convolution);
        }
    });
    switcher.append(&parametric_mode);
    switcher.append(&convolution_mode);
    let editor_switcher = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    editor_switcher.add_css_class("editor-switcher");
    let visual_mode = gtk::ToggleButton::with_label("VISUAL");
    visual_mode.set_active(true);
    let text_mode = gtk::ToggleButton::with_label("TEXT");
    text_mode.set_group(Some(&visual_mode));
    visual_mode.connect_clicked({
        let stack = parametric_stack.clone();
        move |button| {
            button.set_active(true);
            stack.set_visible_child_name("visual");
        }
    });
    text_mode.connect_clicked({
        let stack = parametric_stack.clone();
        move |button| {
            button.set_active(true);
            stack.set_visible_child_name("text");
        }
    });
    parametric_stack.connect_visible_child_name_notify({
        let visual_mode = visual_mode.clone();
        let text_mode = text_mode.clone();
        move |stack| {
            let text = stack.visible_child_name().as_deref() == Some("text");
            visual_mode.set_active(!text);
            text_mode.set_active(text);
        }
    });
    editor_switcher.append(&visual_mode);
    editor_switcher.append(&text_mode);
    let reset_filters_button = gtk::Button::with_label("RESET FILTERS");
    reset_filters_button.add_css_class("reset-filters");
    reset_filters_button.set_tooltip_text(Some(
        "Flatten every parametric band to 0 dB without changing its frequency or Q",
    ));
    let filter_toolbar = gtk::CenterBox::new();
    filter_toolbar.set_hexpand(true);
    filter_toolbar.set_start_widget(Some(&editor_switcher));
    filter_toolbar.set_center_widget(Some(&switcher));
    filter_toolbar.set_end_widget(Some(&reset_filters_button));
    narrow_filters.add_setter(
        &filter_toolbar,
        "orientation",
        Some(&gtk::Orientation::Vertical.to_value()),
    );
    window.add_breakpoint(narrow_filters);
    window.add_breakpoint(narrow_route);
    let add_filter_button = gtk::Button::with_label("＋  ADD PARAMETRIC BAND");
    add_filter_button.add_css_class("band-add");
    add_filter_button.set_hexpand(true);
    let convolution_ui = ConvolutionUi {
        stack: view_stack.clone(),
        file_name: convolution_file_name,
        file_path: convolution_file_path,
        channel: convolution_channel,
        choose: choose_convolution,
        remove: remove_convolution,
        add_filter: add_filter_button.clone(),
        reset_filters: reset_filters_button.clone(),
        syncing: Rc::new(Cell::new(false)),
    };
    view_stack.connect_visible_child_name_notify({
        let add_filter = add_filter_button.clone();
        let reset_filters = reset_filters_button.clone();
        let editor_switcher = editor_switcher.clone();
        let parametric_stack = parametric_stack.clone();
        move |stack| {
            let parametric = stack.visible_child_name().as_deref() == Some("filters");
            let visual = parametric_stack.visible_child_name().as_deref() == Some("visual");
            editor_switcher.set_visible(parametric);
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
    content.append(&filter_toolbar);
    content.append(&add_filter_button);
    content.append(&view_stack);

    let status = gtk::Label::new(Some("Ready"));
    status.set_xalign(0.0);
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
    gtk::glib::timeout_add_local(std::time::Duration::from_secs(2), {
        let model = model.clone();
        let strings = device_strings.clone();
        let drop = device_drop.clone();
        let state = device_state.clone();
        let assign = assign_button.clone();
        let bypass = device_bypass.clone();
        let engine = global_bypass.clone();
        let engine_readout = graph_hint.clone();
        let comparison_ui = comparison_ui.clone();
        move || {
            if let Ok(devices) = model.client.devices() {
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
                        drop.set_selected(index as u32);
                    }
                }
                update_device_controls(&model, &drop, &state, &assign, &bypass);
                if let Ok(comparisons) = model.client.comparisons() {
                    *model.comparisons.borrow_mut() = comparisons;
                }
                // The candidate rows are an unsaved draft while this popover
                // is open. Rebuilding them on the health poll loses clicks
                // and can destroy the widget currently handling input.
                if !comparison_ui.menu.is_active() {
                    refresh_comparison_ui(&model, &drop, &comparison_ui);
                }
            }
            if let Ok(service_status) = model.client.status()
                && let Some(engine_off) = service_status
                    .get("global_bypass")
                    .and_then(serde_json::Value::as_bool)
            {
                engine_readout.set_text(&engine_summary(&service_status));
                engine.set_sensitive(true);
                model.syncing_engine.set(true);
                engine.set_active(!engine_off);
                model.syncing_engine.set(false);
            } else {
                engine_readout.set_text("AUDIO SERVICE UNAVAILABLE");
                engine.set_sensitive(false);
            }
            gtk::glib::ControlFlow::Continue
        }
    });
    if let Some(row) = profile_list.row_at_index(0) {
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
    device_bypass: &gtk::Switch,
    global_bypass: &gtk::Switch,
    comparison_ui: &Rc<ComparisonUi>,
    trim: &gtk::SpinButton,
) {
    let buffer = text_view.buffer();
    add_filter.connect_clicked({
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
                document.convolutions.clear();
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
            buffer.set_text(&text);
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
                document.convolutions.clear();
                document.graphic_eqs.clear();
                for filter in &mut document.filters {
                    filter.gain_db = 0.0;
                }
                (serialize_profile(document), document.clone())
            };
            buffer.set_text(&text);
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
                        model.loading.set(true);
                        buffer.set_text(&profile.text);
                        model.loading.set(false);
                        *model.document.borrow_mut() = Some(document.clone());
                        model.selected_filter.set(None);
                        rebuild_filter_list(&list, &document, &model, &buffer, &graph);
                        update_convolution_ui(&ui, &document);
                        if let Ok(analysis) = model.client.analyze(&id) {
                            set_analysis_label(&analysis_label, &analysis);
                            *model.analysis.borrow_mut() = Some(analysis);
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
            buffer.set_text(&text);
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
            buffer.set_text(&text);
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
    profile_list.connect_row_selected(move |_, row| {
        if let Some(row) = row {
            reload(row.index() as usize);
        }
    });
    profile_list.connect_row_activated({
        let profile_popover = profile_popover.clone();
        move |_, _| profile_popover.popdown()
    });

    let pending_save: Rc<RefCell<Option<gtk::glib::SourceId>>> = Rc::new(RefCell::new(None));
    buffer.connect_changed({
        let model = model.clone();
        let buffer = buffer.clone();
        let text_view = text_view.clone();
        let filter_list = filter_list.clone();
        let convolution_ui = convolution_ui.clone();
        let name = name_entry.clone();
        let status = status.clone();
        let graph = graph.clone();
        let analysis_label = analysis_label.clone();
        let pending = pending_save.clone();
        let profile_menu = profile_menu.clone();
        move |_| {
            if model.loading.get() {
                return;
            }
            profile_menu.set_label(&name.text());
            if let Some(source) = pending.borrow_mut().take() {
                source.remove();
            }
            let model = model.clone();
            let buffer = buffer.clone();
            let text_view = text_view.clone();
            let filter_list = filter_list.clone();
            let convolution_ui = convolution_ui.clone();
            let name = name.clone();
            let status = status.clone();
            let graph = graph.clone();
            let analysis_label = analysis_label.clone();
            let pending_inner = pending.clone();
            *pending.borrow_mut() = Some(gtk::glib::timeout_add_local_once(
                std::time::Duration::from_millis(300),
                move || {
                    pending_inner.borrow_mut().take();
                    let text = buffer
                        .text(&buffer.start_iter(), &buffer.end_iter(), false)
                        .to_string();
                    let draft = parse_text(name.text(), &text);
                    if draft.is_activatable() {
                        text_view.remove_css_class("invalid-draft");
                    } else {
                        text_view.add_css_class("invalid-draft");
                    }
                    let saved = save_current(
                        &model,
                        &name.text(),
                        &text,
                        &status,
                        &graph,
                        &analysis_label,
                    );
                    if saved {
                        rebuild_filter_list(&filter_list, &draft, &model, &buffer, &graph);
                        update_convolution_ui(&convolution_ui, &draft);
                    }
                },
            ));
        }
    });
    name_entry.connect_changed({
        let model = model.clone();
        let buffer = buffer.clone();
        let name = name_entry.clone();
        let status = status.clone();
        let graph = graph.clone();
        let analysis_label = analysis_label.clone();
        let pending = pending_save.clone();
        let profile_menu = profile_menu.clone();
        move |_| {
            if model.loading.get() {
                return;
            }
            profile_menu.set_label(&name.text());
            if let Some(source) = pending.borrow_mut().take() {
                source.remove();
            }
            let model = model.clone();
            let buffer = buffer.clone();
            let name = name.clone();
            let status = status.clone();
            let graph = graph.clone();
            let analysis_label = analysis_label.clone();
            let pending_inner = pending.clone();
            *pending.borrow_mut() = Some(gtk::glib::timeout_add_local_once(
                std::time::Duration::from_millis(300),
                move || {
                    pending_inner.borrow_mut().take();
                    let text = buffer
                        .text(&buffer.start_iter(), &buffer.end_iter(), false)
                        .to_string();
                    save_current(
                        &model,
                        &name.text(),
                        &text,
                        &status,
                        &graph,
                        &analysis_label,
                    );
                },
            ));
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
        move |drop| {
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
            }
        }
    });
    trim.connect_value_changed({
        let model = model.clone();
        let buffer = buffer.clone();
        let name = name_entry.clone();
        let status = status.clone();
        let graph = graph.clone();
        let analysis_label = analysis_label.clone();
        move |spin| {
            if model.loading.get() {
                return;
            }
            model.manual_trim.set(spin.value());
            let text = buffer
                .text(&buffer.start_iter(), &buffer.end_iter(), false)
                .to_string();
            save_current(
                &model,
                &name.text(),
                &text,
                &status,
                &graph,
                &analysis_label,
            );
        }
    });

    let drag_state: Rc<RefCell<Option<(usize, f64, f64)>>> = Rc::new(RefCell::new(None));
    let gesture = gtk::GestureDrag::new();
    gesture.connect_drag_begin({
        let model = model.clone();
        let state = drag_state.clone();
        let graph = graph.clone();
        move |_, x, y| {
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
                    graph.queue_draw();
                }
            }
        }
    });
    gesture.connect_drag_update({
        let model = model.clone();
        let state = drag_state.clone();
        let graph = graph.clone();
        let analysis_label = analysis_label.clone();
        move |_, dx, dy| {
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
                document.convolutions.clear();
                document.graphic_eqs.clear();
                let Some(filter) = document.filters.get_mut(index) else {
                    return;
                };
                filter.frequency =
                    (start_frequency * (1000.0_f64).powf(dx / width)).clamp(20.0, 20_000.0);
                filter.gain_db = graph::gain_after_drag(start_gain, dy, height);
                analyze_profile_preview(document, model.sample_rate.get(), model.manual_trim.get())
            };
            set_analysis_label(&analysis_label, &preview);
            *model.analysis.borrow_mut() = Some(preview);
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
            if state.borrow_mut().take().is_some()
                && let Some(snapshot) = model.document.borrow().as_ref().cloned()
            {
                rebuild_filter_list(&list, &snapshot, &model, &buffer, &graph);
                update_convolution_ui(&convolution_ui, &snapshot);
                buffer.set_text(&serialize_profile(&snapshot));
            }
        }
    });
    graph.add_controller(gesture);
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
    model.loading.set(true);
    *model.current_id.borrow_mut() = Some(profile.id.clone());
    model.selected_filter.set(None);
    profile_menu.set_label(&profile.name);
    name.set_text(&profile.name);
    buffer.set_text(&profile.text);
    model.manual_trim.set(profile.manual_trim_db);
    trim.set_value(profile.manual_trim_db);
    let document = parse_text(&profile.name, &profile.text);
    *model.document.borrow_mut() = Some(document.clone());
    rebuild_filter_list(filter_list, &document, model, buffer, graph);
    update_convolution_ui(convolution_ui, &document);
    if let Ok(analysis) = model.client.analyze(&profile.id) {
        set_analysis_label(analysis_label, &analysis);
        *model.analysis.borrow_mut() = Some(analysis);
        graph.queue_draw();
    }
    model.loading.set(false);
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
        ui.file_path
            .set_text(&convolution.path.display().to_string());
        ui.channel.set_selected(match convolution.channels {
            ChannelSelection::All => 0,
            ChannelSelection::Left => 1,
            ChannelSelection::Right => 2,
        });
        ui.remove.set_sensitive(true);
        ui.stack.set_visible_child_name("convolution");
    } else {
        ui.file_name.set_text("No impulse response selected");
        ui.file_path
            .set_text("WAV, FLAC, AIFF, or OGG · up to 10 seconds");
        ui.channel.set_selected(0);
        ui.remove.set_sensitive(false);
        ui.stack.set_visible_child_name("filters");
    }
    let parametric = profile.convolutions.is_empty();
    ui.add_filter.set_visible(parametric);
    ui.reset_filters.set_visible(true);
    ui.reset_filters.set_sensitive(parametric);
    ui.reset_filters
        .set_opacity(if parametric { 1.0 } else { 0.0 });
    ui.syncing.set(false);
}

fn rebuild_filter_list(
    list: &gtk::FlowBox,
    profile: &ProfileDocument,
    model: &Rc<Model>,
    buffer: &gtk::TextBuffer,
    graph: &gtk::DrawingArea,
) {
    list.remove_all();

    let mut filter_button_group: Option<gtk::ToggleButton> = None;
    for (index, filter) in profile.filters.iter().enumerate() {
        let card = gtk::Box::new(gtk::Orientation::Vertical, 8);
        card.add_css_class("filter-card");
        card.set_hexpand(true);

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let select = gtk::ToggleButton::with_label(&format!("{:02}", index + 1));
        select.add_css_class("filter-index");
        if let Some(group) = &filter_button_group {
            select.set_group(Some(group));
        } else {
            filter_button_group = Some(select.clone());
        }
        select.set_active(model.selected_filter.get() == Some(index));
        select.set_tooltip_text(Some("Select this point on the response graph"));
        header.append(&select);

        let band_title = gtk::Label::new(Some(&format_band_frequency(filter.frequency)));
        band_title.add_css_class("band-frequency");
        header.append(&band_title);
        let enabled = gtk::Switch::new();
        enabled.set_active(filter.enabled);
        enabled.set_valign(gtk::Align::Center);
        enabled.add_css_class("filter-switch");
        enabled.update_property(&[gtk::accessible::Property::Label(&format!(
            "Enable filter at {}",
            format_band_frequency(filter.frequency)
        ))]);

        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        header.append(&spacer);
        header.append(&enabled);

        let kind_names = gtk::StringList::new(&[
            "Peak",
            "Low shelf",
            "High shelf",
            "Low pass",
            "High pass",
            "Band pass",
            "Notch",
            "All pass",
        ]);
        let kind_drop = gtk::DropDown::new(Some(kind_names), gtk::Expression::NONE);
        kind_drop.set_selected(match filter.kind {
            FilterKind::Peaking => 0,
            FilterKind::LowShelf => 1,
            FilterKind::HighShelf => 2,
            FilterKind::LowPass => 3,
            FilterKind::HighPass => 4,
            FilterKind::BandPass => 5,
            FilterKind::Notch => 6,
            FilterKind::AllPass => 7,
        });
        let channel_names = gtk::StringList::new(&["ALL", "L", "R"]);
        let channel_drop = gtk::DropDown::new(Some(channel_names), gtk::Expression::NONE);
        channel_drop.set_selected(match filter.channels {
            ChannelSelection::All => 0,
            ChannelSelection::Left => 1,
            ChannelSelection::Right => 2,
        });
        let move_up = gtk::Button::from_icon_name("go-up-symbolic");
        move_up.add_css_class("flat");
        move_up.set_sensitive(index > 0);
        move_up.set_tooltip_text(Some("Move band up"));
        let move_down = gtk::Button::from_icon_name("go-down-symbolic");
        move_down.add_css_class("flat");
        move_down.set_sensitive(index + 1 < profile.filters.len());
        move_down.set_tooltip_text(Some("Move band down"));
        let remove = gtk::Button::from_icon_name("user-trash-symbolic");
        remove.add_css_class("flat");
        remove.add_css_class("destructive-action");
        remove.set_tooltip_text(Some("Delete band"));
        header.append(&move_up);
        header.append(&move_down);
        header.append(&remove);
        card.append(&header);

        let options = adw::WrapBox::builder()
            .orientation(gtk::Orientation::Horizontal)
            .child_spacing(8)
            .line_spacing(8)
            .line_homogeneous(true)
            .build();
        let kind_field = gtk::Box::new(gtk::Orientation::Vertical, 5);
        kind_field.set_size_request(140, -1);
        let kind_label = gtk::Label::new(Some("FILTER TYPE"));
        kind_label.set_xalign(0.0);
        kind_label.add_css_class("section-label");
        kind_field.append(&kind_label);
        kind_drop.set_hexpand(true);
        kind_field.append(&kind_drop);
        options.append(&kind_field);
        let channel_field = gtk::Box::new(gtk::Orientation::Vertical, 5);
        channel_field.set_size_request(110, -1);
        let channel_label = gtk::Label::new(Some("CHANNEL"));
        channel_label.set_xalign(0.0);
        channel_label.add_css_class("section-label");
        channel_field.append(&channel_label);
        channel_drop.set_hexpand(true);
        channel_field.append(&channel_drop);
        options.append(&channel_field);
        card.append(&options);

        let parameters = adw::WrapBox::builder()
            .orientation(gtk::Orientation::Horizontal)
            .child_spacing(8)
            .line_spacing(8)
            .line_homogeneous(true)
            .build();
        let frequency = gtk::SpinButton::with_range(20.0, 20_000.0, 1.0);
        frequency.set_value(filter.frequency);
        frequency.set_digits(1);
        frequency.set_width_chars(6);
        frequency.set_hexpand(true);
        frequency.add_css_class("band-readout");
        let gain = gtk::SpinButton::with_range(-60.0, 60.0, 0.1);
        gain.set_value(filter.gain_db);
        gain.set_digits(2);
        gain.set_width_chars(5);
        gain.set_hexpand(true);
        gain.add_css_class("band-readout");
        let q = gtk::SpinButton::with_range(0.01, 1000.0, 0.01);
        q.set_value(filter.q);
        q.set_digits(3);
        q.set_width_chars(5);
        q.set_hexpand(true);
        q.add_css_class("band-readout");
        parameters.append(&spin_field("CENTER", "Hz", &frequency));
        parameters.append(&spin_field("GAIN", "dB", &gain));
        parameters.append(&spin_field("Q / WIDTH", "Q", &q));
        card.append(&parameters);

        select.connect_toggled({
            let model = model.clone();
            let graph = graph.clone();
            move |button| {
                if button.is_active() {
                    model.selected_filter.set(Some(index));
                    graph.queue_draw();
                }
            }
        });
        enabled.connect_active_notify({
            let model = model.clone();
            let buffer = buffer.clone();
            let graph = graph.clone();
            move |switch| {
                let enabled = switch.is_active();
                // Response analysis and text serialization can be noticeable
                // for a large bank. Defer them until after GTK has painted the
                // switch, just like the output-level Filters control.
                gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(40), {
                    let model = model.clone();
                    let buffer = buffer.clone();
                    let graph = graph.clone();
                    move || {
                        update_filter(&model, &buffer, &graph, index, |filter| {
                            filter.enabled = enabled
                        });
                    }
                });
            }
        });
        kind_drop.connect_selected_notify({
            let model = model.clone();
            let buffer = buffer.clone();
            let graph = graph.clone();
            move |drop| {
                update_filter(&model, &buffer, &graph, index, |filter| {
                    filter.kind = match drop.selected() {
                        1 => FilterKind::LowShelf,
                        2 => FilterKind::HighShelf,
                        3 => FilterKind::LowPass,
                        4 => FilterKind::HighPass,
                        5 => FilterKind::BandPass,
                        6 => FilterKind::Notch,
                        7 => FilterKind::AllPass,
                        _ => FilterKind::Peaking,
                    }
                })
            }
        });
        channel_drop.connect_selected_notify({
            let model = model.clone();
            let buffer = buffer.clone();
            let graph = graph.clone();
            move |drop| {
                update_filter(&model, &buffer, &graph, index, |filter| {
                    filter.channels = match drop.selected() {
                        1 => ChannelSelection::Left,
                        2 => ChannelSelection::Right,
                        _ => ChannelSelection::All,
                    }
                })
            }
        });
        frequency.connect_value_changed({
            let model = model.clone();
            let buffer = buffer.clone();
            let graph = graph.clone();
            let band_title = band_title.clone();
            move |spin| {
                band_title.set_text(&format_band_frequency(spin.value()));
                update_filter(&model, &buffer, &graph, index, |filter| {
                    filter.frequency = spin.value()
                })
            }
        });
        gain.connect_value_changed({
            let model = model.clone();
            let buffer = buffer.clone();
            let graph = graph.clone();
            move |spin| {
                update_filter(&model, &buffer, &graph, index, |filter| {
                    filter.gain_db = spin.value()
                })
            }
        });
        q.connect_value_changed({
            let model = model.clone();
            let buffer = buffer.clone();
            let graph = graph.clone();
            move |spin| {
                update_filter(&model, &buffer, &graph, index, |filter| {
                    filter.q = spin.value()
                })
            }
        });

        move_up.connect_clicked({
            let model = model.clone();
            let buffer = buffer.clone();
            let graph = graph.clone();
            let list = list.clone();
            move |_| {
                if index == 0 {
                    return;
                }
                let snapshot = {
                    let mut document = model.document.borrow_mut();
                    let Some(document) = document.as_mut() else {
                        return;
                    };
                    document.filters.swap(index, index - 1);
                    model.selected_filter.set(Some(index - 1));
                    document.clone()
                };
                buffer.set_text(&serialize_profile(&snapshot));
                rebuild_filter_list(&list, &snapshot, &model, &buffer, &graph);
                graph.queue_draw();
            }
        });
        move_down.connect_clicked({
            let model = model.clone();
            let buffer = buffer.clone();
            let graph = graph.clone();
            let list = list.clone();
            move |_| {
                let snapshot = {
                    let mut document = model.document.borrow_mut();
                    let Some(document) = document.as_mut() else {
                        return;
                    };
                    if index + 1 >= document.filters.len() {
                        return;
                    }
                    document.filters.swap(index, index + 1);
                    model.selected_filter.set(Some(index + 1));
                    document.clone()
                };
                buffer.set_text(&serialize_profile(&snapshot));
                rebuild_filter_list(&list, &snapshot, &model, &buffer, &graph);
                graph.queue_draw();
            }
        });
        remove.connect_clicked({
            let model = model.clone();
            let buffer = buffer.clone();
            let graph = graph.clone();
            let list = list.clone();
            move |_| {
                let snapshot = {
                    let mut document = model.document.borrow_mut();
                    let Some(document) = document.as_mut() else {
                        return;
                    };
                    if index >= document.filters.len() {
                        return;
                    }
                    document.filters.remove(index);
                    model.selected_filter.set(
                        (!document.filters.is_empty())
                            .then_some(index.min(document.filters.len() - 1)),
                    );
                    document.clone()
                };
                buffer.set_text(&serialize_profile(&snapshot));
                rebuild_filter_list(&list, &snapshot, &model, &buffer, &graph);
                graph.queue_draw();
            }
        });
        list.insert(&card, -1);
    }
    for graphic in &profile.graphic_eqs {
        let channel = match graphic.channels {
            ChannelSelection::All => "ALL",
            ChannelSelection::Left => "L",
            ChannelSelection::Right => "R",
        };
        let row = adw::ActionRow::builder()
            .title(format!("{channel} · GraphicEQ"))
            .subtitle(format!("{} response points", graphic.points.len()))
            .build();
        row.add_css_class("filter-card");
        list.insert(&row, -1);
    }
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
        document.convolutions.clear();
        document.graphic_eqs.clear();
        let Some(filter) = document.filters.get_mut(index) else {
            return;
        };
        update(filter);
        (
            serialize_profile(document),
            analyze_profile_preview(document, model.sample_rate.get(), model.manual_trim.get()),
        )
    };
    *model.analysis.borrow_mut() = Some(preview);
    graph.queue_draw();
    buffer.set_text(&text);
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
    device_bypass: &gtk::Switch,
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
    bypass: &gtk::Switch,
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
        state.set_text(&format!("Active · {name}"));
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

fn engine_summary(status: &serde_json::Value) -> String {
    let Some(active) = status.pointer("/engine/active/0") else {
        return "NATIVE DSP · IDLE".into();
    };
    let rate = active
        .get("sample_rate")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(48_000) as f64
        / 1000.0;
    let latency = active
        .get("latency_ms")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or_default();
    let cpu = active
        .get("cpu_percent_of_deadline")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or_default();
    if active
        .get("cpu_warning")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        format!("NATIVE DSP · {rate:.1} kHz · {latency:.1} ms · CPU {cpu:.0}% ⚠")
    } else {
        format!("NATIVE DSP · {rate:.1} kHz · {latency:.1} ms")
    }
}

fn install_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(
        r#"
        @define-color accent_bg_color #e64b2f;
        @define-color accent_color #ff7358;
        @define-color accent_fg_color #fff8f4;

        window,
        .instrument-root {
            background-color: #101112;
            color: #ecece8;
            font-size: 13px;
        }

        .instrument-header {
            background-color: #141617;
            border-bottom: 1px solid #272a2b;
            box-shadow: 0 8px 24px rgba(0, 0, 0, 0.22);
        }

        .brand-title title,
        .brand-title subtitle,
        .header-data,
        .panel-title,
        .section-label,
        .console-status {
            font-family: monospace;
        }

        .brand-title title {
            font-weight: 800;
            letter-spacing: 1px;
        }

        .brand-title subtitle {
            font-size: 9px;
            opacity: 0.46;
        }

        .header-data {
            font-size: 9px;
            font-weight: 700;
            opacity: 0.54;
        }

        .control-card,
        .graph-card,
        .source-card {
            background-color: #1a1c1d;
            border: 1px solid #292c2d;
            border-radius: 24px;
            padding: 18px;
            box-shadow: 0 14px 28px rgba(0, 0, 0, 0.22);
        }

        .control-card { padding: 13px 16px; }
        .graph-card { padding: 20px 18px 10px 18px; }

        .level-readout {
            border-bottom: 1px solid #292c2d;
            padding: 4px 2px 9px 2px;
        }

        .level-code {
            color: #929798;
            font-family: monospace;
            font-size: 10px;
        }

        .panel-title {
            font-size: 12px;
            font-weight: 700;
            letter-spacing: 0.7px;
        }

        .profile-menu > button {
            background-color: #242729;
            border: 1px solid #303436;
            border-radius: 999px;
            font-family: monospace;
            font-weight: 700;
            min-width: 150px;
            padding: 7px 16px;
        }

        .profile-add {
            border-radius: 999px;
            font-family: monospace;
            font-size: 9px;
            font-weight: 700;
            padding: 7px 12px;
        }

        .comparison-menu > button {
            border-radius: 999px;
            font-family: monospace;
            font-size: 10px;
            font-weight: 700;
            padding: 7px 12px;
        }

        .comparison-candidate {
            background-color: #1b1d1e;
            border: 1px solid #2d3031;
            border-radius: 10px;
            padding: 6px 8px;
            font-family: monospace;
            box-shadow: none;
        }

        .comparison-candidate:checked {
            background-color: rgba(230, 75, 47, 0.10);
            border-color: rgba(230, 75, 47, 0.72);
            color: #f2f1ec;
        }

        .comparison-listen {
            background-color: #1b1d1e;
            border: 1px solid #2d3031;
            border-radius: 10px;
            padding: 8px 10px;
            font-family: monospace;
            box-shadow: none;
        }

        .comparison-listen.active {
            background-color: #e64b2f;
            border-color: #ef5a3e;
            color: #fff8f4;
        }

        .comparison-listen.active .comparison-mark {
            color: #fff8f4;
        }

        .comparison-mark {
            color: #e66a53;
            font-family: monospace;
            font-size: 9px;
            font-weight: 700;
        }

        .editor-switcher button {
            background: transparent;
            border: 0;
            border-radius: 0;
            border-bottom: 2px solid transparent;
            color: #8f9394;
            font-family: monospace;
            font-size: 10px;
            font-weight: 700;
            padding: 8px 12px;
        }

        .editor-switcher button:checked {
            background: transparent;
            border-bottom-color: #e64b2f;
            color: #f1f1ec;
        }

        .code-editor {
            background-color: #151718;
            border: 1px solid #292c2d;
            border-radius: 22px;
        }

        textview.invalid-draft {
            color: #ff9b88;
        }

        .profile-name {
            font-family: monospace;
            font-size: 16px;
            font-weight: 500;
        }

        .section-label {
            font-size: 9px;
            font-weight: 700;
            letter-spacing: 0.6px;
            opacity: 0.46;
        }

        .device-state {
            color: #89ad73;
            font-family: monospace;
            font-size: 11px;
        }

        entry,
        spinbutton,
        dropdown > button {
            background-color: #242729;
            color: #efefeb;
            border: 1px solid #303436;
            border-radius: 12px;
            box-shadow: none;
        }

        entry:focus,
        spinbutton:focus-within,
        dropdown > button:focus {
            border-color: #e64b2f;
            box-shadow: 0 0 0 1px rgba(230, 75, 47, 0.42);
        }

        button {
            border-radius: 12px;
        }

        button.suggested-action {
            background-color: #e64b2f;
            color: #fff8f4;
            border-color: #ef5a3e;
            font-family: monospace;
            font-weight: 700;
        }

        button.suggested-action:disabled {
            background-color: #2b2e30;
            color: #8e9293;
            border-color: #34383a;
        }

        switch:checked,
        togglebutton:checked {
            background-color: #e64b2f;
            color: #fff8f4;
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
            background-color: #2b2e2f;
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

        .filter-card {
            background-color: #191b1c;
            border: 1px solid #292c2d;
            border-radius: 22px;
            padding: 8px 10px;
            box-shadow: 0 10px 22px rgba(0, 0, 0, 0.18);
        }

        .convolution-card {
            background-color: #191b1c;
            border: 1px solid #292c2d;
            border-radius: 22px;
            padding: 18px;
            box-shadow: 0 10px 22px rgba(0, 0, 0, 0.18);
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
            background-color: #e64b2f;
            border-color: #ef5a3e;
        }

        .heading {
            font-family: monospace;
            font-weight: 700;
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
            color: #e66a53;
            border: 1px dashed #4b302b;
            border-radius: 18px;
            font-family: monospace;
            font-weight: 700;
            padding: 10px 14px;
        }

        .band-add:hover {
            background-color: rgba(230, 75, 47, 0.08);
            border-color: #8a4437;
        }

        .reset-filters {
            color: #9a9e9f;
            font-family: monospace;
            font-size: 10px;
            font-weight: 700;
            padding: 7px 12px;
        }

        .mode-switcher button {
            min-width: 130px;
            min-height: 28px;
            background: transparent;
            border: none;
            border-bottom: 2px solid transparent;
            border-radius: 0;
            box-shadow: none;
            color: #858a8b;
            font-family: monospace;
            font-size: 11px;
            font-weight: 700;
            padding: 5px 12px 7px 12px;
        }

        .mode-switcher button:hover {
            background: transparent;
            color: #c6c9c8;
        }

        .mode-switcher button:checked {
            background: transparent;
            border-bottom-color: #e64b2f;
            box-shadow: none;
            color: #ecece8;
        }

        .device-dropdown,
        .device-dropdown > button {
            min-width: 0;
        }

        .console-status {
            color: #8f9495;
            font-size: 10px;
            padding: 2px 6px;
        }

        popover > contents {
            background-color: #191b1c;
            border: 1px solid #303334;
            border-radius: 20px;
            box-shadow: 0 16px 38px rgba(0, 0, 0, 0.38);
        }

        list.boxed-list,
        list.boxed-list > row {
            background-color: #222527;
        }

        scrollbar slider {
            background-color: #4a4e50;
            border-radius: 999px;
            min-width: 5px;
            min-height: 5px;
        }

        textview {
            background-color: #151718;
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

fn save_current(
    model: &Rc<Model>,
    name: &str,
    text: &str,
    status: &gtk::Label,
    graph: &gtk::DrawingArea,
    analysis_label: &gtk::Label,
) -> bool {
    let Some(id) = model.current_id.borrow().clone() else {
        return false;
    };
    let parsed = parse_text(name, text);
    if !parsed.is_activatable() {
        status.set_text(
            &parsed
                .diagnostics
                .first()
                .map(|d| format!("Line {}: {}", d.line, d.message))
                .unwrap_or_else(|| "Invalid profile".into()),
        );
        return false;
    }
    match model.client.put(&id, name, text, model.manual_trim.get()) {
        Ok(profile) => {
            if let Some(existing) = model
                .profiles
                .borrow_mut()
                .iter_mut()
                .find(|item| item.id == id)
            {
                *existing = profile;
            }
            *model.document.borrow_mut() = Some(parsed);
            status.set_text("Saved and applied");
            if let Ok(analysis) = model.client.analyze(&id) {
                set_analysis_label(analysis_label, &analysis);
                *model.analysis.borrow_mut() = Some(analysis);
                graph.queue_draw();
            }
            true
        }
        Err(error) => {
            status.set_text(&error.to_string());
            false
        }
    }
}

fn set_analysis_label(label: &gtk::Label, analysis: &ProfileAnalysis) {
    let left_preamp = analysis.left.preamp_db + analysis.effective_gain_db;
    let right_preamp = analysis.right.preamp_db + analysis.effective_gain_db;
    let preamp = if (left_preamp - right_preamp).abs() < 0.01 {
        format!("AUTO PREAMP {left_preamp:+.2} dB")
    } else {
        format!("AUTO PREAMP  L {left_preamp:+.2} dB  /  R {right_preamp:+.2} dB")
    };
    label.set_text(&format!(
        "{preamp}   •   MATCH {:+.2} dB   •   SAFETY {:+.2} dB{}",
        analysis.match_gain_db,
        analysis.safety_attenuation_db,
        if analysis.headroom_limited {
            "   •   HEADROOM LIMITED"
        } else {
            ""
        }
    ));
}

fn refresh_profiles(model: &Rc<Model>, list: &gtk::ListBox) {
    if let Ok(profiles) = model.client.profiles() {
        *model.profiles.borrow_mut() = profiles;
        repopulate_profiles(list, model);
    }
}
fn repopulate_profiles(list: &gtk::ListBox, model: &Rc<Model>) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
    for profile in model.profiles.borrow().iter() {
        list.append(
            &adw::ActionRow::builder()
                .title(&profile.name)
                .subtitle(if profile.activatable {
                    "Ready"
                } else {
                    "Needs attention"
                })
                .build(),
        );
    }
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
    use super::{PendingDeviceBypass, displayed_device_bypass};

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
}
