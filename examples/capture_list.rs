use mediakit::{available_cameras, available_outputs};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cameras = available_cameras().unwrap_or_default();
    if cameras.is_empty() {
        println!("no cameras found");
    }
    for cam in &cameras {
        println!("{} {} ({})", cam.id, cam.label, cam.node.display());
    }
    for out in available_outputs() {
        println!(
            "output {} primary={} {}x{:?}",
            out.id, out.primary, out.width.unwrap_or(0), out.height
        );
    }
    Ok(())
}
