//! Undo/redo history for profile edits plus broad tone-shaping helpers.
//!
//! This module is deliberately standalone: it depends only on
//! `massiveeq_core` model types, never on GTK, I/O, or time, so it stays
//! deterministic and unit-testable. Bass is a standard biquad shelf; tilt is
//! a profile value that saves as ordinary APO shelves, so exported profiles
//! still play the same in any APO host.

use massiveeq_core::{
    Channel, ChannelSelection, Filter, FilterKind, MAX_FILTERS_PER_CHANNEL, MAX_TILT_DB_PER_OCTAVE,
    ProfileDocument,
};

/// One restorable state of the profile editor: the serialized profile text
/// plus the manual output trim in dB.
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub text: String,
    pub trim: f64,
}

/// Maximum number of snapshots kept on each of the undo and redo stacks.
const HISTORY_LIMIT: usize = 100;

/// Undo/redo history over [`Snapshot`]s with burst coalescing.
///
/// Edits recorded without an intervening [`History::checkpoint`] form one
/// burst: the first distinct edit pushes the pre-edit snapshot onto the undo
/// stack and later edits in the same burst only replace the current snapshot.
/// `checkpoint`, `undo`, and `redo` all end the open burst.
#[derive(Clone, Debug, Default)]
pub struct History {
    /// Snapshot passed to [`History::reset`]; immutable until the next reset.
    baseline: Option<Snapshot>,
    /// Undo stack, oldest first. Holds the pre-edit snapshot of each burst.
    past: Vec<Snapshot>,
    /// The snapshot the editor currently shows.
    current: Option<Snapshot>,
    /// Redo stack, oldest first. Cleared by any recorded edit.
    future: Vec<Snapshot>,
    burst_open: bool,
}

impl History {
    /// Starts a fresh history at `snapshot`, which becomes the immutable
    /// baseline and the current state. Clears both stacks and any open burst.
    pub fn reset(&mut self, snapshot: Snapshot) {
        self.baseline = Some(snapshot.clone());
        self.current = Some(snapshot);
        self.past.clear();
        self.future.clear();
        self.burst_open = false;
    }

    /// Records an edit. The first distinct edit of a burst saves the previous
    /// snapshot for undo; further edits in the same burst only replace the
    /// current snapshot. Any recorded edit clears the redo stack.
    pub fn record(&mut self, snapshot: Snapshot) {
        let Some(current) = self.current.as_mut() else {
            // Recording before any reset simply establishes the baseline.
            self.reset(snapshot);
            return;
        };
        if *current != snapshot {
            if !self.burst_open {
                push_bounded(&mut self.past, current.clone());
            }
            *current = snapshot;
        }
        self.future.clear();
        self.burst_open = true;
    }

    /// Ends the current burst so the next distinct edit starts a new one.
    pub fn checkpoint(&mut self) {
        self.burst_open = false;
    }

    /// Steps back one burst, returning the now-current snapshot, or `None`
    /// when there is nothing to undo. Ends any open burst.
    pub fn undo(&mut self) -> Option<Snapshot> {
        self.burst_open = false;
        let previous = self.past.pop()?;
        let outgoing = self.current.replace(previous)?;
        push_bounded(&mut self.future, outgoing);
        self.current.clone()
    }

    /// Re-applies the most recently undone snapshot, or `None` when there is
    /// nothing to redo. Ends any open burst.
    pub fn redo(&mut self) -> Option<Snapshot> {
        self.burst_open = false;
        let next = self.future.pop()?;
        let outgoing = self.current.replace(next)?;
        push_bounded(&mut self.past, outgoing);
        self.current.clone()
    }

    /// The snapshot passed to the last [`History::reset`], unaffected by
    /// edits, undo, or redo.
    pub fn baseline(&self) -> Option<&Snapshot> {
        self.baseline.as_ref()
    }

    pub fn can_undo(&self) -> bool {
        !self.past.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.future.is_empty()
    }
}

fn push_bounded(stack: &mut Vec<Snapshot>, snapshot: Snapshot) {
    stack.push(snapshot);
    if stack.len() > HISTORY_LIMIT {
        stack.remove(0);
    }
}

// --- Broad tone shaping -----------------------------------------------------

/// Center frequency of the bass shelf, in Hz.
const BASS_FREQUENCY: f64 = 105.0;
/// Shelf Q: Butterworth 1/sqrt(2), matching the `0.70710678` seen in profiles.
const SHELF_Q: f64 = std::f64::consts::FRAC_1_SQRT_2;
/// Tight tolerance for recognizing a filter as one of the shaping shelves.
const MATCH_TOLERANCE: f64 = 1e-6;
/// Serialized profiles keep four decimal places of Q. Recognize the same
/// shelf after that rounding without claiming unrelated shelf shapes.
const Q_TOLERANCE: f64 = 0.00005;
/// User-facing range for bass, in dB.
const SHAPING_LIMIT_DB: f64 = 12.0;

/// Index of the first enabled ALL-channel shelf of `kind` at `frequency`
/// with a Q matching [`SHELF_Q`], if any.
fn find_shelf(profile: &ProfileDocument, kind: FilterKind, frequency: f64) -> Option<usize> {
    profile.filters.iter().position(|filter| {
        filter.enabled
            && filter.kind == kind
            && filter.channels == ChannelSelection::All
            && (filter.frequency - frequency).abs() <= MATCH_TOLERANCE
            && (filter.q - SHELF_Q).abs() <= Q_TOLERANCE
    })
}

fn shelf_gain(profile: &ProfileDocument, kind: FilterKind, frequency: f64) -> f64 {
    find_shelf(profile, kind, frequency)
        .map(|index| profile.filters[index].gain_db)
        .unwrap_or(0.0)
}

/// Gain in dB of the bass shelf: the first enabled ALL-channel LowShelf at
/// Fc 105 Hz, Q ~0.70710678, or `0.0` when the profile has none.
pub fn bass_db(profile: &ProfileDocument) -> f64 {
    shelf_gain(profile, FilterKind::LowShelf, BASS_FREQUENCY)
}

/// Spectral tilt in dB per octave around 1 kHz, applied across the whole
/// range by shelves the DSP generates.
pub fn tilt_db_per_octave(profile: &ProfileDocument) -> f64 {
    profile.tilt_db_per_octave
}

/// Sets the bass shelf gain in dB, clamped by validation to ±12 dB.
///
/// Reuses the first matching shelf when present, otherwise appends one
/// standard enabled ALL-channel LowShelf. Fails without touching the document
/// when `value` is not finite, is outside ±12 dB, or a new filter would
/// exceed [`MAX_FILTERS_PER_CHANNEL`] on either channel.
pub fn set_bass(profile: &mut ProfileDocument, value: f64) -> Result<(), String> {
    validate_shaping("bass", value)?;
    match find_shelf(profile, FilterKind::LowShelf, BASS_FREQUENCY) {
        Some(index) => {
            profile.filters[index].gain_db = value;
        }
        None => {
            ensure_filter_capacity(profile, 1)?;
            profile
                .filters
                .push(shelf(FilterKind::LowShelf, BASS_FREQUENCY, value));
        }
    }
    Ok(())
}

/// Sets the spectral tilt in dB per octave, rounded to 0.01 and validated to
/// ±[`MAX_TILT_DB_PER_OCTAVE`]. Filters are left alone: the tilt is one value
/// on the profile, not editable bands.
pub fn set_tilt(profile: &mut ProfileDocument, value: f64) -> Result<(), String> {
    if !value.is_finite() || value.abs() > MAX_TILT_DB_PER_OCTAVE + 1e-9 {
        return Err(format!(
            "tilt must stay within ±{MAX_TILT_DB_PER_OCTAVE} dB/oct, got {value}"
        ));
    }
    profile.tilt_db_per_octave = (value * 100.0).round() / 100.0 + 0.0;
    Ok(())
}

fn shelf(kind: FilterKind, frequency: f64, gain_db: f64) -> Filter {
    Filter {
        enabled: true,
        kind,
        frequency,
        gain_db,
        q: SHELF_Q,
        bandwidth: None,
        channels: ChannelSelection::All,
    }
}

fn validate_shaping(name: &str, value: f64) -> Result<(), String> {
    if !value.is_finite() {
        return Err(format!("{name} must be a finite dB value, got {value}"));
    }
    if value.abs() > SHAPING_LIMIT_DB {
        return Err(format!(
            "{name} must stay within ±{SHAPING_LIMIT_DB} dB, got {value}"
        ));
    }
    Ok(())
}

/// Fails when adding `additional` ALL-channel filters would push any channel
/// past [`MAX_FILTERS_PER_CHANNEL`]. Checked before any mutation so the
/// document is never left half-updated.
fn ensure_filter_capacity(profile: &ProfileDocument, additional: usize) -> Result<(), String> {
    for channel in [Channel::Left, Channel::Right] {
        let count = profile.filters_for(channel).count();
        if count + additional > MAX_FILTERS_PER_CHANNEL {
            return Err(format!(
                "adding {additional} filter(s) would exceed the \
                 {MAX_FILTERS_PER_CHANNEL}-filter limit for {channel:?}"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use massiveeq_core::{ChannelGain, Convolution};
    use std::path::PathBuf;

    #[test]
    fn tone_controls_survive_profile_serialization_without_duplicate_shelves() {
        let mut profile = massiveeq_core::parse_text("Tone", "");
        set_bass(&mut profile, 4.0).unwrap();
        set_tilt(&mut profile, -0.75).unwrap();
        for _ in 0..3 {
            profile =
                massiveeq_core::parse_text("Tone", &massiveeq_core::serialize_profile(&profile));
            assert_eq!(bass_db(&profile), 4.0);
            assert_eq!(tilt_db_per_octave(&profile), -0.75);
            set_bass(&mut profile, 4.0).unwrap();
            set_tilt(&mut profile, -0.75).unwrap();
            assert_eq!(profile.filters.len(), 1);
        }
    }

    fn snap(text: &str, trim: f64) -> Snapshot {
        Snapshot {
            text: text.into(),
            trim,
        }
    }

    // --- History ------------------------------------------------------------

    #[test]
    fn burst_coalesces_and_undo_restores_exact_pre_edit_baseline() {
        let mut history = History::default();
        let baseline = snap("Preamp: -1.5 dB", -1.5);
        history.reset(baseline.clone());

        // One burst of rapid edits: only the pre-edit state is saved.
        history.record(snap("Preamp: -1.0 dB", -1.0));
        history.record(snap("Preamp: -0.5 dB", -0.5));
        assert!(history.can_undo());
        assert_eq!(history.undo(), Some(baseline.clone()));
        assert!(!history.can_undo());
        assert_eq!(history.baseline(), Some(&baseline));
    }

    #[test]
    fn checkpoint_separates_bursts() {
        let mut history = History::default();
        history.reset(snap("base", 0.0));
        history.record(snap("a", 0.0));
        history.checkpoint();
        history.record(snap("b", 0.0));

        assert_eq!(history.undo(), Some(snap("a", 0.0)));
        assert_eq!(history.undo(), Some(snap("base", 0.0)));
        assert_eq!(history.undo(), None);
    }

    #[test]
    fn undo_restores_manual_trim() {
        let mut history = History::default();
        history.reset(snap("base", -3.0));
        history.record(snap("edited", 2.0));
        let restored = history.undo().expect("one edit is undoable");
        assert_eq!(restored.trim, -3.0);
        assert_eq!(history.redo().map(|snap| snap.trim), Some(2.0));
    }

    #[test]
    fn new_edit_invalidates_redo() {
        let mut history = History::default();
        history.reset(snap("base", 0.0));
        history.record(snap("a", 0.0));
        history.checkpoint();
        assert!(history.undo().is_some());
        assert!(history.can_redo());

        history.record(snap("b", 0.0));
        assert!(!history.can_redo());
        assert_eq!(history.redo(), None);
        // The undone state is still reachable history, not redoable future.
        assert!(history.can_undo());
    }

    #[test]
    fn past_is_bounded_to_one_hundred_snapshots() {
        let mut history = History::default();
        history.reset(snap("base", 0.0));
        for burst in 0..150 {
            history.checkpoint();
            history.record(snap(&format!("edit {burst}"), burst as f64));
        }
        let mut undone = 0;
        while history.undo().is_some() {
            undone += 1;
        }
        assert_eq!(undone, HISTORY_LIMIT);
        assert!(!history.can_undo());
        // The baseline itself stays put even after capacity eviction.
        assert_eq!(history.baseline(), Some(&snap("base", 0.0)));
    }

    #[test]
    fn baseline_survives_edits_until_reset() {
        let mut history = History::default();
        history.reset(snap("base", -1.0));
        history.record(snap("a", 0.0));
        history.checkpoint();
        history.record(snap("b", 1.0));
        assert_eq!(history.baseline(), Some(&snap("base", -1.0)));
        history.reset(snap("new base", 0.5));
        assert_eq!(history.baseline(), Some(&snap("new base", 0.5)));
        assert!(!history.can_undo());
        assert!(!history.can_redo());
    }

    #[test]
    fn undo_ends_open_burst() {
        let mut history = History::default();
        history.reset(snap("base", 0.0));
        history.record(snap("a", 0.0));
        // No checkpoint: undo still ends the burst, so the next record saves
        // a fresh pre-edit state instead of silently coalescing.
        history.undo();
        history.record(snap("b", 0.0));
        assert_eq!(history.undo(), Some(snap("base", 0.0)));
        assert_eq!(history.redo(), Some(snap("b", 0.0)));
    }

    // --- Shaping --------------------------------------------------------------

    fn busy_profile() -> ProfileDocument {
        let mut profile = ProfileDocument::empty("test");
        profile.preamps.push(ChannelGain {
            channels: ChannelSelection::All,
            gain_db: -3.0,
        });
        profile.filters.push(Filter {
            enabled: true,
            kind: FilterKind::Peaking,
            frequency: 200.0,
            gain_db: 2.0,
            q: 1.0,
            bandwidth: None,
            channels: ChannelSelection::Left,
        });
        profile.filters.push(Filter {
            enabled: false,
            kind: FilterKind::LowShelf,
            frequency: 105.0,
            gain_db: 9.0,
            q: SHELF_Q,
            bandwidth: None,
            channels: ChannelSelection::All,
        });
        profile.convolutions.push(Convolution {
            channels: ChannelSelection::All,
            path: PathBuf::from("/tmp/ir.wav"),
        });
        profile
    }

    #[test]
    fn bass_reads_only_enabled_all_channel_shelf() {
        let mut profile = ProfileDocument::empty("flat");
        assert_eq!(bass_db(&profile), 0.0);

        // A disabled bass shelf must not be picked up.
        let mut disabled = busy_profile();
        assert_eq!(bass_db(&disabled), 0.0);

        set_bass(&mut profile, 6.0).unwrap();
        assert_eq!(bass_db(&profile), 6.0);

        disabled.filters[1].enabled = true;
        assert_eq!(bass_db(&disabled), 9.0);
    }

    #[test]
    fn set_bass_reuses_filter_and_preserves_everything_else() {
        let mut profile = busy_profile();
        let preamps = profile.preamps.clone();
        let graphic_eqs = profile.graphic_eqs.clone();
        let convolutions = profile.convolutions.clone();
        let peaking = profile.filters[0].clone();
        let filter_count = profile.filters.len();

        set_bass(&mut profile, 6.0).unwrap();
        assert_eq!(bass_db(&profile), 6.0);
        assert_eq!(profile.filters.len(), filter_count + 1);
        // The disabled shelf at Fc 105 is left alone; a new standard one was added.
        let added = profile.filters.last().unwrap();
        assert_eq!(
            added,
            &Filter {
                enabled: true,
                kind: FilterKind::LowShelf,
                frequency: 105.0,
                gain_db: 6.0,
                q: SHELF_Q,
                bandwidth: None,
                channels: ChannelSelection::All,
            }
        );

        // Repeated changes reuse the same filter instead of stacking new ones.
        set_bass(&mut profile, -4.0).unwrap();
        assert_eq!(bass_db(&profile), -4.0);
        assert_eq!(profile.filters.len(), filter_count + 1);

        // Preamps, graphic EQs, IR convolutions, and unrelated filters are untouched.
        assert_eq!(profile.preamps, preamps);
        assert_eq!(profile.graphic_eqs, graphic_eqs);
        assert_eq!(profile.convolutions, convolutions);
        assert_eq!(profile.filters[0], peaking);
    }

    #[test]
    fn set_bass_respects_channel_filter_limit_atomically() {
        let mut profile = ProfileDocument::empty("full");
        for index in 0..MAX_FILTERS_PER_CHANNEL {
            profile.filters.push(Filter {
                enabled: true,
                kind: FilterKind::Peaking,
                frequency: 100.0 + index as f64,
                gain_db: 1.0,
                q: 1.0,
                bandwidth: None,
                channels: ChannelSelection::Left,
            });
        }
        let before = profile.clone();
        let error = set_bass(&mut profile, 6.0).unwrap_err();
        assert!(error.contains("64"));
        assert_eq!(profile, before);
    }

    #[test]
    fn shaping_rejects_out_of_range_and_non_finite_values() {
        let mut profile = ProfileDocument::empty("flat");
        let before = profile.clone();
        for value in [13.0, -12.5, f64::NAN, f64::INFINITY] {
            assert!(set_bass(&mut profile, value).is_err());
            assert!(set_tilt(&mut profile, value).is_err());
        }
        assert!(set_tilt(&mut profile, 1.6).is_err());
        assert_eq!(profile, before);
    }

    #[test]
    fn tilt_is_a_profile_value_not_bands() {
        let mut profile = busy_profile();
        let before = profile.clone();
        set_tilt(&mut profile, 0.333).unwrap();
        assert_eq!(tilt_db_per_octave(&profile), 0.33);
        assert_eq!(profile.filters, before.filters);
        assert_eq!(
            profile.active_filters_for(Channel::Left).len(),
            before.active_filters_for(Channel::Left).len() + 5
        );
    }
}
