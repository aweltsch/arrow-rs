// Licensed to the Apache Software Foundation (ASF) under one
// or more contributor license agreements.  See the NOTICE file
// distributed with this work for additional information
// regarding copyright ownership.  The ASF licenses this file
// to you under the Apache License, Version 2.0 (the
// "License"); you may not use this file except in compliance
// with the License.  You may obtain a copy of the License at
//
//   http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing,
// software distributed under the License is distributed on an
// "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied.  See the License for the
// specific language governing permissions and limitations
// under the License.

//! A native Rust implementation of [Apache Arrow Flight](https://arrow.apache.org/docs/format/Flight.html)
//! for exchanging [Arrow](https://arrow.apache.org) data between processes.
//!
//! Please see the [arrow-flight crates.io](https://crates.io/crates/arrow-flight)
//! page for feature flags and more information.
//!
//! # Overview
//!
//! This crate contains:
//!
//! 1. Low level [prost] generated structs
//!  for Flight gRPC protobuf messages, such as [`FlightData`].
//!
//! 2. Low level [tonic] generated [`flight_service_client`] and
//! [`flight_service_server`].
//!
//! 3. Experimental support for [Flight SQL] in [`sql`]. Requires the
//! `flight-sql-experimental` feature of this crate to be activated.
//!
//! [Flight SQL]: https://arrow.apache.org/docs/format/FlightSql.html
#![allow(rustdoc::invalid_html_tags)]

use arrow_ipc::{convert, writer, writer::EncodedData, writer::IpcWriteOptions};
use arrow_schema::{ArrowError, Schema};

use arrow_ipc::convert::try_schema_from_ipc_buffer;
use base64::prelude::BASE64_STANDARD;
use base64::Engine;
use bytes::Bytes;
use std::{
    convert::{TryFrom, TryInto},
    fmt,
    ops::Deref,
};

type ArrowResult<T> = std::result::Result<T, ArrowError>;

#[allow(clippy::derive_partial_eq_without_eq)]

mod gen {
    include!("arrow.flight.protocol.rs");
}

/// Defines a `Flight` for generation or retrieval.
pub mod flight_descriptor {
    use super::gen;
    pub use gen::flight_descriptor::DescriptorType;
}

/// Low Level [tonic] [`FlightServiceClient`](gen::flight_service_client::FlightServiceClient).
pub mod flight_service_client {
    use super::gen;
    pub use gen::flight_service_client::FlightServiceClient;
}

/// Low Level [tonic] [`FlightServiceServer`](gen::flight_service_server::FlightServiceServer)
/// and [`FlightService`](gen::flight_service_server::FlightService).
pub mod flight_service_server {
    use super::gen;
    pub use gen::flight_service_server::FlightService;
    pub use gen::flight_service_server::FlightServiceServer;
}

/// Mid Level [`FlightClient`]
pub mod client;
pub use client::FlightClient;

/// Decoder to create [`RecordBatch`](arrow_array::RecordBatch) streams from [`FlightData`] streams.
/// See [`FlightRecordBatchStream`](decode::FlightRecordBatchStream).
pub mod decode;

/// Encoder to create [`FlightData`] streams from [`RecordBatch`](arrow_array::RecordBatch) streams.
/// See [`FlightDataEncoderBuilder`](encode::FlightDataEncoderBuilder).
pub mod encode;

/// Common error types
pub mod error;

pub use gen::Action;
pub use gen::ActionType;
pub use gen::BasicAuth;
pub use gen::Criteria;
pub use gen::Empty;
pub use gen::FlightData;
pub use gen::FlightDescriptor;
pub use gen::FlightEndpoint;
pub use gen::FlightInfo;
pub use gen::HandshakeRequest;
pub use gen::HandshakeResponse;
pub use gen::Location;
pub use gen::PutResult;
pub use gen::Result;
pub use gen::SchemaResult;
pub use gen::Ticket;

pub mod utils;

#[cfg(feature = "flight-sql-experimental")]
pub mod sql;

use flight_descriptor::DescriptorType;

/// SchemaAsIpc represents a pairing of a `Schema` with IpcWriteOptions
pub struct SchemaAsIpc<'a> {
    pub pair: (&'a Schema, &'a IpcWriteOptions),
}

/// IpcMessage represents a `Schema` in the format expected in
/// `FlightInfo.schema`
#[derive(Debug)]
pub struct IpcMessage(pub Bytes);

// Useful conversion functions

fn flight_schema_as_encoded_data(
    arrow_schema: &Schema,
    options: &IpcWriteOptions,
) -> EncodedData {
    let data_gen = writer::IpcDataGenerator::default();
    data_gen.schema_to_bytes(arrow_schema, options)
}

fn flight_schema_as_flatbuffer(schema: &Schema, options: &IpcWriteOptions) -> IpcMessage {
    let encoded_data = flight_schema_as_encoded_data(schema, options);
    IpcMessage(encoded_data.ipc_message.into())
}

// Implement a bunch of useful traits for various conversions, displays,
// etc...

// Deref

impl Deref for IpcMessage {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'a> Deref for SchemaAsIpc<'a> {
    type Target = (&'a Schema, &'a IpcWriteOptions);

    fn deref(&self) -> &Self::Target {
        &self.pair
    }
}

// Display...

/// Limits the output of value to limit...
fn limited_fmt(f: &mut fmt::Formatter<'_>, value: &[u8], limit: usize) -> fmt::Result {
    if value.len() > limit {
        write!(f, "{:?}", &value[..limit])
    } else {
        write!(f, "{:?}", &value)
    }
}

impl fmt::Display for FlightData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FlightData {{")?;
        write!(f, " descriptor: ")?;
        match &self.flight_descriptor {
            Some(d) => write!(f, "{d}")?,
            None => write!(f, "None")?,
        };
        write!(f, ", header: ")?;
        limited_fmt(f, &self.data_header, 8)?;
        write!(f, ", metadata: ")?;
        limited_fmt(f, &self.app_metadata, 8)?;
        write!(f, ", body: ")?;
        limited_fmt(f, &self.data_body, 8)?;
        write!(f, " }}")
    }
}

impl fmt::Display for FlightDescriptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FlightDescriptor {{")?;
        write!(f, " type: ")?;
        match self.r#type() {
            DescriptorType::Cmd => {
                write!(f, "cmd, value: ")?;
                limited_fmt(f, &self.cmd, 8)?;
            }
            DescriptorType::Path => {
                write!(f, "path: [")?;
                let mut sep = "";
                for element in &self.path {
                    write!(f, "{sep}{element}")?;
                    sep = ", ";
                }
                write!(f, "]")?;
            }
            DescriptorType::Unknown => {
                write!(f, "unknown")?;
            }
        }
        write!(f, " }}")
    }
}

impl fmt::Display for FlightEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FlightEndpoint {{")?;
        write!(f, " ticket: ")?;
        match &self.ticket {
            Some(value) => write!(f, "{value}"),
            None => write!(f, " none"),
        }?;
        write!(f, ", location: [")?;
        let mut sep = "";
        for location in &self.location {
            write!(f, "{sep}{location}")?;
            sep = ", ";
        }
        write!(f, "]")?;
        write!(f, " }}")
    }
}

impl fmt::Display for FlightInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ipc_message = IpcMessage(self.schema.clone());
        let schema: Schema = ipc_message.try_into().map_err(|_err| fmt::Error)?;
        write!(f, "FlightInfo {{")?;
        write!(f, " schema: {schema}")?;
        write!(f, ", descriptor:")?;
        match &self.flight_descriptor {
            Some(d) => write!(f, " {d}"),
            None => write!(f, " None"),
        }?;
        write!(f, ", endpoint: [")?;
        let mut sep = "";
        for endpoint in &self.endpoint {
            write!(f, "{sep}{endpoint}")?;
            sep = ", ";
        }
        write!(f, "], total_records: {}", self.total_records)?;
        write!(f, ", total_bytes: {}", self.total_bytes)?;
        write!(f, " }}")
    }
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Location {{")?;
        write!(f, " uri: ")?;
        write!(f, "{}", self.uri)
    }
}

impl fmt::Display for Ticket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Ticket {{")?;
        write!(f, " ticket: ")?;
        write!(f, "{}", BASE64_STANDARD.encode(&self.ticket))
    }
}

// From...

impl From<EncodedData> for FlightData {
    fn from(data: EncodedData) -> Self {
        FlightData {
            data_header: data.ipc_message.into(),
            data_body: data.arrow_data.into(),
            ..Default::default()
        }
    }
}

impl From<SchemaAsIpc<'_>> for FlightData {
    fn from(schema_ipc: SchemaAsIpc) -> Self {
        let IpcMessage(vals) = flight_schema_as_flatbuffer(schema_ipc.0, schema_ipc.1);
        FlightData {
            data_header: vals,
            ..Default::default()
        }
    }
}

impl TryFrom<SchemaAsIpc<'_>> for SchemaResult {
    type Error = ArrowError;

    fn try_from(schema_ipc: SchemaAsIpc) -> ArrowResult<Self> {
        // According to the definition from `Flight.proto`
        // The schema of the dataset in its IPC form:
        //   4 bytes - an optional IPC_CONTINUATION_TOKEN prefix
        //   4 bytes - the byte length of the payload
        //   a flatbuffer Message whose header is the Schema
        let IpcMessage(vals) = schema_to_ipc_format(schema_ipc)?;
        Ok(SchemaResult { schema: vals })
    }
}

// TryFrom...

impl TryFrom<i32> for DescriptorType {
    type Error = ArrowError;

    fn try_from(value: i32) -> ArrowResult<Self> {
        value.try_into()
    }
}

impl TryFrom<SchemaAsIpc<'_>> for IpcMessage {
    type Error = ArrowError;

    fn try_from(schema_ipc: SchemaAsIpc) -> ArrowResult<Self> {
        schema_to_ipc_format(schema_ipc)
    }
}

fn schema_to_ipc_format(schema_ipc: SchemaAsIpc) -> ArrowResult<IpcMessage> {
    let pair = *schema_ipc;
    let encoded_data = flight_schema_as_encoded_data(pair.0, pair.1);

    let mut schema = vec![];
    writer::write_message(&mut schema, encoded_data, pair.1)?;
    Ok(IpcMessage(schema.into()))
}

impl TryFrom<&FlightData> for Schema {
    type Error = ArrowError;
    fn try_from(data: &FlightData) -> ArrowResult<Self> {
        convert::try_schema_from_flatbuffer_bytes(&data.data_header[..]).map_err(|err| {
            ArrowError::ParseError(format!(
                "Unable to convert flight data to Arrow schema: {err}"
            ))
        })
    }
}

impl TryFrom<FlightInfo> for Schema {
    type Error = ArrowError;

    fn try_from(value: FlightInfo) -> ArrowResult<Self> {
        value.try_decode_schema()
    }
}

impl TryFrom<IpcMessage> for Schema {
    type Error = ArrowError;

    fn try_from(value: IpcMessage) -> ArrowResult<Self> {
        try_schema_from_ipc_buffer(&value)
    }
}

impl TryFrom<&SchemaResult> for Schema {
    type Error = ArrowError;
    fn try_from(data: &SchemaResult) -> ArrowResult<Self> {
        try_schema_from_ipc_buffer(&data.schema)
    }
}

impl TryFrom<SchemaResult> for Schema {
    type Error = ArrowError;
    fn try_from(data: SchemaResult) -> ArrowResult<Self> {
        (&data).try_into()
    }
}

// FlightData, FlightDescriptor, etc..

impl FlightData {
    pub fn new(
        flight_descriptor: Option<FlightDescriptor>,
        message: IpcMessage,
        app_metadata: impl Into<Bytes>,
        data_body: impl Into<Bytes>,
    ) -> Self {
        let IpcMessage(vals) = message;
        FlightData {
            flight_descriptor,
            data_header: vals,
            app_metadata: app_metadata.into(),
            data_body: data_body.into(),
        }
    }
}

impl FlightDescriptor {
    pub fn new_cmd(cmd: impl Into<Bytes>) -> Self {
        FlightDescriptor {
            r#type: DescriptorType::Cmd.into(),
            cmd: cmd.into(),
            ..Default::default()
        }
    }

    pub fn new_path(path: Vec<String>) -> Self {
        FlightDescriptor {
            r#type: DescriptorType::Path.into(),
            path,
            ..Default::default()
        }
    }
}

impl FlightInfo {
    pub fn new(
        message: IpcMessage,
        flight_descriptor: Option<FlightDescriptor>,
        endpoint: Vec<FlightEndpoint>,
        total_records: i64,
        total_bytes: i64,
    ) -> Self {
        let IpcMessage(vals) = message;
        FlightInfo {
            schema: vals,
            flight_descriptor,
            endpoint,
            total_records,
            total_bytes,
        }
    }

    /// Try and convert the data in this  `FlightInfo` into a [`Schema`]
    pub fn try_decode_schema(self) -> ArrowResult<Schema> {
        let msg = IpcMessage(self.schema);
        msg.try_into()
    }
}

impl<'a> SchemaAsIpc<'a> {
    pub fn new(schema: &'a Schema, options: &'a IpcWriteOptions) -> Self {
        SchemaAsIpc {
            pair: (schema, options),
        }
    }
}

impl Action {
    /// Create a new Action with type and body
    pub fn new(action_type: impl Into<String>, body: impl Into<Bytes>) -> Self {
        Self {
            r#type: action_type.into(),
            body: body.into(),
        }
    }
}

impl Result {
    /// Create a new Result with the specified body
    pub fn new(body: impl Into<Bytes>) -> Self {
        Self { body: body.into() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_ipc::MetadataVersion;
    use arrow_schema::{DataType, Field, TimeUnit};

    struct TestVector(Vec<u8>, usize);

    impl fmt::Display for TestVector {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            limited_fmt(f, &self.0, self.1)
        }
    }

    #[test]
    fn it_creates_flight_descriptor_command() {
        let expected_cmd = "my_command".as_bytes();
        let fd = FlightDescriptor::new_cmd(expected_cmd.to_vec());
        assert_eq!(fd.r#type(), DescriptorType::Cmd);
        assert_eq!(fd.cmd, expected_cmd.to_vec());
    }

    #[test]
    fn it_accepts_equal_output() {
        let input = TestVector(vec![91; 10], 10);

        let actual = format!("{input}");
        let expected = format!("{:?}", vec![91; 10]);
        assert_eq!(actual, expected);
    }

    #[test]
    fn it_accepts_short_output() {
        let input = TestVector(vec![91; 6], 10);

        let actual = format!("{input}");
        let expected = format!("{:?}", vec![91; 6]);
        assert_eq!(actual, expected);
    }

    #[test]
    fn it_accepts_long_output() {
        let input = TestVector(vec![91; 10], 9);

        let actual = format!("{input}");
        let expected = format!("{:?}", vec![91; 9]);
        assert_eq!(actual, expected);
    }

    #[test]
    fn ser_deser_schema_result() {
        let schema = Schema::new(vec![
            Field::new("c1", DataType::Utf8, false),
            Field::new("c2", DataType::Float64, true),
            Field::new("c3", DataType::UInt32, false),
            Field::new("c4", DataType::Boolean, true),
            Field::new("c5", DataType::Timestamp(TimeUnit::Millisecond, None), true),
            Field::new("c6", DataType::Time32(TimeUnit::Second), false),
        ]);
        // V5 with write_legacy_ipc_format = false
        // this will write the continuation marker
        let option = IpcWriteOptions::default();
        let schema_ipc = SchemaAsIpc::new(&schema, &option);
        let result: SchemaResult = schema_ipc.try_into().unwrap();
        let des_schema: Schema = (&result).try_into().unwrap();
        assert_eq!(schema, des_schema);

        // V4 with write_legacy_ipc_format = true
        // this will not write the continuation marker
        let option = IpcWriteOptions::try_new(8, true, MetadataVersion::V4).unwrap();
        let schema_ipc = SchemaAsIpc::new(&schema, &option);
        let result: SchemaResult = schema_ipc.try_into().unwrap();
        let des_schema: Schema = (&result).try_into().unwrap();
        assert_eq!(schema, des_schema);
    }
}
