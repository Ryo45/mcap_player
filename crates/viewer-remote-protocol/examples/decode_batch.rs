use std::{env, fs, process::ExitCode};

use viewer_remote_protocol::BatchDecoder;

fn main() -> ExitCode {
    let Some(path) = env::args_os().nth(1) else {
        eprintln!("usage: decode_batch BATCH_FILE");
        return ExitCode::FAILURE;
    };
    let body = match fs::read(&path) {
        Ok(body) => body,
        Err(error) => {
            eprintln!("could not read batch: {error}");
            return ExitCode::FAILURE;
        }
    };
    let messages = match BatchDecoder::new(&body).and_then(BatchDecoder::collect) {
        Ok(messages) => messages,
        Err(error) => {
            eprintln!("could not decode batch: {error}");
            return ExitCode::FAILURE;
        }
    };
    println!("message count: {}", messages.len());
    for message in messages {
        println!(
            "stream={} sequence={} log_time={} publish_time={} payload_bytes={}",
            message.stream_id,
            message.sequence,
            message.log_time_ns,
            message.publish_time_ns,
            message.payload.len()
        );
    }
    ExitCode::SUCCESS
}
