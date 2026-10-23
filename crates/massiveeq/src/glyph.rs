//! Small drawn symbols for filter types: the shape each filter gives the
//! response, as audio plugins label their band types.

use gtk4 as gtk;
use gtk4::{cairo, prelude::*};
use massiveeq_core::FilterKind;
use std::f64::consts::PI;

/// The response shape of a filter type, sampled over `t` in 0–1 (low to high
/// frequency), as -1 (cut) to 1 (boost).
pub fn shape(kind: FilterKind, t: f64) -> f64 {
    let bell = |center: f64, width: f64| (-((t - center) / width).powi(2)).exp();
    let step = |edge: f64| 1.0 / (1.0 + (-(t - edge) * 14.0).exp());
    // Pass filters fall off the bottom edge; shelves step between two flats.
    let fall = |edge: f64| (((t - edge) / 0.22).clamp(0.0, 1.0)).powf(1.6) * 1.35;
    match kind {
        FilterKind::Peaking => bell(0.5, 0.16) * 0.9 - 0.15,
        FilterKind::LowShelf => 0.55 - step(0.5) * 0.75,
        FilterKind::HighShelf => -0.2 + step(0.5) * 0.75,
        FilterKind::LowPass => 0.35 - fall(0.5),
        FilterKind::HighPass => 0.35 - (((0.5 - t) / 0.22).clamp(0.0, 1.0)).powf(1.6) * 1.35,
        FilterKind::BandPass => bell(0.5, 0.17) * 1.35 - 0.9,
        FilterKind::Notch => 0.3 - bell(0.5, 0.06) * 1.25,
        // Flat in level; marked by its phase turn instead.
        FilterKind::AllPass => 0.0,
    }
}

/// Draw `kind`'s symbol into `width`×`height` at the current source colour.
pub fn draw(cr: &cairo::Context, kind: FilterKind, width: f64, height: f64) {
    const STEPS: usize = 32;
    let middle = height / 2.0;
    let amplitude = height / 2.0 - 1.5;
    cr.set_line_width(2.0);
    cr.set_line_cap(cairo::LineCap::Round);
    cr.set_line_join(cairo::LineJoin::Round);
    for index in 0..=STEPS {
        let t = index as f64 / STEPS as f64;
        let x = 1.0 + t * (width - 2.0);
        let y = (middle - shape(kind, t) * amplitude).clamp(0.5, height - 0.5);
        if index == 0 {
            cr.move_to(x, y);
        } else {
            cr.line_to(x, y);
        }
    }
    let _ = cr.stroke();
    if kind == FilterKind::AllPass {
        // A half turn around the centre: the phase shift.
        cr.arc(width / 2.0, middle, amplitude * 0.75, PI, 2.0 * PI);
        let _ = cr.stroke();
    }
}

/// A symbol as a widget, drawn in the widget's text colour.
pub fn widget(kind: FilterKind, width: i32, height: i32) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::builder()
        .content_width(width)
        .content_height(height)
        .valign(gtk::Align::Center)
        .halign(gtk::Align::Center)
        .build();
    area.add_css_class("filter-glyph");
    set_kind(&area, kind);
    area
}

/// Redraw an existing symbol for another filter type.
pub fn set_kind(area: &gtk::DrawingArea, kind: FilterKind) {
    area.set_draw_func(move |area, cr, width, height| {
        let color = area.color();
        cr.set_source_rgba(
            color.red() as f64,
            color.green() as f64,
            color.blue() as f64,
            color.alpha() as f64,
        );
        draw(cr, kind, width as f64, height as f64);
    });
    area.queue_draw();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_read_as_their_filters() {
        let at = |kind, t| shape(kind, t);
        assert!(at(FilterKind::Peaking, 0.5) > at(FilterKind::Peaking, 0.1));
        assert!(at(FilterKind::LowShelf, 0.1) > at(FilterKind::LowShelf, 0.9));
        assert!(at(FilterKind::HighShelf, 0.9) > at(FilterKind::HighShelf, 0.1));
        assert!(at(FilterKind::LowPass, 0.1) > at(FilterKind::LowPass, 0.95));
        assert!(at(FilterKind::HighPass, 0.95) > at(FilterKind::HighPass, 0.05));
        assert!(at(FilterKind::Notch, 0.5) < at(FilterKind::Notch, 0.2));
        assert!(at(FilterKind::BandPass, 0.5) > at(FilterKind::BandPass, 0.1));
        for kind in [
            FilterKind::Peaking,
            FilterKind::LowPass,
            FilterKind::HighPass,
        ] {
            for i in 0..=20 {
                assert!(shape(kind, i as f64 / 20.0).abs() <= 1.0);
            }
        }
    }
}
