use crate::segoff::{Off, Seg, SegOff};
use crate::bsl;
use crate::types::{Type, TypeDatabase};
use crate::asm::instr::Reg;
use crate::util::range_set::RangeSet;
use std::rc::Rc;

#[derive(Debug, Clone)]
pub struct CodeSeg {
  pub seg: Seg,
  pub name: String,
}

/// A half-open, same-segment slice of a function body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct FuncRegion {
  pub start: SegOff,
  pub end: SegOff,
}

impl FuncRegion {
  pub fn contains(&self, addr: SegOff) -> bool {
    addr.seg == self.start.seg && addr.off >= self.start.off && addr.off < self.end.off
  }
}

#[derive(Debug, Clone)]
pub struct Func {
  pub name: String,
  pub start: SegOff,
  pub end: Option<SegOff>,
  pub entry: Option<SegOff>,
  pub mode: CallMode,
  pub ret: Option<Type>,
  pub args: Option<u16>,  // None means "unknown", Some(0) means "no args"
  pub regargs: Option<Vec<Reg>>,
  /// Registers whose final value is part of this function's observable result.
  pub regouts: Option<Vec<Reg>>,
  /// Registers a caller must not assume this function preserves.
  pub clobbers: Option<Vec<Reg>>,
  pub dont_pop_args: bool,
  /// Explicit discontiguous body regions. Empty means the legacy
  /// `[start, end)` range.
  pub regions: Vec<FuncRegion>,
}

#[derive(Debug, Clone)]
pub struct Indirect {
  pub addr: SegOff,
  pub ret: Type,
  pub args: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CallMode {
  Near,
  Far,
}

#[derive(Debug, Clone)]
pub struct Struct {
  pub name: String,
  pub size: u16,
  pub members: Vec<StructMember>,
}

#[derive(Debug, Clone)]
pub struct StructMember {
  pub name: String,
  pub typ: Type,
  pub off: u16,
}

#[derive(Debug, Clone)]
pub struct Global {
  pub name: String,
  pub offset: u16,
  pub typ: Type,
}

#[derive(Debug, Clone)]
pub struct TextSectionRegion {
  pub name: String,
  pub start: SegOff,
  pub end: SegOff,
  /// `Some` keeps the region's jump-table meaning. `None` declares plain
  /// data: a hole that the decoder and analyzer must not treat as code.
  pub typ: Option<Type>,
  pub access: Option<SegOff>,
}

impl TextSectionRegion {
  pub fn is_data_hole(&self) -> bool {
    self.typ.is_none()
  }
}

#[derive(Debug, Clone)]
pub struct Config {
  pub types: Rc<TypeDatabase>,
  pub structs: Vec<Struct>,
  pub code_segs: Vec<CodeSeg>,
  pub funcs: Vec<Func>,
  pub indirects: Vec<Indirect>,
  pub globals: Vec<Global>,
  pub text_section: Vec<TextSectionRegion>,
}

impl Func {
  // FIXME: Does not need to be an Option
  fn entry(&self) -> Option<SegOff> {
    if let Some(entry) = self.entry { return Some(entry); }
    Some(self.start)
  }

  pub fn return_type_defaulted(&self) -> Type {
    match &self.ret {
      Some(ret) => ret.clone(),
      None => Type::U32,  // when we don't have more information, we can pessimize and assume u32 might be returned
    }
  }

  /// Ranges the function body occupies, falling back to `[start, end)` when
  /// no explicit regions are configured. Adjacent and overlapping regions are
  /// merged; validation reports overlaps, but execution remains total.
  pub fn effective_regions(&self) -> Vec<FuncRegion> {
    if self.regions.is_empty() {
      let Some(end) = self.end else { return vec![] };
      return vec![FuncRegion { start: self.start, end }];
    }

    let mut regions = self.regions.clone();
    regions.sort();

    let mut merged: Vec<FuncRegion> = vec![];
    for region in regions {
      if let Some(last) = merged.last_mut() {
        if last.start.seg == region.start.seg && region.start.off <= last.end.off {
          if region.end.off > last.end.off {
            last.end = region.end;
          }
          continue;
        }
      }
      merged.push(region);
    }
    merged
  }

  /// Regions to use for control-flow analysis. Explicit regions are
  /// normalized; legacy functions return none so analysis keeps the original
  /// `[start, end)` behavior, including following branches outside `end`.
  pub fn analysis_regions(&self) -> Vec<FuncRegion> {
    if self.regions.is_empty() { vec![] } else { self.effective_regions() }
  }

  /// Exclusive upper bound for the function body. `end` wins when present;
  /// otherwise the highest explicit region end supplies it.
  pub fn outer_end(&self) -> Option<SegOff> {
    self.end.or_else(|| self.effective_regions().last().map(|region| region.end))
  }

  pub fn region_containing(&self, addr: SegOff) -> Option<FuncRegion> {
    self.effective_regions().into_iter().find(|region| region.contains(addr))
  }
}

impl Config {
  pub fn code_seg_lookup(&self, seg: Seg) -> Option<&CodeSeg> {
    // TODO: Consider something better than linear search
    for c in &self.code_segs {
      if seg == c.seg {
        return Some(c)
      }
    }
    None
  }

  pub fn code_seg_lookup_by_name(&self, name: &str) -> Option<&CodeSeg> {
    // TODO: Consider something better than linear search
    for c in &self.code_segs {
      if name == &c.name {
        return Some(c)
      }
    }
    None
  }

  pub fn func_lookup(&self, addr: SegOff) -> Option<&Func> {
    // TODO: Consider something better than linear search
    for f in &self.funcs {
      let Some(entry) = f.entry() else { continue };
      if addr == entry {
        return Some(f)
      }
    }
    None
  }

  pub fn indirect_lookup(&self, addr: SegOff) -> Option<&Indirect> {
    // TODO: Consider something better than linear search
    for i in &self.indirects {
      if addr == i.addr {
        return Some(i)
      }
    }
    None
  }

  pub fn func_lookup_by_name(&self, name: &str) -> Option<&Func> {
    // TODO: Consider something better than linear search
    for f in &self.funcs {
      if name == f.name.as_str() {
        return Some(f)
      }
    }
    None
  }

  pub fn func_lookup_by_seg(&self, seg: Seg) -> Vec<&Func> {
    // TODO: Consider something better than linear search
    let mut ret = vec![];
    for f in &self.funcs {
      if seg == f.start.seg {
        ret.push(f);
      }
    }
    ret
  }

  pub fn text_region_lookup_by_start_addr(&self, addr: SegOff) -> Option<&TextSectionRegion> {
    // TODO: Consider something better than linear search
    for r in &self.text_section {
      if addr == r.start {
        return Some(r)
      }
    }
    None
  }

  pub fn text_region_lookup_by_access(&self, addr: SegOff) -> Option<&TextSectionRegion> {
    // TODO: Consider something better than linear search
    for r in &self.text_section {
      if r.access.is_some() && r.access.unwrap() == addr {
        return Some(r)
      }
    }
    None
  }

  pub fn text_region_lookup(&self, start_addr: SegOff, access: SegOff) -> Option<&TextSectionRegion> {
    if let Some(r) = self.text_region_lookup_by_start_addr(start_addr) {
      return Some(r);
    }
    self.text_region_lookup_by_access(access)
  }

  pub fn text_regions_matching_segment(&self, seg: Seg) -> Vec<&TextSectionRegion> {
    let mut ret = vec![];
    // TODO: Consider something better than linear search
    for r in &self.text_section {
      if seg == r.start.seg {
        ret.push(r);
      }
    }
    ret
  }

  pub fn data_region_containing(&self, addr: SegOff) -> Option<&TextSectionRegion> {
    self.text_section.iter().find(|region|
      addr.seg == region.start.seg
        && addr.off >= region.start.off
        && addr.off < region.end.off)
  }

  pub fn has_data_in_regions(&self, regions: &[FuncRegion]) -> bool {
    regions.iter().any(|region| self.text_section.iter().any(|data|
      data.start.seg == region.start.seg
        && data.start < region.end
        && region.start < data.end))
  }

  /// Subtract every declared data region in the same segment from `regions`.
  /// The result is address-ordered and contains only runs the decoder may
  /// interpret as instructions.
  pub fn code_regions(&self, regions: &[FuncRegion]) -> Vec<FuncRegion> {
    let mut code = vec![];
    for region in regions {
      let mut data = RangeSet::new();
      for text in &self.text_section {
        if text.start.seg != region.start.seg { continue; }
        let start = text.start.off.0.max(region.start.off.0) as u32;
        let end = text.end.off.0.min(region.end.off.0) as u32;
        data.insert(start, end);
      }
      for gap in data.gaps_within(region.start.off.0 as u32, region.end.off.0 as u32) {
        code.push(FuncRegion {
          start: SegOff { seg: region.start.seg, off: Off(gap.start as u16) },
          end: SegOff { seg: region.start.seg, off: Off(gap.end as u16) },
        });
      }
    }
    code.sort();
    code
  }
}

impl Config {
  pub fn from_path(path: &str) -> Result<Config, String> {
    let mut cfg = Config {
      types: Rc::new(TypeDatabase::new()), // dummy
      structs: vec![],
      code_segs: vec![],
      funcs: vec![],
      indirects: vec![],
      globals: vec![],
      text_section: vec![],
    };

    let dat = std::fs::read_to_string(path)
      .map_err(|err| format!("Failed to read file with: {}'", err))?;

    let root = bsl::parse(&dat)
      .ok_or_else(|| format!("Failed to parse config"))?;


    let mut types = TypeDatabase::new();
    cfg.parse_structs(&root, &mut types)?; // Important for this to go first and build types
    cfg.parse_code_segs(&root)?;
    cfg.parse_functions(&root, &types)?;
    cfg.parse_globals(&root, &types)?;
    cfg.parse_text_section(&root, &types)?;

    // final
    cfg.types = Rc::new(types);

    Ok(cfg)
  }

  fn parse_code_segs(&mut self, root: &bsl::Root) -> Result<(), String> {
    let code_segs = root.get_node("dis86.code_segments")
      .ok_or_else(|| format!("Failed to get the code_segments node"))?;

    for (key, val) in code_segs.iter() {
      let f = val.as_node()
        .ok_or_else(|| format!("Expected code_seg properties"))?;

      let seg_str = f.get_str("seg")
        .ok_or_else(|| format!("No function 'seg' property for '{}'", key))?;
      let name = f.get_str("name")
        .ok_or_else(|| format!("No function 'name' property for '{}'", key))?;

      let seg: Seg = seg_str.parse()
        .map_err(|_| format!("Expected segoff for '{}.end', got '{}'", key, seg_str))?;

      self.code_segs.push(CodeSeg { seg, name: name.to_string() });
    }
    Ok(())
  }

  /// Parse the register lists used by the caller/callee contract annotations.
  /// These describe full registers only: fragment aliases and the frame,
  /// instruction-pointer, and flags registers are not call-visible outputs or
  /// clobbers. AX is the return value (see #134), so it is not a separate
  /// contract register either.
  fn parse_contract_regs(key: &str, field: &str, value: Option<&str>)
      -> Result<Option<Vec<Reg>>, String> {
    let Some(value) = value else { return Ok(None) };
    let mut regs = Vec::new();
    for name in value.split(',') {
      let name = name.trim();
      let reg = Reg::from_str_upper(name)
        .ok_or_else(|| format!("Failed to parse '{}.{}' register name: {}", key, field, name))?;
      if !matches!(reg, Reg::CX | Reg::DX | Reg::BX | Reg::SI | Reg::DI
                        | Reg::ES | Reg::SS | Reg::DS) {
        return Err(format!(
          "Unsupported register '{}' in '{}.{}' (expected CX, DX, BX, SI, DI, ES, SS, or DS; AX is the return value)",
          name, key, field));
      }
      if !regs.contains(&reg) {
        regs.push(reg);
      }
    }
    Ok(Some(regs))
  }

  fn parse_functions(&mut self, root: &bsl::Root, types: &TypeDatabase) -> Result<(), String> {
    let func = root.get_node("dis86.functions")
      .ok_or_else(|| format!("Failed to get the functions node"))?;

    for (key, val) in func.iter() {
      let f = val.as_node()
        .ok_or_else(|| format!("Expected function properties"))?;

      let start_str = f.get_str("start")
        .ok_or_else(|| format!("No function 'start' property for '{}'", key))?;
      let regions = parse_regions(&f, key)?;
      let end_str = match f.get_str("end") {
        Some(end) => end,
        None if !regions.is_empty() => "",
        None => return Err(format!("No function 'end' property for '{}'", key)),
      };
      let entry_str = f.get_str("entry").unwrap_or("");
      let mode_str = f.get_str("mode")
        .ok_or_else(|| format!("No function 'mode' property for '{}'", key))?;
      let ret_str = f.get_str("ret")
        .ok_or_else(|| format!("No function 'ret' property for '{}'", key))?;
      let args_str = f.get_str("args")
        .ok_or_else(|| format!("No function 'args' property for '{}'", key))?;

      let dont_pop_args = f.get_str("dont_pop_args").is_some();
      let indirect = f.get_str("indirect_call_location").is_some();

      let regargs = f.get_str("regargs");
      let regouts = Self::parse_contract_regs(key, "regouts", f.get_str("regouts"))?;
      let clobbers = Self::parse_contract_regs(key, "clobbers", f.get_str("clobbers"))?;

      let start: SegOff = start_str.parse()
        .map_err(|_| format!("Expected segoff for '{}.start', got '{}'", key, start_str))?;
      let end: Option<SegOff> = if end_str.len() == 0 { None } else {
        Some(end_str.parse()
             .map_err(|_| format!("Expected segoff for '{}.end', got '{}'", key, end_str))?)
      };
      let entry: Option<SegOff> = if entry_str.len() == 0 { None } else {
        Some(entry_str.parse()
             .map_err(|_| format!("Expected segoff for '{}.entry', got '{}'", key, entry_str))?)
      };
      let mode = match mode_str {
        "near" => CallMode::Near,
        "far" => CallMode::Far,
        _ => return Err(format!("Unsupported mode '{}' for function '{}' (expected 'near' or 'far')", mode_str, key)),
      };
      let mut args: i16 = -1;
      if args_str != "None" {
        args = args_str.parse()
          .map_err(|_| format!("Expected u16 for '{}.args', got '{}'", key, args_str))?;
      }
      let mut ret: Option<Type> = None;
      if ret_str != "None" {
        ret = Some(types.parse_type(ret_str)
          .map_err(|err| format!("Expected type for '{}.ret', got '{}' | {}", key, ret_str, err))?);
      }

      let regargs = match regargs {
        None => None,
        Some(s) => {
          let mut args = vec![];
          for reg in s.split(',') {
            let reg = Reg::from_str_upper(reg)
              .ok_or_else(|| format!("Failed to parse register name: {}", reg))?;
            args.push(reg);
          }
          Some(args)
        }
      };

      if !indirect {
        self.funcs.push(Func {
          name: key.to_string(),
          start,
          end,
          entry,
          mode,
          ret,
          args: if args >= 0 { Some(args as u16) } else { None },
          regargs,
          regouts,
          clobbers,
          dont_pop_args,
          regions,
        });
      } else {
        if !regions.is_empty() {
          return Err(format!("Indirect call '{}' cannot declare 'regions'", key));
        }
        if regouts.is_some() || clobbers.is_some() {
          return Err(format!(
            "Indirect call '{}' cannot declare 'regouts' or 'clobbers'", key));
        }
        if mode != CallMode::Far {
          return Err(format!("Indirect call '{}' must use 'mode far'", key));
        }
        let Some(ret) = ret else {
          return Err(format!("Indirect call '{}' must declare a return type", key));
        };
        if args < 0 {
          return Err(format!("Indirect call '{}' must declare its argument count", key));
        }
        self.indirects.push(Indirect {
          addr: start,
          ret,
          args: args as u16,
        });
      }
    }

    Ok(())
  }

  fn parse_structs(&mut self, root: &bsl::Root, types: &mut TypeDatabase) -> Result<(), String> {
    let structures = root.get_node("dis86.structures")
      .ok_or_else(|| format!("Failed to get the structures node"))?;

    for (key, val) in structures.iter() {
      let name = key;

      let s = val.as_node()
        .ok_or_else(|| format!("Expected structure properties"))?;

      let size_str = s.get_str("size")
        .ok_or_else(|| format!("No function 'size' property for '{}'", name))?;
      let size: u16 = size_str.parse()
        .map_err(|_| format!("Expected u16 for '{}.start', got '{}'", name, size_str))?;
      if size == 0 { return Err(format!("Structure {} must have a non-zero guest layout size", name)); }

      let mbrs = s.get_node("members")
        .ok_or_else(|| format!("Expected {}.members node", name))?;

      let mut members = vec![];
      for (key, val) in mbrs.iter() {
        let mbr = val.as_node()
          .ok_or_else(|| format!("Expected member properties for {}.members.{}", name, key))?;

        let off_str = mbr.get_str("off")
          .ok_or_else(|| format!("No 'off' property for '{}.members.{}'", name, key))?;
        let type_str = mbr.get_str("type")
          .ok_or_else(|| format!("No 'type' property for '{}.members.{}'", name, key))?;

        let off = parse_u16(off_str)
          .map_err(|_| format!("Expected u16 hex for '{}.members.{}.off', got '{}'", name, key, off_str))?;
        let typ = types.parse_type(type_str)
          .map_err(|err| format!("Invalid annotated type for {}.{}: '{}' ({})", name, key, type_str, err))?;
        // let typ: Type = match type_str.parse() {
        //   Ok(typ) => typ,
        //   Err(err) => {
        //     // FIXME: Make this a hard error.. currently the configs have undefined struct names.. need to support that first :-(
        //     eprintln!("WRN: Expected type for '{}.members.{}.type', got '{}' | {}", name, key, type_str, err);
        //     Type::Unknown
        //   }
        // };

        members.push(StructMember {
          name: key.to_string(),
          typ,
          off,
        });
      }

      let mut cursor = 0usize;
      for member in &members {
        if member.off as usize != cursor {
          return Err(format!("Invalid layout for {}.{}: member starts at 0x{:x}, expected 0x{:x}; add explicit padding members", name, member.name, member.off, cursor));
        }
        let size = member.typ.size_in_bytes().ok_or_else(|| format!("Unknown size for annotated member {}.{} ({})", name, member.name, member.typ))?;
        cursor = cursor.checked_add(size).ok_or_else(|| format!("Layout overflow in {}.{}", name, member.name))?;
      }
      if cursor != size as usize {
        return Err(format!("Invalid layout for {}: members occupy {} bytes, declared size is {}", name, cursor, size));
      }


      let s = Struct {
        name: name.to_string(),
        size,
        members,
      };

      types.append_struct(&s);
      self.structs.push(s);
    }

    Ok(())
  }

  fn parse_globals(&mut self, root: &bsl::Root, types: &TypeDatabase) -> Result<(), String> {
    let glob = root.get_node("dis86.globals")
      .ok_or_else(|| format!("Failed to get the globals node"))?;

    for (key, val) in glob.iter() {
      let g = val.as_node()
        .ok_or_else(|| format!("Expected global properties"))?;

      let off_str = g.get_str("off")
        .ok_or_else(|| format!("No global 'off' property for '{}'", key))?;
      let type_str = g.get_str("type")
        .ok_or_else(|| format!("No global 'type' property for '{}'", key))?;

      let off = parse_u16(off_str)
        .map_err(|_| format!("Expected u16 hex for '{}.off', got '{}'", key, off_str))?;
      let typ = match types.parse_type(type_str) {
        Ok(typ) => typ,
        Err(err) => {
          if type_str.chars().any(|ch| matches!(ch, '[' | ']' | '<' | '>')) {
            return Err(format!("Invalid annotated type for global {}: '{}' ({})", key, type_str, err));
          }
          // FIXME: Make this a hard error.. currently the configs have undefined struct names.. need to support that first :-(
          eprintln!("WRN: Expected type for '{}.type', got '{}' | {}", key, type_str, err);
          Type::Unknown
        }
      };

      if typ.has_unknown_array_bound() {
        return Err(format!("Global {} uses an array without all fixed bounds: {}", key, typ));
      }

      if matches!(typ, Type::Array(_, _)) && typ.size_in_bytes().is_none() {
        return Err(format!("Global {} array layout size overflows for {}", key, typ));
      }

      if let Some(size) = typ.size_in_bytes() {
        if (off as usize).checked_add(size).filter(|end| *end <= 0x10000).is_none() {
          return Err(format!("Invalid layout for global {}: {} at 0x{:04x} extends beyond the 64 KiB data segment", key, typ, off));
        }
      }

      self.globals.push(Global {
        name: key.to_string(),
        offset: off,
        typ,
      });
    }
    Ok(())
  }

  fn parse_text_section(&mut self, root: &bsl::Root, types: &TypeDatabase) -> Result<(), String> {
    let func = root.get_node("dis86.text_section")
      .ok_or_else(|| format!("Failed to get the text_section node"))?;

    for (key, val) in func.iter() {
      let f = val.as_node()
        .ok_or_else(|| format!("Expected text_section properties"))?;

      let start_str = f.get_str("start")
        .ok_or_else(|| format!("No text_section 'start' property for '{}'", key))?;
      let end_str = f.get_str("end")
        .ok_or_else(|| format!("No text_section 'end' property for '{}'", key))?;
      let type_str = f.get_str("type");
      let access_str = f.get_str("access");

      let start: SegOff = start_str.parse()
        .map_err(|_| format!("Expected segoff for '{}.start', got '{}'", key, start_str))?;
      let end: SegOff = end_str.parse()
        .map_err(|_| format!("Expected segoff for '{}.end', got '{}'", key, end_str))?;
      let typ: Option<Type> = match type_str {
        None => None,
        Some(type_str) => Some(types.parse_type(type_str)
          .map_err(|err| format!("Expected type for '{}.type', got '{}' | {}", key, type_str, err))?),
      };
      let access: Option<SegOff> = match access_str {
        None => None,
        Some(access) => Some(access.parse()
          .map_err(|err| format!("Expected segoff for '{}.access', got '{}' | {}", key, access, err))?),
      };
      if typ.is_none() && access.is_some() {
        return Err(format!(
          "text_section '{}' declares access without a type; an untyped region is a data hole, not a jump table",
          key));
      }

      self.text_section.push(TextSectionRegion {
        name: key.to_string(),
        start,
        end,
        typ,
        access,
      });
    }

    Ok(())
  }
}

// parse("0x1234") -> 4660
fn parse_hex_u16(s: &str) -> Result<u16, &'static str> {
  if !s.starts_with("0x") {
    return Err("Expected 0x prefix");
  } else {
    crate::util::parse::hex_u16(&s[2..])
  }
}

fn parse_regions(func: &bsl::Node, func_key: &str) -> Result<Vec<FuncRegion>, String> {
  let Some(regions) = func.get_node("regions") else { return Ok(vec![]) };

  let mut parsed = vec![];
  for (key, val) in regions.iter() {
    let region = val.as_node()
      .ok_or_else(|| format!("Expected region properties for '{}.regions.{}'", func_key, key))?;
    let start_str = region.get_str("start")
      .ok_or_else(|| format!("No region 'start' property for '{}.regions.{}'", func_key, key))?;
    let end_str = region.get_str("end")
      .ok_or_else(|| format!("No region 'end' property for '{}.regions.{}'", func_key, key))?;
    let start: SegOff = start_str.parse()
      .map_err(|_| format!("Expected segoff for '{}.regions.{}.start', got '{}'", func_key, key, start_str))?;
    let end: SegOff = end_str.parse()
      .map_err(|_| format!("Expected segoff for '{}.regions.{}.end', got '{}'", func_key, key, end_str))?;
    parsed.push(FuncRegion { start, end });
  }
  if parsed.is_empty() {
    return Err(format!(
      "Function '{}' has an empty 'regions' node; omit it for a single range or declare at least one region",
      func_key));
  }
  Ok(parsed)
}

// parse number: either decimal or hex
fn parse_u16(s: &str) -> Result<u16, String> {
  if s.starts_with("0x") {
    parse_hex_u16(s).map_err(|err| err.to_string())
  } else {
    s.parse().map_err(|err: std::num::ParseIntError| err.to_string())
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  // Removes the temp config on drop, so a failing assertion cannot leak
  // the file into TMPDIR (house pattern).
  struct TempConfig(std::path::PathBuf);

  // Disambiguates temp files beyond the pid: unit tests in one binary share
  // a pid across threads, so a second test reusing the same name would race
  // without this.
  static TEMP_CONFIG_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

  impl TempConfig {
    fn write(name: &str, text: &str) -> Self {
      // Include the pid so concurrent `cargo test` invocations sharing TMPDIR
      // (e.g. worktrees on one machine) cannot race on the same file.
      let n = TEMP_CONFIG_SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
      let path = std::env::temp_dir().join(format!("{}_{}_{}", name, std::process::id(), n));
      std::fs::write(&path, text).unwrap();
      TempConfig(path)
    }

    fn path(&self) -> &std::path::Path {
      &self.0
    }
  }

  impl Drop for TempConfig {
    fn drop(&mut self) {
      let _ = std::fs::remove_file(&self.0);
    }
  }

  #[test]
  fn regions_parse_with_fallback_and_normalized_order() {
    use crate::segoff::Off;

    let tmp = TempConfig::write("dis86_regions_test.bsl", r#"
dis86 {
  code_segments {
  }
  functions {
    F1 { start 0000:0100 end 0000:0300 mode near ret None args None
         regions {
           _0 { start 0000:0200 end 0000:0220 }
           _1 { start 0000:0100 end 0000:0120 }
         }
    }
    F2 { start 0000:0300 end 0000:0340 mode near ret None args None }
    F3 { start 0000:0350 end "" mode near ret None args None }
    F4 { start 0000:0400 mode near ret None args None
         regions { _0 { start 0000:0400 end 0000:0410 } }
    }
  }
  structures {
  }
  globals {
  }
  text_section {
  }
}
"#);
    let cfg = Config::from_path(tmp.path().to_str().unwrap()).unwrap();
    let so = |off| SegOff { seg: Seg::Normal(0), off: Off(off) };

    assert_eq!(cfg.funcs[0].regions, vec![
      FuncRegion { start: so(0x200), end: so(0x220) },
      FuncRegion { start: so(0x100), end: so(0x120) },
    ]);
    assert_eq!(cfg.funcs[0].effective_regions(), vec![
      FuncRegion { start: so(0x100), end: so(0x120) },
      FuncRegion { start: so(0x200), end: so(0x220) },
    ]);
    assert_eq!(cfg.funcs[0].analysis_regions(), cfg.funcs[0].effective_regions());
    assert_eq!(cfg.funcs[0].outer_end(), Some(so(0x300)));

    assert_eq!(cfg.funcs[1].effective_regions(),
      vec![FuncRegion { start: so(0x300), end: so(0x340) }]);
    assert_eq!(cfg.funcs[1].analysis_regions(), vec![]);
    assert_eq!(cfg.funcs[2].effective_regions(), vec![]);
    assert_eq!(cfg.funcs[2].outer_end(), None);
    assert_eq!(cfg.funcs[3].end, None);
    assert_eq!(cfg.funcs[3].effective_regions(),
      vec![FuncRegion { start: so(0x400), end: so(0x410) }]);
    assert_eq!(cfg.funcs[3].outer_end(), Some(so(0x410)));
  }

  #[test]
  fn adjacent_regions_are_normalized_into_one() {
    use crate::segoff::Off;

    let tmp = TempConfig::write("dis86_adjacent_regions_test.bsl", r#"
dis86 {
  code_segments {
  }
  functions {
    F1 { start 0000:0100 end 0000:0140 mode near ret None args None
         regions {
           _0 { start 0000:0100 end 0000:0120 }
           _1 { start 0000:0120 end 0000:0140 }
         }
    }
  }
  structures {
  }
  globals {
  }
  text_section {
  }
}
"#);
    let cfg = Config::from_path(tmp.path().to_str().unwrap()).unwrap();
    let so = |off| SegOff { seg: Seg::Normal(0), off: Off(off) };
    assert_eq!(cfg.funcs[0].effective_regions(),
      vec![FuncRegion { start: so(0x100), end: so(0x140) }]);
  }

  #[test]
  fn missing_end_without_regions_is_still_an_error() {
    let tmp = TempConfig::write("dis86_missing_end_test.bsl", r#"
dis86 {
  code_segments {
  }
  functions {
    F1 { start 0000:0100 mode near ret None args None }
  }
  structures {
  }
  globals {
  }
  text_section {
  }
}
"#);
    let err = Config::from_path(tmp.path().to_str().unwrap()).unwrap_err();
    assert!(err.contains("No function 'end' property"), "unexpected error: {}", err);
  }

  #[test]
  fn region_requires_concrete_start_and_end() {
    let tmp = TempConfig::write("dis86_bad_region_test.bsl", r#"
dis86 {
  code_segments {
  }
  functions {
    F1 { start 0000:0100 end 0000:0300 mode near ret None args None
         regions { _0 { start 0000:0100 } }
    }
  }
  structures {
  }
  globals {
  }
  text_section {
  }
}
"#);
    let err = Config::from_path(tmp.path().to_str().unwrap()).unwrap_err();
    assert!(err.contains("No region 'end' property"), "unexpected error: {}", err);
  }

  #[test]
  fn ret_kind_unknown_marker_is_ignored_by_config_parsing() {
    // confgen emits `ret_kind_unknown 1` for functions whose return kind the
    // analyzer could not infer. The BSL lexer has no comments, so the marker
    // is an inert extra property: the parser must ignore it and keep the
    // explicitly emitted interim mode.
    let tmp = TempConfig::write("dis86_ret_unknown_test.bsl", r#"
dis86 {
  code_segments {
  }
  functions {
    F1 { start 0000:0100 end "" mode near ret None args None ret_kind_unknown 1 }
    F2 { start 0000:0200 end "" mode far ret None args None }
  }
  structures {
  }
  globals {
  }
  text_section {
  }
}
"#);
    let cfg = Config::from_path(tmp.path().to_str().unwrap()).unwrap();
    assert_eq!(cfg.funcs.len(), 2);
    assert_eq!(cfg.funcs[0].mode, CallMode::Near);
    assert_eq!(cfg.funcs[1].mode, CallMode::Far);
  }

  #[test]
  fn unsupported_mode_is_an_error_not_a_panic() {
    let tmp = TempConfig::write("dis86_bad_mode_test.bsl", r#"
dis86 {
  code_segments {
  }
  functions {
    F1 { start 0000:0100 end "" mode sideways ret None args None }
  }
  structures {
  }
  globals {
  }
  text_section {
  }
}
"#);
    let err = Config::from_path(tmp.path().to_str().unwrap()).unwrap_err();
    assert!(err.contains("Unsupported mode"), "unexpected error: {}", err);
  }

  #[test]
  fn empty_regions_node_is_an_error_not_a_fallback() {
    let tmp = TempConfig::write("dis86_empty_regions_test.bsl", r#"
dis86 {
  code_segments {
  }
  functions {
    F1 { start 0000:0100 end 0000:0300 mode near ret None args None regions { } }
  }
  structures {
  }
  globals {
  }
  text_section {
  }
}
"#);
    let err = Config::from_path(tmp.path().to_str().unwrap()).unwrap_err();
    assert!(err.contains("empty 'regions' node"), "unexpected error: {}", err);
  }

  #[test]
  fn indirect_call_cannot_declare_regions() {
    let tmp = TempConfig::write("dis86_indirect_regions_test.bsl", r#"
dis86 {
  code_segments {
  }
  functions {
    F1 { start 0000:0100 end "" mode far ret u16 args 0 indirect_call_location 1
         regions { _0 { start 0000:0100 end 0000:0110 } } }
  }
  structures {
  }
  globals {
  }
  text_section {
  }
}
"#);
    let err = Config::from_path(tmp.path().to_str().unwrap()).unwrap_err();
    assert!(err.contains("cannot declare 'regions'"), "unexpected error: {}", err);
  }

  #[test]
  fn indirect_near_call_is_an_error_not_a_panic() {
    let tmp = TempConfig::write("dis86_indirect_near_test.bsl", r#"
dis86 {
  code_segments {
  }
  functions {
    F1 { start 0000:0100 end "" mode near ret u16 args 0 indirect_call_location 1 }
  }
  structures {
  }
  globals {
  }
  text_section {
  }
}
"#);
    let err = Config::from_path(tmp.path().to_str().unwrap()).unwrap_err();
    assert!(err.contains("must use 'mode far'"), "unexpected error: {}", err);
  }

  #[test]
  fn untyped_text_section_parses_and_subtracts_from_code() {
    use crate::segoff::{Off, Seg, SegOff};
    let tmp = TempConfig::write("dis86_text_hole_test.bsl", r#"
dis86 {
  code_segments {
    _0000 { seg 0 name main }
  }
  functions {
    F1 { start 0000:0000 end 0000:000a mode near ret u16 args 0
         regions { _0 { start 0000:0000 end 0000:0005 } _1 { start 0000:0009 end 0000:000a } } }
  }
  structures {
  }
  globals {
  }
  text_section {
    hole { start 0000:0005 end 0000:0009 }
  }
}
"#);
    let cfg = Config::from_path(tmp.path().to_str().unwrap()).unwrap();
    assert!(cfg.text_section[0].is_data_hole());
    let so = |off| SegOff { seg: Seg::Normal(0), off: Off(off) };
    let regions = cfg.funcs[0].effective_regions();
    assert_eq!(cfg.code_regions(&regions), vec![
      FuncRegion { start: so(0), end: so(5) },
      FuncRegion { start: so(9), end: so(10) },
    ]);
  }
}
