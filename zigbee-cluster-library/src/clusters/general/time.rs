//! Time Cluster
//!
//! See ZCL Section 3.12
//!
//! A Time server keeps a real-time clock; [`TimeClient`] reads the mandatory
//! `Time` (`0x0000`) and `TimeStatus` (`0x0001`) attributes, plus the optional
//! `TimeZone` (`0x0002`), from another device and caches the last response.
//!
//! The client is a [`ClusterRequestHandler`]: put it in the handler chain as a
//! shared reference so it sees the `Read Attributes Response` the stack
//! delivers. Send a request with [`TimeClient::read`], then poll
//! [`TimeClient::time`] from the application. The response is matched by the
//! sequence number of the request, so a late answer to an older read cannot
//! overwrite a newer snapshot.
//!
//! `Time` counts seconds since 2000-01-01 UTC, not since the Unix epoch; use
//! [`UtcTime::unix_seconds`] to convert. `0xffff_ffff` means "time unknown"
//! (ZCL 2.6.2.21).

use core::sync::atomic::AtomicBool;
use core::sync::atomic::AtomicI32;
use core::sync::atomic::AtomicU8;
use core::sync::atomic::AtomicU32;
use core::sync::atomic::Ordering;

use byte::BytesExt;
use byte::TryRead;
use embedded_hal_async::delay::DelayNs;
use heapless::Vec;
use zigbee_core::aps::apsde::ApsdeSapConfirm;
use zigbee_core::zdo::ClusterReply;
use zigbee_core::zdo::ClusterRequest;
use zigbee_core::zdo::ClusterRequestHandler;

use crate::frame::GeneralCommand;
use crate::frame::ReadAttribute;
use crate::frame::Status;
use crate::frame::ZclFrame;
use crate::frame::header::ZclHeader;
use crate::frame::header::command_identifier::CommandIdentifier;
use crate::frame::header::frame_control::FrameControl;
use crate::frame::payload::ZclFramePayload;
use crate::sender::ZclSendError;
use crate::sender::ZclSender;
use crate::sender::ZclUnicast;
use crate::types::ZclBitmap8;
use crate::types::bitmaps::Bitmap8;
use crate::types::codec::zcl_type;
use crate::types::data_types::BitmapN;
use crate::types::data_types::SignedN;
use crate::types::data_types::TimeType;
use crate::types::data_types::ZclDataType;
use crate::types::descriptors::Attribute;
use crate::types::descriptors::Cluster;
use crate::types::ids::AttributeId;
use crate::types::ids::ClusterId;
use crate::types::ids::TypeId;
use crate::types::integers::Int32;

/// Cluster descriptor (ZCL 3.12).
pub const CLUSTER: Cluster = Cluster::new(ClusterId(0x000a), "Time");

/// Cluster identifier (ZCL 3.12), for matching against a received frame.
pub const CLUSTER_ID: u16 = CLUSTER.id().0;

/// Seconds between the Unix epoch and the ZCL time epoch (2000-01-01 UTC).
pub const UNIX_EPOCH_OFFSET_SECONDS: u64 = 946_684_800;

/// Largest number of attributes one `Read Attributes` request can carry.
pub const MAX_ATTRIBUTES: usize = 16;

/// Header (3 bytes) plus one identifier per attribute.
const MAX_REQUEST_LEN: usize = 3 + 2 * MAX_ATTRIBUTES;

zcl_type! {
    #[type_id = TypeId::UtcTime]
    #[null = 0xffff_ffff]
    /// `UTCTime` (ZCL 2.6.2.21), seconds since 2000-01-01 UTC.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    #[cfg_attr(feature = "defmt", derive(defmt::Format))]
    pub struct UtcTime(pub u32);
}

impl UtcTime {
    /// The value that means "time unknown" (ZCL 2.6.2.21).
    pub const INVALID: Self = Self(0xffff_ffff);

    /// Whether this is a real time rather than [`Self::INVALID`].
    pub const fn is_valid(self) -> bool {
        self.0 != Self::INVALID.0
    }

    /// Seconds since the Unix epoch, `None` for [`Self::INVALID`].
    #[allow(clippy::cast_lossless)]
    pub const fn unix_seconds(self) -> Option<u64> {
        if self.is_valid() {
            Some(self.0 as u64 + UNIX_EPOCH_OFFSET_SECONDS)
        } else {
            None
        }
    }
}

/// `TimeStatus` (ZCL 3.12.2.2.2).
///
/// Four flags describing how the server obtained its clock. A server may set
/// more than one; check the flags a use case actually needs rather than
/// requiring an exact value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct TimeStatus(u8);

impl TimeStatus {
    /// The server is a master clock.
    pub const MASTER: u8 = 0b0000_0001;
    /// The server has synchronized with a master.
    pub const SYNCHRONIZED: u8 = 0b0000_0010;
    /// The server is a master for time zone and DST.
    pub const MASTER_FOR_ZONE_AND_DST: u8 = 0b0000_0100;
    /// The server is superseding other time servers.
    pub const SUPERSEDING: u8 = 0b0000_1000;

    /// Wraps a raw attribute value, discarding reserved bits.
    pub const fn from_raw(raw: u8) -> Self {
        Self(raw & 0x0f)
    }

    /// The value as it appears on the wire.
    pub const fn raw(self) -> u8 {
        self.0
    }

    /// Whether `flag` is set.
    pub const fn is_set(self, flag: u8) -> bool {
        self.0 & flag != 0
    }

    /// Whether [`Self::MASTER`] is set.
    pub const fn is_master(self) -> bool {
        self.is_set(Self::MASTER)
    }

    /// Whether [`Self::SYNCHRONIZED`] is set.
    pub const fn is_synchronized(self) -> bool {
        self.is_set(Self::SYNCHRONIZED)
    }

    /// Whether [`Self::MASTER_FOR_ZONE_AND_DST`] is set.
    pub const fn is_master_for_zone_and_dst(self) -> bool {
        self.is_set(Self::MASTER_FOR_ZONE_AND_DST)
    }

    /// Whether [`Self::SUPERSEDING`] is set.
    pub const fn is_superseding(self) -> bool {
        self.is_set(Self::SUPERSEDING)
    }
}

impl ZclBitmap8 for TimeStatus {
    fn from_bits(bits: u8) -> Self {
        Self::from_raw(bits)
    }

    fn into_bits(self) -> u8 {
        self.raw()
    }
}

/// `Time` (`UTCTime`), mandatory and read/write (ZCL 3.12.2.2.1).
pub const TIME: Attribute<UtcTime> = CLUSTER.attribute(AttributeId(0x0000), "Time");

/// `TimeStatus` (`bitmap8`), mandatory and read/write (ZCL 3.12.2.2.2).
pub const TIME_STATUS: Attribute<Bitmap8<TimeStatus>> =
    CLUSTER.attribute(AttributeId(0x0001), "TimeStatus");

/// `TimeZone` (`int32`), optional and read/write (ZCL 3.12.2.2.3).
pub const TIME_ZONE: Attribute<Int32> = CLUSTER.attribute(AttributeId(0x0002), "TimeZone");

/// Bare attribute identifiers, for matching against a received record.
pub mod attribute {
    /// `Time` (`UTCTime`).
    pub const TIME: u16 = super::TIME.id().0;
    /// `TimeStatus` (`bitmap8`).
    pub const TIME_STATUS: u16 = super::TIME_STATUS.id().0;
    /// `TimeZone` (`int32`, seconds east of UTC).
    pub const TIME_ZONE: u16 = super::TIME_ZONE.id().0;
}

/// Attributes [`TimeClient::read`] asks for, in request order.
const READ_ATTRIBUTES: [u16; 3] = [
    attribute::TIME,
    attribute::TIME_STATUS,
    attribute::TIME_ZONE,
];

/// Errors produced while sending a Time cluster read.
#[derive(Debug)]
pub enum TimeClientError {
    /// A read request must carry at least one attribute.
    NoAttributes,
    /// More attributes than one `Read Attributes` frame can carry.
    TooManyAttributes,
    /// The request frame did not fit its buffer.
    Encode(byte::Error),
    /// The APS layer rejected the transfer.
    Send(ZclSendError),
}

impl From<byte::Error> for TimeClientError {
    fn from(error: byte::Error) -> Self {
        Self::Encode(error)
    }
}

impl From<ZclSendError> for TimeClientError {
    fn from(error: ZclSendError) -> Self {
        Self::Send(error)
    }
}

/// Caches the last `Read Attributes Response` from a Time server.
///
/// One client handles one outstanding request at a time: [`read`](Self::read)
/// records the sequence number it sent, and a response with a different
/// sequence number is ignored. Poll [`time`](Self::time) after sending;
/// [`is_pending`](Self::is_pending) tells whether the answer has arrived.
#[derive(Debug)]
pub struct TimeClient {
    time: AtomicU32,
    time_status: AtomicU8,
    time_zone: AtomicI32,
    sequence: AtomicU8,
    pending_sequence: AtomicU8,
    pending: AtomicBool,
    last_status: AtomicU8,
}

#[cfg(feature = "defmt")]
impl defmt::Format for TimeClient {
    fn format(&self, f: defmt::Formatter<'_>) {
        defmt::write!(
            f,
            "TimeClient {{ time: {}, status: {}, zone: {} }}",
            self.time().0,
            self.time_status().raw(),
            self.time_zone().unwrap_or(i32::MIN)
        );
    }
}

impl TimeClient {
    /// A client with no cached response.
    pub const fn new() -> Self {
        Self {
            time: AtomicU32::new(UtcTime::INVALID.0),
            time_status: AtomicU8::new(0),
            time_zone: AtomicI32::new(i32::MIN),
            sequence: AtomicU8::new(0),
            pending_sequence: AtomicU8::new(0),
            pending: AtomicBool::new(false),
            last_status: AtomicU8::new(0xff),
        }
    }

    /// Last `Time` value, [`UtcTime::INVALID`] until a response arrives.
    pub fn time(&self) -> UtcTime {
        UtcTime(self.time.load(Ordering::Relaxed))
    }

    /// Last `TimeStatus` value; all flags clear until a response arrives.
    pub fn time_status(&self) -> TimeStatus {
        TimeStatus::from_raw(self.time_status.load(Ordering::Relaxed))
    }

    /// Last `TimeZone` value in seconds east of UTC, `None` until a response
    /// arrives or when the server did not return the optional attribute.
    pub fn time_zone(&self) -> Option<i32> {
        let zone = self.time_zone.load(Ordering::Relaxed);
        (zone != i32::MIN).then_some(zone)
    }

    /// Whether a valid `Time` value has been received.
    pub fn has_time(&self) -> bool {
        self.time().is_valid()
    }

    /// Whether a request is waiting for its response.
    pub fn is_pending(&self) -> bool {
        self.pending.load(Ordering::Acquire)
    }

    /// Status of the last matching response, `None` until one arrives.
    pub fn last_status(&self) -> Option<Status> {
        Status::from_u8(self.last_status.load(Ordering::Relaxed))
    }

    /// Read `Time`, `TimeStatus` and `TimeZone` from `target`.
    ///
    /// The cluster identifier in `target` is replaced with [`CLUSTER_ID`], so
    /// callers only supply the addressing fields. `target` usually addresses
    /// the coordinator's endpoint 1 with the Home Automation profile.
    pub async fn read<S: ZclSender>(
        &self,
        sender: &S,
        target: ZclUnicast,
        delay: &mut impl DelayNs,
    ) -> Result<ApsdeSapConfirm, TimeClientError> {
        self.read_attributes(sender, target, &READ_ATTRIBUTES, delay)
            .await
    }

    /// Read an arbitrary list of Time cluster attributes.
    ///
    /// At most [`MAX_ATTRIBUTES`] identifiers fit one request. The cluster
    /// identifier in `target` is replaced with [`CLUSTER_ID`].
    pub async fn read_attributes<S: ZclSender>(
        &self,
        sender: &S,
        target: ZclUnicast,
        attributes: &[u16],
        delay: &mut impl DelayNs,
    ) -> Result<ApsdeSapConfirm, TimeClientError> {
        if attributes.is_empty() {
            return Err(TimeClientError::NoAttributes);
        }
        if attributes.len() > MAX_ATTRIBUTES {
            return Err(TimeClientError::TooManyAttributes);
        }

        let sequence_number = self.sequence.fetch_add(1, Ordering::Relaxed);
        let mut buf = [0u8; MAX_REQUEST_LEN];
        let asdu = encode_read_request(&mut buf, sequence_number, attributes)?;

        self.pending_sequence
            .store(sequence_number, Ordering::Relaxed);
        self.pending.store(true, Ordering::Release);

        let result = sender
            .send_zcl_unicast(
                ZclUnicast {
                    cluster_id: CLUSTER_ID,
                    ..target
                },
                asdu,
                delay,
            )
            .await;

        if result.is_err() {
            self.pending.store(false, Ordering::Release);
        }
        result.map_err(TimeClientError::Send)
    }
}

impl Default for TimeClient {
    fn default() -> Self {
        Self::new()
    }
}

impl ClusterRequestHandler for TimeClient {
    fn handle(&self, request: &ClusterRequest<'_>, _out: &mut [u8]) -> Option<ClusterReply> {
        if request.cluster_id != CLUSTER_ID {
            return None;
        }

        let (frame, _) = ZclFrame::try_read(request.asdu, ()).ok()?;
        if !self.pending.load(Ordering::Acquire)
            || frame.header.sequence_number != self.pending_sequence.load(Ordering::Relaxed)
        {
            return None;
        }

        match frame.header.command_identifier {
            CommandIdentifier::ReadAttributesResponse => {
                let ZclFramePayload::GeneralCommand(GeneralCommand::ReadAttributesResponse(
                    records,
                )) = frame.payload
                else {
                    return None;
                };

                for record in records {
                    if record.status != Status::Success {
                        continue;
                    }
                    match (record.attribute_id, record.value) {
                        (attribute::TIME, Some(ZclDataType::Time(TimeType::UTCTime(value)))) => {
                            self.time.store(value, Ordering::Relaxed);
                        }
                        (
                            attribute::TIME_STATUS,
                            Some(ZclDataType::Bitmap(BitmapN::Bitmap8(value))),
                        ) => {
                            self.time_status.store(value, Ordering::Relaxed);
                        }
                        (
                            attribute::TIME_ZONE,
                            Some(ZclDataType::SignedInt(SignedN::Int32(value))),
                        ) if value != i32::MIN => {
                            self.time_zone.store(value, Ordering::Relaxed);
                        }
                        _ => {}
                    }
                }

                self.last_status
                    .store(Status::Success.to_u8().unwrap_or(0xff), Ordering::Relaxed);
                self.pending.store(false, Ordering::Release);
            }
            CommandIdentifier::DefaultResponse => {
                let ZclFramePayload::GeneralCommand(GeneralCommand::DefaultResponse(response)) =
                    frame.payload
                else {
                    return None;
                };

                self.last_status
                    .store(response.status.to_u8().unwrap_or(0xff), Ordering::Relaxed);
                self.pending.store(false, Ordering::Release);
            }
            _ => return None,
        }

        None
    }
}

/// Serialize a `Read Attributes` request for the Time cluster.
fn encode_read_request<'a>(
    buf: &'a mut [u8],
    sequence_number: u8,
    attributes: &[u16],
) -> Result<&'a [u8], TimeClientError> {
    let mut records = Vec::<ReadAttribute, MAX_ATTRIBUTES>::new();
    for &attribute_id in attributes {
        records
            .push(ReadAttribute { attribute_id })
            .map_err(|_| TimeClientError::TooManyAttributes)?;
    }

    let frame = ZclFrame {
        header: ZclHeader {
            frame_control: FrameControl(0x00),
            manufacturer_code: None,
            sequence_number,
            command_identifier: CommandIdentifier::ReadAttributes,
        },
        payload: ZclFramePayload::GeneralCommand(GeneralCommand::ReadAttributes(records)),
    };

    let offset = &mut 0;
    buf.write_with(offset, frame, ())?;
    Ok(&buf[..*offset])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(asdu: &[u8]) -> ClusterRequest<'_> {
        ClusterRequest {
            profile_id: 0x0104,
            cluster_id: CLUSTER_ID,
            src_endpoint: 1,
            dst_endpoint: 1,
            unicast: true,
            asdu,
        }
    }

    #[test]
    fn read_request_asks_for_time_status_and_zone() {
        let mut buf = [0u8; MAX_REQUEST_LEN];
        let asdu = encode_read_request(&mut buf, 0x2a, &READ_ATTRIBUTES).expect("encode");
        let (frame, used) = ZclFrame::try_read(asdu, ()).expect("parse");

        assert_eq!(used, asdu.len());
        assert_eq!(frame.header.sequence_number, 0x2a);
        assert_eq!(
            frame.header.command_identifier,
            CommandIdentifier::ReadAttributes
        );

        let ZclFramePayload::GeneralCommand(GeneralCommand::ReadAttributes(records)) =
            frame.payload
        else {
            panic!("expected ReadAttributes");
        };
        assert_eq!(records.len(), 3);
        assert_eq!(records[0].attribute_id, attribute::TIME);
        assert_eq!(records[1].attribute_id, attribute::TIME_STATUS);
        assert_eq!(records[2].attribute_id, attribute::TIME_ZONE);
    }

    #[test]
    fn response_updates_the_cached_snapshot() {
        let client = TimeClient::new();
        client.pending_sequence.store(7, Ordering::Relaxed);
        client.pending.store(true, Ordering::Release);

        // Read Attributes Response: Time, TimeStatus and TimeZone.
        let asdu = [
            0x18, 0x07, 0x01, // frame control, seq, ReadAttributesResponse
            0x00, 0x00, 0x00, 0xe2, 0x10, 0x20, 0x30, 0x40, // Time = 0x40302010
            0x01, 0x00, 0x00, 0x18, 0x03, // TimeStatus = Master | Synchronized
            0x02, 0x00, 0x00, 0x2b, 0x10, 0x0e, 0x00, 0x00, // TimeZone = 3600 s
        ];
        let mut out = [0u8; 8];

        assert_eq!(client.handle(&request(&asdu), &mut out), None);
        assert_eq!(client.time().0, 0x4030_2010);
        assert_eq!(client.time_status().raw(), 0x03);
        assert!(client.time_status().is_master());
        assert!(client.time_status().is_synchronized());
        assert_eq!(client.time_zone(), Some(3600));
        assert!(!client.is_pending());
        assert_eq!(client.last_status(), Some(Status::Success));
    }

    #[test]
    fn response_with_a_different_sequence_is_ignored() {
        let client = TimeClient::new();
        client.pending_sequence.store(7, Ordering::Relaxed);
        client.pending.store(true, Ordering::Release);

        let asdu = [
            0x18, 0x08, 0x01, // frame control, seq, ReadAttributesResponse
            0x00, 0x00, 0x00, 0xe2, 0x01, 0x00, 0x00, 0x00, // Time
        ];
        let mut out = [0u8; 8];

        assert_eq!(client.handle(&request(&asdu), &mut out), None);
        assert!(!client.has_time());
        assert!(client.is_pending());
        assert_eq!(client.last_status(), None);
    }

    #[test]
    fn invalid_time_is_not_reported_as_a_time() {
        assert!(!UtcTime::INVALID.is_valid());
        assert_eq!(UtcTime::INVALID.unix_seconds(), None);
        assert_eq!(UtcTime(0).unix_seconds(), Some(UNIX_EPOCH_OFFSET_SECONDS));
    }

    #[test]
    fn other_clusters_are_left_alone() {
        let client = TimeClient::new();
        let asdu = [0x18, 0x01, 0x01];
        let mut out = [0u8; 8];
        let other = ClusterRequest {
            cluster_id: 0x0006,
            ..request(&asdu)
        };

        assert_eq!(client.handle(&other, &mut out), None);
        assert!(!client.is_pending());
    }
}
