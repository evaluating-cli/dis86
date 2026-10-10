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

#[derive(Debug)]
pub struct CodeSegments(pub Vec<CodeSegment>);

impl CodeSegments {
  // Should basically match those that were manually found in annotations.py
  pub fn from_binary(binary: &Binary) -> Result<CodeSegments, String> {
    let exe = binary.exe().ok_or_else(|| "analyze mode requires an MZ executable binary".to_string())?;
    match (exe.seginfo.as_ref(), exe.ovr.as_ref()) {
      (Some(seginfo), Some(ovr)) => Self::from_overlay_info(seginfo, ovr),
      // Plain MZ image without FBOV metadata: a single segment covering the
      // whole load image, addressed relative to the load base (seg 0).
      // Segment offsets are u16, so an image that needs offsets past 0xFFFF
      // has no single-segment representation: reject upfront rather than
      // panicking later in CodeSegment::end().
      _ => {
        // usize comparison first: the narrowing below is then provably safe,
        // with no panic-shaped try_into on the oversized path.
        let size = exe.exe_data().len();
        if size > u16::MAX as usize {
          return Err(format!(
            "plain-MZ load image is {} bytes: no single 64K segment can cover it (segment offsets are u16); analysis needs FBOV seginfo metadata (or a smaller image)",
            size));
        }
        let primary = Region { seg: Seg::Normal(0), skip_off: 0, size: size as u32 };
        Ok(CodeSegments(vec![CodeSegment { primary, stub: None }]))
      }
    }
  }

  // Collect ordinary code segments and stub segments
  fn from_overlay_info(seginfo: &[mz::SegInfo], ovr: &mz::OverlayInfo) -> Result<CodeSegments, String> {
    let mut code_segments = vec![];
    let mut stub_segments = vec![];
    for s in seginfo {
      // Copy packed-struct fields out before use: taking references to them
      // is unaligned UB, even inside format!.
      let seg = s.seg;
      let skip_off = s.minoff as u32;
      let size = s.size() as u32;
      // Same u16-offset bound as the plain-MZ fallback: a wrapped seginfo
      // entry (maxoff < minoff) would panic in CodeSegment::end().
      if skip_off + size > u16::MAX as u32 {
        return Err(format!(
          "seginfo segment {} spans {:#06x}..{:#06x}: no single 64K segment can cover it (segment offsets are u16); corrupt FBOV metadata?",
          seg, skip_off, skip_off + size));
      }
      let region = Region {
        seg: Seg::Normal(seg),
        skip_off,
        size,
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

    Ok(CodeSegments(code_segments))
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

  fn synthetic_plain_mz_with_image(image_len: usize) -> Vec<u8> {
    // MZ header (cparhdr=2 paragraphs => 32-byte header/load-base gap) plus
    // a load image of image_len bytes, no FBOV. exe_end - exe_start must
    // equal image_len, with exe_end = cp*512 - (512-cblp when cblp != 0).
    let exe_end = 32 + image_len as u32;
    let (cp, cblp) = if exe_end % 512 == 0 {
      (exe_end / 512, 0)
    } else {
      (exe_end / 512 + 1, exe_end % 512)
    };
    let mut data = vec![0u8; exe_end as usize];
    data[0..2].copy_from_slice(b"MZ");
    data[2..4].copy_from_slice(&(cblp as u16).to_le_bytes()); // cblp
    data[4..6].copy_from_slice(&(cp as u16).to_le_bytes());  // cp
    data[8..10].copy_from_slice(&2u16.to_le_bytes()); // cparhdr
    data[24..26].copy_from_slice(&28u16.to_le_bytes()); // lfarlc
    data
  }

  #[test]
  fn plain_mz_yields_single_load_base_segment() {
    let exe = mz::Exe::decode(&synthetic_plain_mz_with_image(32)).expect("valid synthetic plain MZ");
    assert!(exe.seginfo.is_none());
    assert!(exe.ovr.is_none());
    let binary = Binary::from_exe(&exe, None);
    let segs = CodeSegments::from_binary(&binary).expect("small image fits one segment");
    assert_eq!(segs.0.len(), 1);
    let primary = &segs.0[0].primary;
    assert_eq!(primary.seg, Seg::Normal(0));
    assert_eq!(primary.skip_off, 0);
    assert_eq!(primary.size, 32);
    assert!(segs.0[0].stub.is_none());
  }

  #[test]
  fn plain_mz_at_64k_boundary_is_accepted() {
    // Largest representable image: end() lands exactly on 0xFFFF.
    let exe = mz::Exe::decode(&synthetic_plain_mz_with_image(u16::MAX as usize))
      .expect("valid synthetic plain MZ");
    let binary = Binary::from_exe(&exe, None);
    let segs = CodeSegments::from_binary(&binary).expect("64K image fits one segment");
    assert_eq!(segs.0.len(), 1);
    assert_eq!(segs.0[0].primary.size, u16::MAX as u32);
    assert_eq!(segs.0[0].end(), SegOff { seg: Seg::Normal(0), off: Off(u16::MAX) });
  }

  #[test]
  fn plain_mz_above_64k_is_rejected_upfront() {
    // One byte over: no single-segment representation exists, so from_binary
    // errs instead of deferring a panic to CodeSegment::end().
    let exe = mz::Exe::decode(&synthetic_plain_mz_with_image(u16::MAX as usize + 1))
      .expect("valid synthetic plain MZ");
    let binary = Binary::from_exe(&exe, None);
    let err = CodeSegments::from_binary(&binary).expect_err(">64K image must be rejected");
    assert!(err.contains("64K"), "unexpected error: {}", err);
  }

  #[test]
  fn wrapped_seginfo_segment_is_rejected_upfront() {
    // Corrupt seginfo (maxoff < minoff) would wrap end() to a bogus offset;
    // reject with the segment named instead of panicking later.
    let mut exe = mz::Exe::decode(&synthetic_plain_mz_with_image(32)).expect("valid synthetic plain MZ");
    exe.seginfo = Some(vec![mz::SegInfo { seg: 1, minoff: 0x100, maxoff: 0x0000, typ: mz::SegInfoType::CODE }]);
    exe.ovr = Some(mz::OverlayInfo { file_offset: 0, segs: vec![], stubs: vec![] });
    let binary = Binary::from_exe(&exe, None);
    let err = CodeSegments::from_binary(&binary).expect_err("wrapped seginfo must be rejected");
    assert!(err.contains("segment 1"), "unexpected error: {}", err);
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
