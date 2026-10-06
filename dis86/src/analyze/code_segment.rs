use crate::segoff::{Seg, Off, SegOff};
use crate::binfmt::mz;
use crate::binary::Binary;
use crate::config::{Config, Func};

#[derive(Debug, Clone)]
pub struct Region {
  pub seg: Seg,
  pub skip_off: u32, // The segment data might not start at 0
  pub size: u32,
}

#[derive(Debug)]
pub struct CodeSegment {
  pub primary: Region,
  pub stub: Option<Region>,
}

impl CodeSegment {
  pub fn start(&self) -> SegOff {
    SegOff { seg: self.primary.seg, off: Off(self.primary.skip_off as u16) }
  }
  pub fn end(&self) -> SegOff {
    let end_off: u16 = (self.primary.skip_off + self.primary.size).try_into().unwrap();
    SegOff { seg: self.primary.seg, off: Off(end_off) }
  }
}

pub struct CodeSegments(pub Vec<CodeSegment>);

impl CodeSegments {
  // Should basically match those that were manually found in annotations.py
  pub fn from_binary(binary: &Binary) -> CodeSegments {
    let exe = binary.exe().unwrap(); // FIXME
    match (exe.seginfo.as_ref(), exe.ovr.as_ref()) {
      (Some(seginfo), Some(ovr)) => Self::from_overlay_info(seginfo, ovr),
      // Plain MZ image without FBOV metadata: a single segment covering the
      // whole load image, addressed relative to the load base (seg 0).
      _ => {
        let size: u32 = exe.exe_data().len().try_into().unwrap();
        let primary = Region { seg: Seg::Normal(0), skip_off: 0, size };
        CodeSegments(vec![CodeSegment { primary, stub: None }])
      }
    }
  }

  fn from_overlay_info(seginfo: &[mz::SegInfo], ovr: &mz::OverlayInfo) -> CodeSegments {    // Collect ordinary code segments and stub segments
    let mut code_segments = vec![];
    let mut stub_segments = vec![];
    for s in seginfo {
      let region = Region {
        seg: Seg::Normal(s.seg),
        skip_off: s.minoff as u32,
        size: s.size() as u32,
      };
      if s.typ == mz::SegInfoType::CODE && s.size() != 0 {
        code_segments.push(CodeSegment { primary: region, stub: None, });
      }
      else if s.typ == mz::SegInfoType::STUB {
        stub_segments.push(region);
      }
    }

    // Iterate all overlay segments and match them up with the stubs
    for (i, seg) in ovr.segs.iter().enumerate() {
      let region = Region {
      seg: Seg::Overlay(i as u16),
        skip_off: 0,
        size: seg.segment_size as u32,
      };
      let stub = stub_segments[i].clone();
      assert!(stub.skip_off == 0);
      code_segments.push(CodeSegment { primary: region, stub: Some(stub) });
    }

    CodeSegments(code_segments)
  }

  pub fn find_by_segment(&self, seg: Seg) -> Option<&CodeSegment> {
    for c in &self.0 {
      if c.primary.seg == seg {
        return Some(c);
      }
    }
    None
  }

  pub fn find_for_function(&self, func: &Func) -> Option<&CodeSegment> {
    self.find_by_segment(func.start.seg)
  }

  pub fn dump(&self) {
    for (i, s) in self.0.iter().enumerate() {
      let seg_str = format!("{},", s.primary.seg);
      let mut ex = "".to_string();
      if let Some(stub) = &s.stub {
        ex = format!("    entry-seg: {},   entry-seg-size: {}",
                     stub.seg, stub.size);
      }
      println!("{:3} | seg: {:<15} skip_off: 0x{:04x},   size: {:>6}{}",
               i, seg_str, s.primary.skip_off, s.primary.size, ex);
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::binary::Binary;

  fn synthetic_plain_mz() -> Vec<u8> {
    // MZ header: 32-byte header followed by a 32-byte load image, no FBOV.
    let mut data = vec![0u8; 96];
    data[0..2].copy_from_slice(b"MZ");
    data[2..4].copy_from_slice(&64u16.to_le_bytes()); // cblp
    data[4..6].copy_from_slice(&1u16.to_le_bytes());  // cp
    data[8..10].copy_from_slice(&2u16.to_le_bytes()); // cparhdr
    data[24..26].copy_from_slice(&28u16.to_le_bytes()); // lfarlc
    data
  }

  #[test]
  fn plain_mz_yields_single_load_base_segment() {
    let exe = mz::Exe::decode(&synthetic_plain_mz()).expect("valid synthetic plain MZ");
    assert!(exe.seginfo.is_none());
    assert!(exe.ovr.is_none());
    let binary = Binary::from_exe(&exe, None);
    let segs = CodeSegments::from_binary(&binary);
    assert_eq!(segs.0.len(), 1);
    let primary = &segs.0[0].primary;
    assert_eq!(primary.seg, Seg::Normal(0));
    assert_eq!(primary.skip_off, 0);
    assert_eq!(primary.size, 32);
    assert!(segs.0[0].stub.is_none());
  }
}

#[derive(Debug)]
pub struct CodeDetail {
  pub function_entries: Vec<Func>,
}

impl CodeDetail {
  pub fn build(code_seg: &CodeSegment, cfg: &Config) -> CodeDetail {
    let mut function_entries = vec![];
    for f in &cfg.funcs {
      if f.start.seg != code_seg.primary.seg { continue };
      function_entries.push(f.clone());
    }

    CodeDetail { function_entries }
  }
}
