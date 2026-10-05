use crate::binfmt::mz::*;

fn decode_exe(data: &[u8]) -> Result<Exe, String> {
  // Decode the header
  let hdr = decode_hdr(data)?;

  // Compute the EXE Region
  let exe_start = hdr.cparhdr as u32 * 16;
  let mut exe_end = hdr.cp as u32 * 512;
  if hdr.cblp != 0 { exe_end -= 512 - hdr.cblp as u32; }
  if exe_end as usize > data.len() {
    return Err(format!("End of exe region is beyond the end of data"));
  }

  // Determine the relocs array
  let relocs = unsafe { util::try_slice_from_bytes(&data[hdr.lfarlc as usize..], hdr.crlc as usize) }?;

  // Optional FBOV
  let data_rem = &data[exe_end as usize..];
  let fbov = decode_fbov(data_rem);

  // Optional seginfo
  let mut seginfo = None;
  if let Some(fbov) = fbov {
    // Decode seginfo
    if fbov.segnum < 0 {
      let segnum = fbov.segnum; // unaligned
      return Err(format!("Negative FBOV segnum: {}", segnum));
    }
    let slice = unsafe { util::try_slice_from_bytes(&data[fbov.exeinfo as usize..], fbov.segnum as usize) }?;
    seginfo = Some(slice.to_vec());
  }

  // Optional overlay info
  let mut ovr = None;
  if let Some(fbov) = fbov {
    ovr = Some(overlay::decode_overlay_info(data, exe_start, fbov, seginfo.as_ref().unwrap())?);
  }

  Ok(Exe {
    hdr: hdr.clone(),
    exe_start,
    exe_end,
    relocs: relocs.to_vec(),
    fbov: fbov.cloned(),
    seginfo,
    ovr,
    rawdata: data.to_vec(),
  })
}

fn decode_hdr<'a>(data: &'a [u8]) -> Result<&'a Header, String> {
  // Get the header and perform magic check
  let hdr: &Header = unsafe { util::try_struct_from_bytes(data) }?;
  let magic_expect = ['M' as u8, 'Z' as u8];
  if hdr.magic != magic_expect {
    return Err(format!("Magic number mismatch: got {:?}, expected {:?}", hdr.magic, magic_expect));
  }

  Ok(hdr)
}

fn decode_fbov<'a>(data: &'a [u8]) -> Option<&'a FBOV> {
  // Get the struct and perform magic check
  let fbov: &FBOV = unsafe { util::try_struct_from_bytes(data) }.ok()?;
  let magic_expect = ['F' as u8, 'B' as u8, 'O' as u8, 'V' as u8];
  if fbov.magic != magic_expect {
    return None;
  }

  // All good
  Some(fbov)
}

impl Exe {
  #[cfg(target_endian = "big")]
  pub fn decode(data: &[u8]) -> Result<Self, String> {
    panic!("MZ decoding only works on little-endian machines");
  }

  #[cfg(target_endian = "little")]
  pub fn decode(data: &[u8]) -> Result<Self, String> {
    decode_exe(data)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn synthetic_fbov_exe() -> Vec<u8> {
    let mut data = vec![0u8; 116];
    // MZ header: 32-byte header followed by a 64-byte load image.
    data[0..2].copy_from_slice(b"MZ");
    data[2..4].copy_from_slice(&96u16.to_le_bytes());
    data[4..6].copy_from_slice(&1u16.to_le_bytes());
    data[8..10].copy_from_slice(&2u16.to_le_bytes());
    data[24..26].copy_from_slice(&28u16.to_le_bytes());

    // FBOV record follows the MZ image; its seginfo points inside the image.
    data[96..100].copy_from_slice(b"FBOV");
    data[100..104].copy_from_slice(&4u32.to_le_bytes());
    data[104..108].copy_from_slice(&88u32.to_le_bytes());
    data[108..112].copy_from_slice(&1i32.to_le_bytes());

    // Overlay stub-segment header and one five-byte dispatch stub.
    data[32..36].copy_from_slice(&[0xcd, 0x3f, 0, 0]);
    data[36..40].copy_from_slice(&0u32.to_le_bytes());
    data[40..42].copy_from_slice(&4u16.to_le_bytes());
    data[88..90].copy_from_slice(&0u16.to_le_bytes()); // segment
    data[90..92].copy_from_slice(&37u16.to_le_bytes()); // header + one stub
    data[92..94].copy_from_slice(&SegInfoType::STUB.to_le_bytes());
    data[94..96].copy_from_slice(&0u16.to_le_bytes());
    data[64..66].copy_from_slice(&[0xcd, 0x3f]);
    data[66..68].copy_from_slice(&0u16.to_le_bytes());
    data[112..116].copy_from_slice(&[0xb8, 0x77, 0x0a, 0xcb]);
    data
  }

  #[test]
  fn decodes_synthetic_mz_fbov_stub_and_overlay_payload() {
    let exe = Exe::decode(&synthetic_fbov_exe()).expect("valid synthetic MZ/FBOV fixture");
    assert_eq!(exe.exe_start, 32);
    assert_eq!(exe.exe_end, 96);
    assert_eq!(exe.num_overlay_segments(), 1);
    assert_eq!(exe.overlay_data(0), &[0xb8, 0x77, 0x0a, 0xcb]);

    let stub = &exe.ovr.as_ref().unwrap().stubs[0];
    assert_eq!(stub.stub_addr().seg, crate::segoff::Seg::Normal(0));
    assert_eq!(stub.stub_addr().off.0, 32);
    assert_eq!(stub.dest_addr().off.0, 0);
    assert!(matches!(stub.dest_addr().seg, crate::segoff::Seg::Overlay(0)));
  }
}
