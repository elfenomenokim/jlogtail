use std::env;

fn main() {
    match jlogtail::run_cli(env::args().skip(1).collect()) {
        Ok(()) => (),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    }
}
