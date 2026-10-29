use crate::asm::decode::Decoder;
use crate::asm::instr::Instr;
use crate::binary::Binary;
use crate::segoff::SegOff;
use crate::config::{self, Config, Func};

pub struct Spec<'a> {
  pub func: Option<&'a config::Func>,
  pub name: String,
  pub start: SegOff,
  pub end: SegOff,
  pub regions: Vec<config::FuncRegion>,
}

impl<'a> Spec<'a> {
  pub fn from_config_name(cfg: &'a Config, name: &str) -> Option<Self> {
    let func = cfg.func_lookup_by_name(name)?;
    Some(Self::from_func(func))
  }

  pub fn from_func(func: &'a Func) -> Self {
    let start = func.start;
    let regions = func.effective_regions();
    let Some(end) = func.end.or_else(|| regions.last().map(|region| region.end)) else {
      panic!("Function has no 'end' addr defined in config");
    };
    Spec {
      name: func.name.to_string(),
      func: Some(func),
      start,
      end,
      regions,
    }
  }

  pub fn from_start_and_end(start: Option<SegOff>, end: Option<SegOff>) -> Self {
    let Some(start) = start else { panic!("No start address provided") };
    let Some(end) = end else { panic!("No end address provided") };
    let name = format!("func_{}_{}", start.seg, start.off);
    Self {
      name,
      func: None,
      start,
      end,
      regions: vec![config::FuncRegion { start, end }],
    }
  }

  pub fn contains_addr(&self, addr: SegOff) -> bool {
    self.regions.iter().any(|region| region.contains(addr))
  }

  fn is_default_range(&self) -> bool {
    self.regions.len() == 1 &&
      self.regions[0].start == self.start &&
      self.regions[0].end == self.end
  }

  /// Human-readable range label used by IR diagnostics. The legacy
  /// `range [start, end)` wording is preserved for ordinary single-range
  /// functions; explicit non-contiguous bodies list every region.
  pub fn range_description(&self) -> String {
    if self.is_default_range() {
      return format!("range [{}, {})", self.start, self.end);
    }
    let regions = self.regions.iter()
      .map(|region| format!("[{}, {})", region.start, region.end))
      .collect::<Vec<_>>()
      .join(", ");
    format!("regions {}", regions)
  }
}

/// Decode each configured region independently. The entry region is decoded
/// first (split at `spec.start` when a shared epilogue begins before it), so
/// the first instruction belongs to the function entry. Remaining regions
/// follow in ascending order; gaps between them are never decoded.
pub fn decode_regions<'a>(
  binary: &'a Binary,
  spec: &Spec<'_>,
) -> Result<(Vec<Instr>, Vec<&'a [u8]>), String> {
  let Some(entry_region) = spec.regions.iter().find(|region| region.contains(spec.start)) else {
    return Err(format!("function start {} is outside every configured region", spec.start));
  };

  let mut chunks = vec![(spec.start, entry_region.end)];
  if entry_region.start < spec.start {
    chunks.push((entry_region.start, spec.start));
  }

  let mut others: Vec<_> = spec.regions.iter()
    .filter(|region| *region != entry_region)
    .copied()
    .collect();
  others.sort();
  chunks.extend(others.into_iter().map(|region| (region.start, region.end)));

  let mut instr_list = vec![];
  let mut raw_list = vec![];
  for (start, end) in chunks {
    let mut decoder = Decoder::new(binary.region_iter(start, end));
    loop {
      match decoder.try_next() {
        Ok(Some((instr, raw))) => {
          instr_list.push(instr);
          raw_list.push(raw);
        }
        Ok(None) => break,
        Err(e) => return Err(format!("decode failed at {}: {} in region [{}, {})", start, e, start, end)),
      }
    }
  }
  Ok((instr_list, raw_list))
}

pub fn specs_from_codeseg_name<'a>(cfg: &'a Config, name: &str) -> Vec<Spec<'a>> {
  let Some(codeseg) = cfg.code_seg_lookup_by_name(name) else {
    panic!("Failed to find code segment with name '{}'", name);
  };

  let mut ret = vec![];
  for func in cfg.func_lookup_by_seg(codeseg.seg) {
    ret.push(Spec::from_func(func));
  }
  ret
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::config::{CallMode, FuncRegion};
  use crate::segoff::{Off, Seg};

  fn so(off: u16) -> SegOff {
    SegOff { seg: Seg::Normal(0), off: Off(off) }
  }

  fn func(start: u16, end: Option<u16>, regions: Vec<FuncRegion>) -> Func {
    Func {
      name: "f".to_string(),
      start: so(start),
      end: end.map(so),
      entry: None,
      mode: CallMode::Near,
      ret: None,
      args: Some(0),
      regargs: None,
      regouts: None,
      clobbers: None,
      dont_pop_args: false,
      regions,
    }
  }

  #[test]
  fn default_spec_has_one_region_and_exclusive_end() {
    let spec = Spec::from_start_and_end(Some(so(0x100)), Some(so(0x120)));
    assert_eq!(spec.regions, vec![FuncRegion { start: so(0x100), end: so(0x120) }]);
    assert!(spec.contains_addr(so(0x100)));
    assert!(spec.contains_addr(so(0x11f)));
    assert!(!spec.contains_addr(so(0x120)));
    assert_eq!(spec.range_description(), "range [0000:0100, 0000:0120)");
  }

  #[test]
  fn explicit_regions_drive_spec_and_can_derive_end() {
    let f = func(0x200, None, vec![
      FuncRegion { start: so(0x240), end: so(0x250) },
      FuncRegion { start: so(0x100), end: so(0x120) },
    ]);
    let spec = Spec::from_func(&f);
    assert_eq!(spec.end, so(0x250));
    assert_eq!(spec.regions, vec![
      FuncRegion { start: so(0x100), end: so(0x120) },
      FuncRegion { start: so(0x240), end: so(0x250) },
    ]);
    assert!(!spec.contains_addr(so(0x200)));
    assert!(spec.contains_addr(so(0x245)));
    assert_eq!(spec.range_description(),
      "regions [0000:0100, 0000:0120), [0000:0240, 0000:0250)");
  }
}
