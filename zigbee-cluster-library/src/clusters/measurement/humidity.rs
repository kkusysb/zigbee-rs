//! Relative Humidity Measurement
//!
//! See Section 4.7
//!
//! Provides an interface to relative humidity measurement functionality,
//! including configuration and provision of notifications of relative humidity
//! measurements.
//!
//! The specification defines this cluster as one of three sharing the Water
//! Content Measurement attribute set, so the definitions live in
//! [`super::water_content`] and are re-exported here.

pub use super::water_content::FULL_SCALE;
pub use super::water_content::UNKNOWN;
pub use super::water_content::WaterContent;
pub use super::water_content::attribute;

/// Cluster identifier (ZCL 4.7).
pub const CLUSTER_ID: u16 = super::water_content::RELATIVE_HUMIDITY_CLUSTER_ID;

use core::sync::atomic::AtomicU16;
use core::sync::atomic::Ordering;

use zigbee_core::zdo::ClusterReply;
use zigbee_core::zdo::ClusterRequest;
use zigbee_core::zdo::ClusterRequestHandler;

use crate::frame::Status;
use crate::reporting::AttributeReporting;
use crate::server::ClusterServer;
use crate::types::descriptors::AttrInfo;
use crate::types::descriptors::Attribute;
use crate::types::descriptors::Cluster;
use crate::types::descriptors::ReadOnly;
use crate::types::descriptors::Reportable;
use crate::types::ids::AttributeId;
use crate::types::ids::ClusterId;
use crate::types::integers::Uint16;

/// Cluster descriptor (ZCL 4.7).
pub const CLUSTER: Cluster = Cluster::new(
    ClusterId(super::water_content::RELATIVE_HUMIDITY_CLUSTER_ID),
    "RelativeHumidity",
);

/// `MeasuredValue` (`Uint16`, hundredths of a percent), mandatory and reportable.
pub const MEASURED_VALUE: Attribute<Uint16, ReadOnly, Reportable> =
    CLUSTER.attribute(AttributeId(0x0000), "MeasuredValue");

/// `MinMeasuredValue` (`Uint16`).
pub const MIN_MEASURED_VALUE: Attribute<Uint16> =
    CLUSTER.attribute(AttributeId(0x0001), "MinMeasuredValue");

/// `MaxMeasuredValue` (`Uint16`).
pub const MAX_MEASURED_VALUE: Attribute<Uint16> =
    CLUSTER.attribute(AttributeId(0x0002), "MaxMeasuredValue");

/// `Tolerance` (`Uint16`).
pub const TOLERANCE: Attribute<Uint16> = CLUSTER.attribute(AttributeId(0x0003), "Tolerance");

const ATTRIBUTES: &[AttrInfo] = &[
    MEASURED_VALUE.attr_info(),
    MIN_MEASURED_VALUE.attr_info(),
    MAX_MEASURED_VALUE.attr_info(),
    TOLERANCE.attr_info(),
];

/// Relative Humidity Measurement cluster server (ZCL 4.7).
pub struct RelativeHumidityServer<'a> {
    measured_value: AtomicU16,
    min_measured_value: u16,
    max_measured_value: u16,
    tolerance: Option<u16>,
    reporting: Option<&'a dyn AttributeReporting>,
}

#[cfg(feature = "defmt")]
impl defmt::Format for RelativeHumidityServer<'_> {
    fn format(&self, f: defmt::Formatter<'_>) {
        defmt::write!(
            f,
            "RelativeHumidityServer {{ measured_value: {} }}",
            self.measured_value()
        );
    }
}

impl<'a> RelativeHumidityServer<'a> {
    pub const fn new(min: u16, max: u16) -> Self {
        Self {
            measured_value: AtomicU16::new(min),
            min_measured_value: min,
            max_measured_value: max,
            tolerance: None,
            reporting: None,
        }
    }

    #[must_use]
    pub const fn with_reporting(mut self, store: &'a dyn AttributeReporting) -> Self {
        self.reporting = Some(store);
        self
    }

    #[must_use]
    pub const fn with_tolerance(mut self, tolerance: u16) -> Self {
        self.tolerance = Some(tolerance);
        self
    }

    pub fn measured_value(&self) -> u16 {
        self.measured_value.load(Ordering::Relaxed)
    }

    pub fn set_measured_value(&self, value: u16) {
        let clamped = value.clamp(self.min_measured_value, self.max_measured_value);
        self.measured_value.store(clamped, Ordering::Relaxed);
    }
}

impl ClusterServer for RelativeHumidityServer<'_> {
    fn cluster(&self) -> Cluster {
        CLUSTER
    }

    fn attributes(&self) -> &'static [AttrInfo] {
        if self.tolerance.is_some() {
            ATTRIBUTES
        } else {
            &ATTRIBUTES[..3]
        }
    }

    fn encode_value(&self, id: AttributeId, out: &mut [u8], offset: &mut usize) -> Status {
        let encoded = match id.0 {
            attribute::MEASURED_VALUE => {
                MEASURED_VALUE.encode(Uint16(self.measured_value()), out, offset)
            }
            attribute::MIN_MEASURED_VALUE => {
                MIN_MEASURED_VALUE.encode(Uint16(self.min_measured_value), out, offset)
            }
            attribute::MAX_MEASURED_VALUE => {
                MAX_MEASURED_VALUE.encode(Uint16(self.max_measured_value), out, offset)
            }
            attribute::TOLERANCE => {
                let Some(tolerance) = self.tolerance else {
                    return Status::UnsupportedAttribute;
                };
                TOLERANCE.encode(Uint16(tolerance), out, offset)
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

impl ClusterRequestHandler for RelativeHumidityServer<'_> {
    fn handle(&self, request: &ClusterRequest<'_>, out: &mut [u8]) -> Option<ClusterReply> {
        self.handle_request(request, out)
    }
}
