//! Binary Output (Basic) Cluster
//!
//! See ZCL Section 3.14.4
//!
//! Reports and controls the state of a binary (on/off) output.

use core::sync::atomic::AtomicBool;
use core::sync::atomic::AtomicU32;
use core::sync::atomic::AtomicU8;
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
use crate::types::descriptors::ReadWrite;
use crate::types::descriptors::Reportable;
use crate::types::ids::AttributeId;
use crate::types::ids::ClusterId;
use crate::types::ids::TypeId;

/// Cluster descriptor (ZCL 3.14.4).
pub const CLUSTER: Cluster = Cluster::new(ClusterId(0x0010), "BinaryOutput");

/// Cluster identifier (ZCL 3.14.4).
pub const CLUSTER_ID: u16 = CLUSTER.id().0;

/// `PresentValue` (`Bool`), mandatory, writable and reportable.
pub const PRESENT_VALUE: Attribute<Bool, ReadWrite, Reportable> =
    CLUSTER.attribute(AttributeId(0x0055), "PresentValue");

/// `Description` (`String`).
pub const DESCRIPTION: Attribute<ShortText> =
    CLUSTER.attribute(AttributeId(0x001c), "Description");

const ATTRIBUTES: &[AttrInfo] = &[DESCRIPTION.attr_info(), PRESENT_VALUE.attr_info()];

/// Binary Output cluster server.
pub struct BinaryOutputServer<'a> {
    present_value: AtomicBool,
    description: &'static str,
    writes: AtomicU32,
    handles: AtomicU32,
    last_status: AtomicU8,
    reporting: Option<&'a dyn AttributeReporting>,
}

impl<'a> BinaryOutputServer<'a> {
    pub const fn new(initial: bool) -> Self {
        Self {
            present_value: AtomicBool::new(initial),
            description: "Binary output",
            writes: AtomicU32::new(0),
            handles: AtomicU32::new(0),
            last_status: AtomicU8::new(0),
            reporting: None,
        }
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

    pub fn write_count(&self) -> u32 {
        self.writes.load(Ordering::Relaxed)
    }

    pub fn handle_count(&self) -> u32 {
        self.handles.load(Ordering::Relaxed)
    }

    pub fn last_write_status(&self) -> u8 {
        self.last_status.load(Ordering::Relaxed)
    }
}

impl ClusterServer for BinaryOutputServer<'_> {
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
            attribute::PRESENT_VALUE => {
                PRESENT_VALUE.encode(self.present_value(), out, offset)
            }
            _ => return Status::UnsupportedAttribute,
        };

        match encoded {
            Ok(()) => Status::Success,
            Err(_) => Status::InsufficientSpace,
        }
    }

    fn decode_value(
        &self,
        id: AttributeId,
        type_id: TypeId,
        bytes: &[u8],
        offset: &mut usize,
    ) -> Status {
        if id.0 != attribute::PRESENT_VALUE {
            let status = Status::UnsupportedAttribute;
            self.last_status.store(status as u8, Ordering::Relaxed);
            return status;
        }
        self.writes.fetch_add(1, Ordering::Relaxed);
        let status = match PRESENT_VALUE.decode(type_id, bytes, offset) {
            Ok(value) => {
                self.set_present_value(value);
                Status::Success
            }
            Err(_) => Status::InvalidDataType,
        };
        self.last_status.store(status as u8, Ordering::Relaxed);
        status
    }

    fn reporting(&self) -> Option<&dyn AttributeReporting> {
        self.reporting
    }
}

impl ClusterRequestHandler for BinaryOutputServer<'_> {
    fn handle(&self, request: &ClusterRequest<'_>, out: &mut [u8]) -> Option<ClusterReply> {
        self.handles.fetch_add(1, Ordering::Relaxed);
        self.handle_request(request, out)
    }
}

pub mod attribute {
    pub const DESCRIPTION: u16 = super::DESCRIPTION.id().0;
    pub const PRESENT_VALUE: u16 = super::PRESENT_VALUE.id().0;
}