//! mtr client binary. GPL-2.0-only.
#![forbid(unsafe_code)]

fn main() {
    use std::io::Write as _;
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let code = runtime.block_on(mtr::run_from_env());
    let _ = std::io::stdout().flush();
    std::process::exit(code);
}
