//! A rotary control in the style of audio plugins: drag up or down, scroll,
//! or use the arrow keys; double-click returns to the default. It exposes
//! the accessible slider role and range, so assistive technology can read
//! and set it like a `gtk::Scale`.

use gtk4 as gtk;
use gtk4::{cairo, glib, prelude::*, subclass::prelude::*};
use std::cell::{Cell, RefCell};
use std::f64::consts::PI;

/// How a knob's travel maps to its value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Taper {
    #[default]
    Linear,
    /// Equal travel per octave, for frequency and Q.
    Log,
}

/// Told `true` when a knob drag starts and `false` when it ends.
type DragWatcher = Box<dyn Fn(bool)>;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Knob {
        pub adjustment: RefCell<Option<gtk::Adjustment>>,
        pub default: Cell<f64>,
        pub taper: Cell<Taper>,
        /// Fill from the centre rather than from the minimum (gain, tilt).
        pub bipolar: Cell<bool>,
        pub drag_start: Cell<f64>,
        /// Pointer height in window coordinates when the drag began.
        pub drag_origin: Cell<Option<f64>>,
        pub drag_watchers: RefCell<Vec<DragWatcher>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Knob {
        const NAME: &'static str = "MassiveEqKnob";
        type Type = super::Knob;
        type ParentType = gtk::Widget;
        type Interfaces = (gtk::AccessibleRange,);

        fn class_init(class: &mut Self::Class) {
            class.set_css_name("knob");
            class.set_accessible_role(gtk::AccessibleRole::Slider);
        }
    }

    impl ObjectImpl for Knob {
        fn constructed(&self) {
            self.parent_constructed();
            let knob = self.obj();
            // Reachable with Tab, but a click leaves focus where it was, so
            // the graph's arrow keys keep editing the selected band.
            knob.set_focusable(true);
            knob.set_focus_on_click(false);

            let drag = gtk::GestureDrag::new();
            drag.connect_drag_begin({
                let knob = knob.downgrade();
                move |gesture, _, _| {
                    if let Some(knob) = knob.upgrade() {
                        let imp = knob.imp();
                        imp.drag_start.set(knob.fraction());
                        imp.drag_origin.set(window_y(gesture));
                        knob.notify_dragging(true);
                    }
                }
            });
            drag.connect_drag_update({
                let knob = knob.downgrade();
                move |gesture, _, dy| {
                    let Some(knob) = knob.upgrade() else {
                        return;
                    };
                    // Claim the press once it moves, so a window handle around
                    // the knob (the footer bar) cannot start moving the window.
                    // Claiming at the press itself would deny double-click reset.
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                    // Measure travel in window coordinates: the knob may sit
                    // on a panel that moves, and a widget-relative offset
                    // would then feed the knob's own movement back into it.
                    let dy = match (knob.imp().drag_origin.get(), window_y(gesture)) {
                        (Some(origin), Some(now)) => now - origin,
                        _ => dy,
                    };
                    let fine = gesture
                        .current_event_state()
                        .contains(gtk::gdk::ModifierType::SHIFT_MASK);
                    // 200 px of travel covers the range; Shift is ten times finer.
                    let travel = if fine { 2_000.0 } else { 200.0 };
                    knob.set_fraction(knob.imp().drag_start.get() - dy / travel);
                }
            });
            let done = std::rc::Rc::new({
                let knob = knob.downgrade();
                move || {
                    if let Some(knob) = knob.upgrade() {
                        knob.imp().drag_origin.set(None);
                        knob.notify_dragging(false);
                    }
                }
            });
            drag.connect_drag_end({
                let done = done.clone();
                move |_, _, _| done()
            });
            drag.connect_cancel(move |_, _| done());
            knob.add_controller(drag);

            let scroll = gtk::EventControllerScroll::new(
                gtk::EventControllerScrollFlags::VERTICAL
                    | gtk::EventControllerScrollFlags::DISCRETE,
            );
            scroll.connect_scroll({
                let knob = knob.downgrade();
                move |controller, _, dy| {
                    if let Some(knob) = knob.upgrade() {
                        let fine = controller
                            .current_event_state()
                            .contains(gtk::gdk::ModifierType::SHIFT_MASK);
                        knob.step(-dy * if fine { 0.002 } else { 0.02 });
                    }
                    glib::Propagation::Stop
                }
            });
            knob.add_controller(scroll);

            let click = gtk::GestureClick::new();
            click.connect_pressed({
                let knob = knob.downgrade();
                move |_, presses, _, _| {
                    if presses == 2
                        && let Some(knob) = knob.upgrade()
                    {
                        knob.set_value(knob.imp().default.get());
                    }
                }
            });
            knob.add_controller(click);

            let keys = gtk::EventControllerKey::new();
            keys.connect_key_pressed({
                let knob = knob.downgrade();
                move |_, key, _, modifiers| {
                    let Some(knob) = knob.upgrade() else {
                        return glib::Propagation::Proceed;
                    };
                    let step = if modifiers.contains(gtk::gdk::ModifierType::SHIFT_MASK) {
                        0.002
                    } else {
                        0.02
                    };
                    match key {
                        gtk::gdk::Key::Up | gtk::gdk::Key::Right => knob.step(step),
                        gtk::gdk::Key::Down | gtk::gdk::Key::Left => knob.step(-step),
                        gtk::gdk::Key::Home => knob.set_fraction(0.0),
                        gtk::gdk::Key::End => knob.set_fraction(1.0),
                        gtk::gdk::Key::BackSpace | gtk::gdk::Key::Delete => {
                            knob.set_value(knob.imp().default.get())
                        }
                        _ => return glib::Propagation::Proceed,
                    }
                    glib::Propagation::Stop
                }
            });
            knob.add_controller(keys);
        }
    }

    impl WidgetImpl for Knob {
        fn measure(&self, _: gtk::Orientation, _: i32) -> (i32, i32, i32, i32) {
            (40, 40, -1, -1)
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let knob = self.obj();
            let (width, height) = (knob.width() as f64, knob.height() as f64);
            let cr = snapshot.append_cairo(&gtk::graphene::Rect::new(
                0.0,
                0.0,
                width as f32,
                height as f32,
            ));
            draw(&cr, &knob, width, height);
        }
    }

    impl AccessibleImpl for Knob {}

    impl AccessibleRangeImpl for Knob {
        fn set_current_value(&self, value: f64) -> bool {
            self.obj().set_value(value);
            true
        }
    }
}

/// The pointer's height in the window's surface coordinates.
fn window_y(gesture: &gtk::GestureDrag) -> Option<f64> {
    let event = gesture.last_event(gesture.current_sequence().as_ref())?;
    event.position().map(|(_, y)| y)
}

glib::wrapper! {
    pub struct Knob(ObjectSubclass<imp::Knob>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::AccessibleRange, gtk::Buildable, gtk::ConstraintTarget;
}

impl Knob {
    /// A knob over `lower..=upper`, starting and resetting to `default`.
    pub fn new(lower: f64, upper: f64, default: f64, taper: Taper, bipolar: bool) -> Self {
        let knob: Self = glib::Object::new();
        let adjustment = gtk::Adjustment::new(default, lower, upper, 0.0, 0.0, 0.0);
        let imp = knob.imp();
        imp.default.set(default);
        imp.taper.set(taper);
        imp.bipolar.set(bipolar);
        adjustment.connect_value_changed({
            let knob = knob.downgrade();
            move |_| {
                if let Some(knob) = knob.upgrade() {
                    knob.update_accessible();
                    knob.queue_draw();
                }
            }
        });
        *imp.adjustment.borrow_mut() = Some(adjustment);
        knob.update_accessible();
        knob
    }

    pub fn adjustment(&self) -> gtk::Adjustment {
        self.imp()
            .adjustment
            .borrow()
            .clone()
            .expect("knob adjustment")
    }

    pub fn value(&self) -> f64 {
        self.adjustment().value()
    }

    pub fn set_value(&self, value: f64) {
        self.adjustment().set_value(value);
    }

    pub fn set_range(&self, lower: f64, upper: f64) {
        let adjustment = self.adjustment();
        adjustment.set_lower(lower);
        adjustment.set_upper(upper);
        self.update_accessible();
    }

    /// Call `callback` whenever the value changes, as `SpinButton` does.
    pub fn connect_value_changed(
        &self,
        callback: impl Fn(&Self) + 'static,
    ) -> glib::SignalHandlerId {
        let knob = self.downgrade();
        self.adjustment().connect_value_changed(move |_| {
            if let Some(knob) = knob.upgrade() {
                callback(&knob);
            }
        })
    }

    /// Position of the value along the knob's travel, 0–1.
    pub fn fraction(&self) -> f64 {
        let adjustment = self.adjustment();
        to_fraction(
            adjustment.value(),
            adjustment.lower(),
            adjustment.upper(),
            self.imp().taper.get(),
        )
    }

    pub fn set_fraction(&self, fraction: f64) {
        let adjustment = self.adjustment();
        self.set_value(from_fraction(
            fraction.clamp(0.0, 1.0),
            adjustment.lower(),
            adjustment.upper(),
            self.imp().taper.get(),
        ));
    }

    pub fn is_dragging(&self) -> bool {
        self.imp().drag_origin.get().is_some()
    }

    /// Call `callback` with `true` when a drag starts and `false` when it ends.
    pub fn connect_dragging(&self, callback: impl Fn(bool) + 'static) {
        self.imp()
            .drag_watchers
            .borrow_mut()
            .push(Box::new(callback));
    }

    fn notify_dragging(&self, dragging: bool) {
        for watcher in self.imp().drag_watchers.borrow().iter() {
            watcher(dragging);
        }
    }

    fn step(&self, amount: f64) {
        self.set_fraction(self.fraction() + amount);
    }

    fn update_accessible(&self) {
        let Some(adjustment) = self.imp().adjustment.borrow().clone() else {
            return;
        };
        self.update_property(&[
            gtk::accessible::Property::ValueMin(adjustment.lower()),
            gtk::accessible::Property::ValueMax(adjustment.upper()),
            gtk::accessible::Property::ValueNow(adjustment.value()),
        ]);
    }
}

pub fn to_fraction(value: f64, lower: f64, upper: f64, taper: Taper) -> f64 {
    let fraction = match taper {
        Taper::Linear => (value - lower) / (upper - lower),
        Taper::Log => (value / lower).ln() / (upper / lower).ln(),
    };
    if fraction.is_finite() {
        fraction.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

pub fn from_fraction(fraction: f64, lower: f64, upper: f64, taper: Taper) -> f64 {
    match taper {
        Taper::Linear => lower + fraction * (upper - lower),
        Taper::Log => lower * (upper / lower).powf(fraction),
    }
}

/// The arc sweeps 270°, from lower left round to lower right.
const START: f64 = 0.75 * PI;
const SWEEP: f64 = 1.5 * PI;

fn draw(cr: &cairo::Context, knob: &Knob, width: f64, height: f64) {
    let imp = knob.imp();
    let size = width.min(height);
    let (cx, cy) = (width / 2.0, height / 2.0);
    let radius = size / 2.0 - 3.0;
    let fraction = knob.fraction();
    let disabled = !knob.is_sensitive();
    cr.set_line_cap(cairo::LineCap::Round);
    cr.set_line_width(3.5);
    // Track
    cr.set_source_rgb(0.17, 0.17, 0.19);
    cr.arc(cx, cy, radius, START, START + SWEEP);
    let _ = cr.stroke();
    // Value arc, from the minimum or from the centre.
    let origin = if imp.bipolar.get() { 0.5 } else { 0.0 };
    if (fraction - origin).abs() > 0.002 {
        let (from, to) = if fraction > origin {
            (origin, fraction)
        } else {
            (fraction, origin)
        };
        if disabled {
            cr.set_source_rgb(0.34, 0.34, 0.36);
        } else {
            cr.set_source_rgb(1.0, 0.48, 0.10);
        }
        cr.arc(cx, cy, radius, START + from * SWEEP, START + to * SWEEP);
        let _ = cr.stroke();
    }
    // Cap and pointer
    cr.set_source_rgb(0.125, 0.125, 0.137);
    cr.arc(cx, cy, radius - 6.0, 0.0, 2.0 * PI);
    let _ = cr.fill();
    if knob.has_focus() {
        cr.set_source_rgba(1.0, 0.48, 0.10, 0.6);
        cr.set_line_width(1.0);
        cr.arc(cx, cy, radius - 6.0, 0.0, 2.0 * PI);
        let _ = cr.stroke();
    }
    let angle = START + fraction * SWEEP;
    cr.set_line_width(2.0);
    cr.set_source_rgb(0.925, 0.92, 0.90);
    cr.move_to(cx, cy);
    cr.line_to(
        cx + (radius - 8.0) * angle.cos(),
        cy + (radius - 8.0) * angle.sin(),
    );
    let _ = cr.stroke();
}

/// A knob with its value and caption below it. Double-click its value to
/// enter an exact number; Enter applies it and Escape or focus loss cancels.
pub struct LabeledKnob {
    pub root: gtk::Box,
    pub knob: Knob,
    /// Kept by the selected-band UI so a new selection cancels unfinished input.
    pub value_editor: gtk::Stack,
}

impl LabeledKnob {
    pub fn new(knob: Knob, caption: &str, format: impl Fn(f64) -> String + 'static) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 2);
        root.add_css_class("labeled-knob");
        knob.set_halign(gtk::Align::Center);
        knob.update_property(&[gtk::accessible::Property::Label(caption)]);
        let value = gtk::Label::new(Some(&format(knob.value())));
        value.add_css_class("knob-value");
        // A fixed width, so "-12.0" does not widen the column and shift the
        // footer under a knob being dragged.
        value.set_width_chars(6);
        let label = gtk::Label::new(Some(&caption.to_uppercase()));
        label.add_css_class("knob-caption");
        let readout = gtk::Stack::new();
        readout.set_hhomogeneous(false);
        readout.set_vhomogeneous(false);
        readout.add_named(&value, Some("value"));
        let entry = gtk::Entry::new();
        entry.add_css_class("knob-value");
        entry.set_has_frame(false);
        entry.set_width_chars(10);
        entry.set_max_width_chars(10);
        gtk::prelude::EditableExt::set_alignment(&entry, 0.5);
        entry.update_property(&[gtk::accessible::Property::Label(&format!(
            "{caption} exact value"
        ))]);
        readout.add_named(&entry, Some("entry"));
        let edited = std::rc::Rc::new(Cell::new(false));
        entry.connect_changed({
            let edited = edited.clone();
            move |_| edited.set(true)
        });
        value.set_tooltip_text(Some(
            "Double-click to enter a value; Enter applies, Escape cancels",
        ));
        let click = gtk::GestureClick::new();
        click.set_button(1);
        click.connect_pressed({
            let knob = knob.downgrade();
            let readout = readout.downgrade();
            let entry = entry.downgrade();
            let edited = edited.clone();
            move |gesture, presses, _, _| {
                if presses == 2
                    && let (Some(knob), Some(readout), Some(entry)) =
                        (knob.upgrade(), readout.upgrade(), entry.upgrade())
                    && knob.is_sensitive()
                {
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                    // Keep the initial text readable without changing the stored
                    // precision unless the user actually edits the field.
                    entry.set_text(&edit_value_text(knob.value()));
                    edited.set(false);
                    entry.remove_css_class("error");
                    let adjustment = knob.adjustment();
                    entry.set_tooltip_text(Some(&format!(
                        "Enter a number from {} to {}",
                        adjustment.lower(),
                        adjustment.upper()
                    )));
                    readout.set_visible_child_name("entry");
                    entry.grab_focus();
                    entry.select_region(0, -1);
                }
            }
        });
        value.add_controller(click);
        entry.connect_activate({
            let knob = knob.downgrade();
            let readout = readout.downgrade();
            let edited = edited.clone();
            move |entry| {
                let (Some(knob), Some(readout)) = (knob.upgrade(), readout.upgrade()) else {
                    return;
                };
                if readout.visible_child_name().as_deref() != Some("entry") || !knob.is_sensitive()
                {
                    return;
                }
                if !edited.get() {
                    readout.set_visible_child_name("value");
                    knob.grab_focus();
                    return;
                }
                let adjustment = knob.adjustment();
                match parse_exact_value(&entry.text(), adjustment.lower(), adjustment.upper()) {
                    Some(value) => {
                        readout.set_visible_child_name("value");
                        // All existing live editing, history and autosave callbacks
                        // run once, only after a valid value is confirmed.
                        knob.set_value(value);
                        knob.grab_focus();
                    }
                    None => {
                        entry.add_css_class("error");
                    }
                }
            }
        });
        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed({
            let readout = readout.downgrade();
            let knob = knob.downgrade();
            move |_, key, _, _| {
                if key != gtk::gdk::Key::Escape {
                    return glib::Propagation::Proceed;
                }
                if let Some(readout) = readout.upgrade() {
                    readout.set_visible_child_name("value");
                }
                if let Some(knob) = knob.upgrade() {
                    knob.grab_focus();
                }
                glib::Propagation::Stop
            }
        });
        entry.add_controller(keys);
        let focus = gtk::EventControllerFocus::new();
        focus.connect_leave({
            let readout = readout.downgrade();
            move |_| {
                if let Some(readout) = readout.upgrade() {
                    readout.set_visible_child_name("value");
                }
            }
        });
        entry.add_controller(focus);
        root.connect_unmap({
            let readout = readout.downgrade();
            move |_| {
                if let Some(readout) = readout.upgrade() {
                    readout.set_visible_child_name("value");
                }
            }
        });
        root.append(&knob);
        root.append(&readout);
        root.append(&label);
        knob.connect_value_changed({
            let readout = readout.downgrade();
            let value = value.clone();
            move |knob| {
                let text = format(knob.value());
                knob.update_property(&[gtk::accessible::Property::ValueText(&text)]);
                value.set_text(&text);
                // A selection change or another control changing this parameter
                // invalidates an unfinished edit of its previous value.
                if let Some(readout) = readout.upgrade() {
                    readout.set_visible_child_name("value");
                }
            }
        });
        Self {
            root,
            knob,
            value_editor: readout,
        }
    }
}

fn edit_value_text(value: f64) -> String {
    let text = format!("{value:.4}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" {
        "0".into()
    } else {
        text.into()
    }
}

/// Parse user input at the UI boundary, before it reaches GtkAdjustment's
/// clamping or the DSP model. Values outside the displayed range stay editable.
fn parse_exact_value(text: &str, lower: f64, upper: f64) -> Option<f64> {
    let value = text.trim().parse::<f64>().ok()?;
    (value.is_finite() && value >= lower && value <= upper).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires a display; run with xvfb-run cargo test -p massiveeq exact_value_editor -- --ignored"]
    fn exact_value_editor_commits_only_confirmed_input() {
        gtk::init().expect("GTK display");
        let control = LabeledKnob::new(
            Knob::new(20.0, 20_000.0, 1234.56789, Taper::Log, false),
            "Frequency",
            |value| format!("{value:.0} Hz"),
        );
        let window = gtk::Window::new();
        window.set_child(Some(&control.root));
        window.present();
        let context = glib::MainContext::default();
        while context.pending() {
            context.iteration(false);
        }
        let readout = &control.value_editor;
        let label = readout
            .child_by_name("value")
            .unwrap()
            .downcast::<gtk::Label>()
            .unwrap();
        let entry = readout
            .child_by_name("entry")
            .unwrap()
            .downcast::<gtk::Entry>()
            .unwrap();
        let clicks = label.observe_controllers();
        let click = (0..clicks.n_items())
            .find_map(|i| clicks.item(i)?.downcast::<gtk::GestureClick>().ok())
            .unwrap();
        let begin = || click.emit_by_name::<()>("pressed", &[&2i32, &0.0f64, &0.0f64]);
        let changes = std::rc::Rc::new(Cell::new(0));
        control.knob.connect_value_changed({
            let changes = changes.clone();
            move |_| changes.set(changes.get() + 1)
        });
        begin();
        assert_eq!(readout.visible_child_name().as_deref(), Some("entry"));
        assert_eq!(entry.text(), "1234.5679");
        entry.emit_activate();
        assert_eq!(control.knob.value(), 1234.56789);
        assert_eq!(changes.get(), 0);
        begin();
        entry.set_text("2345.67891");
        assert_eq!(changes.get(), 0);
        entry.emit_activate();
        assert_eq!(control.knob.value(), 2345.67891);
        assert_eq!(changes.get(), 1);
        assert_eq!(readout.visible_child_name().as_deref(), Some("value"));
        begin();
        entry.set_text("NaN");
        entry.emit_activate();
        assert_eq!(changes.get(), 1);
        assert_eq!(readout.visible_child_name().as_deref(), Some("entry"));
        assert!(entry.has_css_class("error"));
        entry.set_text("3000");
        let controllers = entry.observe_controllers();
        let keys = (0..controllers.n_items())
            .find_map(|i| {
                controllers
                    .item(i)?
                    .downcast::<gtk::EventControllerKey>()
                    .ok()
            })
            .unwrap();
        assert!(keys.emit_by_name::<bool>(
            "key-pressed",
            &[
                &gtk::gdk::Key::Escape,
                &0u32,
                &gtk::gdk::ModifierType::empty()
            ]
        ));
        assert_eq!(changes.get(), 1);
        assert_eq!(readout.visible_child_name().as_deref(), Some("value"));
        begin();
        entry.set_text("4000");
        control.knob.grab_focus();
        while context.pending() {
            context.iteration(false);
        }
        assert_eq!(readout.visible_child_name().as_deref(), Some("value"));
        assert_eq!(changes.get(), 1);
        begin();
        entry.set_text("5000");
        control.root.set_visible(false);
        assert_eq!(readout.visible_child_name().as_deref(), Some("value"));
        // A cancelled editor cannot commit to a different band later.
        entry.emit_activate();
        assert_eq!(changes.get(), 1);
        window.close();
    }

    #[test]
    fn editor_opens_with_readable_numbers() {
        assert_eq!(edit_value_text(-3.62969354804), "-3.6297");
        assert_eq!(edit_value_text(623.123456789), "623.1235");
        assert_eq!(edit_value_text(0.7291300000000001), "0.7291");
        assert_eq!(edit_value_text(1.0), "1");
        assert_eq!(edit_value_text(-0.0000001), "0");
    }

    #[test]
    fn exact_value_accepts_full_precision_and_rejects_invalid_numbers() {
        assert_eq!(
            parse_exact_value(" 1234.56789 ", 20.0, 20_000.0),
            Some(1234.56789)
        );
        assert_eq!(parse_exact_value("-7.25", -24.0, 24.0), Some(-7.25));
        assert_eq!(parse_exact_value("0.125", 0.1, 20.0), Some(0.125));
        assert_eq!(parse_exact_value("20", 20.0, 20_000.0), Some(20.0));
        assert_eq!(parse_exact_value("20000", 20.0, 20_000.0), Some(20_000.0));
        for text in [
            "", "abc", "NaN", "inf", "-inf", "1e999", "19.99", "20000.01",
        ] {
            assert_eq!(parse_exact_value(text, 20.0, 20_000.0), None, "{text}");
        }
    }

    #[test]
    fn tapers_round_trip() {
        for taper in [Taper::Linear, Taper::Log] {
            for value in [20.0, 440.0, 1_000.0, 20_000.0] {
                let fraction = to_fraction(value, 20.0, 20_000.0, taper);
                let back = from_fraction(fraction, 20.0, 20_000.0, taper);
                assert!((back - value).abs() / value < 1e-9, "{taper:?} {value}");
            }
        }
        // Log travel puts 632 Hz (the geometric middle) at half way.
        let middle = (20.0_f64 * 20_000.0).sqrt();
        assert!((to_fraction(middle, 20.0, 20_000.0, Taper::Log) - 0.5).abs() < 1e-9);
        assert_eq!(to_fraction(-100.0, -12.0, 12.0, Taper::Linear), 0.0);
    }
}
