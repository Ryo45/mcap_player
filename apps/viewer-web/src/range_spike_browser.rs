//! Browser-only adapter for the range-read diagnostic spike.
//!
//! This intentionally uses concrete `File.slice()` calls. It is not a production RangeSource or
//! an async trait intended for shared code.

use crate::range_spike::{
    ByteRange, ChunkInspection, FooterInfo, PipelineProbeResult, RequestGeneration, SummaryCatalog,
    chunk_range, feed_pipeline, footer_tail_range, inspect_chunk, parse_footer_tail,
    parse_summary_range, resolve_seek, validate_range,
};
use js_sys::{Date, Uint8Array};
use mcap::sans_io::{
    IndexedReadEvent, IndexedReader, IndexedReaderOptions, SummaryReadEvent, SummaryReader,
    SummaryReaderOptions,
};
use std::{cell::RefCell, fmt::Write};
use wasm_bindgen::{JsCast, closure::Closure};
use wasm_bindgen_futures::{JsFuture, spawn_local};
use web_sys::{Event, File, HtmlElement, HtmlInputElement};

#[derive(Default)]
struct ReadMetrics {
    total_bytes_requested: u64,
    range_reads: u32,
    footer_read_ms: f64,
    summary_read_ms: f64,
    summary_parse_ms: f64,
    chunk_read_ms: f64,
    chunk_parse_ms: f64,
}

struct SpikeReport {
    file_name: String,
    file_size: u64,
    catalog_bytes: u64,
    footer: FooterInfo,
    catalog: SummaryCatalog,
    selected_chunk: mcap::records::ChunkIndex,
    selected_chunk_range: ByteRange,
    chunk_inspection: ChunkInspection,
    indexed: IndexedProbeReport,
    metrics: ReadMetrics,
}

struct IndexedProbeReport {
    target_topic: String,
    target_log_time: u64,
    message_log_time: u64,
    chunk_data_range: ByteRange,
    chunk_start_offset: u64,
    message_index_length: u64,
    message_index_lists_topic: bool,
    topic_filter_matched: bool,
    absolute_offset_matched: bool,
    pipeline: PipelineProbeResult,
    summary_range_reads: u32,
    summary_bytes_requested: u64,
}

const SUMMARY_READ_AHEAD: usize = 256 * 1024;

thread_local! {
    static REQUEST_GENERATION: RefCell<RequestGeneration> = RefCell::new(RequestGeneration::default());
}

pub(crate) fn install() {
    let input: HtmlInputElement = element("range-spike-file");
    let callback = Closure::<dyn FnMut(Event)>::new(move |event: Event| {
        let input = event
            .target()
            .and_then(|target| target.dyn_into::<HtmlInputElement>().ok());
        let Some(file) = input
            .and_then(|input| input.files())
            .and_then(|files| files.get(0))
        else {
            return;
        };
        let generation = REQUEST_GENERATION.with(|state| state.borrow_mut().begin());
        set_output("Reading Footer range…");
        spawn_local(async move {
            match inspect_file(file, generation).await {
                Ok(report) if is_current(generation) => set_output(&format_report(&report)),
                Ok(_) => {}
                Err(error) if error == "stale range-read generation" => {}
                Err(error) if is_current(generation) => set_output(&format!("Error: {error}")),
                Err(_) => {}
            }
        });
    });
    input
        .add_event_listener_with_callback("change", callback.as_ref().unchecked_ref())
        .expect("range spike file listener");
    callback.forget();
}

async fn inspect_file(file: File, generation: u64) -> Result<SpikeReport, String> {
    let file_name = file.name();
    let file_size = browser_file_size(&file)?;
    let mut metrics = ReadMetrics::default();

    let footer_range = footer_tail_range(file_size).map_err(|error| error.to_string())?;
    let started = Date::now();
    let footer_bytes =
        read_file_range(&file, file_size, footer_range, generation, &mut metrics).await?;
    metrics.footer_read_ms = Date::now() - started;
    let footer = parse_footer_tail(file_size, &footer_bytes).map_err(|error| error.to_string())?;

    let summary_range = footer
        .summary_range
        .ok_or_else(|| "Footer reports that this MCAP has no Summary section".to_owned())?;
    let started = Date::now();
    let summary_bytes =
        read_file_range(&file, file_size, summary_range, generation, &mut metrics).await?;
    metrics.summary_read_ms = Date::now() - started;
    let catalog_bytes = metrics.total_bytes_requested;
    let started = Date::now();
    let catalog = parse_summary_range(&summary_bytes).map_err(|error| error.to_string())?;
    metrics.summary_parse_ms = Date::now() - started;

    let selected_chunk = catalog
        .chunk_indexes
        .first()
        .cloned()
        .ok_or_else(|| "Summary contains no Chunk Index".to_owned())?;
    let selected_chunk_range =
        chunk_range(file_size, &selected_chunk).map_err(|error| error.to_string())?;
    let started = Date::now();
    let chunk_bytes = read_file_range(
        &file,
        file_size,
        selected_chunk_range,
        generation,
        &mut metrics,
    )
    .await?;
    metrics.chunk_read_ms = Date::now() - started;
    let started = Date::now();
    let chunk_inspection = inspect_chunk(&chunk_bytes).map_err(|error| error.to_string())?;
    metrics.chunk_parse_ms = Date::now() - started;

    let indexed = run_indexed_probe(&file, file_size, generation, &mut metrics).await?;

    Ok(SpikeReport {
        file_name,
        file_size,
        catalog_bytes,
        footer,
        catalog,
        selected_chunk,
        selected_chunk_range,
        chunk_inspection,
        indexed,
        metrics,
    })
}

async fn read_file_range(
    file: &File,
    file_size: u64,
    requested: ByteRange,
    generation: u64,
    metrics: &mut ReadMetrics,
) -> Result<Vec<u8>, String> {
    ensure_current(generation)?;
    let range = validate_range(file_size, requested.offset, requested.length)
        .map_err(|error| error.to_string())?;
    if range.length == 0 {
        return Ok(Vec::new());
    }
    if range.length > u32::MAX as usize {
        return Err(format!(
            "range length {} exceeds Uint8Array addressable length",
            range.length
        ));
    }
    metrics.range_reads = metrics.range_reads.saturating_add(1);
    metrics.total_bytes_requested = metrics
        .total_bytes_requested
        .checked_add(range.length as u64)
        .ok_or_else(|| "total requested byte counter overflow".to_owned())?;
    let blob = file
        .slice_with_f64_and_f64(range.offset as f64, range.end() as f64)
        .map_err(js_error)?;
    let buffer = JsFuture::from(blob.array_buffer())
        .await
        .map_err(js_error)?;
    ensure_current(generation)?;
    let bytes = Uint8Array::new(&buffer);
    let actual = bytes.length() as usize;
    if actual != range.length {
        return Err(format!(
            "short range read: requested {}, received {actual}",
            range.length
        ));
    }
    Ok(bytes.to_vec())
}

async fn run_indexed_probe(
    file: &File,
    file_size: u64,
    generation: u64,
    metrics: &mut ReadMetrics,
) -> Result<IndexedProbeReport, String> {
    let summary_read_count = metrics.range_reads;
    let summary_read_bytes = metrics.total_bytes_requested;
    let summary = read_summary_with_sans_io(file, file_size, generation, metrics).await?;
    let summary_range_reads = metrics.range_reads.saturating_sub(summary_read_count);
    let summary_bytes_requested = metrics
        .total_bytes_requested
        .saturating_sub(summary_read_bytes);

    let target_channel = summary
        .channels
        .values()
        .find(|channel| channel.topic == viewer_core::ODOM_TOPIC)
        .or_else(|| {
            summary.channels.values().find(|channel| {
                channel
                    .schema
                    .as_ref()
                    .is_some_and(|schema| schema.name == "sensor_msgs/msg/CompressedImage")
            })
        })
        .ok_or_else(|| "Summary contains neither /odom nor CompressedImage".to_owned())?;
    let target_topic = target_channel.topic.clone();
    let target_channel_id = target_channel.id;
    let (start, end) = summary
        .stats
        .as_ref()
        .map(|stats| (stats.message_start_time, stats.message_end_time))
        .or_else(|| {
            Some((
                summary
                    .chunk_indexes
                    .iter()
                    .map(|chunk| chunk.message_start_time)
                    .min()?,
                summary
                    .chunk_indexes
                    .iter()
                    .map(|chunk| chunk.message_end_time)
                    .max()?,
            ))
        })
        .ok_or_else(|| "Summary contains no indexed time range".to_owned())?;
    let target_log_time = start + end.saturating_sub(start) / 2;
    let options = IndexedReaderOptions::new()
        .include_topics([target_topic.clone()])
        .log_time_on_or_after(target_log_time);
    let mut reader =
        IndexedReader::new_with_options(&summary, options).map_err(|error| error.to_string())?;
    let mut first_chunk = None;
    let mut all_absolute_offsets_match = true;
    let mut any_requested_chunk_lists_topic = false;

    while let Some(event) = reader.next_event() {
        match event.map_err(|error| error.to_string())? {
            IndexedReadEvent::ReadChunkRequest { offset, length } => {
                let range =
                    validate_range(file_size, offset, length).map_err(|error| error.to_string())?;
                let index = summary
                    .chunk_indexes
                    .iter()
                    .find(|index| {
                        index
                            .compressed_data_offset()
                            .is_ok_and(|data_offset| data_offset == offset)
                    })
                    .ok_or_else(|| {
                        format!("IndexedReader requested unknown chunk data offset {offset}")
                    })?;
                let absolute_matches = index.compressed_size == length as u64;
                all_absolute_offsets_match &= absolute_matches;
                let lists_topic = index.message_index_offsets.contains_key(&target_channel_id);
                any_requested_chunk_lists_topic |= lists_topic;
                first_chunk.get_or_insert_with(|| {
                    (range, index.chunk_start_offset, index.message_index_length)
                });
                let bytes = read_file_range(file, file_size, range, generation, metrics).await?;
                reader
                    .insert_chunk_record_data(offset, &bytes)
                    .map_err(|error| error.to_string())?;
            }
            IndexedReadEvent::Message { header, data } => {
                let message_log_time = header.log_time;
                let topic_filter_matched = summary
                    .channels
                    .get(&header.channel_id)
                    .is_some_and(|channel| channel.topic == target_topic);
                if message_log_time < target_log_time {
                    return Err(format!(
                        "IndexedReader yielded {message_log_time} before seek {target_log_time}"
                    ));
                }
                let pipeline = feed_pipeline(&summary, header, data, &target_topic)?;
                let (chunk_data_range, chunk_start_offset, message_index_length) = first_chunk
                    .ok_or_else(|| {
                        "IndexedReader yielded a message without a chunk read".to_owned()
                    })?;
                return Ok(IndexedProbeReport {
                    target_topic,
                    target_log_time,
                    message_log_time,
                    chunk_data_range,
                    chunk_start_offset,
                    message_index_length,
                    message_index_lists_topic: any_requested_chunk_lists_topic,
                    topic_filter_matched,
                    absolute_offset_matched: all_absolute_offsets_match,
                    pipeline,
                    summary_range_reads,
                    summary_bytes_requested,
                });
            }
        }
    }
    Err(format!(
        "IndexedReader found no {target_topic} message at or after {target_log_time}"
    ))
}

async fn read_summary_with_sans_io(
    file: &File,
    file_size: u64,
    generation: u64,
    metrics: &mut ReadMetrics,
) -> Result<mcap::Summary, String> {
    let options = SummaryReaderOptions::default().with_file_size(file_size);
    let mut reader = SummaryReader::new_with_options(options);
    let mut position = 0_u64;
    let mut seek_count = 0_u32;
    while let Some(event) = reader.next_event() {
        match event.map_err(|error| error.to_string())? {
            SummaryReadEvent::SeekRequest(seek) => {
                position =
                    resolve_seek(file_size, position, seek).map_err(|error| error.to_string())?;
                seek_count = seek_count.saturating_add(1);
                reader.notify_seeked(position);
            }
            SummaryReadEvent::ReadRequest(need) => {
                let remaining =
                    usize::try_from(file_size.saturating_sub(position)).unwrap_or(usize::MAX);
                let requested = if seek_count >= 2 {
                    need.max(SUMMARY_READ_AHEAD).min(remaining)
                } else {
                    need
                };
                let range = validate_range(file_size, position, requested)
                    .map_err(|error| error.to_string())?;
                let bytes = read_file_range(file, file_size, range, generation, metrics).await?;
                reader.insert(bytes.len()).copy_from_slice(&bytes);
                reader.notify_read(bytes.len());
                position = position
                    .checked_add(bytes.len() as u64)
                    .ok_or_else(|| "SummaryReader position overflow".to_owned())?;
            }
        }
    }
    reader
        .finish()
        .ok_or_else(|| "SummaryReader reports no Summary section".to_owned())
}

fn is_current(generation: u64) -> bool {
    REQUEST_GENERATION.with(|state| state.borrow().is_current(generation))
}

fn ensure_current(generation: u64) -> Result<(), String> {
    if is_current(generation) {
        Ok(())
    } else {
        Err("stale range-read generation".to_owned())
    }
}

fn browser_file_size(file: &File) -> Result<u64, String> {
    let size = file.size();
    if !size.is_finite() || size < 0.0 || size.fract() != 0.0 {
        return Err(format!("browser returned invalid File.size: {size}"));
    }
    let size = size as u64;
    validate_range(size, 0, 0).map_err(|error| error.to_string())?;
    Ok(size)
}

fn js_error(error: wasm_bindgen::JsValue) -> String {
    error
        .as_string()
        .unwrap_or_else(|| format!("Browser File API error: {error:?}"))
}

fn format_report(report: &SpikeReport) -> String {
    let mut output = String::new();
    let ratio = if report.file_size == 0 {
        0.0
    } else {
        report.catalog_bytes as f64 / report.file_size as f64 * 100.0
    };
    let summary_range = report.footer.summary_range.expect("report has summary");
    let crc = if report.footer.summary_crc == 0 {
        "absent (0)".to_owned()
    } else {
        format!("{:#010x}", report.footer.summary_crc)
    };
    let time_range = report.catalog.time_range().map_or_else(
        || "unavailable".to_owned(),
        |(start, end)| format!("{start}..{end} ns"),
    );

    writeln!(output, "File name: {}", report.file_name).unwrap();
    writeln!(output, "File size: {} bytes", report.file_size).unwrap();
    writeln!(
        output,
        "Total bytes requested: {}",
        report.metrics.total_bytes_requested
    )
    .unwrap();
    writeln!(
        output,
        "Number of range reads: {}",
        report.metrics.range_reads
    )
    .unwrap();
    writeln!(output, "Footer status: OK").unwrap();
    writeln!(output, "Footer offset: {}", report.footer.footer_offset).unwrap();
    writeln!(output, "Summary status: OK").unwrap();
    writeln!(
        output,
        "Summary byte range: {}..{} ({} bytes)",
        summary_range.offset,
        summary_range.end(),
        summary_range.length
    )
    .unwrap();
    writeln!(
        output,
        "Summary offset section start: {}",
        report.footer.summary_offset_start
    )
    .unwrap();
    writeln!(output, "Summary CRC: {crc}").unwrap();
    writeln!(output, "Schema count: {}", report.catalog.schemas.len()).unwrap();
    writeln!(output, "Channel count: {}", report.catalog.channels.len()).unwrap();
    writeln!(
        output,
        "Chunk index count: {}",
        report.catalog.chunk_indexes.len()
    )
    .unwrap();
    writeln!(
        output,
        "Attachment index count: {}",
        report.catalog.attachment_indexes.len()
    )
    .unwrap();
    writeln!(
        output,
        "Metadata index count: {}",
        report.catalog.metadata_indexes.len()
    )
    .unwrap();
    writeln!(
        output,
        "Summary offset count: {}",
        report.catalog.summary_offsets.len()
    )
    .unwrap();
    writeln!(
        output,
        "Message index information: {}",
        if report.catalog.has_message_indexes() {
            "present"
        } else {
            "absent"
        }
    )
    .unwrap();
    writeln!(output, "Time range: {time_range}").unwrap();
    writeln!(
        output,
        "Bytes fetched for catalog: {}",
        report.catalog_bytes
    )
    .unwrap();
    writeln!(output, "Catalog read ratio: {ratio:.6}%").unwrap();
    writeln!(
        output,
        "Footer read: {:.3} ms",
        report.metrics.footer_read_ms
    )
    .unwrap();
    writeln!(
        output,
        "Summary read: {:.3} ms",
        report.metrics.summary_read_ms
    )
    .unwrap();
    writeln!(
        output,
        "Summary parse: {:.3} ms",
        report.metrics.summary_parse_ms
    )
    .unwrap();
    writeln!(
        output,
        "Selected chunk read: {:.3} ms",
        report.metrics.chunk_read_ms
    )
    .unwrap();
    writeln!(
        output,
        "Selected chunk parse/decompress: {:.3} ms",
        report.metrics.chunk_parse_ms
    )
    .unwrap();

    writeln!(output, "\nSchemas:").unwrap();
    for schema in &report.catalog.schemas {
        writeln!(
            output,
            "  id={} name={} encoding={}",
            schema.id, schema.name, schema.encoding
        )
        .unwrap();
    }
    writeln!(output, "\nChannels:").unwrap();
    for channel in &report.catalog.channels {
        writeln!(
            output,
            "  id={} schema={} topic={} messageEncoding={}",
            channel.id, channel.schema_id, channel.topic, channel.message_encoding
        )
        .unwrap();
    }
    writeln!(output, "\nChunk indexes:").unwrap();
    for (number, chunk) in report.catalog.chunk_indexes.iter().enumerate() {
        writeln!(
            output,
            "  #{number} time={}..{} offset={} length={} compression={} compressed={} uncompressed={} messageIndexLength={} messageIndexChannels={}",
            chunk.message_start_time,
            chunk.message_end_time,
            chunk.chunk_start_offset,
            chunk.chunk_length,
            compression_name(&chunk.compression),
            chunk.compressed_size,
            chunk.uncompressed_size,
            chunk.message_index_length,
            chunk.message_index_offsets.len(),
        )
        .unwrap();
    }
    writeln!(output, "\nSummary offsets:").unwrap();
    for offset in &report.catalog.summary_offsets {
        writeln!(
            output,
            "  groupOpcode={:#04x} start={} length={}",
            offset.group_opcode, offset.group_start, offset.group_length
        )
        .unwrap();
    }
    writeln!(output, "\nSelected chunk:").unwrap();
    writeln!(
        output,
        "  file offset: {}",
        report.selected_chunk_range.offset
    )
    .unwrap();
    writeln!(
        output,
        "  requested bytes: {}",
        report.selected_chunk_range.length
    )
    .unwrap();
    writeln!(
        output,
        "  compression: {}",
        compression_name(&report.selected_chunk.compression)
    )
    .unwrap();
    writeln!(
        output,
        "  compressed size: {}",
        report.selected_chunk.compressed_size
    )
    .unwrap();
    writeln!(
        output,
        "  uncompressed size: {}",
        report.selected_chunk.uncompressed_size
    )
    .unwrap();
    writeln!(
        output,
        "  time range: {}..{}",
        report.selected_chunk.message_start_time, report.selected_chunk.message_end_time
    )
    .unwrap();
    writeln!(output, "  status: {}", report.chunk_inspection.status).unwrap();
    writeln!(
        output,
        "  parsed records: {}",
        optional_count(report.chunk_inspection.record_count)
    )
    .unwrap();
    writeln!(
        output,
        "  parsed messages: {}",
        optional_count(report.chunk_inspection.message_count)
    )
    .unwrap();
    writeln!(output, "\nIndexedReader end-to-end:").unwrap();
    writeln!(
        output,
        "  SummaryReader File.slice reads: {} ({} bytes)",
        report.indexed.summary_range_reads, report.indexed.summary_bytes_requested
    )
    .unwrap();
    writeln!(output, "  target topic: {}", report.indexed.target_topic).unwrap();
    writeln!(
        output,
        "  arbitrary seek: {} ns → {} ns",
        report.indexed.target_log_time, report.indexed.message_log_time
    )
    .unwrap();
    writeln!(
        output,
        "  chunk record/data offsets: {} / {}",
        report.indexed.chunk_start_offset, report.indexed.chunk_data_range.offset
    )
    .unwrap();
    writeln!(
        output,
        "  chunk payload request: {} bytes",
        report.indexed.chunk_data_range.length
    )
    .unwrap();
    writeln!(
        output,
        "  absolute file offset mapping: {}",
        if report.indexed.absolute_offset_matched {
            "OK"
        } else {
            "FAILED"
        }
    )
    .unwrap();
    writeln!(
        output,
        "  topic filter: {}",
        if report.indexed.topic_filter_matched {
            "OK"
        } else {
            "FAILED"
        }
    )
    .unwrap();
    writeln!(
        output,
        "  ChunkIndex message-index map includes topic: {} (index bytes={})",
        report.indexed.message_index_lists_topic, report.indexed.message_index_length
    )
    .unwrap();
    writeln!(
        output,
        "  DomainPipelineSet → DomainState: OK ({} at {} ns)",
        report.indexed.pipeline.domain, report.indexed.pipeline.arrival_time.0
    )
    .unwrap();
    writeln!(
        output,
        "  generation replacement: stale reads are discarded after every File.slice await"
    )
    .unwrap();
    output
}

fn compression_name(compression: &str) -> &str {
    if compression.is_empty() {
        "uncompressed"
    } else {
        compression
    }
}

fn optional_count(count: Option<usize>) -> String {
    count.map_or_else(|| "not attempted".to_owned(), |count| count.to_string())
}

fn element<T: JsCast>(id: &str) -> T {
    web_sys::window()
        .expect("window")
        .document()
        .expect("document")
        .get_element_by_id(id)
        .unwrap_or_else(|| panic!("missing #{id}"))
        .dyn_into()
        .unwrap_or_else(|_| panic!("wrong element type for #{id}"))
}

fn set_output(text: &str) {
    let output: HtmlElement = element("range-spike-output");
    output.set_inner_text(text);
}
