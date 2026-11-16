//! Analog Input (Basic) Cluster
//!
//! See Section 3.14.2
//!
//! Provides an interface for reading the value of an analog measurement and
//! accessing various characteristics of that measurement. Carries a reading
//! the library has no dedicated cluster for — wind direction, for instance —
//! by pairing `PresentValue` with an `EngineeringUnits` code.

use core::sync::atomic::AtomicBool;
use core::sync::atomic::AtomicU8;
use core::sync::atomic::AtomicU32;
use core::sync::atomic::Ordering;

use zigbee_core::zdo::ClusterReply;
use zigbee_core::zdo::ClusterRequest;
use zigbee_core::zdo::ClusterRequestHandler;

use crate::frame::Status;
use crate::reporting::AttributeReporting;
use crate::server::ClusterServer;
use crate::types::Bitmap8;
use crate::types::Bool;
use crate::types::ShortStr;
use crate::types::ShortText;
use crate::types::Single;
use crate::types::ZclBitmap8;
use crate::types::descriptors::AttrInfo;
use crate::types::descriptors::Attribute;
use crate::types::descriptors::Cluster;
use crate::types::descriptors::NotReportable;
use crate::types::descriptors::ReadOnly;
use crate::types::descriptors::Reportable;
use crate::types::ids::AttributeId;
use crate::types::ids::ClusterId;
use crate::types::integers::Uint32;

/// Cluster identifier (ZCL 3.14.2).
pub const CLUSTER_ID: u16 = 0x000c;

/// Attribute identifiers (ZCL 3.14.2.4.2, Table 3-78).
///
/// `PresentValue`, `OutOfService` and `StatusFlags` are mandatory; the rest are
/// optional. `single` attributes carry an IEEE 754 single-precision float.
pub mod attribute {
    /// `Description` (`String`).
    pub const DESCRIPTION: u16 = 0x001c;
    /// `MaxPresentValue` (`Single`).
    pub const MAX_PRESENT_VALUE: u16 = 0x0041;
    /// `MinPresentValue` (`Single`).
    pub const MIN_PRESENT_VALUE: u16 = 0x0045;
    /// `OutOfService` (`Bool`), mandatory.
    pub const OUT_OF_SERVICE: u16 = 0x0051;
    /// `PresentValue` (`Single`), mandatory and reportable.
    pub const PRESENT_VALUE: u16 = 0x0055;
    /// `Reliability` (`Enum8`).
    pub const RELIABILITY: u16 = 0x0067;
    /// `Resolution` (`Single`).
    pub const RESOLUTION: u16 = 0x006a;
    /// `StatusFlags` (`Map8`), mandatory and reportable.
    pub const STATUS_FLAGS: u16 = 0x006f;
    /// `EngineeringUnits` (`Enum16`), see ZCL 3.14.11.10.
    pub const ENGINEERING_UNITS: u16 = 0x0075;
    /// `ApplicationType` (`Uint32`).
    pub const APPLICATION_TYPE: u16 = 0x0100;
}

/// `StatusFlags` (ZCL 3.14.11.3).
///
/// Four flags describing the health of the analog sensor. The relationship
/// between individual flags is not defined by the specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct StatusFlags(u8);

impl StatusFlags {
    /// The `EventState` attribute is not NORMAL. Always clear unless the
    /// cluster implementing `EventState` sits on the same endpoint.
    pub const IN_ALARM: u8 = 0b0000_0001;
    /// `Reliability` is present and not NO FAULT DETECTED.
    pub const FAULT: u8 = 0b0000_0010;
    /// `PresentValue` and `Reliability` no longer track the physical input.
    pub const OVERRIDDEN: u8 = 0b0000_0100;
    /// `OutOfService` is TRUE.
    pub const OUT_OF_SERVICE: u8 = 0b0000_1000;

    /// Wraps a raw attribute value, discarding the reserved upper bits
    /// (the valid range is 0x00 to 0x0f).
    pub const fn from_raw(raw: u8) -> Self {
        Self(raw & 0x0f)
    }

    /// The value as it appears on the wire.
    pub const fn raw(self) -> u8 {
        self.0
    }

    pub const fn is_set(self, flag: u8) -> bool {
        self.0 & flag != 0
    }

    #[must_use]
    pub const fn with(self, flag: u8) -> Self {
        Self(self.0 | (flag & 0x0f))
    }

    pub const fn in_alarm(self) -> bool {
        self.is_set(Self::IN_ALARM)
    }

    pub const fn fault(self) -> bool {
        self.is_set(Self::FAULT)
    }

    pub const fn overridden(self) -> bool {
        self.is_set(Self::OVERRIDDEN)
    }

    pub const fn out_of_service(self) -> bool {
        self.is_set(Self::OUT_OF_SERVICE)
    }
}

impl ZclBitmap8 for StatusFlags {
    fn from_bits(bits: u8) -> Self {
        Self::from_raw(bits)
    }

    fn into_bits(self) -> u8 {
        self.raw()
    }
}

/// Cluster descriptor (ZCL 3.14.2).
pub const CLUSTER: Cluster = Cluster::new(ClusterId(0x000c), "AnalogInput");

/// `Description` (`String`).
pub const DESCRIPTION: Attribute<ShortText> = CLUSTER.attribute(AttributeId(0x001c), "Description");

/// `OutOfService` (`Bool`), mandatory.
pub const OUT_OF_SERVICE: Attribute<Bool, ReadOnly, Reportable> =
    CLUSTER.attribute(AttributeId(0x0051), "OutOfService");

/// `PresentValue` (`Single`), mandatory and reportable.
pub const PRESENT_VALUE: Attribute<Single, ReadOnly, Reportable> =
    CLUSTER.attribute(AttributeId(0x0055), "PresentValue");

/// `StatusFlags` (`Bitmap8`), mandatory and reportable.
pub const STATUS_FLAGS: Attribute<Bitmap8<StatusFlags>, ReadOnly, Reportable> =
    CLUSTER.attribute(AttributeId(0x006f), "StatusFlags");

/// `Resolution` (`Single`), optional: the smallest change the input can
/// detect. Home Assistant uses it to pick the display precision.
pub const RESOLUTION: Attribute<Single, ReadOnly, NotReportable> =
    CLUSTER.attribute(AttributeId(0x006a), "Resolution");

/// `ApplicationType` (`Uint32`), optional: what the reading is. Bits 0-15 are
/// the application index, bits 16-23 the `AnalogInputType` (ZCL 3.14.11.6) and
/// bits 24-31 the application group. Home Assistant derives the unit and
/// device class from it — for a plain counter, `AnalogInputType::Count`
/// (0x0c) in the type byte: `0x000c_0000`.
pub const APPLICATION_TYPE: Attribute<Uint32, ReadOnly, NotReportable> =
    CLUSTER.attribute(AttributeId(0x0100), "ApplicationType");

/// `AnalogInputType::Count` (ZCL 3.14.11.6), the type byte of an
/// [`APPLICATION_TYPE`] value for a plain counter reading.
pub const APPLICATION_TYPE_COUNT: u32 = 0x000c_0000;

/// `AnalogInputType::Pressure_Pascal` (ZCL 3.14.11.6), the type byte of an
/// [`APPLICATION_TYPE`] value for a pressure reading in Pa. Home Assistant
/// maps it to a pressure sensor whose unit it can display in hPa with
/// decimals — beyond the whole-hPa granularity of the Pressure Measurement
/// cluster.
pub const APPLICATION_TYPE_PRESSURE: u32 = 0x0002_0000;

const ATTRIBUTES: &[AttrInfo] = &[
    DESCRIPTION.attr_info(),
    OUT_OF_SERVICE.attr_info(),
    PRESENT_VALUE.attr_info(),
    STATUS_FLAGS.attr_info(),
    RESOLUTION.attr_info(),
    APPLICATION_TYPE.attr_info(),
];

/// Analog Input cluster server.
///
/// `PresentValue` carries a raw reading, so it accepts any finite `f32`; keep
/// non-finite values out at the call site.
pub struct AnalogInputServer<'a> {
    present_value: AtomicU32,
    out_of_service: AtomicBool,
    status_flags: AtomicU8,
    resolution: AtomicU32,
    application_type: AtomicU32,
    description: &'static str,
    reporting: Option<&'a dyn AttributeReporting>,
}

#[cfg(feature = "defmt")]
impl defmt::Format for AnalogInputServer<'_> {
    fn format(&self, f: defmt::Formatter<'_>) {
        defmt::write!(
            f,
            "AnalogInputServer {{ present_value: {} }}",
            self.present_value()
        );
    }
}

impl<'a> AnalogInputServer<'a> {
    pub const fn new(initial: f32) -> Self {
        Self {
            present_value: AtomicU32::new(initial.to_bits()),
            out_of_service: AtomicBool::new(false),
            status_flags: AtomicU8::new(0),
            resolution: AtomicU32::new(1.0f32.to_bits()),
            application_type: AtomicU32::new(0),
            description: "Analog input",
            reporting: None,
        }
    }

    #[must_use]
    pub const fn with_description(mut self, description: &'static str) -> Self {
        self.description = description;
        self
    }

    #[must_use]
    pub const fn with_resolution(mut self, resolution: f32) -> Self {
        self.resolution = AtomicU32::new(resolution.to_bits());
        self
    }

    #[must_use]
    pub const fn with_application_type(mut self, application_type: u32) -> Self {
        self.application_type = AtomicU32::new(application_type);
        self
    }

    #[must_use]
    pub const fn with_reporting(mut self, store: &'a dyn AttributeReporting) -> Self {
        self.reporting = Some(store);
        self
    }

    pub fn present_value(&self) -> f32 {
        f32::from_bits(self.present_value.load(Ordering::Relaxed))
    }

    pub fn set_present_value(&self, value: f32) {
        self.present_value.store(value.to_bits(), Ordering::Relaxed);
    }

    pub fn out_of_service(&self) -> bool {
        self.out_of_service.load(Ordering::Relaxed)
    }

    pub fn set_out_of_service(&self, value: bool) {
        self.out_of_service.store(value, Ordering::Relaxed);
    }

    pub fn status_flags(&self) -> StatusFlags {
        StatusFlags::from_raw(self.status_flags.load(Ordering::Relaxed))
    }

    pub fn set_status_flags(&self, value: StatusFlags) {
        self.status_flags.store(value.raw(), Ordering::Relaxed);
    }

    /// The smallest change the input can detect; 1.0 unless overridden.
    pub fn resolution(&self) -> f32 {
        f32::from_bits(self.resolution.load(Ordering::Relaxed))
    }

    /// What the reading is, encoded per [`APPLICATION_TYPE`].
    pub fn application_type(&self) -> u32 {
        self.application_type.load(Ordering::Relaxed)
    }
}

impl ClusterServer for AnalogInputServer<'_> {
    fn cluster(&self) -> Cluster {
        CLUSTER
    }

    fn attributes(&self) -> &'static [AttrInfo] {
        ATTRIBUTES
    }

    fn encode_value(&self, id: AttributeId, out: &mut [u8], offset: &mut usize) -> Status {
        let encoded = match id.0 {
            attribute::DESCRIPTION => {
                let Some(text) = ShortStr::new(self.description) else {
                    return Status::InvalidValue;
                };
                DESCRIPTION.encode(text, out, offset)
            }
            attribute::OUT_OF_SERVICE => OUT_OF_SERVICE.encode(self.out_of_service(), out, offset),
            attribute::PRESENT_VALUE => {
                PRESENT_VALUE.encode(Single(self.present_value()), out, offset)
            }
            attribute::STATUS_FLAGS => STATUS_FLAGS.encode(self.status_flags(), out, offset),
            attribute::RESOLUTION => RESOLUTION.encode(Single(self.resolution()), out, offset),
            attribute::APPLICATION_TYPE => {
                APPLICATION_TYPE.encode(Uint32(self.application_type()), out, offset)
            }
            _ => return Status::UnsupportedAttribute,
        };

        match encoded {
            Ok(()) => Status::Success,
            Err(_) => Status::InsufficientSpace,
        }
    }

    fn reporting(&self) -> Option<&dyn AttributeReporting> {
        self.reporting
    }
}

impl ClusterRequestHandler for AnalogInputServer<'_> {
    fn handle(&self, request: &ClusterRequest<'_>, out: &mut [u8]) -> Option<ClusterReply> {
        self.handle_request(request, out)
    }
}

/// `Reliability` (ZCL 3.14.11.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[repr(u8)]
pub enum Reliability {
    NoFaultDetected = 0x00,
    NoSensor = 0x01,
    OverRange = 0x02,
    UnderRange = 0x03,
    OpenLoop = 0x04,
    ShortedLoop = 0x05,
    NoOutput = 0x06,
    UnreliableOther = 0x07,
    ProcessError = 0x08,
    MultiStateFault = 0x09,
    ConfigurationError = 0x0a,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ids::TypeId;

    // 3.14.11.3: bit 0 IN ALARM, 1 FAULT, 2 OVERRIDDEN, 3 OUT OF SERVICE
    #[test]
    fn status_flags_map_to_their_bits() {
        let flags = StatusFlags::from_raw(0b0000_1010);
        assert!(!flags.in_alarm());
        assert!(flags.fault());
        assert!(!flags.overridden());
        assert!(flags.out_of_service());
    }

    // the attribute range is 0x00 to 0x0f, so the upper nibble is reserved
    #[test]
    fn status_flags_discard_reserved_bits() {
        assert_eq!(StatusFlags::from_raw(0xff).raw(), 0x0f);
        assert_eq!(
            StatusFlags::default()
                .with(StatusFlags::FAULT)
                .with(StatusFlags::OVERRIDDEN)
                .raw(),
            0b0000_0110
        );
    }

    #[test]
    fn present_value_encodes_as_a_single() {
        let server = AnalogInputServer::new(12.5);
        let mut buf = [0u8; 8];
        let offset = &mut 0;

        let status = server.encode_value(AttributeId(attribute::PRESENT_VALUE), &mut buf, offset);

        assert_eq!(status, Status::Success);
        assert_eq!(buf[0], TypeId::SinglePrecision as u8);
        assert_eq!(
            f32::from_le_bytes(buf[1..5].try_into().expect("slice")),
            12.5
        );
        assert_eq!(*offset, 5);
    }

    #[test]
    fn an_unknown_attribute_is_unsupported() {
        let server = AnalogInputServer::new(0.0);

        assert_eq!(
            server.encode_value(AttributeId(0xffff), &mut [], &mut 0),
            Status::UnsupportedAttribute
        );
    }

    #[test]
    fn application_type_encodes_as_a_uint32() {
        let server = AnalogInputServer::new(1.0).with_application_type(APPLICATION_TYPE_COUNT);
        let mut buf = [0u8; 8];
        let offset = &mut 0;

        let status =
            server.encode_value(AttributeId(attribute::APPLICATION_TYPE), &mut buf, offset);

        assert_eq!(status, Status::Success);
        assert_eq!(buf[0], TypeId::Uint32 as u8);
        assert_eq!(
            u32::from_le_bytes(buf[1..5].try_into().expect("slice")),
            0x000c_0000
        );
        assert_eq!(*offset, 5);
    }
}
