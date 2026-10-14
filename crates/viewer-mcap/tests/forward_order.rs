use mcap::{WriteOptions, Writer, records::MessageHeader};
use std::{collections::BTreeMap, io::Cursor};
use viewer_core::{ArrivalTime, DataWindowTimeRange};
use viewer_mcap::{McapSource, QueryLimits, RangeQuery};

fn fixture() -> Vec<u8> {
    fixture_with_tail_start(5)
}

fn fixture_with_tail_start(start: u64) -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    {
        let mut writer = Writer::with_options(
            &mut bytes,
            WriteOptions::new().compression(None).chunk_size(None),
        )
        .unwrap();
        let schema = writer
            .add_schema("demo/msg/Sample", "ros2msg", b"uint8 value")
            .unwrap();
        let a = writer
            .add_channel(schema, "/a", "cdr", &BTreeMap::new())
            .unwrap();
        let b = writer
            .add_channel(schema, "/b", "cdr", &BTreeMap::new())
            .unwrap();
        let other = writer
            .add_channel(schema, "/other", "cdr", &BTreeMap::new())
            .unwrap();
        // Overlapping intervals, nonmonotonic chunk starts/ends, and repeated same-stream ties.
        for (channel, records) in [
            (a, vec![(0, 0), (10, 1), (100, 2)]),
            (b, vec![(0, 3), (10, 4), (10, 5), (20, 6)]),
            (other, vec![(2, 7), (200, 8)]),
            (a, vec![(start, 9), (start + 5, 10), (start + 25, 11)]),
        ] {
            for (time, value) in records {
                writer
                    .write_to_known_channel(
                        &MessageHeader {
                            channel_id: channel,
                            sequence: value,
                            log_time: time,
                            publish_time: time,
                        },
                        &[value as u8],
                    )
                    .unwrap();
            }
            writer.flush().unwrap();
        }
        writer.finish().unwrap();
    }
    bytes.into_inner()
}
fn source() -> McapSource {
    McapSource::from_bytes(fixture().into()).unwrap()
}

#[test]
fn grouped_forward_is_chronological_on_fine_ticks_and_prunes_unselected_chunks() {
    let mut source = source();
    let a = source.catalog().by_topic("/a").unwrap().id;
    let b = source.catalog().by_topic("/b").unwrap().id;
    source.select_streams([a, b]).unwrap();
    let mut delivered = Vec::new();
    for time in [0, 1, 5, 9, 10, 11, 20, 30, 99, 100, 200] {
        let batch = source.read_through(ArrivalTime(time)).unwrap();
        assert!(batch.iter().all(|m| m.arrival_time.0 <= time));
        delivered.extend(batch);
    }
    assert_eq!(
        delivered.iter().map(|m| m.payload[0]).collect::<Vec<_>>(),
        [0, 3, 9, 1, 10, 4, 5, 6, 11, 2]
    );
    assert_eq!(source.read_diagnostics().chunks_expanded, 3);
    assert_eq!(source.read_diagnostics().sequential_cache_backing_bytes, 0);
    assert!(source.read_through(ArrivalTime(200)).unwrap().is_empty());
}

#[test]
fn grouped_seek_and_bounded_range_use_the_same_exact_order() {
    let mut source = source();
    let streams = [
        source.catalog().by_topic("/a").unwrap().id,
        source.catalog().by_topic("/b").unwrap().id,
    ];
    source.select_streams(streams).unwrap();
    for target in [10, 0, 31, 5, 101, 10] {
        source.seek(ArrivalTime(target)).unwrap();
        let messages = source.read_through(ArrivalTime(100)).unwrap();
        let expected = [
            (0, 0),
            (0, 3),
            (5, 9),
            (10, 1),
            (10, 10),
            (10, 4),
            (10, 5),
            (20, 6),
            (30, 11),
            (100, 2),
        ]
        .into_iter()
        .filter(|(t, _)| *t >= target)
        .map(|(_, p)| p)
        .collect::<Vec<_>>();
        assert_eq!(
            messages.iter().map(|m| m.payload[0]).collect::<Vec<_>>(),
            expected
        );
    }
    let result = source
        .query_range(&RangeQuery {
            streams: streams.to_vec(),
            range: DataWindowTimeRange::new(ArrivalTime(5), ArrivalTime(20)).unwrap(),
            limits: QueryLimits::new(3, 100).unwrap(),
        })
        .unwrap();
    assert_eq!(
        result
            .messages
            .iter()
            .map(|m| m.payload[0])
            .collect::<Vec<_>>(),
        [9, 1, 10]
    );
    assert!(!result.complete);
}

#[test]
fn corrupt_unselected_chunk_is_not_read_but_selected_read_failure_rolls_back() {
    let mut bytes = fixture();
    let summary = mcap::Summary::read(&bytes).unwrap().unwrap();
    let offset = summary.chunk_indexes[2].compressed_data_offset().unwrap() as usize;
    bytes[offset] ^= 0xff;
    let mut source = McapSource::from_bytes(bytes.into()).unwrap();
    let a = source.catalog().by_topic("/a").unwrap().id;
    source.select_streams([a]).unwrap();
    assert_eq!(source.read_through(ArrivalTime(100)).unwrap().len(), 6);
    let mut bytes = fixture_with_tail_start(30);
    let summary = mcap::Summary::read(&bytes).unwrap().unwrap();
    let offset = summary.chunk_indexes[3].compressed_data_offset().unwrap() as usize;
    bytes[offset] ^= 0xff;
    let mut source = McapSource::from_bytes(bytes.into()).unwrap();
    source.select_streams([a]).unwrap();
    assert_eq!(
        source.read_through(ArrivalTime(0)).unwrap()[0].payload[0],
        0
    );
    assert!(source.read_through(ArrivalTime(100)).is_err());
    // The failed tick must not consume the already cached t=10 message.
    assert!(source.read_through(ArrivalTime(100)).is_err());
    assert_eq!(
        source.read_through(ArrivalTime(10)).unwrap()[0].payload[0],
        1
    );
    assert!(source.seek(ArrivalTime(40)).is_err());
    assert!(source.read_through(ArrivalTime(20)).unwrap().is_empty());
}
