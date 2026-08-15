use dis86::emu86::validator;

fn print_help() {
  let appname = std::env::args().next().unwrap();
  println!("usage: {} OPTIONS", appname);
  println!("");
  println!("OPTIONS:");
  println!("  --exe             path to MZ format exe on the filesystem");
  println!("  --corpus <dir>    run the declarative differential corpus (overrides --exe)");
  println!("  --backend         emulator backend (currently only 'dosemu2')");
}

#[derive(Debug)]
struct Args {
  exe: Option<String>,
  corpus: Option<String>,
  backend: Option<String>,
}

fn parse_args() -> Result<Args, pico_args::Error> {
  let mut pargs = pico_args::Arguments::from_env();

  if pargs.contains(["-h", "--help"]) {
    print_help();
    std::process::exit(0);
  }

  let args = Args {
    exe: pargs.opt_value_from_str("--exe")?,
    corpus: pargs.opt_value_from_str("--corpus")?,
    backend: pargs.opt_value_from_str("--backend")?,
  };

  let remaining = pargs.finish();
  if !remaining.is_empty() {
    eprintln!("Error: unused arguments left: {:?}.", remaining);
    std::process::exit(1);
  }

  Ok(args)
}

pub fn run() -> i32 {
  let args = match parse_args() {
    Ok(v) => v,
    Err(e) => {
      eprintln!("Error: {}.", e);
      return 1;
    }
  };

  if let Some(backend_name) = &args.backend {
    match backend_name.to_lowercase().as_str() {
      "dosemu2" | "dosemu" => (),
      other => {
        eprintln!("Error: unknown emulator backend '{}'. Expected 'dosemu2'.", other);
        return 1;
      }
    }
  }

  let result = if let Some(corpus) = &args.corpus {
    if args.exe.is_some() {
      eprintln!("Error: --corpus and --exe are mutually exclusive.");
      return 1;
    }
    validator::run_corpus(std::path::Path::new(corpus))
  } else {
    let exe = match &args.exe {
      Some(exe) => exe,
      None => {
        eprintln!("Error: one of --exe or --corpus is required.");
        return 1;
      }
    };
    validator::run(exe)
  };

  match result {
    Ok(_) => 0,
    Err(err) => {
      eprintln!("Error: {}", err);
      1
    }
  }
}

fn main() {
  std::process::exit(run());
}
