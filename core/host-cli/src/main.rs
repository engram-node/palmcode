//! Headless CLI running the pocket core on a host machine.
//!
//! M0: smoke test against `pocket-ffi`. The real harness (HostExecutor,
//! eval/bench flags) lands in M2 (tools) and M6 (bench).

fn main() {
    println!("{}", pocketffi::pocket_greet("host-cli".to_string()));
}
