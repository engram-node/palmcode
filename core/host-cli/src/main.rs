//! Headless CLI running the palmcode core on a host machine.
//!
//! M0: smoke test against `palmcode-ffi`. The real harness (HostExecutor,
//! eval/bench flags) lands in M2 (tools) and M6 (bench).

fn main() {
    println!("{}", palmcode_ffi::palmcode_greet("host-cli".to_string()));
}
