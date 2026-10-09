//! Selected-field Plot queries. These products never enter FeatureRuntime.
use crate::{
    ArrivalTime, DataWindowTimeRange, DecodeError, McapSource, NumericFieldExtractor, QueryLimits,
    RangeQuery, RawMessage, SignalOverviewReducer, SignalSample, StreamId,
};

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
    ) -> Result<NumericFieldExtractor, String> {
        let stream = self
            .catalog()
            .by_id(stream)
            .ok_or("unknown numeric stream")?;
        NumericFieldExtractor::for_stream(stream, path).map_err(|e| e.to_string())
    }
    /// Streaming indexed read of only the requested stream/range. Bounded by point budget plus
    /// one source chunk. A future source-fingerprinted overview artifact can supply this product.
    pub fn query_numeric_overview(
        &self,
        request: &NumericSeriesRequest,
    ) -> Result<NumericPlotOverview, String> {
        self.query_numeric_overview_cancellable(request, || false)
    }
    /// Cancellation belongs to the requesting worker; it never changes playback state.
    pub fn query_numeric_overview_cancellable(
        &self,
        request: &NumericSeriesRequest,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<NumericPlotOverview, String> {
        if !(1..=65_536).contains(&request.max_points) {
            return Err("point budget must be 1..=65536".into());
        }
        if request.range.start >= request.range.end_exclusive {
            return Err("numeric query range must be non-empty".into());
        }
        let extractor = self.numeric_extractor(request.stream_id, &request.field_path)?;
        let mut reducer = SignalOverviewReducer::new(
            request.range.start,
            request.range.end_exclusive,
            request.max_points,
        );
        let mut decode_errors = 0;
        self.visit_stream_range(request.stream_id, request.range, |message| {
            if cancelled() {
                return false;
            }
            match extractor.extract(&message.payload) {
                Ok(value) => reducer.push(SignalSample {
                    measurement_time: None,
                    arrival_time: message.arrival_time,
                    value,
                }),
                Err(_) => decode_errors += 1,
            }
            true
        })
        .map_err(|e| e.to_string())?;
        Ok(NumericPlotOverview {
            samples: reducer.samples(),
            input_sample_count: reducer.input_sample_count(),
            decode_errors,
        })
    }
    /// Exact bounded visible-range detail, using the existing serialized query limits.
    pub fn query_numeric_detail(
        &self,
        request: &NumericSeriesRequest,
        limits: QueryLimits,
    ) -> Result<NumericPlotDetail, String> {
        if request.range.start >= request.range.end_exclusive {
            return Err("numeric query range must be non-empty".into());
        }
        let extractor = self.numeric_extractor(request.stream_id, &request.field_path)?;
        let raw = self
            .query_range(&RangeQuery {
                streams: vec![request.stream_id],
                range: request.range,
                limits,
            })
            .map_err(|e| e.to_string())?;
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
    ) -> Result<Option<NumericSample>, String> {
        let extractor = self.numeric_extractor(stream, path)?;
        let raw = self
            .latest_before(&[stream], cursor)
            .map_err(|e| e.to_string())?;
        raw.messages
            .first()
            .map(|message| sample(&extractor, message).map_err(|e| e.to_string()))
            .transpose()
    }
}
