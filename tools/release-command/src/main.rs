use std::env;
use std::path::Path;
use std::process::{Command, ExitCode};

fn usage(bump: &str) {
    println!("Usage: cargo {bump}\n\nRun Finery {bump} release workflow.");
}

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let Some(bump) = args.next() else {
        eprintln!("error: missing release bump");
        return ExitCode::from(2);
    };
    let remaining: Vec<_> = args.collect();
    if remaining.as_slice() == ["-h"] || remaining.as_slice() == ["--help"] {
        usage(&bump);
        return ExitCode::SUCCESS;
    }
    if !remaining.is_empty() {
        usage(&bump);
        return ExitCode::from(2);
    }
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let Some(repo_root) = manifest_dir.ancestors().nth(2) else {
        eprintln!("error: cannot locate Finery root from {manifest_dir:?}");
        return ExitCode::FAILURE;
    };
    let release_script = repo_root.join("scripts/release.sh");
    match Command::new(&release_script)
        .arg(&bump)
        .current_dir(repo_root)
        .status()
    {
        Ok(status) => exit_code(status),
        Err(error) => {
            eprintln!("error: failed to run {}: {error}", release_script.display());
            ExitCode::FAILURE
        }
    }
}

fn exit_code(status: std::process::ExitStatus) -> ExitCode {
    if let Some(code) = status.code() {
        return ExitCode::from(u8::try_from(code).unwrap_or(1));
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        return ExitCode::from(
            status
                .signal()
                .and_then(|signal| u8::try_from(128 + signal).ok())
                .unwrap_or(1),
        );
    }
    #[cfg(not(unix))]
    ExitCode::FAILURE
}
