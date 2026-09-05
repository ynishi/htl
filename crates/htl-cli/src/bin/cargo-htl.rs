// `cargo htl <verb>`: cargo invokes this binary as `cargo-htl htl <verb>`; run() drops the "htl".
fn main() -> std::process::ExitCode {
    htl_cli::run()
}
