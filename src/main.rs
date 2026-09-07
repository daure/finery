fn main() -> Result<(), Box<dyn std::error::Error>> {
    finery::diagnostics::install();
    let result = finery::cli::run();
    if let Err(error) = &result {
        finery::diagnostics::record_error("process exited with an error", error.as_ref());
    }
    result
}
