//! Binary Input (Basic) Cluster
//!
//! See ZCL Section 3.14.3
//!
//! Reports the state of a binary (on/off) input.

use core::sync::atomic::AtomicBool;
use core::sync::atomic::Ordering;

use zigbee_core::zdo::ClusterReply;
use zigbee_core::zdo::ClusterRequest;
use zigbee_core::zdo::ClusterRequestHandler;

use crate::frame::Status;
use crate::reporting::AttributeReporting;
use crate::server::ClusterServer;
use crate::types::Bool;
use crate::types::ShortStr;
use crate::types::ShortText;
use crate::types::descriptors::AttrInfo;
use crate::types::descriptors::Attribute;
use crate::types::descriptors::Cluster;
use crate::types::descriptors::ReadOnly;
use crate::types::descriptors::Reportable;
use crate::types::ids::AttributeId;
use crate::types::ids::ClusterId;

/// Cluster descriptor (ZCL 3.14.3).
pub const CLUSTER: Cluster = Cluster::new(ClusterId(0x000f), "BinaryInput");

/// Cluster identifier (ZCL 3.14.3).
pub const CLUSTER_ID: u16 = CLUSTER.id().0;

/// `PresentValue` (`Bool`), mandatory and reportable.
pub const PRESENT_VALUE: Attribute<Bool, ReadOnly, Reportable> =
    CLUSTER.attribute(AttributeId(0x0055), "PresentValue");

/// `Description` (`String`).
pub const DESCRIPTION: Attribute<ShortText> = CLUSTER.attribute(AttributeId(0x001c), "Description");

const ATTRIBUTES: &[AttrInfo] = &[DESCRIPTION.attr_info(), PRESENT_VALUE.attr_info()];

/// Binary Input cluster server.
pub struct BinaryInputServer<'a> {
    present_value: AtomicBool,
    description: &'static str,
    reporting: Option<&'a dyn AttributeReporting>,
}

#[cfg(feature = "defmt")]
impl defmt::Format for BinaryInputServer<'_> {
    fn format(&self, f: defmt::Formatter<'_>) {
        defmt::write!(
            f,
            "BinaryInputServer {{ present_value: {} }}",
            self.present_value()
        );
    }
}

impl<'a> BinaryInputServer<'a> {
    pub const fn new(initial: bool) -> Self {
        Self {
            present_value: AtomicBool::new(initial),
            description: "Binary input",
            reporting: None,
        }
    }

    #[must_use]
    pub const fn with_description(mut self, description: &'static str) -> Self {
        self.description = description;
        self
    }

    #[must_use]
    pub const fn with_reporting(mut self, store: &'a dyn AttributeReporting) -> Self {
        self.reporting = Some(store);
        self
    }

    pub fn present_value(&self) -> bool {
        self.present_value.load(Ordering::Relaxed)
    }

    pub fn set_present_value(&self, value: bool) {
        self.present_value.store(value, Ordering::Relaxed);
    }
}

impl ClusterServer for BinaryInputServer<'_> {
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
            attribute::PRESENT_VALUE => PRESENT_VALUE.encode(self.present_value(), out, offset),
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

impl ClusterRequestHandler for BinaryInputServer<'_> {
    fn handle(&self, request: &ClusterRequest<'_>, out: &mut [u8]) -> Option<ClusterReply> {
        self.handle_request(request, out)
    }
}

pub mod attribute {
    pub const DESCRIPTION: u16 = super::DESCRIPTION.id().0;
    pub const PRESENT_VALUE: u16 = super::PRESENT_VALUE.id().0;
}
