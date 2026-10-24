//! The measurement import workspace. Draft changes stay here until Apply.
use crate::client::Client;
use gtk4 as gtk;
use libadwaita::{self as adw, prelude::*};
use massiveeq_core::{
    Curves, MAX_MEASUREMENT_POINTS, MAX_MEASUREMENT_TAKES, MeasurementChannels, MeasurementEar,
    MeasurementOutput, MeasurementSet, MeasurementTake, alignment_level, curve_at,
    measurement_ear_hint,
};
use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
};

struct Input {
    name: String,
    ear: Option<MeasurementEar>,
    points: Vec<massiveeq_core::CurvePoint>,
}

struct Manager {
    window: gtk::glib::WeakRef<adw::Window>,
    client: Client,
    profile: String,
    inputs: RefCell<Vec<Input>>,
    output: Cell<MeasurementOutput>,
    align: Cell<bool>,
    busy: Cell<bool>,
    rows: gtk::Box,
    summary: gtk::Label,
    error: gtk::Label,
    preview: gtk::DrawingArea,
    averages: RefCell<Option<MeasurementChannels>>,
    apply: gtk::Button,
    cancel: gtk::Button,
    add: gtk::Button,
    options: gtk::Box,
    applied: Rc<dyn Fn(Curves)>,
}

fn background<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
    done: impl FnOnce(Result<T, String>) + 'static,
) {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let _ = tx.send(work());
    });
    let mut done = Some(done);
    gtk::glib::timeout_add_local(std::time::Duration::from_millis(30), move || {
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return gtk::glib::ControlFlow::Continue,
            Err(_) => Err("The measurement operation stopped. Try again.".into()),
        };
        if let Some(done) = done.take() {
            done(result);
        }
        gtk::glib::ControlFlow::Break
    });
}

pub fn show(
    parent: &adw::ApplicationWindow,
    client: Client,
    profile: String,
    curves: &Curves,
    applied: Rc<dyn Fn(Curves)>,
) {
    let window = adw::Window::builder()
        .title("Measurements")
        .transient_for(parent)
        .modal(true)
        .default_width(820)
        .default_height(760)
        .build();
    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&gtk::Label::new(Some("Measurements"))));
    toolbar.add_top_bar(&header);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 14);
    body.set_margin_start(18);
    body.set_margin_end(18);
    body.set_margin_top(14);
    body.set_margin_bottom(18);
    let intro = gtk::Label::new(Some(
        "Add takes from the same headphone and fixture. Assign each file to an ear; originals are kept so you can change the average later.",
    ));
    intro.set_wrap(true);
    intro.set_xalign(0.0);
    body.append(&intro);
    let options = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let output = gtk::DropDown::from_strings(&["Separate left / right", "Merge both ears"]);
    output.update_property(&[gtk::accessible::Property::Label("Measurement output")]);
    let align = gtk::CheckButton::with_label("Match take levels at 630 Hz");
    align.set_tooltip_text(Some("Align the takes within each ear to its first take, using 315–1260 Hz. Leave off to preserve measured level differences."));
    options.append(&output);
    options.append(&align);
    body.append(&options);
    let preview = gtk::DrawingArea::builder()
        .height_request(230)
        .hexpand(true)
        .build();
    preview.update_property(&[gtk::accessible::Property::Label(
        "Measurement average preview",
    )]);
    body.append(&preview);
    let summary = gtk::Label::new(None);
    summary.set_xalign(0.0);
    summary.set_wrap(true);
    body.append(&summary);
    let hint = gtk::Label::new(Some(
        "Arithmetic dB averages over the shared frequency range. In merged mode each ear has equal weight. Blue: left. Orange: right.",
    ));
    hint.set_wrap(true);
    hint.set_xalign(0.0);
    hint.add_css_class("dim-label");
    body.append(&hint);
    let add = gtk::Button::with_label("Add measurement files…");
    add.set_halign(gtk::Align::Start);
    body.append(&add);
    let rows = gtk::Box::new(gtk::Orientation::Vertical, 6);
    body.append(&rows);
    let error = gtk::Label::new(None);
    error.set_wrap(true);
    error.set_xalign(0.0);
    error.add_css_class("error");
    body.append(&error);
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&body)
        .build();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&scroll);
    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    footer.set_margin_top(12);
    footer.set_margin_bottom(16);
    footer.set_margin_start(18);
    footer.set_margin_end(18);
    let cancel = gtk::Button::with_label("Cancel");
    let apply = gtk::Button::with_label("Use measurements");
    apply.add_css_class("suggested-action");
    apply.set_hexpand(true);
    footer.append(&cancel);
    footer.append(&apply);
    content.append(&footer);
    toolbar.set_content(Some(&content));
    window.set_content(Some(&toolbar));
    let initial = curves
        .measurement_set
        .clone()
        .unwrap_or_else(|| MeasurementSet {
            takes: curves
                .measurement
                .iter()
                .chain(&curves.placements)
                .enumerate()
                .map(|(index, points)| MeasurementTake {
                    name: if index == 0 {
                        "Current measurement".into()
                    } else {
                        format!("Placement {}", index + 1)
                    },
                    ear: MeasurementEar::Both,
                    points: points.clone(),
                })
                .collect(),
            ..Default::default()
        });
    output.set_selected(if initial.output == MeasurementOutput::Separate {
        0
    } else {
        1
    });
    align.set_active(initial.align_takes);
    let manager = Rc::new(Manager {
        window: window.downgrade(),
        client,
        profile,
        inputs: RefCell::new(
            initial
                .takes
                .into_iter()
                .map(|take| Input {
                    name: take.name,
                    ear: Some(take.ear),
                    points: take.points,
                })
                .collect(),
        ),
        output: Cell::new(initial.output),
        align: Cell::new(initial.align_takes),
        busy: Cell::new(false),
        rows,
        summary,
        error,
        preview,
        averages: RefCell::new(None),
        apply,
        cancel: cancel.clone(),
        add,
        options,
        applied,
    });
    manager.rebuild();
    manager.refresh();
    output.connect_selected_notify({
        let weak = Rc::downgrade(&manager);
        move |dropdown| {
            if let Some(manager) = weak.upgrade() {
                manager.output.set(if dropdown.selected() == 0 {
                    MeasurementOutput::Separate
                } else {
                    MeasurementOutput::Merged
                });
                manager.refresh();
            }
        }
    });
    align.connect_toggled({
        let weak = Rc::downgrade(&manager);
        move |check| {
            if let Some(manager) = weak.upgrade() {
                manager.align.set(check.is_active());
                manager.refresh();
            }
        }
    });
    manager.preview.set_draw_func({
        let weak = Rc::downgrade(&manager);
        move |_, cr, width, height| {
            if let Some(manager) = weak.upgrade() {
                manager.draw(cr, width as f64, height as f64);
            }
        }
    });
    manager.add.connect_clicked({
        let weak = Rc::downgrade(&manager);
        move |_| {
            if let Some(manager) = weak.upgrade() {
                manager.choose_files();
            }
        }
    });
    manager.apply.connect_clicked({
        let weak = Rc::downgrade(&manager);
        move |_| {
            if let Some(manager) = weak.upgrade() {
                manager.save();
            }
        }
    });
    cancel.connect_clicked({
        let window = window.downgrade();
        move |_| {
            if let Some(window) = window.upgrade() {
                window.close();
            }
        }
    });
    // The window owns the state; callbacks in its children use weak references.
    window.connect_close_request(move |_| {
        if manager.busy.get() {
            gtk::glib::Propagation::Stop
        } else {
            gtk::glib::Propagation::Proceed
        }
    });
    window.present();
}

impl Manager {
    fn snapshot(&self) -> Result<MeasurementSet, String> {
        let takes = self
            .inputs
            .borrow()
            .iter()
            .map(|input| {
                Ok(MeasurementTake {
                    name: input.name.clone(),
                    ear: input
                        .ear
                        .ok_or_else(|| format!("Choose an ear for {}.", input.name))?,
                    points: input.points.clone(),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(MeasurementSet {
            takes,
            output: self.output.get(),
            align_takes: self.align.get(),
        })
    }
    fn refresh(&self) {
        let result = self.snapshot().and_then(|set| set.averages());
        let inputs = self.inputs.borrow();
        let count = |ear| {
            inputs
                .iter()
                .filter(|input| input.ear == Some(ear) || input.ear == Some(MeasurementEar::Both))
                .count()
        };
        self.summary.set_text(&format!(
            "{} files · {} left takes · {} right takes",
            inputs.len(),
            count(MeasurementEar::Left),
            count(MeasurementEar::Right)
        ));
        self.apply.set_sensitive(result.is_ok() && !self.busy.get());
        self.cancel.set_sensitive(!self.busy.get());
        self.add
            .set_sensitive(!self.busy.get() && inputs.len() < MAX_MEASUREMENT_TAKES);
        self.options.set_sensitive(!self.busy.get());
        self.rows.set_sensitive(!self.busy.get());
        match result {
            Ok(averages) => {
                *self.averages.borrow_mut() = Some(averages);
                self.error.set_text("");
            }
            Err(error) => {
                *self.averages.borrow_mut() = None;
                self.error
                    .set_text(if inputs.is_empty() { "" } else { &error });
            }
        }
        self.preview.queue_draw();
    }
    fn rebuild(self: &Rc<Self>) {
        while let Some(child) = self.rows.first_child() {
            self.rows.remove(&child);
        }
        for (index, input) in self.inputs.borrow().iter().enumerate() {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let label = gtk::Label::new(Some(&input.name));
            label.set_xalign(0.0);
            label.set_hexpand(true);
            label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            label.set_width_chars(10);
            label.set_tooltip_text(Some(&input.name));
            let ear = gtk::DropDown::from_strings(&["Choose ear", "Left", "Right", "Both / mono"]);
            ear.update_property(&[gtk::accessible::Property::Label(&format!(
                "Ear for {}",
                input.name
            ))]);
            ear.set_selected(match input.ear {
                None => 0,
                Some(MeasurementEar::Left) => 1,
                Some(MeasurementEar::Right) => 2,
                Some(MeasurementEar::Both) => 3,
            });
            ear.connect_selected_notify({
                let weak = Rc::downgrade(self);
                move |dropdown| {
                    if let Some(manager) = weak.upgrade() {
                        if let Some(input) = manager.inputs.borrow_mut().get_mut(index) {
                            input.ear = match dropdown.selected() {
                                1 => Some(MeasurementEar::Left),
                                2 => Some(MeasurementEar::Right),
                                3 => Some(MeasurementEar::Both),
                                _ => None,
                            };
                        }
                        manager.refresh();
                    }
                }
            });
            let remove = gtk::Button::from_icon_name("edit-delete-symbolic");
            remove.set_tooltip_text(Some(&format!("Remove {}", input.name)));
            remove.update_property(&[gtk::accessible::Property::Label(&format!(
                "Remove {}",
                input.name
            ))]);
            remove.connect_clicked({
                let weak = Rc::downgrade(self);
                move |_| {
                    if let Some(manager) = weak.upgrade() {
                        manager.inputs.borrow_mut().remove(index);
                        manager.rebuild();
                        manager.refresh();
                    }
                }
            });
            row.append(&label);
            row.append(&ear);
            row.append(&remove);
            self.rows.append(&row);
        }
    }
    fn choose_files(self: &Rc<Self>) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("Frequency response (TXT, CSV)"));
        for pattern in ["*.txt", "*.csv", "*.TXT", "*.CSV"] {
            filter.add_pattern(pattern);
        }
        let chooser = gtk::FileDialog::builder()
            .title("Add left and right measurement files")
            .accept_label("Add measurements")
            .default_filter(&filter)
            .build();
        let weak = Rc::downgrade(self);
        chooser.open_multiple(Some(&window), gtk::gio::Cancellable::NONE, move |result| {
            let (Some(manager), Ok(files)) = (weak.upgrade(), result) else {
                return;
            };
            let paths = (0..files.n_items())
                .map(|index| {
                    files
                        .item(index)
                        .and_then(|item| item.downcast::<gtk::gio::File>().ok())
                        .and_then(|file| file.path())
                        .ok_or_else(|| "Choose local measurement files.".to_owned())
                })
                .collect::<Result<Vec<_>, _>>();
            let paths = match paths {
                Ok(paths) => paths,
                Err(error) => {
                    manager.error.set_text(&error);
                    return;
                }
            };
            if paths.len() + manager.inputs.borrow().len() > MAX_MEASUREMENT_TAKES {
                manager.error.set_text(&format!(
                    "Use at most {MAX_MEASUREMENT_TAKES} files. Remove a take before adding more."
                ));
                return;
            }
            manager.busy.set(true);
            manager.refresh();
            manager.error.set_text("Reading measurements…");
            let weak = Rc::downgrade(&manager);
            background(
                move || read_files(paths),
                move |result| {
                    if let Some(manager) = weak.upgrade() {
                        manager.busy.set(false);
                        match result {
                            Ok(inputs) => {
                                manager.inputs.borrow_mut().extend(inputs);
                                manager.rebuild();
                                manager.refresh();
                            }
                            Err(error) => {
                                manager.refresh();
                                manager.error.set_text(&error);
                            }
                        }
                    }
                },
            );
        });
    }
    fn save(self: &Rc<Self>) {
        let Ok(set) = self.snapshot() else {
            return;
        };
        if set.averages().is_err() || self.busy.replace(true) {
            return;
        }
        self.refresh();
        self.error.set_text("Saving measurements…");
        let client = self.client.clone();
        let id = self.profile.clone();
        let weak = Rc::downgrade(self);
        background(
            move || {
                client
                    .set_measurements(&id, &set)
                    .map_err(|e| format!("{e:#}"))
            },
            move |result| {
                if let Some(manager) = weak.upgrade() {
                    manager.busy.set(false);
                    match result {
                        Ok(curves) => {
                            (manager.applied)(curves);
                            if let Some(window) = manager.window.upgrade() {
                                window.close();
                            }
                        }
                        Err(error) => {
                            manager.refresh();
                            manager.error.set_text(&error);
                        }
                    }
                }
            },
        );
    }
    fn draw(&self, cr: &gtk::cairo::Context, width: f64, height: f64) {
        let left = 42.0;
        let top = 20.0;
        let w = (width - 56.0).max(1.0);
        let h = (height - 48.0).max(1.0);
        let x = |f: f64| left + (f / 20.0).log10() / 3.0 * w;
        let y = |v: f64| top + (20.0 - v) / 40.0 * h;
        cr.set_source_rgba(0.6, 0.6, 0.6, 0.16);
        cr.set_line_width(1.0);
        for level in [-20, -10, 0, 10, 20] {
            cr.move_to(left, y(level as f64));
            cr.line_to(left + w, y(level as f64));
        }
        for frequency in [20.0, 100.0, 1000.0, 10000.0, 20000.0] {
            cr.move_to(x(frequency), top);
            cr.line_to(x(frequency), top + h);
        }
        let _ = cr.stroke();
        cr.set_source_rgba(0.7, 0.7, 0.7, 0.9);
        cr.set_font_size(10.0);
        for (frequency, label) in [
            (20.0, "20"),
            (100.0, "100"),
            (1000.0, "1k"),
            (10000.0, "10k"),
            (20000.0, "20k"),
        ] {
            cr.move_to(x(frequency) - 6.0, top + h + 18.0);
            let _ = cr.show_text(label);
        }
        for level in [-20, 0, 20] {
            cr.move_to(0.0, y(level as f64) + 3.0);
            let _ = cr.show_text(&format!("{level:+}"));
        }
        let averages = self.averages.borrow();
        let Some(averages) = averages.as_ref() else {
            return;
        };
        let merged = averages.merged();
        let offset = alignment_level(&|f| curve_at(&merged, f).0);
        let _ = cr.save();
        cr.rectangle(left, top, w, h);
        cr.clip();
        let stroke = |points: &[massiveeq_core::CurvePoint], color: (f64, f64, f64)| {
            let mut started = false;
            for point in points
                .iter()
                .filter(|point| (20.0..=20000.0).contains(&point.frequency))
            {
                if started {
                    cr.line_to(x(point.frequency), y(point.low - offset));
                } else {
                    cr.move_to(x(point.frequency), y(point.low - offset));
                    started = true;
                }
            }
            cr.set_source_rgb(color.0, color.1, color.2);
            cr.set_line_width(2.0);
            let _ = cr.stroke();
        };
        if self.output.get() == MeasurementOutput::Merged {
            stroke(&merged, (0.93, 0.92, 0.90));
        } else {
            stroke(&averages.left, (0.33, 0.68, 0.94));
            stroke(&averages.right, (1.0, 0.48, 0.10));
        }
        let _ = cr.restore();
    }
}

fn read_files(paths: Vec<PathBuf>) -> Result<Vec<Input>, String> {
    let mut inputs = Vec::new();
    let mut total = 0;
    for path in paths {
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let size = std::fs::metadata(&path)
            .map_err(|error| format!("{name}: {error}"))?
            .len();
        if size > 4_000_000 {
            return Err(format!(
                "{name} exceeds 4 MB. Export fewer frequency points."
            ));
        }
        let text = std::fs::read_to_string(&path).map_err(|error| format!("{name}: {error}"))?;
        let points = massiveeq_core::parse_curve(&text, massiveeq_core::CurveKind::Measurement)
            .map_err(|error| format!("{name}: {error}"))?;
        total += points.len();
        if total > MAX_MEASUREMENT_POINTS {
            return Err(format!(
                "Use at most {MAX_MEASUREMENT_POINTS} frequency points across the files."
            ));
        }
        inputs.push(Input {
            ear: measurement_ear_hint(&name),
            name,
            points,
        });
    }
    Ok(inputs)
}
