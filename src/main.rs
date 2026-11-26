use std::process::ExitCode;

fn main() -> ExitCode {
    let code = autotyper::run(
        std::env::args_os().skip(1).collect(),
        &autotyper::Hooks::real(),
    );
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}
