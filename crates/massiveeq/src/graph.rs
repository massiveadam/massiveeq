use gtk4::{DrawingArea, cairo, prelude::*};
use massiveeq_core::{
    ChannelSelection, CurvePoint, Filter, ProfileAnalysis, ProfileDocument, analysis::ResponsePoint,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

const LEFT: f64 = 58.0;
const RIGHT: f64 = 24.0;
const TOP: f64 = 34.0;
const BOTTOM: f64 = 42.0;
/// Half the vertical range: ±10 dB for EQ alone, wider behind curves.
const EQ_RANGE_DB: f64 = 10.0;
const CURVE_RANGE_DB: f64 = 20.0;

thread_local! {
    // Set each draw; read by the hit-testing helpers so points and drags
    // always agree with what is on screen.
    static RANGE_DB: Cell<f64> = const { Cell::new(EQ_RANGE_DB) };
}

fn range_db() -> f64 {
    RANGE_DB.with(Cell::get)
}
const MIN_FREQUENCY: f64 = 20.0;
const MAX_FREQUENCY: f64 = 20_000.0;
const FINE_FREQUENCY_OCTAVES: f64 = 1.0 / 48.0;
const COARSE_FREQUENCY_OCTAVES: f64 = 1.0 / 6.0;
const Q_STEP_FACTOR: f64 = 1.1;

/// Things drawn on the graph besides the EQ response.
#[derive(Debug, Default)]
pub struct Overlay {
    /// Frequency the tone finder is playing.
    pub tone: Option<f64>,
    /// A headphone measurement and a target (line or range) for this profile.
    pub measurement: Option<Vec<CurvePoint>>,
    pub target: Option<Vec<CurvePoint>>,
}

impl Overlay {
    fn has_curves(&self) -> bool {
        self.measurement.is_some() || self.target.is_some()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NudgeDirection {
    Up,
    Down,
    Left,
    Right,
}

pub fn response_graph(
    analysis: Rc<RefCell<Option<ProfileAnalysis>>>,
    document: Rc<RefCell<Option<ProfileDocument>>>,
    selected_filter: Rc<Cell<Option<usize>>>,
    overlay: Rc<RefCell<Overlay>>,
) -> DrawingArea {
    let area = DrawingArea::builder()
        .height_request(300)
        .hexpand(true)
        .build();
    area.set_draw_func(move |widget, context, width, height| {
        let width = width as f64;
        let height = height as f64;
        let foreground = widget.color();
        let foreground = (
            foreground.red() as f64,
            foreground.green() as f64,
            foreground.blue() as f64,
        );
        let overlay_state = overlay.borrow();
        RANGE_DB.with(|range| {
            range.set(if overlay_state.has_curves() {
                CURVE_RANGE_DB
            } else {
                EQ_RANGE_DB
            })
        });
        draw_background(context, width, height, foreground);
        draw_curves(
            context,
            &overlay_state,
            analysis.borrow().as_ref(),
            width,
            height,
        );

        if let Some(result) = analysis.borrow().as_ref() {
            let stereo = responses_match(&result.left.response, &result.right.response);
            draw_combined(
                context,
                &result.left.response,
                &result.right.response,
                width,
                height,
            );
            if stereo {
                draw_curve(
                    context,
                    &result.left.response,
                    width,
                    height,
                    (0.92, 0.92, 0.89, 0.96),
                );
            } else {
                draw_curve(
                    context,
                    &result.left.response,
                    width,
                    height,
                    (0.33, 0.68, 0.94, 0.96),
                );
                draw_curve(
                    context,
                    &result.right.response,
                    width,
                    height,
                    (0.93, 0.31, 0.18, 0.96),
                );
            }
            let _ = context.save();
            if overlay_state.has_curves() && width < 430.0 {
                context.translate(0.0, 14.0);
            }
            draw_channel_legend(context, width, stereo);
            let _ = context.restore();
        }

        if let Some(profile) = document.borrow().as_ref() {
            draw_filter_points(context, profile, selected_filter.get(), width, height);
        }
        if let Some(frequency) = overlay_state.tone {
            draw_tone_marker(context, frequency, width, height);
        }
    });
    area
}

fn draw_background(cr: &cairo::Context, width: f64, height: f64, foreground: (f64, f64, f64)) {
    let (plot_width, plot_height) = plot_size(width, height);
    cr.set_line_width(1.0);
    cr.set_font_size(10.0);
    cr.select_font_face(
        "Monospace",
        cairo::FontSlant::Normal,
        cairo::FontWeight::Normal,
    );

    let frequencies: &[f64] = if plot_width < 640.0 {
        &[20.0, 100.0, 500.0, 1000.0, 5000.0, 20000.0]
    } else {
        &[
            20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0, 20000.0,
        ]
    };
    for frequency in frequencies {
        let x = x_for(*frequency, width);
        cr.set_source_rgba(foreground.0, foreground.1, foreground.2, 0.085);
        cr.move_to(x, TOP);
        cr.line_to(x, TOP + plot_height);
        let _ = cr.stroke();

        let label = frequency_label(*frequency);
        let extents = cr.text_extents(label).ok();
        let label_width = extents.map_or(0.0, |value| value.width());
        let label_x = (x - label_width / 2.0).clamp(LEFT, LEFT + plot_width - label_width);
        cr.set_source_rgba(foreground.0, foreground.1, foreground.2, 0.48);
        cr.move_to(label_x, height - 14.0);
        let _ = cr.show_text(label);
    }

    let range = range_db();
    for db in [-range, -range / 2.0, 0.0, range / 2.0, range] {
        let y = y_for(db, height);
        let alpha = if db == 0.0 { 0.25 } else { 0.085 };
        cr.set_source_rgba(foreground.0, foreground.1, foreground.2, alpha);
        cr.set_line_width(if db == 0.0 { 1.3 } else { 1.0 });
        cr.move_to(LEFT, y);
        cr.line_to(LEFT + plot_width, y);
        let _ = cr.stroke();

        let label = format!("{db:+.0}");
        let extents = cr.text_extents(&label).ok();
        let label_width = extents.map_or(0.0, |value| value.width());
        cr.set_source_rgba(foreground.0, foreground.1, foreground.2, 0.48);
        cr.move_to(LEFT - label_width - 12.0, y + 4.0);
        let _ = cr.show_text(&label);
    }

    cr.set_source_rgba(foreground.0, foreground.1, foreground.2, 0.48);
    cr.move_to(14.0, TOP + 3.0);
    let _ = cr.show_text("dB");
}

fn draw_combined(
    cr: &cairo::Context,
    left: &[ResponsePoint],
    right: &[ResponsePoint],
    width: f64,
    height: f64,
) {
    let count = left.len().min(right.len());
    if count == 0 {
        return;
    }
    cr.new_path();
    for index in 0..count {
        let point = &left[index];
        let gain = (point.gain_db + right[index].gain_db) / 2.0;
        let x = x_for(point.frequency, width);
        let y = y_for(gain, height);
        if index == 0 {
            cr.move_to(x, y);
        } else {
            cr.line_to(x, y);
        }
    }
    let last_x = x_for(left[count - 1].frequency, width);
    let first_x = x_for(left[0].frequency, width);
    let zero = y_for(0.0, height);
    cr.line_to(last_x, zero);
    cr.line_to(first_x, zero);
    cr.close_path();
    cr.set_source_rgba(0.92, 0.28, 0.14, 0.07);
    let _ = cr.fill();
}

fn draw_curve(
    cr: &cairo::Context,
    points: &[ResponsePoint],
    width: f64,
    height: f64,
    color: (f64, f64, f64, f64),
) {
    cr.new_path();
    cr.set_source_rgba(color.0, color.1, color.2, color.3);
    cr.set_line_width(1.8);
    for (index, point) in points.iter().enumerate() {
        let x = x_for(point.frequency, width);
        let y = y_for(point.gain_db, height);
        if index == 0 {
            cr.move_to(x, y);
        } else {
            cr.line_to(x, y);
        }
    }
    let _ = cr.stroke();
}

fn draw_filter_points(
    cr: &cairo::Context,
    profile: &ProfileDocument,
    selected: Option<usize>,
    width: f64,
    height: f64,
) {
    for (index, filter) in profile.filters.iter().enumerate() {
        if !filter.enabled {
            continue;
        }
        let (x, y) = filter_point_position(filter.frequency, filter.gain_db, width, height);
        let is_selected = selected == Some(index);
        if is_selected {
            cr.new_sub_path();
            cr.set_source_rgba(0.93, 0.92, 0.90, 0.12);
            cr.arc(x, y, 17.0, 0.0, std::f64::consts::TAU);
            let _ = cr.fill();
        }

        // Start every point as a fresh sub-path. Without this, Cairo joins the
        // previous text position to the next arc with a visible diagonal line.
        cr.new_sub_path();
        cr.set_source_rgba(0.07, 0.08, 0.09, 0.96);
        cr.arc(
            x,
            y,
            if is_selected { 10.0 } else { 8.0 },
            0.0,
            std::f64::consts::TAU,
        );
        let _ = cr.fill_preserve();
        let outline = if is_selected {
            (0.93, 0.92, 0.90)
        } else {
            match filter.channels {
                ChannelSelection::Left => (0.33, 0.68, 0.94),
                ChannelSelection::Right => (0.93, 0.31, 0.18),
                ChannelSelection::All => (0.72, 0.74, 0.74),
            }
        };
        cr.set_source_rgba(outline.0, outline.1, outline.2, 0.98);
        cr.set_line_width(if is_selected { 2.3 } else { 1.5 });
        let _ = cr.stroke();

        cr.set_source_rgba(1.0, 1.0, 1.0, 0.96);
        cr.set_font_size(10.0);
        cr.select_font_face(
            "Monospace",
            cairo::FontSlant::Normal,
            cairo::FontWeight::Bold,
        );
        let label = (index + 1).to_string();
        let extents = cr.text_extents(&label).ok();
        let label_width = extents.map_or(0.0, |value| value.width());
        cr.move_to(x - label_width / 2.0, y + 3.5);
        let _ = cr.show_text(&label);
    }
}

fn responses_match(left: &[ResponsePoint], right: &[ResponsePoint]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| (left.gain_db - right.gain_db).abs() < 0.01)
}

fn draw_channel_legend(cr: &cairo::Context, width: f64, stereo: bool) {
    cr.select_font_face(
        "Monospace",
        cairo::FontSlant::Normal,
        cairo::FontWeight::Bold,
    );
    cr.set_font_size(10.0);
    if stereo {
        cr.set_source_rgba(0.76, 0.77, 0.76, 0.70);
        cr.move_to(width - RIGHT - 48.0, 18.0);
        let _ = cr.show_text("STEREO");
    } else {
        cr.set_source_rgba(0.33, 0.68, 0.94, 0.92);
        cr.move_to(width - RIGHT - 62.0, 18.0);
        let _ = cr.show_text("L");
        cr.set_source_rgba(0.93, 0.31, 0.18, 0.92);
        cr.move_to(width - RIGHT - 22.0, 18.0);
        let _ = cr.show_text("R");
    }
}

/// Mean of a curve over 300 Hz–3 kHz, where curves are aligned for comparison.
fn midband_level(value: &dyn Fn(f64) -> f64) -> f64 {
    const STEPS: usize = 32;
    (0..STEPS)
        .map(|index| value(300.0 * 10.0_f64.powf(index as f64 / (STEPS - 1) as f64)))
        .sum::<f64>()
        / STEPS as f64
}

/// Curve value at `frequency` (log-interpolated), as (low, high).
pub fn curve_at(points: &[CurvePoint], frequency: f64) -> (f64, f64) {
    let Some(first) = points.first() else {
        return (0.0, 0.0);
    };
    if frequency <= first.frequency {
        return (first.low, first.high);
    }
    for pair in points.windows(2) {
        if frequency <= pair[1].frequency {
            let span = (pair[1].frequency / pair[0].frequency).ln();
            let amount = if span > 0.0 {
                (frequency / pair[0].frequency).ln() / span
            } else {
                0.0
            };
            let lerp = |a: f64, b: f64| a + amount * (b - a);
            return (
                lerp(pair[0].low, pair[1].low),
                lerp(pair[0].high, pair[1].high),
            );
        }
    }
    points
        .last()
        .map_or((0.0, 0.0), |point| (point.low, point.high))
}

fn eq_at(response: &[ResponsePoint], frequency: f64) -> f64 {
    let index = response.partition_point(|point| point.frequency < frequency);
    match (
        index.checked_sub(1).and_then(|i| response.get(i)),
        response.get(index),
    ) {
        (Some(low), Some(high)) => {
            let amount = (frequency / low.frequency).ln() / (high.frequency / low.frequency).ln();
            low.gain_db + amount * (high.gain_db - low.gain_db)
        }
        (Some(only), None) | (None, Some(only)) => only.gain_db,
        (None, None) => 0.0,
    }
}

/// Target (range or line), the raw measurement, and the measurement with the
/// current EQ applied. Each is shifted to 0 dB across 300 Hz–3 kHz.
fn draw_curves(
    cr: &cairo::Context,
    overlay: &Overlay,
    analysis: Option<&ProfileAnalysis>,
    width: f64,
    height: f64,
) {
    const POINTS: usize = 400;
    let frequencies = (0..POINTS)
        .map(|index| 20.0 * 1000.0_f64.powf(index as f64 / (POINTS - 1) as f64))
        .collect::<Vec<_>>();
    let trace = |values: &dyn Fn(f64) -> f64, offset: f64| {
        for (index, frequency) in frequencies.iter().enumerate() {
            let (x, y) = (
                x_for(*frequency, width),
                y_for(values(*frequency) - offset, height),
            );
            if index == 0 {
                cr.move_to(x, y);
            } else {
                cr.line_to(x, y);
            }
        }
    };
    let stroke = |values: &dyn Fn(f64) -> f64, rgba: (f64, f64, f64, f64), line: f64| {
        trace(values, midband_level(values));
        cr.set_source_rgba(rgba.0, rgba.1, rgba.2, rgba.3);
        cr.set_line_width(line);
        let _ = cr.stroke();
    };

    if let Some(target) = &overlay.target {
        let center = |frequency: f64| {
            let (low, high) = curve_at(target, frequency);
            (low + high) / 2.0
        };
        if target.iter().any(|point| point.high > point.low) {
            let offset = midband_level(&center);
            trace(&|frequency| curve_at(target, frequency).1, offset);
            for frequency in frequencies.iter().rev() {
                let (low, _) = curve_at(target, *frequency);
                cr.line_to(x_for(*frequency, width), y_for(low - offset, height));
            }
            cr.close_path();
            cr.set_source_rgba(0.80, 0.78, 0.72, 0.10);
            let _ = cr.fill();
        }
        cr.set_dash(&[6.0, 4.0], 0.0);
        stroke(&center, (0.80, 0.78, 0.72, 0.6), 1.3);
        cr.set_dash(&[], 0.0);
    }

    if let Some(measurement) = &overlay.measurement {
        let raw = |frequency: f64| curve_at(measurement, frequency).0;
        stroke(&raw, (0.62, 0.64, 0.64, 0.55), 1.3);
        if let Some(analysis) = analysis {
            let ears = if responses_match(&analysis.left.response, &analysis.right.response) {
                vec![(&analysis.left.response, (1.0, 0.48, 0.10, 0.95))]
            } else {
                vec![
                    (&analysis.left.response, (0.45, 0.72, 0.86, 0.95)),
                    (&analysis.right.response, (0.86, 0.52, 0.42, 0.95)),
                ]
            };
            for (response, color) in ears {
                let corrected = |frequency: f64| raw(frequency) + eq_at(response, frequency);
                stroke(&corrected, color, 2.0);
            }
        }
    }

    // Legend, above the plot on the left.
    cr.select_font_face(
        "Monospace",
        cairo::FontSlant::Normal,
        cairo::FontWeight::Bold,
    );
    cr.set_font_size(10.0);
    let mut x = if width < 330.0 { LEFT } else { LEFT + 8.0 };
    if width < 330.0 {
        cr.set_font_size(8.5);
    }
    let mut key = |label: &str, rgba: (f64, f64, f64)| {
        cr.set_source_rgba(rgba.0, rgba.1, rgba.2, 0.9);
        cr.rectangle(x, 11.0, 14.0, 3.0);
        let _ = cr.fill();
        cr.move_to(x + 20.0, 16.0);
        let _ = cr.show_text(label);
        x += 32.0
            + cr.text_extents(label)
                .map(|extents| extents.width())
                .unwrap_or(60.0);
    };
    if overlay.measurement.is_some() {
        key("MEASURED", (0.62, 0.64, 0.64));
        key("PREDICTED", (1.0, 0.48, 0.10));
    }
    if overlay.target.is_some() {
        key("TARGET", (0.80, 0.78, 0.72));
    }
}

fn draw_tone_marker(cr: &cairo::Context, frequency: f64, width: f64, height: f64) {
    let x = x_for(frequency, width);
    cr.set_source_rgba(1.0, 0.48, 0.10, 0.9);
    cr.set_line_width(1.5);
    cr.set_dash(&[4.0, 4.0], 0.0);
    cr.move_to(x, TOP);
    cr.line_to(x, height - BOTTOM);
    let _ = cr.stroke();
    cr.set_dash(&[], 0.0);
    cr.select_font_face(
        "Monospace",
        cairo::FontSlant::Normal,
        cairo::FontWeight::Bold,
    );
    cr.set_font_size(10.0);
    let label = if frequency >= 1000.0 {
        format!("TONE {:.2} kHz", frequency / 1000.0)
    } else {
        format!("TONE {frequency:.0} Hz")
    };
    let text_width = cr
        .text_extents(&label)
        .map(|extents| extents.width())
        .unwrap_or(60.0);
    let left = (x + 6.0).min(width - RIGHT - text_width);
    cr.move_to(left, TOP + 12.0);
    let _ = cr.show_text(&label);
}

pub fn point_position(frequency: f64, gain: f64, width: f64, height: f64) -> (f64, f64) {
    (x_for(frequency, width), y_for(gain, height))
}

pub fn filter_point_position(
    frequency: f64,
    filter_gain: f64,
    width: f64,
    height: f64,
) -> (f64, f64) {
    point_position(frequency, filter_gain, width, height)
}

pub fn gain_after_drag(start_gain: f64, delta_y: f64, height: f64) -> f64 {
    let (_, plot_height) = plot_size(1.0 + LEFT + RIGHT, height);
    (start_gain - delta_y / plot_height * (2.0 * range_db())).clamp(-range_db(), range_db())
}

pub fn values_at_position(x: f64, y: f64, width: f64, height: f64) -> Option<(f64, f64)> {
    let (plot_width, plot_height) = plot_size(width, height);
    if x < LEFT || x > LEFT + plot_width || y < TOP || y > TOP + plot_height {
        return None;
    }
    let frequency_position = ((x - LEFT) / plot_width).clamp(0.0, 1.0);
    let frequency = MIN_FREQUENCY * (MAX_FREQUENCY / MIN_FREQUENCY).powf(frequency_position);
    let gain_position = ((y - TOP) / plot_height).clamp(0.0, 1.0);
    let gain = range_db() - gain_position * (2.0 * range_db());
    Some((frequency, gain))
}

pub fn nudge_filter(filter: &mut Filter, direction: NudgeDirection, shifted: bool) {
    match (direction, shifted) {
        (NudgeDirection::Up, false) => {
            filter.gain_db = (filter.gain_db + 0.1).clamp(-range_db(), range_db());
        }
        (NudgeDirection::Down, false) => {
            filter.gain_db = (filter.gain_db - 0.1).clamp(-range_db(), range_db());
        }
        (NudgeDirection::Up, true) => {
            filter.q = (filter.q * Q_STEP_FACTOR).clamp(0.01, 1_000.0);
        }
        (NudgeDirection::Down, true) => {
            filter.q = (filter.q / Q_STEP_FACTOR).clamp(0.01, 1_000.0);
        }
        (NudgeDirection::Left, shifted) | (NudgeDirection::Right, shifted) => {
            let octaves = if shifted {
                COARSE_FREQUENCY_OCTAVES
            } else {
                FINE_FREQUENCY_OCTAVES
            };
            let direction = if matches!(direction, NudgeDirection::Left) {
                -1.0
            } else {
                1.0
            };
            filter.frequency = (filter.frequency * 2.0_f64.powf(direction * octaves))
                .clamp(MIN_FREQUENCY, MAX_FREQUENCY);
        }
    }
}

fn plot_size(width: f64, height: f64) -> (f64, f64) {
    (
        (width - LEFT - RIGHT).max(1.0),
        (height - TOP - BOTTOM).max(1.0),
    )
}

fn x_for(frequency: f64, width: f64) -> f64 {
    let (plot_width, _) = plot_size(width, 1.0 + TOP + BOTTOM);
    LEFT + ((frequency / 20.0).ln() / (20000.0_f64 / 20.0).ln()).clamp(0.0, 1.0) * plot_width
}

fn y_for(db: f64, height: f64) -> f64 {
    let (_, plot_height) = plot_size(1.0 + LEFT + RIGHT, height);
    TOP + plot_height
        * (1.0 - ((db.clamp(-range_db(), range_db()) + range_db()) / (2.0 * range_db())))
}

fn frequency_label(frequency: f64) -> &'static str {
    match frequency as u32 {
        20 => "20 Hz",
        50 => "50",
        100 => "100",
        200 => "200",
        500 => "500",
        1000 => "1k",
        2000 => "2k",
        5000 => "5k",
        10000 => "10k",
        _ => "20 kHz",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_handle_is_independent_of_output_preamp() {
        assert_eq!(
            filter_point_position(1_000.0, 9.0, 1_000.0, 500.0),
            point_position(1_000.0, 9.0, 1_000.0, 500.0)
        );
    }

    #[test]
    fn display_and_drag_use_the_ten_db_range() {
        let height = 500.0;
        let (_, plot_height) = plot_size(1.0 + LEFT + RIGHT, height);
        assert_eq!(point_position(1_000.0, 10.0, 1_000.0, height).1, TOP);
        assert_eq!(
            point_position(1_000.0, -10.0, 1_000.0, height).1,
            TOP + plot_height
        );
        assert_eq!(gain_after_drag(0.0, -plot_height / 2.0, height), 10.0);
        assert_eq!(gain_after_drag(0.0, plot_height / 2.0, height), -10.0);
    }

    #[test]
    fn graph_coordinates_round_trip_to_frequency_and_gain() {
        let width = 1_000.0;
        let height = 500.0;
        for (frequency, gain) in [(20.0, 10.0), (632.455_532, 0.0), (20_000.0, -10.0)] {
            let (x, y) = point_position(frequency, gain, width, height);
            let (actual_frequency, actual_gain) =
                values_at_position(x, y, width, height).expect("point should be inside graph");
            assert!((actual_frequency - frequency).abs() < 0.001);
            assert!((actual_gain - gain).abs() < 0.001);
        }
        assert!(values_at_position(0.0, 0.0, width, height).is_none());
    }

    #[test]
    fn keyboard_nudges_gain_frequency_and_q() {
        let mut filter = Filter {
            enabled: true,
            kind: massiveeq_core::FilterKind::Peaking,
            frequency: 1_000.0,
            gain_db: 0.0,
            q: 1.0,
            channels: ChannelSelection::All,
        };

        nudge_filter(&mut filter, NudgeDirection::Up, false);
        assert!((filter.gain_db - 0.1).abs() < f64::EPSILON);
        nudge_filter(&mut filter, NudgeDirection::Right, false);
        let fine_frequency = filter.frequency;
        nudge_filter(&mut filter, NudgeDirection::Right, true);
        assert!(filter.frequency / fine_frequency > 1.1);
        nudge_filter(&mut filter, NudgeDirection::Up, true);
        assert!((filter.q - 1.1).abs() < f64::EPSILON);
        nudge_filter(&mut filter, NudgeDirection::Down, true);
        assert!((filter.q - 1.0).abs() < 0.000_001);
    }
}
