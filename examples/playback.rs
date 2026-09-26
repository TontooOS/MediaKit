use mediakit::{MediaKit, SubtitleTrack};
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "sample.mp4".to_string());
    let mut kit = MediaKit::new();
    kit.player_mut().open(Path::new(&path))?;
    println!("duration: {:.1}s", kit.player().duration_secs());
    kit.player_mut().play()?;
    kit.player_mut().set_speed(1.5)?;
    kit.player_mut().seek(10.0).unwrap_or(());
    kit.player_mut().tick(2.0);
    println!(
        "state={:?} pos={:.1}s speed={}x",
        kit.player().state(),
        kit.player().position_secs(),
        kit.player().speed()
    );
    let track = SubtitleTrack::from_srt(
        "1\n00:00:01,000 --> 00:00:04,000\nHello\n",
    )?;
    kit.player_mut().set_subtitles(Some(track));
    println!("subtitle: {:?}", kit.player().active_subtitle());
    kit.player_mut().toggle_fullscreen();
    println!("display: {:?}", kit.player().display());
    kit.player_mut().pause()?;
    kit.player_mut().stop()?;
    Ok(())
}
