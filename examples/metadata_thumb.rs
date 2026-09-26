use mediakit::{extract_frame, read_metadata};
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "sample.mp4".to_string());
    match read_metadata(Path::new(&path)) {
        Ok(meta) => println!(
            "{:.1}s {}x{} {} {:.1}fps container={}",
            meta.duration_secs,
            meta.width,
            meta.height,
            meta.video_codec,
            meta.framerate,
            meta.container
        ),
        Err(e) => println!("metadata unavailable (ffprobe missing?): {e}"),
    }
    let out = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "thumb.png".to_string());
    match extract_frame(Path::new(&path), 5.0, Path::new(&out)) {
        Ok(()) => println!("thumbnail written to {out}"),
        Err(e) => println!("thumbnail skipped: {e}"),
    }
    Ok(())
}
