mod client;
mod graph;

use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use massiveeq_core::{
    ChannelSelection, DeviceInfo, Filter, FilterKind, ProfileAnalysis, ProfileDocument,
    ProfileInfo, analyze_profile_preview, parse_text, serialize_profile,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use client::Client;

struct Model {
    client: Client,
    profiles: RefCell<Vec<ProfileInfo>>,
    devices: RefCell<Vec<DeviceInfo>>,
    current_id: RefCell<Option<String>>,
    document: Rc<RefCell<Option<ProfileDocument>>>,
    analysis: Rc<RefCell<Option<ProfileAnalysis>>>,
    selected_filter: Rc<Cell<Option<usize>>>,
    manual_trim: Cell<f64>,
    sample_rate: Cell<f64>,
    loading: Cell<bool>,
    syncing_device: Cell<bool>,
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
    let engine_status = client.status().unwrap_or_default();
    let active_sample_rate = engine_status
        .pointer("/engine/active/0/sample_rate")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(48_000) as f64;
    let model = Rc::new(Model {
        profiles: RefCell::new(client.profiles().unwrap_or_default()),
        devices: RefCell::new(client.devices().unwrap_or_default()),
        client,
        current_id: RefCell::new(None),
        document: Rc::new(RefCell::new(None)),
        analysis: Rc::new(RefCell::new(None)),
        selected_filter: Rc::new(Cell::new(None)),
        manual_trim: Cell::new(0.0),
        sample_rate: Cell::new(active_sample_rate),
        loading: Cell::new(false),
        syncing_device: Cell::new(false),
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

    let global_bypass = gtk::Switch::new();
    global_bypass.set_valign(gtk::Align::Center);
    global_bypass.set_active(
        model
            .client
            .status()
            .ok()
            .and_then(|value| value.get("global_bypass").and_then(|value| value.as_bool()))
            .unwrap_or(false),
    );
    global_bypass.set_tooltip_text(Some("Bypass every MassiveEQ filter"));
    header.pack_end(&global_bypass);
    let global_bypass_label = gtk::Label::new(Some("ENGINE BYPASS"));
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
            .is_some_and(|device| device.bypassed),
    );
    let bypass_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    bypass_row.set_halign(gtk::Align::Center);
    bypass_row.append(&gtk::Label::new(Some("Bypass output")));
    bypass_row.append(&device_bypass);
    device_actions.append(&bypass_row);
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
    narrow_filters.add_setter(
        &controls_card,
        "orientation",
        Some(&gtk::Orientation::Vertical.to_value()),
    );
    narrow_filters.add_setter(
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
    view_stack.add_titled(&filter_list, Some("filters"), "Parametric");
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
    let switcher = adw::ViewSwitcher::new();
    switcher.set_policy(adw::ViewSwitcherPolicy::Wide);
    switcher.set_stack(Some(&view_stack));
    switcher.set_halign(gtk::Align::Center);
    switcher.add_css_class("mode-switcher");
    let reset_filters_button = gtk::Button::with_label("RESET FILTERS");
    reset_filters_button.add_css_class("reset-filters");
    reset_filters_button.set_tooltip_text(Some(
        "Flatten every parametric band to 0 dB without changing its frequency or Q",
    ));
    let filter_toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    switcher.set_hexpand(true);
    filter_toolbar.append(&switcher);
    filter_toolbar.append(&reset_filters_button);
    narrow_filters.add_setter(
        &filter_toolbar,
        "orientation",
        Some(&gtk::Orientation::Vertical.to_value()),
    );
    window.add_breakpoint(narrow_filters);
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
        move |stack| {
            let parametric = stack.visible_child_name().as_deref() == Some("filters");
            add_filter.set_visible(parametric);
            reset_filters.set_visible(parametric);
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
        &trim,
    );
    gtk::glib::timeout_add_local(std::time::Duration::from_secs(2), {
        let model = model.clone();
        let strings = device_strings.clone();
        let drop = device_drop.clone();
        let state = device_state.clone();
        let assign = assign_button.clone();
        let bypass = device_bypass.clone();
        move || {
            if let Ok(devices) = model.client.devices() {
                let selected_key = model
                    .devices
                    .borrow()
                    .get(drop.selected() as usize)
                    .map(|device| device.key.as_storage_key());
                let names = devices
                    .iter()
                    .map(|device| device.description.as_str())
                    .collect::<Vec<_>>();
                strings.splice(0, strings.n_items(), &names);
                *model.devices.borrow_mut() = devices;
                if let Some(key) = selected_key
                    && let Some(index) = model
                        .devices
                        .borrow()
                        .iter()
                        .position(|device| device.key.as_storage_key() == key)
                {
                    drop.set_selected(index as u32);
                }
                update_device_controls(&model, &drop, &state, &assign, &bypass);
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
    profile_list.connect_row_selected({
        let profile_popover = profile_popover.clone();
        move |_, row| {
            if let Some(row) = row {
                reload(row.index() as usize);
                profile_popover.popdown();
            }
        }
    });

    let pending_save: Rc<RefCell<Option<gtk::glib::SourceId>>> = Rc::new(RefCell::new(None));
    buffer.connect_changed({
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

    add.connect_clicked({
        let model = model.clone();
        let list = profile_list.clone();
        move |_| {
            if model.client.create("Untitled Profile").is_ok() {
                refresh_profiles(&model, &list);
                if let Some(row) = list.row_at_index((model.profiles.borrow().len() - 1) as i32) {
                    list.select_row(Some(&row));
                }
            }
        }
    });
    delete.connect_clicked({
        let model = model.clone();
        let list = profile_list.clone();
        move |_| {
            if let Some(id) = model.current_id.borrow().clone() {
                let _ = model.client.delete(&id);
                refresh_profiles(&model, &list);
                if let Some(row) = list.row_at_index(0) {
                    list.select_row(Some(&row));
                }
            }
        }
    });
    import.connect_clicked({
        let model = model.clone();
        let list = profile_list.clone();
        let window = window.clone();
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
            chooser.open(Some(&window), gtk::gio::Cancellable::NONE, move |result| {
                if let Ok(file) = result
                    && let Some(path) = file.path()
                {
                    let _ = model.client.import(&path.display().to_string());
                    refresh_profiles(&model, &list);
                }
            });
        }
    });
    duplicate.connect_clicked({
        let model = model.clone();
        let list = profile_list.clone();
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
            if let Ok(created) = model.client.create(&format!("{} Copy", source.name)) {
                let _ = model.client.put(
                    &created.id,
                    &created.name,
                    &source.text,
                    source.manual_trim_db,
                );
                refresh_profiles(&model, &list);
            }
        }
    });
    export.connect_clicked({
        let model = model.clone();
        let window = window.clone();
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
            dialog.save(Some(&window), gtk::gio::Cancellable::NONE, move |result| {
                if let Ok(file) = result
                    && let Some(path) = file.path()
                {
                    let _ = model
                        .client
                        .export(&profile.id, &path.display().to_string());
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
            let unassigning =
                !engine_has_error && device.assigned_profile.as_deref() == Some(&selected_profile);
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
        move |drop| {
            update_device_controls(&model, drop, &state, &assign, &bypass);
        }
    });
    device_bypass.connect_active_notify({
        let model = model.clone();
        let drop = device_drop.clone();
        let state = device_state.clone();
        let assign = assign.clone();
        move |switch| {
            if model.syncing_device.get() {
                return;
            }
            if let Some(device) = model.devices.borrow().get(drop.selected() as usize) {
                let _ = model
                    .client
                    .set_device_bypass(&device.key.as_storage_key(), switch.is_active());
                if let Ok(devices) = model.client.devices() {
                    *model.devices.borrow_mut() = devices;
                }
                update_device_controls(&model, &drop, &state, &assign, switch);
            }
        }
    });
    global_bypass.connect_active_notify({
        let model = model.clone();
        move |switch| {
            let _ = model.client.set_global_bypass(switch.is_active());
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
                filter.gain_db = (start_gain - dy / height * 36.0).clamp(-24.0, 24.0);
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
    ui.reset_filters.set_visible(parametric);
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
                update_filter(&model, &buffer, &graph, index, |filter| {
                    filter.enabled = switch.is_active()
                })
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
        return;
    };

    model.syncing_device.set(true);
    if bypass.is_active() != device.bypassed {
        bypass.set_active(device.bypassed);
    }
    model.syncing_device.set(false);

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

    if let Some(error) = engine_error {
        state.set_text(&format!("Last valid audio retained · {error}"));
        assign.set_label("Retry profile");
        assign.set_sensitive(model.current_id.borrow().is_some());
    } else if device.channels > 2 {
        state.set_text("Unsupported surround output · bypassed");
        assign.set_label("Unavailable");
        assign.set_sensitive(false);
    } else if device.bypassed {
        state.set_text(&match assigned_name {
            Some(name) => format!("Assigned to {name} · currently bypassed"),
            None => "Not applied · currently bypassed".to_owned(),
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
            font-family: monospace;
            font-size: 11px;
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
) {
    let Some(id) = model.current_id.borrow().clone() else {
        return;
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
        return;
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
        }
        Err(error) => status.set_text(&error.to_string()),
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
