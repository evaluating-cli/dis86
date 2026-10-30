use crate::config::Config;
use crate::segoff::Seg;
use crate::util::range_set::RangeSet;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Coverage {
  pub code: u32,
  pub data: u32,
  pub unknown: u32,
  pub total: u32,
}

impl Coverage {
  pub fn annotated(&self) -> u32 {
    self.code + self.data
  }

  pub fn percent(&self, bytes: u32) -> f64 {
    if self.total == 0 { 100.0 } else { 100.0 * bytes as f64 / self.total as f64 }
  }
}

fn bytes_in(ranges: &RangeSet, start: u32, end: u32) -> u32 {
  (end.saturating_sub(start)).saturating_sub(gaps_size(ranges, start, end))
}

fn gaps_size(ranges: &RangeSet, start: u32, end: u32) -> u32 {
  ranges.gaps_within(start, end).iter().map(|gap| gap.end - gap.start).sum()
}

pub fn for_span(cfg: &Config, seg: Seg, seg_start: u32, seg_end: u32) -> Coverage {
  let mut code = RangeSet::new();
  let mut data = RangeSet::new();
  let mut known = RangeSet::new();

  for func in &cfg.funcs {
    if func.start.seg != seg { continue; }
    for region in cfg.code_regions(&func.effective_regions()) {
      if region.start.seg != seg || region.end.seg != seg { continue; }
      let start = (region.start.off.0 as u32).max(seg_start);
      let end = (region.end.off.0 as u32).min(seg_end);
      if start < end {
        code.insert(start, end);
        known.insert(start, end);
      }
    }
  }

  for region in cfg.text_regions_matching_segment(seg) {
    if region.end.seg != seg { continue; }
    let start = (region.start.off.0 as u32).max(seg_start);
    let end = (region.end.off.0 as u32).min(seg_end);
    if start < end {
      data.insert(start, end);
      known.insert(start, end);
    }
  }

  Coverage {
    code: bytes_in(&code, seg_start, seg_end),
    data: bytes_in(&data, seg_start, seg_end),
    unknown: gaps_size(&known, seg_start, seg_end),
    total: seg_end.saturating_sub(seg_start),
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::config::{CallMode, Func, FuncRegion, TextSectionRegion};
  use crate::segoff::{Off, SegOff};
  use crate::types::TypeDatabase;
  use std::rc::Rc;

  fn so(off: u16) -> SegOff {
    SegOff { seg: Seg::Normal(0), off: Off(off) }
  }

  fn cfg() -> Config {
    Config {
      types: Rc::new(TypeDatabase::new()),
      structs: vec![],
      code_segs: vec![],
      funcs: vec![Func {
        name: "f".to_string(),
        start: so(0),
        end: Some(so(10)),
        entry: None,
        mode: CallMode::Near,
        ret: None,
        args: None,
        regargs: None,
        regouts: None,
        clobbers: None,
        dont_pop_args: false,
        regions: vec![
          FuncRegion { start: so(0), end: so(10) },
          FuncRegion {
            start: SegOff { seg: Seg::Normal(1), off: Off(12) },
            end: SegOff { seg: Seg::Normal(1), off: Off(14) },
          },
        ],
      }],
      indirects: vec![],
      globals: vec![],
      text_section: vec![
        TextSectionRegion {
          name: "hole".to_string(), start: so(4), end: so(6), typ: None, access: None,
        },
        TextSectionRegion {
          name: "table".to_string(), start: so(15), end: so(18), typ: None, access: None,
        },
        TextSectionRegion {
          name: "cross".to_string(),
          start: so(5),
          end: SegOff { seg: Seg::Normal(1), off: Off(7) },
          typ: None,
          access: None,
        },
      ],
    }
  }

  #[test]
  fn splits_configured_code_data_and_unknown() {
    assert_eq!(for_span(&cfg(), Seg::Normal(0), 0, 20), Coverage {
      code: 8,
      data: 5,
      unknown: 7,
      total: 20,
    });
  }

  #[test]
  fn clamps_regions_to_the_requested_span() {
    assert_eq!(for_span(&cfg(), Seg::Normal(0), 0, 17), Coverage {
      code: 8,
      data: 4,
      unknown: 5,
      total: 17,
    });
  }

  #[test]
  fn empty_span_is_fully_annotated() {
    let coverage = for_span(&cfg(), Seg::Normal(0), 0, 0);
    assert_eq!(coverage.total, 0);
    assert_eq!(coverage.percent(coverage.annotated()), 100.0);
  }
}
