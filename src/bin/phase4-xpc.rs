//! The `Phase4Engine` XPC service entry point.

#[cfg(target_os = "macos")]
fn main() {
    phase4::xpc::run()
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("phase4-xpc runs only on macOS");
    std::process::exit(1);
}
