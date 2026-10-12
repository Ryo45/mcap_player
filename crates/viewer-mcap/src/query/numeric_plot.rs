//! Selected-field Plot queries. These products never enter FeatureRuntime.
use crate::{
    ArrivalTime, DataWindowTimeRange, DecodeError, McapSource, NumericFieldExtractor, QueryLimits,
    RangeQuery, RawMessage, RosMessageSchema, SignalOverviewReducer, SignalSample, StreamId,
};

#[derive(Debug)]
pub enum NumericPlotError {
    UnknownStream(StreamId),
    EmptyBatch,
    InvalidBatchSize {
        actual: usize,
    },
    InvalidRange(DataWindowTimeRange),
    InvalidPointBudget {
        points: usize,
    },
    MixedStreamOrRange,
    Decode {
        stream: StreamId,
        path: String,
        source: DecodeError,
    },
    Source(crate::McapSourceError),
    RangeQuery(crate::RangeQueryError),
}
impl std::fmt::Display for NumericPlotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownStream(stream) => write!(f, "unknown numeric stream {}", stream.0),
            Self::EmptyBatch => write!(f, "numeric batch requires fields"),
            Self::InvalidBatchSize { actual } => write!(
                f,
                "numeric batch requires at most eight fields, got {actual}"
            ),
            Self::InvalidRange(range) => write!(
                f,
                "numeric query range must be non-empty: {}..{}",
                range.start.0, range.end_exclusive.0
            ),
            Self::InvalidPointBudget { points } => {
                write!(f, "numeric point budget must be 1..=65536, got {points}")
            }
            Self::MixedStreamOrRange => write!(f, "numeric batch requires one stream/range"),
            Self::Decode {
                stream,
                path,
                source,
            } => write!(f, "numeric field {path} on stream {}: {source}", stream.0),
            Self::Source(error) => error.fmt(f),
            Self::RangeQuery(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for NumericPlotError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Decode { source, .. } => Some(source),
            Self::Source(error) => Some(error),
            Self::RangeQuery(error) => Some(error),
            _ => None,
        }
    }
}
impl From<crate::McapSourceError> for NumericPlotError {
    fn from(error: crate::McapSourceError) -> Self {
        Self::Source(error)
    }
}
impl From<crate::RangeQueryError> for NumericPlotError {
    fn from(error: crate::RangeQueryError) -> Self {
        Self::RangeQuery(error)
    }
}

/// Runtime Plot demand, independent of the workspace's fixed SessionPlan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NumericSeriesRequest {
    pub stream_id: StreamId,
    pub field_path: String,
    pub range: DataWindowTimeRange,
    pub max_points: usize,
}
#[derive(Clone, Debug, PartialEq)]
pub struct NumericPlotOverview {
    pub samples: Vec<SignalSample>,
    pub input_sample_count: u64,
    pub decode_errors: u64,
}
#[derive(Clone, Debug, PartialEq)]
pub struct NumericPlotOverviewBatch {
    pub series: Vec<NumericPlotOverview>,
    pub chunks_read: usize,
}
#[derive(Clone, Debug, PartialEq)]
pub struct NumericSample {
    pub log_time: ArrivalTime,
    pub publish_time_ns: Option<u64>,
    pub sequence: Option<u32>,
    pub value: f64,
}
#[derive(Clone, Debug, PartialEq)]
pub struct NumericPlotDetail {
    pub samples: Vec<NumericSample>,
    pub complete: bool,
    pub decode_errors: u64,
}
fn sample(
    extractor: &NumericFieldExtractor,
    message: &RawMessage,
) -> Result<NumericSample, DecodeError> {
    Ok(NumericSample {
        log_time: message.arrival_time,
        publish_time_ns: message.publish_time_ns,
        sequence: message.sequence,
        value: extractor.extract(&message.payload)?,
    })
}
impl McapSource {
    fn numeric_extractor(
        &self,
        stream: StreamId,
        path: &str,
    ) -> Result<NumericFieldExtractor, NumericPlotError> {
        let descriptor = self
            .catalog()
            .by_id(stream)
            .ok_or(NumericPlotError::UnknownStream(stream))?;
        NumericFieldExtractor::for_stream(descriptor, path).map_err(|source| {
            NumericPlotError::Decode {
                stream,
                path: path.into(),
                source,
            }
        })
    }
    /// Streaming indexed read of only the requested stream/range. Bounded by point budget plus
    /// one source chunk. A future source-fingerprinted overview artifact can supply this product.
    pub fn query_numeric_overview(
        &self,
        request: &NumericSeriesRequest,
    ) -> Result<NumericPlotOverview, NumericPlotError> {
        self.query_numeric_overview_cancellable(request, || false)
    }
    /// Cancellation belongs to the requesting worker; it never changes playback state.
    pub fn query_numeric_overview_cancellable(
        &self,
        request: &NumericSeriesRequest,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<NumericPlotOverview, NumericPlotError> {
        Ok(self
            .query_numeric_overviews_cancellable(std::slice::from_ref(request), &mut cancelled)?
            .series
            .remove(0))
    }

    /// At most eight selected fields on one stream/range share each physical chunk read.
    pub fn query_numeric_overviews_cancellable(
        &self,
        requests: &[NumericSeriesRequest],
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<NumericPlotOverviewBatch, NumericPlotError> {
        if cancelled() {
            return Err(crate::McapSourceError::QueryCancelled.into());
        }
        let first = requests.first().ok_or(NumericPlotError::EmptyBatch)?;
        if requests.len() > 8 {
            return Err(NumericPlotError::InvalidBatchSize {
                actual: requests.len(),
            });
        }
        if first.range.start >= first.range.end_exclusive {
            return Err(NumericPlotError::InvalidRange(first.range));
        }
        let stream = self
            .catalog()
            .by_id(first.stream_id)
            .ok_or(NumericPlotError::UnknownStream(first.stream_id))?;
        let schema =
            RosMessageSchema::for_stream(stream).map_err(|source| NumericPlotError::Decode {
                stream: stream.id,
                path: first.field_path.clone(),
                source,
            })?;
        let mut fields = Vec::new();
        for request in requests {
            if request.stream_id != first.stream_id || request.range != first.range {
                return Err(NumericPlotError::MixedStreamOrRange);
            }
            if !(1..=65_536).contains(&request.max_points) {
                return Err(NumericPlotError::InvalidPointBudget {
                    points: request.max_points,
                });
            }
            fields.push((
                schema
                    .compile_numeric(&request.field_path)
                    .map_err(|source| NumericPlotError::Decode {
                        stream: request.stream_id,
                        path: request.field_path.clone(),
                        source,
                    })?,
                SignalOverviewReducer::new(
                    request.range.start,
                    request.range.end_exclusive,
                    request.max_points,
                ),
                0u64,
            ));
        }
        let chunks_read = self
            .visit_stream_range(first.stream_id, first.range, cancelled, |message| {
                for (extractor, reducer, decode_errors) in &mut fields {
                    match extractor.extract(&message.payload) {
                        Ok(value) => reducer.push(SignalSample {
                            measurement_time: None,
                            arrival_time: message.arrival_time,
                            value,
                        }),
                        Err(_) => *decode_errors += 1,
                    }
                }
            })
            .map_err(NumericPlotError::from)?;
        Ok(NumericPlotOverviewBatch {
            chunks_read,
            series: fields
                .into_iter()
                .map(|(_, reducer, decode_errors)| NumericPlotOverview {
                    samples: reducer.samples(),
                    input_sample_count: reducer.input_sample_count(),
                    decode_errors,
                })
                .collect(),
        })
    }
    /// Exact bounded visible-range detail, using the existing serialized query limits.
    pub fn query_numeric_detail(
        &self,
        request: &NumericSeriesRequest,
        limits: QueryLimits,
    ) -> Result<NumericPlotDetail, NumericPlotError> {
        if request.range.start >= request.range.end_exclusive {
            return Err(NumericPlotError::InvalidRange(request.range));
        }
        let extractor = self.numeric_extractor(request.stream_id, &request.field_path)?;
        let raw = self
            .query_range(&RangeQuery {
                streams: vec![request.stream_id],
                range: request.range,
                limits,
            })
            .map_err(NumericPlotError::from)?;
        let mut decode_errors = 0;
        let samples = raw
            .messages
            .iter()
            .filter_map(|message| match sample(&extractor, message) {
                Ok(sample) => Some(sample),
                Err(_) => {
                    decode_errors += 1;
                    None
                }
            })
            .collect();
        Ok(NumericPlotDetail {
            samples,
            complete: raw.complete,
            decode_errors,
        })
    }
    /// Exact latest source message at cursor, using Message Index restoration; no overview lookup.
    pub fn query_numeric_current(
        &self,
        stream: StreamId,
        path: &str,
        cursor: ArrivalTime,
    ) -> Result<Option<NumericSample>, NumericPlotError> {
        let extractor = self.numeric_extractor(stream, path)?;
        let raw = self
            .latest_before(&[stream], cursor)
            .map_err(NumericPlotError::from)?;
        raw.messages
            .first()
            .map(|message| {
                sample(&extractor, message).map_err(|source| NumericPlotError::Decode {
                    stream,
                    path: path.into(),
                    source,
                })
            })
            .transpose()
    }
}
