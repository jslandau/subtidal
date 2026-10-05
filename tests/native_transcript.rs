//! Deliberately bypasses Rust's worker-thread test harness: AppKit requires main.
fn main() {
    #[cfg(target_os = "macos")]
    subtidal::overlay::run_native_transcript_scenarios();
    #[cfg(target_os = "linux")]
    panic!("Linux native scenarios run via the serialized library test under Xvfb");
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    panic!("native transcript scenarios require Linux or macOS");
}
