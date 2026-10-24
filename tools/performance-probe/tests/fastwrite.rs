//! Opt-in integration checks against the official MCAP CLI, not an in-player scan.
use mcap::{
    Attachment, WriteOptions, Writer,
    records::{MessageHeader, Metadata},
};
use std::{
    borrow::Cow,
    collections::BTreeMap,
    fs,
    io::BufWriter,
    path::{Path, PathBuf},
    process::Command,
};
use viewer_mcap::{FileMessageStream, McapSource, McapSourceError};

fn compare(input: &Path, output: &Path) {
    let mut before = FileMessageStream::open(input).unwrap();
    let mut after = FileMessageStream::open(output).unwrap();
    let (a, identity_a) = before.read_metadata().unwrap();
    let (b, identity_b) = after.read_metadata().unwrap();
    assert!(a.chunk_indexes.is_empty());
    assert!(!b.chunk_indexes.is_empty());
    assert_eq!(a.schemas, b.schemas);
    // CLI 0.3.0 drops channels that have no messages. Require every populated
    // channel below to retain its schema/metadata instead of claiming byte identity.
    for (id, channel) in &b.channels {
        assert_eq!(a.channels.get(id), Some(channel));
    }
    assert_eq!(identity_a.message_count, identity_b.message_count);
    assert_eq!(identity_a.message_start_time, identity_b.message_start_time);
    assert_eq!(identity_a.message_end_time, identity_b.message_end_time);
    let mut count = 0;
    loop {
        match (before.next(), after.next()) {
            (None, None) => break,
            (Some(a), Some(b)) => {
                let (a, b) = (a.unwrap(), b.unwrap());
                assert_eq!(a.channel, b.channel);
                assert_eq!(a.log_time, b.log_time);
                assert_eq!(a.publish_time, b.publish_time);
                assert_eq!(a.sequence, b.sequence);
                assert_eq!(a.data, b.data);
                count += 1;
            }
            _ => panic!("conversion changed the message count"),
        }
    }
    assert_eq!(count, identity_a.message_count);
    assert!(matches!(
        McapSource::open_file(input),
        Err(McapSourceError::ChunkIndexRequired)
    ));
    McapSource::open_file(output).unwrap();
    eprintln!(
        "verified {count} messages, schemas/populated channels, exact stored order, log/publish time and sequence"
    );
}

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
#[ignore = "set MCAP_CLI to an official mcap CLI binary"]
fn official_conversion_preserves_fastwrite_facts_and_adds_indexes() {
    let name = format!(
        "mcap-fastwrite-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let scratch = Scratch(std::env::temp_dir().join(name));
    fs::create_dir(&scratch.0).unwrap();
    let input = scratch.0.join("fastwrite.mcap");
    let output = scratch.0.join("indexed.mcap");
    {
        let mut writer = Writer::with_options(
            BufWriter::new(fs::File::create_new(&input).unwrap()),
            WriteOptions::new()
                .profile("ros2")
                .use_chunks(false)
                .calculate_summary_section_crc(false),
        )
        .unwrap();
        let schema = writer
            .add_schema("demo/msg/Test", "ros2msg", b"uint8 value")
            .unwrap();
        let channel = writer
            .add_channel(
                schema,
                "/data",
                "cdr",
                &BTreeMap::from([("publisher".into(), "fixture".into())]),
            )
            .unwrap();
        writer
            .add_channel(schema, "/empty", "cdr", &BTreeMap::new())
            .unwrap();
        for (sequence, time) in [(7, 100), (8, 50), (9, 50)] {
            writer
                .write_to_known_channel(
                    &MessageHeader {
                        channel_id: channel,
                        sequence,
                        log_time: time,
                        publish_time: time + 3,
                    },
                    &[sequence as u8],
                )
                .unwrap();
        }
        writer
            .write_metadata(&Metadata {
                name: "calibration".into(),
                metadata: BTreeMap::from([("unit".into(), "metres".into())]),
            })
            .unwrap();
        writer
            .attach(&Attachment {
                name: "notes.txt".into(),
                media_type: "text/plain".into(),
                log_time: 10,
                create_time: 20,
                data: Cow::Borrowed(b"fixture notes"),
            })
            .unwrap();
        writer.finish().unwrap();
    }
    let cli = std::env::var_os("MCAP_CLI").expect("set MCAP_CLI");
    let result = Command::new(cli)
        .arg("compress")
        .arg(&input)
        .arg("-o")
        .arg(&output)
        .args(["--order", "preserve", "--chunk-size", "1048576"])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    compare(&input, &output);
    let a = fs::read(&input).unwrap();
    let b = fs::read(&output).unwrap();
    let sa = mcap::Summary::read(&a).unwrap().unwrap();
    let sb = mcap::Summary::read(&b).unwrap().unwrap();
    assert_eq!(sa.metadata_indexes.len(), sb.metadata_indexes.len());
    assert_eq!(sa.attachment_indexes.len(), sb.attachment_indexes.len());
    assert!(
        sa.channels
            .values()
            .any(|channel| channel.topic == "/empty")
    );
    assert!(
        !sb.channels
            .values()
            .any(|channel| channel.topic == "/empty")
    );
    let metadata = |data: &[u8], index: &mcap::records::MetadataIndex| {
        let mcap::records::Record::Metadata(value) = mcap::parse_record(
            mcap::records::op::METADATA,
            &data[index.offset as usize + 9..(index.offset + index.length) as usize],
        )
        .unwrap()
        .into_owned() else {
            panic!("expected Metadata")
        };
        value
    };
    assert_eq!(
        metadata(&a, &sa.metadata_indexes[0]),
        metadata(&b, &sb.metadata_indexes[0])
    );
    let attachment = |data: &[u8], index: &mcap::records::AttachmentIndex| {
        let mcap::records::Record::Attachment { header, data, .. } = mcap::parse_record(
            mcap::records::op::ATTACHMENT,
            &data[index.offset as usize + 9..(index.offset + index.length) as usize],
        )
        .unwrap()
        .into_owned() else {
            panic!("expected Attachment")
        };
        (header, data)
    };
    assert_eq!(
        attachment(&a, &sa.attachment_indexes[0]),
        attachment(&b, &sb.attachment_indexes[0])
    );
}

#[test]
#[ignore = "set MCAP_FASTWRITE_INPUT and MCAP_INDEXED_OUTPUT to the real rosbag2 fixture pair"]
fn real_rosbag2_fastwrite_conversion_preserves_serialized_messages() {
    let input = std::env::var_os("MCAP_FASTWRITE_INPUT").expect("set MCAP_FASTWRITE_INPUT");
    let output = std::env::var_os("MCAP_INDEXED_OUTPUT").expect("set MCAP_INDEXED_OUTPUT");
    compare(Path::new(&input), Path::new(&output));
}
