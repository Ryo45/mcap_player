use viewer_core::{LoadedSignal, SignalId, SignalSample, TelemetryFrame};

/// One signal's current Session-owned query result, borrowed for a single UI frame.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SignalDataView<'a> {
    pub(crate) signal: Option<&'a LoadedSignal>,
    pub(crate) current: Option<SignalSample>,
    pub(crate) loading: bool,
    pub(crate) error: Option<&'a str>,
}

/// Narrow, read-only view of exact Signal query results exposed by `ViewerSession`.
///
/// The closed `SignalId` match intentionally keeps this separate from Preview, Inspection, and
/// continuous feature presentation data.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SignalQueryView<'a> {
    speed: SignalDataView<'a>,
    yaw_rate: SignalDataView<'a>,
}

impl<'a> SignalQueryView<'a> {
    pub(crate) fn new(speed: SignalDataView<'a>, yaw_rate: SignalDataView<'a>) -> Self {
        Self { speed, yaw_rate }
    }

    #[cfg(test)]
    pub(crate) fn empty() -> Self {
        Self::new(SignalDataView::default(), SignalDataView::default())
    }

    pub(crate) fn get(self, signal_id: SignalId) -> SignalDataView<'a> {
        match signal_id {
            SignalId::Speed => self.speed,
            SignalId::YawRate => self.yaw_rate,
        }
    }

    pub(crate) fn first_error(self) -> Option<&'a str> {
        self.speed.error.or(self.yaw_rate.error)
    }

    pub(crate) fn with_current_odometry(mut self, frame: Option<&TelemetryFrame>) -> Self {
        let Some(frame) = frame else {
            return self;
        };
        let sample = |value| SignalSample {
            measurement_time: Some(frame.measurement_time),
            arrival_time: frame.arrival_time,
            value,
        };
        self.speed.current = Some(sample(frame.speed));
        self.yaw_rate.current = Some(sample(frame.yaw_rate));
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use viewer_core::{ArrivalTime, PlotSeries};

    fn signal(signal_id: SignalId, value: f64) -> LoadedSignal {
        LoadedSignal {
            display: PlotSeries {
                signal_id,
                origin: ArrivalTime(0),
                x_seconds: vec![0.0],
                values: vec![value],
            },
            input_sample_count: 1,
        }
    }

    #[test]
    fn gets_speed_and_yaw_rate_with_independent_snapshot_state() {
        let speed = signal(SignalId::Speed, 3.0);
        let yaw_rate = signal(SignalId::YawRate, 0.2);
        let view = SignalQueryView::new(
            SignalDataView {
                signal: Some(&speed),
                current: None,
                loading: true,
                error: None,
            },
            SignalDataView {
                signal: Some(&yaw_rate),
                current: None,
                loading: false,
                error: Some("yaw query failed"),
            },
        );

        let speed_view = view.get(SignalId::Speed);
        assert_eq!(speed_view.signal, Some(&speed));
        assert!(speed_view.loading);
        assert!(speed_view.error.is_none());

        let yaw_view = view.get(SignalId::YawRate);
        assert_eq!(yaw_view.signal, Some(&yaw_rate));
        assert!(!yaw_view.loading);
        assert_eq!(yaw_view.error, Some("yaw query failed"));
        assert_eq!(view.first_error(), Some("yaw query failed"));
    }
}
