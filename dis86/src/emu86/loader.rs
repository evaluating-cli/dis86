use super::machine::*;
use super::dos;
use crate::binfmt::mz;

impl Machine {
  pub fn code_load_seg(&self) -> Seg {
    let load_seg = Seg::Normal(self.psp_segment);
    let code_seg = Seg::Normal(load_seg.unwrap_normal() + 0x10);
    code_seg
  }

  pub fn load_exe(&mut self, exe: &mz::Exe) -> Result<(), String> {
    let code_seg = self.code_load_seg();
    let code_seg_u16 = code_seg.unwrap_normal();

    // Configure the PSP
    let psp = self.mem.program_segment_prefix_mut(self.psp_segment);
    // NOTE: JUST TO MATCH DOSBOX
    psp.mem_top = dos::MEM_TOP;
    psp.env_seg = dos::ENV_SEG;
    psp.cmd_tail[0] = 0x0d;
    // ... missing fields ...

    // Determine image region to copy
    let image_start  = exe.hdr.cparhdr as usize * 16;
    let image_end    = exe.hdr.cp as usize * 512;
    let image_length = image_end - image_start;
    let image        = &exe.rawdata[image_start..image_end];

    // println!("image_start:   0x{:x}", image_start);
    // println!("image_end:     0x{:x}", image_end);
    // println!("image_length:  0x{:x}", image_length);

    // Copy into memory
    let mem_start = code_seg.abs_normal();
    let mem_end   = mem_start + image_length;
    self.mem.0[mem_start..mem_end].copy_from_slice(image);

    // Perform relocations
    for reloc in &exe.relocs {
      let addr = SegOff::new(code_seg_u16 + reloc.segment, reloc.offset);
      let val = self.mem.read_u16(addr);
      self.mem.write_u16(addr, code_seg_u16 + val);
    }

    // Set up CS:IP
    self.reg_set(CS, code_seg_u16 + exe.hdr.cs as u16);
    self.reg_set(IP, exe.hdr.ip as u16);

    // Set up SS:SP
    self.reg_set(SS, code_seg_u16 + exe.hdr.ss as u16);
    self.reg_set(SP, exe.hdr.sp as u16);

    // Set up DS and ES to point at the PSP
    self.reg_set(DS, self.psp_segment);
    self.reg_set(ES, self.psp_segment);

    // IF flag should be set
    self.reg_set(FLAGS, 1<<9); // IF

    Ok(())
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn exe() -> mz::Exe {
    let mut rawdata = vec![0; 512];
    rawdata[32] = 0xcc;
    rawdata[34..36].copy_from_slice(&7u16.to_le_bytes());
    mz::Exe {
      hdr: mz::Header { magic: *b"MZ", cblp: 0, cp: 1, crlc: 1, cparhdr: 2,
        minalloc: 0, maxalloc: 0, ss: 4, sp: 0x1234, csum: 0, ip: 0x5678,
        cs: 3, lfarlc: 0, ovno: 0 },
      exe_start: 32, exe_end: 512, relocs: vec![mz::Reloc { offset: 2, segment: 0 }],
      fbov: None, seginfo: None, ovr: None, rawdata,
    }
  }

  #[test]
  fn default_machine_retains_legacy_psp_segment() {
    let mut m = Machine::new(None);
    m.load_exe(&exe()).unwrap();
    assert_eq!((m.psp_segment, m.reg_read_u16(DS), m.reg_read_u16(ES)),
      (0x0813, 0x0813, 0x0813));
  }

  #[test]
  fn custom_psp_rebases_all_load_state() {
    let mut m = Machine::new_with_psp_segment(None, 0x2000);
    m.load_exe(&exe()).unwrap();
    let image = 0x2010;
    assert_eq!(m.mem.read_u8(SegOff::new(0x2000, 0x81)), 0x0d); // PSP placement
    assert_eq!(m.mem.read_u8(SegOff::new(image, 0)), 0xcc); // image placement
    assert_eq!(m.mem.read_u16(SegOff::new(image, 2)), image + 7); // relocation
    assert_eq!((m.reg_read_u16(CS), m.reg_read_u16(SS)), (image + 3, image + 4));
    assert_eq!((m.reg_read_u16(DS), m.reg_read_u16(ES)), (0x2000, 0x2000));
  }
}
