//! Mirrors `ffmpeg-sys-next`'s `ffmpeg_7_1` feature flag as a cfg of this crate.

fn main() {
    println!("cargo::rustc-check-cfg=cfg(ffmpeg_7_1)");
    println!("cargo::rerun-if-env-changed=DEP_FFMPEG_FFMPEG_7_1");
    // Older FFmpeg sets the variable to an empty string; only "true" means the flag is on.
    if std::env::var("DEP_FFMPEG_FFMPEG_7_1").is_ok_and(|v| v == "true") {
        println!("cargo::rustc-cfg=ffmpeg_7_1");
    }
}
