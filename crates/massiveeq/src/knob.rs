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

/// A knob with its value and caption below it.
pub struct LabeledKnob {
    pub root: gtk::Box,
    pub knob: Knob,
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
        root.append(&knob);
        root.append(&value);
        root.append(&label);
        knob.connect_value_changed({
            let value = value.clone();
            move |knob| {
                let text = format(knob.value());
                knob.update_property(&[gtk::accessible::Property::ValueText(&text)]);
                value.set_text(&text);
            }
        });
        Self { root, knob }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
