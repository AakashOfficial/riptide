#![feature(async_await)]

use riptide_runtime::fd::{self, ReadPipe};
use riptide_runtime::prelude::*;
use riptide_runtime::syntax::source::SourceFile;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process;
use std::rc::Rc;
use structopt::StructOpt;

mod buffer;
mod editor;

#[derive(Debug, StructOpt)]
struct Options {
    /// Evaluate the specified commands
    #[structopt(short = "c", long = "command")]
    commands: Vec<String>,

    /// Run as a login shell
    #[structopt(short = "l", long = "login")]
    login: bool,

    /// Set the verbosity level
    #[structopt(short = "v", long = "verbose", parse(from_occurrences))]
    verbosity: usize,

    /// File to execute
    #[structopt(parse(from_os_str))]
    file: Option<PathBuf>,
}

fn main() {
    // Set up logger.
    clogger::init();

    // Parse command line args.
    let options = Options::from_args();

    // Increase log level by the number of -v flags given.
    clogger::set_verbosity(options.verbosity);

    let stdin = fd::stdin();
    let mut runtime = Runtime::default();

    // An executor for running the asynchronous runtime.
    let mut executor = futures::executor::LocalPool::default();

    // If at least one command is given, execute those in order and exit.
    if !options.commands.is_empty() {
        for command in options.commands {
            match executor.run_until(runtime.execute(None, command)) {
                Ok(_) => {}
                Err(e) => {
                    log::error!("{}", e);
                    runtime.exit(1);
                    break;
                }
            }
        }
    }
    // If a file is given, execute it and exit.
    else if let Some(file) = options.file.as_ref() {
        executor.run_until(execute_file(&mut runtime, file));
    }
    // Interactive mode.
    else if stdin.is_tty() {
        executor.run_until(interactive_main(&mut runtime));
    }
    // Execute stdin
    else {
        executor.run_until(execute_stdin(&mut runtime, stdin));
    }

    // End this process with a particular exit code if specified.
    if let Some(exit_code) = runtime.exit_code() {
        process::exit(exit_code);
    }
}

async fn execute_file(runtime: &mut Runtime, path: impl AsRef<Path>) {
    let path = path.as_ref();
    let source = match SourceFile::open(path) {
        Ok(s) => s,
        Err(e) => {
            log::error!("opening file {:?}: {}", path, e);
            runtime.exit(1);
            return;
        }
    };

    if let Err(e) = runtime.execute(None, source).await {
        log::error!("{}", e);
        runtime.exit(1);
    }
}

async fn execute_stdin(runtime: &mut Runtime, mut stdin: ReadPipe) {
    let mut source = String::new();

    if let Err(e) = stdin.read_to_string(&mut source) {
        log::error!("{}", e);
        runtime.exit(1);
        return;
    }

    if let Err(e) = runtime.execute(None, SourceFile::named("<stdin>", source)).await {
        log::error!("{}", e);
        runtime.exit(1);
    }
}

/// Main loop for an interactive shell session.
///
/// It is also worth noting that this function is infallible. Once set up, the
/// shell ensures that it stays alive until the user actually requests it to
/// exit.
async fn interactive_main(runtime: &mut Runtime) {
    // We want successive commands to act like they are being executed in the
    // same file, so set up a shared scope to execute them in.
    let scope = Rc::new(riptide_runtime::table!());

    let mut editor = editor::Editor::new();

    while runtime.exit_code().is_none() {
        let line = editor.read_line();

        if !line.is_empty() {
            match runtime.execute_in_scope(Some("main"), SourceFile::named("<input>", line), scope.clone()).await {
                Ok(Value::Nil) => {}
                Ok(value) => println!("{}", value),
                Err(e) => eprintln!("error: {}", e),
            }
        }
    }
}
