//! Transcribed DCX2496 output-parameter dump layout and the reviewed address set.
//!
//! The 6 x 74 output location table below is transcribed mechanically from
//! the pinned MIT `DuinoDCX` revision
//! `00b9d70d6192e993f31ec94ff0bc20e6e2265b2c`, `Ultradrive.cpp`
//! `outputLocations[6][74]` (Copyright (c) 2018 Lasse Lukkari; license in
//! `third_party/duinodcx/LICENSE`). Row `r` of output `o` is the location of
//! direct parameter `0x02 + r` on channel `4 + o`. Each location names a dump
//! part, the frame offset of the low seven bits, and optionally the 7-of-8
//! high-bit carrier and a ninth-bit-and-above byte, exactly as `patchBuffer`
//! applies a direct-parameter change to its mirrored dump.
//!
//! Every wide row satisfies the general 7-of-8 packing rule documented by
//! Legalab (`docs/dcx2496-dump-frame-format.md`): the carrier of payload
//! offset `f` is `13 + 8 * ((f - 13) / 8) + 7`, bit `(f - 13) % 8`, and the
//! high byte is the next non-carrier offset. Unit tests assert that rule for
//! all 444 rows and equality with the earlier hand-reviewed O1/PEQ9 and
//! O4-mute offsets.
//!
//! The one reviewed setup address, input sum type (setup channel 0, parameter
//! `0x02`), is `setupLocations[0]` of the same pinned revision: Dump0 frame
//! offset 117, low bits only. Its value encoding (0 off, 4 A+B) and the
//! output source encoding (`0x41`: 0 A, 1 B, 2 C, 3 SUM) follow the public
//! `UltradrivePi` `protocol.md` notes cited in `NOTICE`.
//!
//! These are behavioral-reference transcriptions. The O1/PEQ9 Dump0
//! locations and the O4 PEQ on/off, band count, and band 1 frequency, Q, gain
//! and slope Dump1 locations (with the Dump1 trailer balance) have been
//! exercised on the named device; every other address, including the MVP
//! routing addresses admitted by `dec-autonomous-muted-bench-20261007`, is an
//! implementation hypothesis until exact device readback confirms it.

use thiserror::Error;

use crate::protocol::{DUMP0_RESPONSE_LEN, DUMP1_RESPONSE_LEN, DumpPart};

/// First direct-parameter number described by the output location table.
pub const FIRST_OUTPUT_TABLE_PARAMETER: u8 = 0x02;
/// Number of transcribed parameter rows per output.
pub const OUTPUT_TABLE_ROWS: usize = 74;
/// Number of physical DCX outputs.
pub const OUTPUT_COUNT: u8 = 6;
/// Direct-parameter channel of physical output 1.
pub const FIRST_OUTPUT_CHANNEL: u8 = 5;
/// First packed payload offset in both dump frames.
pub const PACKED_PAYLOAD_START: usize = 13;
/// Number of PEQ bands per output.
pub const PEQ_BANDS: u8 = 9;
/// First band-1 frequency parameter.
pub const FIRST_BAND_PARAMETER: u8 = 0x13;
/// Direct parameters per PEQ band.
pub const BAND_PARAMETER_STRIDE: u8 = 5;
/// Last band-9 slope parameter.
pub const LAST_BAND_PARAMETER: u8 = FIRST_BAND_PARAMETER + PEQ_BANDS * BAND_PARAMETER_STRIDE - 1;
/// Output mute parameter (1 = muted).
pub const MUTE_PARAMETER: u8 = 0x03;
/// Output input-source parameter.
pub const SOURCE_PARAMETER: u8 = 0x41;
/// Largest output source code: 0 A, 1 B, 2 C, 3 SUM.
pub const MAX_SOURCE_CODE: u16 = 3;
/// Output source code selecting the setup input sum.
pub const SOURCE_SUM_CODE: u16 = 3;
/// Direct-parameter channel of the setup page.
pub const SETUP_CHANNEL: u8 = 0;
/// [`ReviewedAddress::output`] value of the setup input-sum address.
pub const SETUP_TARGET: u8 = 0;
/// Setup input sum type parameter.
pub const INPUT_SUM_PARAMETER: u8 = 0x02;
/// Input sum type code: off.
pub const INPUT_SUM_OFF_CODE: u16 = 0;
/// Input sum type code: A+B. The only non-off value the allowlist admits.
pub const INPUT_SUM_A_PLUS_B_CODE: u16 = 4;
/// Pinned `DuinoDCX` `00b9d70` `setupLocations[0]`: setup parameter `0x02`.
pub const INPUT_SUM_LOCATION: DumpLocation = low(0, 117);
/// Output PEQ enable parameter.
pub const EQ_ENABLED_PARAMETER: u8 = 0x06;
/// Output active PEQ band count parameter.
pub const EQ_COUNT_PARAMETER: u8 = 0x07;
/// Largest device frequency code (20 Hz * 2^(320/32) = 20.48 kHz).
pub const MAX_FREQUENCY_CODE: u16 = 320;
/// Largest device Q code (Q 10).
pub const MAX_Q_CODE: u16 = 40;
/// Largest device gain code (+15 dB).
pub const MAX_GAIN_CODE: u16 = 300;
/// Gain code representing 0 dB; the apply path admits only codes at or below it.
pub const UNITY_GAIN_CODE: u16 = 150;
/// Peaking ("bandpass" in the protocol notes) filter kind code.
pub const BELL_KIND_CODE: u16 = 1;

/// One dump location for a direct parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DumpLocation {
    part: DumpPart,
    low: usize,
    carrier: Option<(usize, u8)>,
    high: Option<usize>,
}

const fn part(index: u8) -> DumpPart {
    match index {
        0 => DumpPart::Part0,
        _ => DumpPart::Part1,
    }
}

const fn low(part_index: u8, low: usize) -> DumpLocation {
    DumpLocation {
        part: part(part_index),
        low,
        carrier: None,
        high: None,
    }
}

const fn carried(part_index: u8, low: usize, carrier: usize, bit: u8) -> DumpLocation {
    DumpLocation {
        part: part(part_index),
        low,
        carrier: Some((carrier, bit)),
        high: None,
    }
}

const fn split(part_index: u8, low: usize, carrier: usize, bit: u8, high: usize) -> DumpLocation {
    DumpLocation {
        part: part(part_index),
        low,
        carrier: Some((carrier, bit)),
        high: Some(high),
    }
}

impl DumpLocation {
    /// Dump part holding every byte of this location.
    pub const fn part(self) -> DumpPart {
        self.part
    }

    /// Frame offset of the low seven value bits.
    pub const fn low_offset(self) -> usize {
        self.low
    }

    /// Optional 7-of-8 carrier offset and bit holding value bit seven.
    pub const fn carrier(self) -> Option<(usize, u8)> {
        self.carrier
    }

    /// Optional frame offset holding value bits eight and above.
    pub const fn high_offset(self) -> Option<usize> {
        self.high
    }

    /// Largest value this location can represent.
    pub const fn max_representable(self) -> u16 {
        match (self.carrier, self.high) {
            (None, _) => 0x7f,
            (Some(_), None) => 0xff,
            (Some(_), Some(_)) => 0x7fff,
        }
    }

    /// Decode the value stored at this location.
    ///
    /// A low-only location ignores its carrier bit, matching the reference
    /// `patchBuffer`, which never writes it.
    pub fn read(self, frame: &[u8]) -> u16 {
        let mut value = u16::from(frame[self.low] & 0x7f);
        if let Some((carrier, bit)) = self.carrier {
            value |= u16::from((frame[carrier] >> bit) & 1) << 7;
        }
        if let Some(high) = self.high {
            value |= u16::from(frame[high] & 0x7f) << 8;
        }
        value
    }

    /// Patch one value exactly as the reference `patchBuffer` does.
    ///
    /// # Errors
    ///
    /// Rejects a value wider than the location can carry.
    pub fn write(self, frame: &mut [u8], value: u16) -> Result<(), LayoutError> {
        if value > self.max_representable() {
            return Err(LayoutError::ValueTooWide {
                value,
                maximum: self.max_representable(),
            });
        }
        let [high_byte, low_byte] = value.to_be_bytes();
        frame[self.low] = low_byte & 0x7f;
        if let Some((carrier, bit)) = self.carrier {
            let mask = 1_u8 << bit;
            frame[carrier] = (frame[carrier] & !mask) | ((low_byte >> 7) << bit);
        }
        if let Some(high) = self.high {
            frame[high] = high_byte;
        }
        Ok(())
    }

    /// Every frame offset this location may modify, in ascending order.
    pub fn offsets(self) -> Vec<usize> {
        let mut offsets = vec![self.low];
        if let Some((carrier, _)) = self.carrier {
            offsets.push(carrier);
        }
        if let Some(high) = self.high {
            offsets.push(high);
        }
        offsets.sort_unstable();
        offsets
    }
}

/// Carrier offset and bit of one packed payload offset under the 7-of-8 rule.
///
/// Returns `None` for an offset before the payload or one that is itself a
/// carrier.
pub const fn packing_carrier(offset: usize) -> Option<(usize, u8)> {
    if offset < PACKED_PAYLOAD_START {
        return None;
    }
    let relative = offset - PACKED_PAYLOAD_START;
    let position = relative % 8;
    if position == 7 {
        return None;
    }
    #[allow(clippy::cast_possible_truncation)]
    let bit = position as u8;
    Some((PACKED_PAYLOAD_START + relative - position + 7, bit))
}

/// Frame length of one dump part.
pub const fn dump_len(part: DumpPart) -> usize {
    match part {
        DumpPart::Part0 => DUMP0_RESPONSE_LEN,
        DumpPart::Part1 => DUMP1_RESPONSE_LEN,
    }
}

/// Transcribed output location of one direct parameter.
pub fn output_location(output: u8, parameter: u8) -> Option<DumpLocation> {
    if !(1..=OUTPUT_COUNT).contains(&output) || parameter < FIRST_OUTPUT_TABLE_PARAMETER {
        return None;
    }
    OUTPUT_LOCATIONS[usize::from(output - 1)]
        .get(usize::from(parameter - FIRST_OUTPUT_TABLE_PARAMETER))
        .copied()
}

/// One field of a PEQ band.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BandField {
    /// Frequency code, 0 through 320.
    Frequency,
    /// Q code, 0 through 40.
    Q,
    /// Gain code, 0 through 300.
    Gain,
    /// Filter kind code: 0 low shelf, 1 bell, 2 high shelf.
    Kind,
    /// Shelf slope code: 0 is 6 dB, 1 is 12 dB.
    Slope,
}

impl BandField {
    const ORDER: [Self; 5] = [
        Self::Frequency,
        Self::Q,
        Self::Gain,
        Self::Kind,
        Self::Slope,
    ];

    /// Fields in direct-parameter order.
    pub const fn all() -> [Self; 5] {
        Self::ORDER
    }

    const fn offset(self) -> u8 {
        match self {
            Self::Frequency => 0,
            Self::Q => 1,
            Self::Gain => 2,
            Self::Kind => 3,
            Self::Slope => 4,
        }
    }

    /// Largest device code for this field.
    pub const fn device_max(self) -> u16 {
        match self {
            Self::Frequency => MAX_FREQUENCY_CODE,
            Self::Q => MAX_Q_CODE,
            Self::Gain => MAX_GAIN_CODE,
            Self::Kind => 2,
            Self::Slope => 1,
        }
    }
}

/// Direct parameter number of one PEQ band field.
///
/// # Panics
///
/// Never for a band from 1 through 9; other bands are a caller bug.
pub fn band_parameter(band: u8, field: BandField) -> u8 {
    assert!(
        (1..=PEQ_BANDS).contains(&band),
        "PEQ band must be 1 through 9"
    );
    FIRST_BAND_PARAMETER + (band - 1) * BAND_PARAMETER_STRIDE + field.offset()
}

/// Direct parameter channel of one physical output.
///
/// # Panics
///
/// Never for an output from 1 through 6; other outputs are a caller bug.
pub fn output_channel(output: u8) -> u8 {
    assert!(
        (1..=OUTPUT_COUNT).contains(&output),
        "output must be 1 through 6"
    );
    FIRST_OUTPUT_CHANNEL + output - 1
}

/// Semantic meaning of one reviewed address.
///
/// Every variant except [`Self::InputSum`] is an output field; the input sum
/// is the single reviewed setup-channel field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum OutputField {
    /// Output mute (1 = muted), reviewed on O4, O5, and O6 only.
    Mute,
    /// Output input source, 0 A, 1 B, 2 C, 3 SUM, reviewed on O3 and O4 only.
    Source,
    /// Setup input sum type, admitted only as 0 (off) or 4 (A+B).
    InputSum,
    /// PEQ on/off.
    EqEnabled,
    /// Number of active PEQ bands, 0 through 9.
    EqCount,
    /// One field of one PEQ band.
    Band {
        /// Band number, 1 through 9.
        band: u8,
        /// Band field.
        field: BandField,
    },
}

impl OutputField {
    /// Largest value the device documents for this field.
    pub const fn device_max(self) -> u16 {
        match self {
            Self::Mute | Self::EqEnabled => 1,
            Self::EqCount => 9,
            Self::Source => MAX_SOURCE_CODE,
            Self::InputSum => INPUT_SUM_A_PLUS_B_CODE,
            Self::Band { field, .. } => field.device_max(),
        }
    }

    /// Whether the reviewed domain admits this value.
    ///
    /// Every field admits `0..=device_max()` except the input sum, which
    /// admits exactly off (0) and A+B (4).
    pub const fn admits(self, value: u16) -> bool {
        match self {
            Self::InputSum => value == INPUT_SUM_OFF_CODE || value == INPUT_SUM_A_PLUS_B_CODE,
            _ => value <= self.device_max(),
        }
    }

    /// Whether the field belongs to the PEQ bank of an output.
    pub const fn is_peq(self) -> bool {
        matches!(self, Self::EqEnabled | Self::EqCount | Self::Band { .. })
    }

    /// Whether the field is one of the MVP routing fields.
    pub const fn is_routing(self) -> bool {
        matches!(self, Self::Mute | Self::Source | Self::InputSum)
    }

    /// Whether the field is an on/off switch.
    pub const fn is_switch(self) -> bool {
        matches!(self, Self::Mute | Self::EqEnabled)
    }

    /// Whether the field is a PEQ band gain.
    pub const fn is_gain(self) -> bool {
        matches!(
            self,
            Self::Band {
                field: BandField::Gain,
                ..
            }
        )
    }

    /// Stable dotted label for receipts, for example `band3.gain`.
    pub fn label(self) -> String {
        match self {
            Self::Mute => "mute".to_owned(),
            Self::Source => "source".to_owned(),
            Self::InputSum => "setup.input_sum".to_owned(),
            Self::EqEnabled => "eq_enabled".to_owned(),
            Self::EqCount => "eq_count".to_owned(),
            Self::Band { band, field } => format!(
                "band{band}.{}",
                match field {
                    BandField::Frequency => "frequency",
                    BandField::Q => "q",
                    BandField::Gain => "gain",
                    BandField::Kind => "kind",
                    BandField::Slope => "slope",
                }
            ),
        }
    }
}

/// One allowlisted direct-parameter address with its dump location.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReviewedAddress {
    /// Physical output, 1 through 6, or [`SETUP_TARGET`] for the input sum.
    pub output: u8,
    /// Semantic field.
    pub field: OutputField,
    /// Transcribed dump location.
    pub location: DumpLocation,
}

/// Classify one direct-parameter address against the closed allowlist.
///
/// The allowlist is: PEQ on/off (`0x06`), PEQ band count (`0x07`), and the
/// nine PEQ bands (`0x13` through `0x3f`) on every output channel 5 through
/// 10; plus, under `dec-autonomous-muted-bench-20261007`, the O4/O5/O6 output
/// mutes (channels 8 through 10, `0x03`), the O3/O4 output sources (channels 7
/// and 8, `0x41`), and the setup input sum type (channel 0, `0x02`).
/// Crossover, dynamic EQ, delay, limiter, polarity, gain, the O1/O2/O5/O6
/// sources, the O1/O2/O3 mutes, every input address, and every other setup
/// address (Input C gain, Mute Outs, links, ...) fail closed.
pub fn reviewed_address(channel: u8, parameter: u8) -> Option<ReviewedAddress> {
    if channel == SETUP_CHANNEL {
        return (parameter == INPUT_SUM_PARAMETER).then_some(ReviewedAddress {
            output: SETUP_TARGET,
            field: OutputField::InputSum,
            location: INPUT_SUM_LOCATION,
        });
    }
    if !(FIRST_OUTPUT_CHANNEL..FIRST_OUTPUT_CHANNEL + OUTPUT_COUNT).contains(&channel) {
        return None;
    }
    let output = channel - FIRST_OUTPUT_CHANNEL + 1;
    let field = match parameter {
        MUTE_PARAMETER if (4..=6).contains(&output) => OutputField::Mute,
        SOURCE_PARAMETER if output == 3 || output == 4 => OutputField::Source,
        EQ_ENABLED_PARAMETER => OutputField::EqEnabled,
        EQ_COUNT_PARAMETER => OutputField::EqCount,
        FIRST_BAND_PARAMETER..=LAST_BAND_PARAMETER => {
            let relative = parameter - FIRST_BAND_PARAMETER;
            OutputField::Band {
                band: relative / BAND_PARAMETER_STRIDE + 1,
                field: BandField::ORDER[usize::from(relative % BAND_PARAMETER_STRIDE)],
            }
        }
        _ => return None,
    };
    Some(ReviewedAddress {
        output,
        field,
        location: output_location(output, parameter)?,
    })
}

/// Apply rank of one MVP routing address, or `None` for any other address.
///
/// A command must write routing fields in strictly ascending rank: the O4,
/// O5, then O6 mutes; then the setup input sum; then the O4 and O3 sources.
/// Mutes therefore precede sources, and the input sum precedes the O3 SUM
/// source that consumes it. Rollback is the exact reverse.
pub fn routing_rank(address: ReviewedAddress) -> Option<u8> {
    match (address.field, address.output) {
        (OutputField::Mute, output @ 4..=6) => Some(output - 4),
        (OutputField::InputSum, SETUP_TARGET) => Some(3),
        (OutputField::Source, 4) => Some(4),
        (OutputField::Source, 3) => Some(5),
        _ => None,
    }
}

/// Dump layout failures.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum LayoutError {
    /// The location cannot carry the value.
    #[error("value {value} does not fit a location whose maximum is {maximum}")]
    ValueTooWide { value: u16, maximum: u16 },
}

/// Pinned `DuinoDCX` `00b9d70` `outputLocations[6][74]`, outputs O1 through O6.
#[rustfmt::skip]
pub static OUTPUT_LOCATIONS: [[DumpLocation; OUTPUT_TABLE_ROWS]; OUTPUT_COUNT as usize] = [
    [
        split(0, 713, 716, 4, 714), // 0x02
        low(0, 715), // 0x03
        low(0, 718), // 0x04
        split(0, 720, 724, 3, 721), // 0x05
        low(0, 722), // 0x06
        low(0, 725), // 0x07
        low(0, 727), // 0x08
        low(0, 729), // 0x09
        carried(0, 731, 732, 6), // 0x0a
        low(0, 734), // 0x0b
        split(0, 736, 740, 3, 737), // 0x0c
        low(0, 738), // 0x0d
        split(0, 741, 748, 0, 742), // 0x0e
        low(0, 743), // 0x0f
        split(0, 745, 748, 4, 746), // 0x10
        low(0, 747), // 0x11
        low(0, 750), // 0x12
        split(0, 752, 756, 3, 753), // 0x13
        low(0, 754), // 0x14
        split(0, 757, 764, 0, 758), // 0x15
        low(0, 759), // 0x16
        low(0, 761), // 0x17
        split(0, 763, 764, 6, 765), // 0x18
        low(0, 766), // 0x19
        split(0, 768, 772, 3, 769), // 0x1a
        low(0, 770), // 0x1b
        low(0, 773), // 0x1c
        split(0, 775, 780, 2, 776), // 0x1d
        low(0, 777), // 0x1e
        split(0, 779, 780, 6, 781), // 0x1f
        low(0, 782), // 0x20
        low(0, 784), // 0x21
        split(0, 786, 788, 5, 787), // 0x22
        low(0, 789), // 0x23
        split(0, 791, 796, 2, 792), // 0x24
        low(0, 793), // 0x25
        low(0, 795), // 0x26
        split(0, 798, 804, 1, 799), // 0x27
        low(0, 800), // 0x28
        split(0, 802, 804, 5, 803), // 0x29
        low(0, 805), // 0x2a
        low(0, 807), // 0x2b
        split(0, 809, 812, 4, 810), // 0x2c
        low(0, 811), // 0x2d
        split(0, 814, 820, 1, 815), // 0x2e
        low(0, 816), // 0x2f
        low(0, 818), // 0x30
        split(0, 821, 828, 0, 822), // 0x31
        low(0, 823), // 0x32
        split(0, 825, 828, 4, 826), // 0x33
        low(0, 827), // 0x34
        low(0, 830), // 0x35
        split(0, 832, 836, 3, 833), // 0x36
        low(0, 834), // 0x37
        split(0, 837, 844, 0, 838), // 0x38
        low(0, 839), // 0x39
        low(0, 841), // 0x3a
        split(0, 843, 844, 6, 845), // 0x3b
        low(0, 846), // 0x3c
        split(0, 848, 852, 3, 849), // 0x3d
        low(0, 850), // 0x3e
        low(0, 853), // 0x3f
        low(0, 855), // 0x40
        low(0, 857), // 0x41
        low(0, 859), // 0x42
        split(0, 862, 868, 1, 863), // 0x43
        low(0, 864), // 0x44
        split(0, 866, 868, 5, 867), // 0x45
        low(0, 869), // 0x46
        carried(0, 871, 876, 2), // 0x47
        carried(0, 873, 876, 4), // 0x48
        low(0, 875), // 0x49
        low(0, 878), // 0x4a
        split(0, 880, 884, 3, 881), // 0x4b
    ],
    [
        split(0, 882, 884, 5, 883), // 0x02
        low(0, 885), // 0x03
        low(0, 887), // 0x04
        split(0, 889, 892, 4, 890), // 0x05
        low(0, 891), // 0x06
        low(0, 894), // 0x07
        low(0, 896), // 0x08
        low(0, 898), // 0x09
        carried(0, 901, 908, 0), // 0x0a
        low(0, 903), // 0x0b
        split(0, 905, 908, 4, 906), // 0x0c
        low(0, 907), // 0x0d
        split(0, 910, 916, 1, 911), // 0x0e
        low(0, 912), // 0x0f
        split(0, 914, 916, 5, 915), // 0x10
        low(0, 917), // 0x11
        low(0, 919), // 0x12
        split(0, 921, 924, 4, 922), // 0x13
        low(0, 923), // 0x14
        split(0, 926, 932, 1, 927), // 0x15
        low(0, 928), // 0x16
        low(0, 930), // 0x17
        split(0, 933, 940, 0, 934), // 0x18
        low(0, 935), // 0x19
        split(0, 937, 940, 4, 938), // 0x1a
        low(0, 939), // 0x1b
        low(0, 942), // 0x1c
        split(0, 944, 948, 3, 945), // 0x1d
        low(0, 946), // 0x1e
        split(0, 949, 956, 0, 950), // 0x1f
        low(0, 951), // 0x20
        low(0, 953), // 0x21
        split(0, 955, 956, 6, 957), // 0x22
        low(0, 958), // 0x23
        split(0, 960, 964, 3, 961), // 0x24
        low(0, 962), // 0x25
        low(0, 965), // 0x26
        split(0, 967, 972, 2, 968), // 0x27
        low(0, 969), // 0x28
        split(0, 971, 972, 6, 973), // 0x29
        low(0, 974), // 0x2a
        low(0, 976), // 0x2b
        split(0, 978, 980, 5, 979), // 0x2c
        low(0, 981), // 0x2d
        split(0, 983, 988, 2, 984), // 0x2e
        low(0, 985), // 0x2f
        low(0, 987), // 0x30
        split(0, 990, 996, 1, 991), // 0x31
        low(0, 992), // 0x32
        split(0, 994, 996, 5, 995), // 0x33
        low(0, 997), // 0x34
        low(0, 999), // 0x35
        split(0, 1001, 1004, 4, 1002), // 0x36
        low(0, 1003), // 0x37
        split(0, 1006, 1012, 1, 1007), // 0x38
        low(0, 1008), // 0x39
        low(0, 1010), // 0x3a
        split(1, 13, 20, 0, 14), // 0x3b
        low(1, 15), // 0x3c
        split(1, 17, 20, 4, 18), // 0x3d
        low(1, 19), // 0x3e
        low(1, 22), // 0x3f
        low(1, 24), // 0x40
        low(1, 26), // 0x41
        low(1, 29), // 0x42
        split(1, 31, 36, 2, 32), // 0x43
        low(1, 33), // 0x44
        split(1, 35, 36, 6, 37), // 0x45
        low(1, 38), // 0x46
        carried(1, 40, 44, 3), // 0x47
        carried(1, 42, 44, 5), // 0x48
        low(1, 45), // 0x49
        low(1, 47), // 0x4a
        split(1, 49, 52, 4, 50), // 0x4b
    ],
    [
        split(1, 51, 52, 6, 53), // 0x02
        low(1, 54), // 0x03
        low(1, 56), // 0x04
        split(1, 58, 60, 5, 59), // 0x05
        low(1, 61), // 0x06
        low(1, 63), // 0x07
        low(1, 65), // 0x08
        low(1, 67), // 0x09
        carried(1, 70, 76, 1), // 0x0a
        low(1, 72), // 0x0b
        split(1, 74, 76, 5, 75), // 0x0c
        low(1, 77), // 0x0d
        split(1, 79, 84, 2, 80), // 0x0e
        low(1, 81), // 0x0f
        split(1, 83, 84, 6, 85), // 0x10
        low(1, 86), // 0x11
        low(1, 88), // 0x12
        split(1, 90, 92, 5, 91), // 0x13
        low(1, 93), // 0x14
        split(1, 95, 100, 2, 96), // 0x15
        low(1, 97), // 0x16
        low(1, 99), // 0x17
        split(1, 102, 108, 1, 103), // 0x18
        low(1, 104), // 0x19
        split(1, 106, 108, 5, 107), // 0x1a
        low(1, 109), // 0x1b
        low(1, 111), // 0x1c
        split(1, 113, 116, 4, 114), // 0x1d
        low(1, 115), // 0x1e
        split(1, 118, 124, 1, 119), // 0x1f
        low(1, 120), // 0x20
        low(1, 122), // 0x21
        split(1, 125, 132, 0, 126), // 0x22
        low(1, 127), // 0x23
        split(1, 129, 132, 4, 130), // 0x24
        low(1, 131), // 0x25
        low(1, 134), // 0x26
        split(1, 136, 140, 3, 137), // 0x27
        low(1, 138), // 0x28
        split(1, 141, 148, 0, 142), // 0x29
        low(1, 143), // 0x2a
        low(1, 145), // 0x2b
        split(1, 147, 148, 6, 149), // 0x2c
        low(1, 150), // 0x2d
        split(1, 152, 156, 3, 153), // 0x2e
        low(1, 154), // 0x2f
        low(1, 157), // 0x30
        split(1, 159, 164, 2, 160), // 0x31
        low(1, 161), // 0x32
        split(1, 163, 164, 6, 165), // 0x33
        low(1, 166), // 0x34
        low(1, 168), // 0x35
        split(1, 170, 172, 5, 171), // 0x36
        low(1, 173), // 0x37
        split(1, 175, 180, 2, 176), // 0x38
        low(1, 177), // 0x39
        low(1, 179), // 0x3a
        split(1, 182, 188, 1, 183), // 0x3b
        low(1, 184), // 0x3c
        split(1, 186, 188, 5, 187), // 0x3d
        low(1, 189), // 0x3e
        low(1, 191), // 0x3f
        low(1, 193), // 0x40
        low(1, 195), // 0x41
        low(1, 198), // 0x42
        split(1, 200, 204, 3, 201), // 0x43
        low(1, 202), // 0x44
        split(1, 205, 212, 0, 206), // 0x45
        low(1, 207), // 0x46
        carried(1, 209, 212, 4), // 0x47
        carried(1, 211, 212, 6), // 0x48
        low(1, 214), // 0x49
        low(1, 216), // 0x4a
        split(1, 218, 220, 5, 219), // 0x4b
    ],
    [
        split(1, 221, 228, 0, 222), // 0x02
        low(1, 223), // 0x03
        low(1, 225), // 0x04
        split(1, 227, 228, 6, 229), // 0x05
        low(1, 230), // 0x06
        low(1, 232), // 0x07
        low(1, 234), // 0x08
        low(1, 237), // 0x09
        carried(1, 239, 244, 2), // 0x0a
        low(1, 241), // 0x0b
        split(1, 243, 244, 6, 245), // 0x0c
        low(1, 246), // 0x0d
        split(1, 248, 252, 3, 249), // 0x0e
        low(1, 250), // 0x0f
        split(1, 253, 260, 0, 254), // 0x10
        low(1, 255), // 0x11
        low(1, 257), // 0x12
        split(1, 259, 260, 6, 261), // 0x13
        low(1, 262), // 0x14
        split(1, 264, 268, 3, 265), // 0x15
        low(1, 266), // 0x16
        low(1, 269), // 0x17
        split(1, 271, 276, 2, 272), // 0x18
        low(1, 273), // 0x19
        split(1, 275, 276, 6, 277), // 0x1a
        low(1, 278), // 0x1b
        low(1, 280), // 0x1c
        split(1, 282, 284, 5, 283), // 0x1d
        low(1, 285), // 0x1e
        split(1, 287, 292, 2, 288), // 0x1f
        low(1, 289), // 0x20
        low(1, 291), // 0x21
        split(1, 294, 300, 1, 295), // 0x22
        low(1, 296), // 0x23
        split(1, 298, 300, 5, 299), // 0x24
        low(1, 301), // 0x25
        low(1, 303), // 0x26
        split(1, 305, 308, 4, 306), // 0x27
        low(1, 307), // 0x28
        split(1, 310, 316, 1, 311), // 0x29
        low(1, 312), // 0x2a
        low(1, 314), // 0x2b
        split(1, 317, 324, 0, 318), // 0x2c
        low(1, 319), // 0x2d
        split(1, 321, 324, 4, 322), // 0x2e
        low(1, 323), // 0x2f
        low(1, 326), // 0x30
        split(1, 328, 332, 3, 329), // 0x31
        low(1, 330), // 0x32
        split(1, 333, 340, 0, 334), // 0x33
        low(1, 335), // 0x34
        low(1, 337), // 0x35
        split(1, 339, 340, 6, 341), // 0x36
        low(1, 342), // 0x37
        split(1, 344, 348, 3, 345), // 0x38
        low(1, 346), // 0x39
        low(1, 349), // 0x3a
        split(1, 351, 356, 2, 352), // 0x3b
        low(1, 353), // 0x3c
        split(1, 355, 356, 6, 357), // 0x3d
        low(1, 358), // 0x3e
        low(1, 360), // 0x3f
        low(1, 362), // 0x40
        low(1, 365), // 0x41
        low(1, 367), // 0x42
        split(1, 369, 372, 4, 370), // 0x43
        low(1, 371), // 0x44
        split(1, 374, 380, 1, 375), // 0x45
        low(1, 376), // 0x46
        carried(1, 378, 380, 5), // 0x47
        carried(1, 381, 388, 0), // 0x48
        low(1, 383), // 0x49
        low(1, 385), // 0x4a
        split(1, 387, 388, 6, 389), // 0x4b
    ],
    [
        split(1, 390, 396, 1, 391), // 0x02
        low(1, 392), // 0x03
        low(1, 394), // 0x04
        split(1, 397, 404, 0, 398), // 0x05
        low(1, 399), // 0x06
        low(1, 401), // 0x07
        low(1, 403), // 0x08
        low(1, 406), // 0x09
        carried(1, 408, 412, 3), // 0x0a
        low(1, 410), // 0x0b
        split(1, 413, 420, 0, 414), // 0x0c
        low(1, 415), // 0x0d
        split(1, 417, 420, 4, 418), // 0x0e
        low(1, 419), // 0x0f
        split(1, 422, 428, 1, 423), // 0x10
        low(1, 424), // 0x11
        low(1, 426), // 0x12
        split(1, 429, 436, 0, 430), // 0x13
        low(1, 431), // 0x14
        split(1, 433, 436, 4, 434), // 0x15
        low(1, 435), // 0x16
        low(1, 438), // 0x17
        split(1, 440, 444, 3, 441), // 0x18
        low(1, 442), // 0x19
        split(1, 445, 452, 0, 446), // 0x1a
        low(1, 447), // 0x1b
        low(1, 449), // 0x1c
        split(1, 451, 452, 6, 453), // 0x1d
        low(1, 454), // 0x1e
        split(1, 456, 460, 3, 457), // 0x1f
        low(1, 458), // 0x20
        low(1, 461), // 0x21
        split(1, 463, 468, 2, 464), // 0x22
        low(1, 465), // 0x23
        split(1, 467, 468, 6, 469), // 0x24
        low(1, 470), // 0x25
        low(1, 472), // 0x26
        split(1, 474, 476, 5, 475), // 0x27
        low(1, 477), // 0x28
        split(1, 479, 484, 2, 480), // 0x29
        low(1, 481), // 0x2a
        low(1, 483), // 0x2b
        split(1, 486, 492, 1, 487), // 0x2c
        low(1, 488), // 0x2d
        split(1, 490, 492, 5, 491), // 0x2e
        low(1, 493), // 0x2f
        low(1, 495), // 0x30
        split(1, 497, 500, 4, 498), // 0x31
        low(1, 499), // 0x32
        split(1, 502, 508, 1, 503), // 0x33
        low(1, 504), // 0x34
        low(1, 506), // 0x35
        split(1, 509, 516, 0, 510), // 0x36
        low(1, 511), // 0x37
        split(1, 513, 516, 4, 514), // 0x38
        low(1, 515), // 0x39
        low(1, 518), // 0x3a
        split(1, 520, 524, 3, 521), // 0x3b
        low(1, 522), // 0x3c
        split(1, 525, 532, 0, 526), // 0x3d
        low(1, 527), // 0x3e
        low(1, 529), // 0x3f
        low(1, 531), // 0x40
        low(1, 534), // 0x41
        low(1, 536), // 0x42
        split(1, 538, 540, 5, 539), // 0x43
        low(1, 541), // 0x44
        split(1, 543, 548, 2, 544), // 0x45
        low(1, 545), // 0x46
        carried(1, 547, 548, 6), // 0x47
        carried(1, 550, 556, 1), // 0x48
        low(1, 552), // 0x49
        low(1, 554), // 0x4a
        split(1, 557, 564, 0, 558), // 0x4b
    ],
    [
        split(1, 559, 564, 2, 560), // 0x02
        low(1, 561), // 0x03
        low(1, 563), // 0x04
        split(1, 566, 572, 1, 567), // 0x05
        low(1, 568), // 0x06
        low(1, 570), // 0x07
        low(1, 573), // 0x08
        low(1, 575), // 0x09
        carried(1, 577, 580, 4), // 0x0a
        low(1, 579), // 0x0b
        split(1, 582, 588, 1, 583), // 0x0c
        low(1, 584), // 0x0d
        split(1, 586, 588, 5, 587), // 0x0e
        low(1, 589), // 0x0f
        split(1, 591, 596, 2, 592), // 0x10
        low(1, 593), // 0x11
        low(1, 595), // 0x12
        split(1, 598, 604, 1, 599), // 0x13
        low(1, 600), // 0x14
        split(1, 602, 604, 5, 603), // 0x15
        low(1, 605), // 0x16
        low(1, 607), // 0x17
        split(1, 609, 612, 4, 610), // 0x18
        low(1, 611), // 0x19
        split(1, 614, 620, 1, 615), // 0x1a
        low(1, 616), // 0x1b
        low(1, 618), // 0x1c
        split(1, 621, 628, 0, 622), // 0x1d
        low(1, 623), // 0x1e
        split(1, 625, 628, 4, 626), // 0x1f
        low(1, 627), // 0x20
        low(1, 630), // 0x21
        split(1, 632, 636, 3, 633), // 0x22
        low(1, 634), // 0x23
        split(1, 637, 644, 0, 638), // 0x24
        low(1, 639), // 0x25
        low(1, 641), // 0x26
        split(1, 643, 644, 6, 645), // 0x27
        low(1, 646), // 0x28
        split(1, 648, 652, 3, 649), // 0x29
        low(1, 650), // 0x2a
        low(1, 653), // 0x2b
        split(1, 655, 660, 2, 656), // 0x2c
        low(1, 657), // 0x2d
        split(1, 659, 660, 6, 661), // 0x2e
        low(1, 662), // 0x2f
        low(1, 664), // 0x30
        split(1, 666, 668, 5, 667), // 0x31
        low(1, 669), // 0x32
        split(1, 671, 676, 2, 672), // 0x33
        low(1, 673), // 0x34
        low(1, 675), // 0x35
        split(1, 678, 684, 1, 679), // 0x36
        low(1, 680), // 0x37
        split(1, 682, 684, 5, 683), // 0x38
        low(1, 685), // 0x39
        low(1, 687), // 0x3a
        split(1, 689, 692, 4, 690), // 0x3b
        low(1, 691), // 0x3c
        split(1, 694, 700, 1, 695), // 0x3d
        low(1, 696), // 0x3e
        low(1, 698), // 0x3f
        low(1, 701), // 0x40
        low(1, 703), // 0x41
        low(1, 705), // 0x42
        split(1, 707, 708, 6, 709), // 0x43
        low(1, 710), // 0x44
        split(1, 712, 716, 3, 713), // 0x45
        low(1, 714), // 0x46
        carried(1, 717, 724, 0), // 0x47
        carried(1, 719, 724, 2), // 0x48
        low(1, 721), // 0x49
        low(1, 723), // 0x4a
        split(1, 726, 732, 1, 727), // 0x4b
    ],
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_wide_row_follows_the_seven_of_eight_packing_rule() {
        let mut wide = 0;
        for (output, rows) in OUTPUT_LOCATIONS.iter().enumerate() {
            for (row, location) in rows.iter().enumerate() {
                let length = dump_len(location.part);
                for offset in location.offsets() {
                    assert!(
                        offset >= PACKED_PAYLOAD_START && offset < length - 2,
                        "O{} row {row} offset {offset} escapes the payload",
                        output + 1
                    );
                }
                assert!(
                    packing_carrier(location.low).is_some(),
                    "O{} row {row} low byte is itself a carrier",
                    output + 1
                );
                if let Some(carrier) = location.carrier {
                    wide += 1;
                    assert_eq!(Some(carrier), packing_carrier(location.low));
                }
                if let Some(high) = location.high {
                    let mut next = location.low + 1;
                    if packing_carrier(next).is_none() {
                        next += 1;
                    }
                    assert_eq!(high, next, "O{} row {row} high byte", output + 1);
                }
            }
        }
        assert!(wide > 0);
        assert_eq!(OUTPUT_LOCATIONS.len() * OUTPUT_TABLE_ROWS, 444);
    }

    #[test]
    fn table_equals_the_earlier_hand_reviewed_offsets() {
        assert_eq!(output_location(1, 0x3b), Some(split(0, 843, 844, 6, 845)));
        assert_eq!(output_location(1, 0x3c), Some(low(0, 846)));
        assert_eq!(output_location(1, 0x3d), Some(split(0, 848, 852, 3, 849)));
        assert_eq!(output_location(1, 0x3e), Some(low(0, 850)));
        assert_eq!(output_location(4, MUTE_PARAMETER), Some(low(1, 223)));
        // Offsets the practice-route diagnostic reads for all six mutes.
        for (output, part_index, offset) in [
            (1, 0, 715),
            (2, 0, 885),
            (3, 1, 54),
            (4, 1, 223),
            (5, 1, 392),
            (6, 1, 561),
        ] {
            assert_eq!(
                output_location(output, MUTE_PARAMETER),
                Some(low(part_index, offset))
            );
        }
        // O4 PEQ on/off, count, band 1, and band 9.
        assert_eq!(output_location(4, EQ_ENABLED_PARAMETER), Some(low(1, 230)));
        assert_eq!(output_location(4, EQ_COUNT_PARAMETER), Some(low(1, 232)));
        assert_eq!(
            output_location(4, band_parameter(1, BandField::Frequency)),
            Some(split(1, 259, 260, 6, 261))
        );
        assert_eq!(
            output_location(4, band_parameter(9, BandField::Gain)),
            Some(split(1, 355, 356, 6, 357))
        );
        assert_eq!(
            output_location(4, band_parameter(9, BandField::Slope)),
            Some(low(1, 360))
        );
        assert_eq!(output_location(0, 0x06), None);
        assert_eq!(output_location(7, 0x06), None);
        assert_eq!(output_location(1, 0x01), None);
        assert_eq!(output_location(1, 0x4c), None);
    }

    #[test]
    fn reviewed_peq_fields_fit_their_locations_and_never_share_bytes_unsafely() {
        for output in 1..=OUTPUT_COUNT {
            let channel = output_channel(output);
            let mut owned = std::collections::BTreeMap::new();
            for parameter in 0..=0x7f {
                let Some(address) = reviewed_address(channel, parameter) else {
                    continue;
                };
                assert_eq!(address.output, output);
                assert!(address.field.device_max() <= address.location.max_representable());
                assert!(owned.insert(address.location.low, parameter).is_none());
                if let Some(high) = address.location.high {
                    assert!(owned.insert(high, parameter).is_none());
                }
            }
            // Low and high bytes are owned by exactly one reviewed address.
            let lows = (0..=0x7f_u8)
                .filter_map(|parameter| reviewed_address(channel, parameter))
                .count();
            let expected = 2
                + 45
                + usize::from((4..=6).contains(&output))
                + usize::from(output == 3 || output == 4);
            assert_eq!(lows, expected);
        }
    }

    fn every_reviewed_address() -> Vec<(u8, u8, ReviewedAddress)> {
        let mut addresses = Vec::new();
        for channel in 0..=0x7f_u8 {
            for parameter in 0..=0x7f_u8 {
                if let Some(address) = reviewed_address(channel, parameter) {
                    addresses.push((channel, parameter, address));
                }
            }
        }
        addresses
    }

    #[test]
    fn reviewed_addresses_never_share_a_byte_across_channels() {
        let mut owned = std::collections::BTreeMap::new();
        for (channel, parameter, address) in every_reviewed_address() {
            let part = u8::from(address.location.part() == DumpPart::Part1);
            for offset in [Some(address.location.low), address.location.high]
                .into_iter()
                .flatten()
            {
                assert!(
                    owned.insert((part, offset), (channel, parameter)).is_none(),
                    "{channel}/{parameter:#04x} reuses {part}:{offset}"
                );
            }
        }
    }

    #[test]
    fn routing_addresses_equal_the_pinned_tables_and_baseline_map() {
        // DuinoDCX 00b9d70 setupLocations[0] and outputLocations rows 0x03/0x41.
        let routing = [
            (0, INPUT_SUM_PARAMETER, SETUP_TARGET, low(0, 117)),
            (7, SOURCE_PARAMETER, 3, low(1, 195)),
            (8, SOURCE_PARAMETER, 4, low(1, 365)),
            (8, MUTE_PARAMETER, 4, low(1, 223)),
            (9, MUTE_PARAMETER, 5, low(1, 392)),
            (10, MUTE_PARAMETER, 6, low(1, 561)),
        ];
        for (channel, parameter, output, location) in routing {
            let address = reviewed_address(channel, parameter).unwrap();
            assert_eq!(address.output, output);
            assert_eq!(address.location, location);
            assert!(address.field.is_routing() && !address.field.is_peq());
            assert!(routing_rank(address).is_some());
            if channel != 0 {
                assert_eq!(output_location(output, parameter), Some(location));
            }
        }
        let routing_count = every_reviewed_address()
            .into_iter()
            .filter(|(_, _, address)| address.field.is_routing())
            .count();
        assert_eq!(routing_count, routing.len());
    }

    #[test]
    fn routing_rank_orders_mutes_then_input_sum_then_sources() {
        let rank = |channel, parameter| routing_rank(reviewed_address(channel, parameter).unwrap());
        let ordered = [
            rank(8, MUTE_PARAMETER),
            rank(9, MUTE_PARAMETER),
            rank(10, MUTE_PARAMETER),
            rank(0, INPUT_SUM_PARAMETER),
            rank(8, SOURCE_PARAMETER),
            rank(7, SOURCE_PARAMETER),
        ];
        assert_eq!(ordered, [0, 1, 2, 3, 4, 5].map(Some));
        assert_eq!(rank(8, EQ_ENABLED_PARAMETER), None);
        assert_eq!(rank(8, band_parameter(1, BandField::Gain)), None);
    }

    #[test]
    fn input_sum_admits_only_off_and_a_plus_b() {
        let field = reviewed_address(0, INPUT_SUM_PARAMETER).unwrap().field;
        let admitted: Vec<u16> = (0..=0x7f).filter(|value| field.admits(*value)).collect();
        assert_eq!(admitted, [INPUT_SUM_OFF_CODE, INPUT_SUM_A_PLUS_B_CODE]);
        let source = reviewed_address(7, SOURCE_PARAMETER).unwrap().field;
        assert!((0..=3).all(|value| source.admits(value)) && !source.admits(4));
        let mute = reviewed_address(9, MUTE_PARAMETER).unwrap().field;
        assert!(mute.admits(0) && mute.admits(1) && !mute.admits(2));
    }

    #[test]
    fn forbidden_setup_and_input_bytes_are_owned_by_no_reviewed_address() {
        // Dump0 121 is setup 0x04 Input C gain (line/mic); Dump0 57 is setup
        // 0x15 Mute Outs. Neither byte, nor the 7-of-8 carrier of either, may
        // be written by any reviewed address.
        assert_eq!(packing_carrier(121), Some((124, 4)));
        for (_, _, address) in every_reviewed_address() {
            if address.location.part() != DumpPart::Part0 {
                continue;
            }
            for offset in address.location.offsets() {
                assert!(
                    ![121, 57, 124].contains(&offset),
                    "reviewed address writes forbidden Dump0 byte {offset}"
                );
            }
        }
        assert_eq!(INPUT_SUM_LOCATION.carrier(), None);
    }

    #[test]
    fn allowlist_is_closed() {
        for channel in 0..=0x7f_u8 {
            for parameter in 0..=0x7f_u8 {
                let reviewed = reviewed_address(channel, parameter).is_some();
                let output_channel = (5..=10).contains(&channel);
                let reviewed_peq = output_channel
                    && (parameter == EQ_ENABLED_PARAMETER
                        || parameter == EQ_COUNT_PARAMETER
                        || (FIRST_BAND_PARAMETER..=LAST_BAND_PARAMETER).contains(&parameter));
                // dec-autonomous-muted-bench-20261007: O4/O5/O6 mute, O3/O4
                // source, setup input sum. Nothing else.
                let reviewed_routing = ((8..=10).contains(&channel) && parameter == MUTE_PARAMETER)
                    || ((channel == 7 || channel == 8) && parameter == SOURCE_PARAMETER)
                    || (channel == SETUP_CHANNEL && parameter == INPUT_SUM_PARAMETER);
                let expected = reviewed_peq || reviewed_routing;
                assert_eq!(
                    reviewed, expected,
                    "channel {channel} parameter {parameter:#04x}"
                );
            }
        }
        assert_eq!(
            reviewed_address(8, band_parameter(3, BandField::Gain)).map(|address| address.field),
            Some(OutputField::Band {
                band: 3,
                field: BandField::Gain
            })
        );
    }

    #[test]
    fn every_representable_value_round_trips_through_every_location() {
        for rows in &OUTPUT_LOCATIONS {
            for location in rows {
                let mut frame = vec![0x55_u8 & 0x7f; dump_len(location.part)];
                let maximum = location.max_representable().min(0x3fff);
                for value in 0..=maximum {
                    location.write(&mut frame, value).unwrap();
                    assert_eq!(location.read(&frame), value);
                }
                assert!(location.write(&mut frame, maximum + 1).is_err() || maximum == 0x3fff);
            }
        }
    }
}
