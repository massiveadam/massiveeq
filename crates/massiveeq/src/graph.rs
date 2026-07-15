use gtk4::{DrawingArea, cairo, prelude::*};
use massiveeq_core::{
    ChannelSelection, Filter, ProfileAnalysis, ProfileDocument, analysis::ResponsePoint,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

const LEFT: f64 = 58.0;
const RIGHT: f64 = 24.0;
const TOP: f64 = 34.0;
const BOTTOM: f64 = 42.0;
const MIN_DB: f64 = -10.0;
const MAX_DB: f64 = 10.0;
const MIN_FREQUENCY: f64 = 20.0;
const MAX_FREQUENCY: f64 = 20_000.0;
const FINE_FREQUENCY_OCTAVES: f64 = 1.0 / 48.0;
const COARSE_FREQUENCY_OCTAVES: f64 = 1.0 / 6.0;
const Q_STEP_FACTOR: f64 = 1.1;

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
) -> DrawingArea {
    let area = DrawingArea::builder()
        .height_request(410)
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
        draw_background(context, width, height, foreground);

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
            draw_channel_legend(context, width, stereo);
        }

        if let Some(profile) = document.borrow().as_ref() {
            draw_filter_points(context, profile, selected_filter.get(), width, height);
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

    for db in [-10.0_f64, -5.0, 0.0, 5.0, 10.0] {
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
            cr.set_source_rgba(0.94, 0.29, 0.16, 0.24);
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
            (0.94, 0.29, 0.16)
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

        if is_selected {
            let text = format_filter_readout(filter);
            cr.select_font_face(
                "Monospace",
                cairo::FontSlant::Normal,
                cairo::FontWeight::Normal,
            );
            cr.set_font_size(11.0);
            let extents = cr.text_extents(&text).ok();
            let text_width = extents.map_or(0.0, |value| value.width());
            let box_width = text_width + 20.0;
            let box_height = 26.0;
            let box_x = (x - box_width / 2.0).clamp(LEFT, width - RIGHT - box_width);
            let box_y = if y > TOP + 46.0 { y - 40.0 } else { y + 18.0 };
            rounded_rectangle(cr, box_x, box_y, box_width, box_height, 8.0);
            cr.set_source_rgba(0.94, 0.29, 0.16, 0.98);
            let _ = cr.fill();
            cr.set_source_rgba(0.06, 0.07, 0.08, 0.98);
            cr.move_to(box_x + 10.0, box_y + 17.0);
            let _ = cr.show_text(&text);
        }
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

fn rounded_rectangle(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64, radius: f64) {
    cr.new_sub_path();
    cr.arc(
        x + w - radius,
        y + radius,
        radius,
        -std::f64::consts::FRAC_PI_2,
        0.0,
    );
    cr.arc(
        x + w - radius,
        y + h - radius,
        radius,
        0.0,
        std::f64::consts::FRAC_PI_2,
    );
    cr.arc(
        x + radius,
        y + h - radius,
        radius,
        std::f64::consts::FRAC_PI_2,
        std::f64::consts::PI,
    );
    cr.arc(
        x + radius,
        y + radius,
        radius,
        std::f64::consts::PI,
        std::f64::consts::PI * 1.5,
    );
    cr.close_path();
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
    (start_gain - delta_y / plot_height * (MAX_DB - MIN_DB)).clamp(MIN_DB, MAX_DB)
}

pub fn values_at_position(x: f64, y: f64, width: f64, height: f64) -> Option<(f64, f64)> {
    let (plot_width, plot_height) = plot_size(width, height);
    if x < LEFT || x > LEFT + plot_width || y < TOP || y > TOP + plot_height {
        return None;
    }
    let frequency_position = ((x - LEFT) / plot_width).clamp(0.0, 1.0);
    let frequency = MIN_FREQUENCY * (MAX_FREQUENCY / MIN_FREQUENCY).powf(frequency_position);
    let gain_position = ((y - TOP) / plot_height).clamp(0.0, 1.0);
    let gain = MAX_DB - gain_position * (MAX_DB - MIN_DB);
    Some((frequency, gain))
}

pub fn nudge_filter(filter: &mut Filter, direction: NudgeDirection, shifted: bool) {
    match (direction, shifted) {
        (NudgeDirection::Up, false) => {
            filter.gain_db = (filter.gain_db + 0.1).clamp(MIN_DB, MAX_DB);
        }
        (NudgeDirection::Down, false) => {
            filter.gain_db = (filter.gain_db - 0.1).clamp(MIN_DB, MAX_DB);
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
    TOP + plot_height * (1.0 - ((db.clamp(MIN_DB, MAX_DB) - MIN_DB) / (MAX_DB - MIN_DB)))
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

fn format_filter_readout(filter: &Filter) -> String {
    let frequency = if filter.frequency >= 1000.0 {
        format!("{:.2} kHz", filter.frequency / 1000.0)
    } else {
        format!("{:.0} Hz", filter.frequency)
    };
    format!("{frequency}  {:+.1} dB  Q {:.2}", filter.gain_db, filter.q)
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
