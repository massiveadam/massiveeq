//! Shared node-anchored panel positioning for playback and microphone EQ.
use crate::{PanelSide, best_panel_side, graph, knob::Knob, panel_origin};
use gtk::prelude::*;
use gtk4 as gtk;
use std::{cell::Cell, rc::Rc};

pub struct Anchor {
    pub index: usize,
    pub frequency: f64,
    pub gain_db: f64,
    pub graph_dragging: bool,
}

/// Keep controls beside their node, avoiding response lines and graph edges.
/// Hold their position while hovering, dragging a knob, or typing a value.
pub fn attach(
    overlay: &gtk::Overlay,
    graph: &gtk::DrawingArea,
    panel: &gtk::Box,
    knobs: &[&Knob],
    anchor: impl Fn() -> Option<Anchor> + 'static,
    lines: impl Fn(f64, f64) -> Vec<(f64, f64)> + 'static,
) {
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
    panel.add_controller(motion);
    for control in knobs {
        let dragging = dragging.clone();
        let overlay = overlay.clone();
        control.connect_dragging(move |active| {
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
        let graph = graph.clone();
        let panel = panel.clone();
        move |_, child| {
            if child != panel.upcast_ref::<gtk::Widget>() {
                return None;
            }
            let anchor = anchor()?;
            let index = anchor.index;
            if (hovered.get()
                || dragging.get()
                || panel
                    .root()
                    .and_then(|root| root.focus())
                    .is_some_and(|focus| focus.is::<gtk::Text>() && focus.is_ancestor(&panel)))
                && let Some((placed_index, rectangle)) = placed.get()
                && placed_index == index
            {
                return Some(rectangle);
            }
            let (frequency, gain_db) = (anchor.frequency, anchor.gain_db);
            let (width, height) = (graph.width() as f64, graph.height() as f64);
            let node = graph::filter_point_position(frequency, gain_db, width, height);
            // Narrow graphs get a narrower panel; its top row wraps to fit.
            let (minimum, natural, _, _) = panel.measure(gtk::Orientation::Horizontal, -1);
            let panel_width = (natural as f64).min(width - 16.0).max(minimum as f64);
            let (_, panel_height, _, _) =
                panel.measure(gtk::Orientation::Vertical, panel_width as i32);
            let size = (panel_width, panel_height as f64);
            let lines = lines(width, height);
            let previous = (placed
                .get()
                .is_some_and(|(placed_index, _)| placed_index == index))
            .then(|| side.get());
            // Moving the node keeps the panel on its side rather than
            // letting it hop around under the pointer.
            let chosen = match previous {
                Some(previous) if anchor.graph_dragging => previous,
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
}
