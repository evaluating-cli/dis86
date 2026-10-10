use crate::analyze::analyze::Analyze;
use crate::config::Config;

pub fn run(cfg: &Config, exe_path: &str) -> i32 {
  let a = match Analyze::new(cfg, exe_path) {
    Ok(a) => a,
    Err(e) => {
      eprintln!("Error: {}.", e);
      return 1;
    }
  };
  a.scan_for_all_functions(true);

  //a.analyze_code_segments_and_report();

  1
}
