use crate::types::*;

// FIXME: Unify this and the ast code

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathAccess {
  Array(usize),
  Struct(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Access {
  // Access path
  pub path: Vec<PathAccess>,

  // After applying access path
  pub off: usize,
  pub sz: usize,
  pub typ: Type,
}

fn determine_path_recurse(mut path: Vec<PathAccess>, types: &TypeDatabase, typ: &Type, mut access_off: usize, access_sz: usize) -> Result<Access, String> {
  if typ.is_primitive() || matches!(typ, Type::GuestPtr(_, _)) {
    return Ok(Access {
      path,
      off: access_off,
      sz: access_sz,
      typ: typ.clone(),
    });
  }

  match typ {
    Type::Array(basetype, len) => {
      let ArraySize::Known(len) = len else { return Err(format!("Array {} has unknown bound", typ)); };
      let basetype_sz = basetype.size_in_bytes().ok_or_else(|| format!("Cannot determine element size for {}", typ))?;
      if basetype_sz == 0 { return Err(format!("Array {} has zero-sized elements", typ)); }
      let idx = access_off as usize / basetype_sz;
      if idx >= *len || access_sz > basetype_sz || access_off.checked_add(access_sz).filter(|end| *end <= typ.size_in_bytes().unwrap_or(0)).is_none() {
        return Err(format!("Access range {}..{} is outside array {}", access_off, access_off.saturating_add(access_sz), typ));
      }

      path.push(PathAccess::Array(idx));
      access_off -= idx * basetype_sz;

      return determine_path_recurse(path, types, basetype, access_off, access_sz);
    }
    Type::Struct(struct_ref) => {
      let access_start = access_off;
      let access_end = access_off + access_sz;
      let s = types.lookup_struct(*struct_ref).ok_or_else(|| format!("Missing layout for {}", typ))?;
      for mbr in &s.members {
        let mbr_start = mbr.off as usize;
        let mbr_end = mbr_start.checked_add(mbr.typ.size_in_bytes().ok_or_else(|| format!("Unknown size for member {}", mbr.name))?)
          .ok_or_else(|| format!("Layout overflow at member {}", mbr.name))?;
        if !(mbr_start <= access_start && access_end <= mbr_end) { continue; }

        path.push(PathAccess::Struct(mbr.name.clone()));
        access_off -= mbr.off as usize;

        return determine_path_recurse(path, types, &mbr.typ, access_off, access_sz);
      }
      Err(format!("Access range {}..{} does not fit a member of {}", access_off, access_off.saturating_add(access_sz), typ))
    }
    _ => {
      Err(format!("Unsupported annotated access type {}", typ))
    }
  }
}

pub fn from_type_and_offset(types: &TypeDatabase, typ: &Type, off: usize, sz: usize) -> Result<Access, String> {
  determine_path_recurse(vec![], types, typ, off, sz)
}
