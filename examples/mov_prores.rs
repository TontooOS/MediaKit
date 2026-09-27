use mediakit::{detect_profile_from_mov, read_mov_info, sniff_mov, VideoPlayer};
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "sample.mov".to_string());
    let path = Path::new(&path);
    println!("sniff_mov: {}", sniff_mov(path));
    let mut player = VideoPlayer::new();
    match player.open(path) {
        Ok(()) => println!(
            "open ok state={:?} duration={:.1}s",
            player.state(),
            player.duration_secs()
        ),
        Err(e) => println!("open (expected without file): {e}"),
    }
    match read_mov_info(path) {
        Ok(info) => {
            println!(
                "mov brand={} {}x{} {:.2}s {} {}fps",
                info.container_name(),
                info.width,
                info.height,
                info.duration_secs,
                info.video_fourcc,
                info.framerate
            );
            match detect_profile_from_mov(&info) {
                Some(p) => println!("prores: {} ({})", p.label(), p.as_str()),
                None => println!("prores: none (fourcc={})", info.video_fourcc),
            }
            let meta = mediakit::read_metadata(path)?;
            println!("metadata native: {} {}x{}", meta.container, meta.width, meta.height);
        }
        Err(e) => println!("mov info (expected without file): {e}"),
    }
    Ok(())
}
