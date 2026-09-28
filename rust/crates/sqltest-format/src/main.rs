use clap::Parser;

#[derive(Parser)]
#[command(about = "Format SQLLogicTest snippets without executing SQL")]
struct Cli {
    #[command(flatten)]
    args: sirius_sqltest_format::FormatArgs,
}

fn main() {
    match sirius_sqltest_format::execute(&Cli::parse().args) {
        Ok(true) => {}
        Ok(false) => std::process::exit(1),
        Err(error) => {
            eprintln!("{error:#}");
            std::process::exit(2);
        }
    }
}
