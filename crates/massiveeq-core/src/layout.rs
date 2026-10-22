//! How a convolution file's channels map speaker inputs to output channels.

use crate::ChannelSelection;
use serde::{Deserialize, Serialize};

/// Speaker order of a 7.1 input, matching PipeWire's `audio.position`.
pub const SURROUND_71_POSITIONS: [&str; 8] = ["FL", "FR", "FC", "LFE", "RL", "RR", "SL", "SR"];

/// The layout of an impulse-response file, chosen from its channel count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ImpulseLayout {
    /// One or two channels: each output channel is filtered by its own IR.
    PerChannel,
    /// Four channels: L→L, L→R, R→L, R→R, as stereo speakers heard by two ears.
    TrueStereo,
    /// Fourteen channels in HeSuVi's order: 7.1 speakers heard by two ears.
    Surround71,
}

/// One input channel convolved into one output channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImpulseRoute {
    pub input: usize,
    pub output: usize,
    pub ir_channel: usize,
}

const fn route(input: usize, output: usize, ir_channel: usize) -> ImpulseRoute {
    ImpulseRoute {
        input,
        output,
        ir_channel,
    }
}

const TRUE_STEREO_ROUTES: [ImpulseRoute; 4] = [
    route(0, 0, 0),
    route(0, 1, 1),
    route(1, 0, 2),
    route(1, 1, 3),
];

// HeSuVi's file order is FL-L, FL-R, SL-L, SL-R, RL-L, RL-R, FC-L, FR-R,
// FR-L, SR-R, SR-L, RR-R, RR-L, FC-R. LFE has no pair of its own and is heard
// through the center speaker, as in PipeWire's virtual-surround sink. Inputs
// are indexed in SURROUND_71_POSITIONS order.
const SURROUND_71_ROUTES: [ImpulseRoute; 16] = [
    route(0, 0, 0),
    route(0, 1, 1),
    route(6, 0, 2),
    route(6, 1, 3),
    route(4, 0, 4),
    route(4, 1, 5),
    route(2, 0, 6),
    route(2, 1, 13),
    route(1, 1, 7),
    route(1, 0, 8),
    route(7, 1, 9),
    route(7, 0, 10),
    route(5, 1, 11),
    route(5, 0, 12),
    route(3, 0, 6),
    route(3, 1, 13),
];

impl ImpulseLayout {
    pub fn from_channels(channels: usize) -> Option<Self> {
        match channels {
            1 | 2 => Some(Self::PerChannel),
            4 => Some(Self::TrueStereo),
            14 => Some(Self::Surround71),
            _ => None,
        }
    }

    /// Whether the IR mixes inputs into outputs, rather than filtering each channel.
    pub fn is_matrix(self) -> bool {
        !matches!(self, Self::PerChannel)
    }

    /// Input channels the processing expects; `None` means the output's own count.
    pub fn input_channels(self) -> Option<u32> {
        match self {
            Self::PerChannel => None,
            Self::TrueStereo => Some(2),
            Self::Surround71 => Some(8),
        }
    }

    /// Matrix routes, empty for per-channel IRs. Outputs are 0 = left ear, 1 = right ear.
    pub fn routes(self) -> &'static [ImpulseRoute] {
        match self {
            Self::PerChannel => &[],
            Self::TrueStereo => &TRUE_STEREO_ROUTES,
            Self::Surround71 => &SURROUND_71_ROUTES,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::PerChannel => "Per channel",
            Self::TrueStereo => "True stereo · 4-channel",
            Self::Surround71 => "7.1 virtual surround · 14-channel HeSuVi",
        }
    }
}

/// Check a convolution file's channel count against how the profile uses it.
pub fn validate_impulse(
    ir_channels: usize,
    selection: ChannelSelection,
    convolution_count: usize,
) -> Result<ImpulseLayout, String> {
    let layout = ImpulseLayout::from_channels(ir_channels).ok_or_else(|| {
        format!(
            "{ir_channels}-channel impulse responses are not supported; use 1 or 2 channels \
             (per ear), 4 (true stereo: L→L, L→R, R→L, R→R), or 14 (HeSuVi 7.1)"
        )
    })?;
    if layout.is_matrix() && selection != ChannelSelection::All {
        return Err(format!(
            "A {ir_channels}-channel impulse response feeds both ears; remove the Channel: \
             restriction before it"
        ));
    }
    if layout.is_matrix() && convolution_count > 1 {
        return Err(format!(
            "A {ir_channels}-channel impulse response must be the profile's only convolution"
        ));
    }
    Ok(layout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_follow_channel_count() {
        assert_eq!(
            ImpulseLayout::from_channels(2),
            Some(ImpulseLayout::PerChannel)
        );
        assert_eq!(
            ImpulseLayout::from_channels(4),
            Some(ImpulseLayout::TrueStereo)
        );
        assert_eq!(
            ImpulseLayout::from_channels(14),
            Some(ImpulseLayout::Surround71)
        );
        assert_eq!(ImpulseLayout::from_channels(8), None);
    }

    #[test]
    fn every_surround_speaker_reaches_both_ears_once() {
        for layout in [ImpulseLayout::TrueStereo, ImpulseLayout::Surround71] {
            let inputs = layout.input_channels().unwrap() as usize;
            for input in 0..inputs {
                for output in 0..2 {
                    let count = layout
                        .routes()
                        .iter()
                        .filter(|route| route.input == input && route.output == output)
                        .count();
                    assert_eq!(count, 1, "{layout:?} input {input} output {output}");
                }
            }
        }
    }

    #[test]
    fn surround_routes_use_every_hesuvi_channel() {
        let mut used = ImpulseLayout::Surround71
            .routes()
            .iter()
            .map(|route| route.ir_channel)
            .collect::<Vec<_>>();
        used.sort_unstable();
        used.dedup();
        assert_eq!(used, (0..14).collect::<Vec<_>>());
        // Front left reaches the left ear through HeSuVi channel 0, front right
        // the right ear through channel 7.
        let routes = ImpulseLayout::Surround71.routes();
        assert!(routes.contains(&route(0, 0, 0)));
        assert!(routes.contains(&route(1, 1, 7)));
    }

    #[test]
    fn matrix_impulses_must_cover_both_ears_alone() {
        assert!(validate_impulse(14, ChannelSelection::All, 1).is_ok());
        assert!(validate_impulse(4, ChannelSelection::Left, 1).is_err());
        assert!(validate_impulse(14, ChannelSelection::All, 2).is_err());
        assert!(validate_impulse(2, ChannelSelection::Left, 2).is_ok());
        assert!(validate_impulse(6, ChannelSelection::All, 1).is_err());
    }
}
