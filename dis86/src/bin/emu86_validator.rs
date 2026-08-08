use dis86::emu86::validator::{self, EmulatorBackend};

fn print_help() {
  let appname = std::env::args().next().unwrap();
  println!("usage: {} OPTIONS", appname);
  println!("");
  println!("REQUIRED OPTIONS:");
  println!("  --exe             path to MZ format exe on the filesystem (required)");
  println!("");
  println!("OPTIONAL OPTIONS:");
  println!("  --backend         emulator backend: 'dosbox-x' (default) or 'dosemu2'");
}

#[derive(Debug)]
struct Args {
  exe: String,
  backend: Option<String>,
}

fn parse_args() -> Result<Args, pico_args::Error> {
  let mut pargs = pico_args::Arguments::from_env();

  if pargs.contains(["-h", "--help"]) {
    print_help();
    std::process::exit(0);
  }

  let args = Args {
    exe: pargs.value_from_str("--exe")?,
    backend: pargs.opt_value_from_str("--backend")?,
  };

  let remaining = pargs.finish();

  if remaining.len() != 0 {
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
    let backend = match backend_name.to_lowercase().as_str() {
      "dosemu2" | "dosemu" => EmulatorBackend::Dosemu2,
      "dosbox-x" | "dosbox" => EmulatorBackend::DosboxX,
      other => {
        eprintln!("Error: unknown emulator backend '{}'. Expected 'dosbox-x' or 'dosemu2'.", other);
        return 1;
      }
    };
    match validator::run_with_backend(&args.exe, backend) {
      Ok(_) => (),
      Err(err) => {
        eprintln!("Error: {}", err);
        return 1;
      }
    }
  } else {
    match validator::run(&args.exe) {
      Ok(_) => (),
      Err(err) => {
        eprintln!("Error: {}", err);
        return 1;
      }
    }
  }

  0
}

fn main() {
  std::process::exit(run());
}
