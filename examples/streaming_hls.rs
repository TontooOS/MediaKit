use mediakit::{detect_kind, fetch_hls_playlist, fetch_progressive, StreamKind};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "https://example.com/stream.m3u8".to_string());
    match detect_kind(&url) {
        StreamKind::Hls => match fetch_hls_playlist(&url) {
            Ok(list) => println!("HLS: {} segments live={}", list.segments.len(), list.live),
            Err(e) => println!("HLS unavailable: {e}"),
        },
        StreamKind::Progressive => match fetch_progressive(&url) {
            Ok((status, len)) => println!("progressive HTTP {status} len={len:?}"),
            Err(e) => println!("stream unavailable: {e}"),
        },
    }
    Ok(())
}
