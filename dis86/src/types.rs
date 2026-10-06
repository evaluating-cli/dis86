use crate::config;
use std::fmt;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StructRef {
  idx: usize,
  size: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Type {
  Void, U8, U16, U32, I8, I16, I32,
  Array(Box<Type>, ArraySize),
  Ptr(Box<Type>),
  GuestPtr(Box<Type>, GuestPtrKind),
  Struct(StructRef),
  Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GuestPtrKind { Near, NearSs, NearEs, Far }

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ArraySize {
  Known(usize),
  Unknown,
}

impl Type {
  pub fn ptr(base: Type) -> Type {
    Type::Ptr(Box::new(base))
  }

  pub fn is_primitive(&self) -> bool {
    match self {
      Type::Void => true,
      Type::U8  => true,
      Type::U16 => true,
      Type::U32 => true,
      Type::I8  => true,
      Type::I16 => true,
      Type::I32 => true,
      Type::Unknown => true,
      _ => false,
    }
  }

  pub fn size_in_bytes(&self) -> Option<usize> {
    match self {
      Type::Void => None,
      Type::U8 => Some(1),
      Type::U16 => Some(2),
      Type::U32 => Some(4),
      Type::I8 => Some(1),
      Type::I16 => Some(2),
      Type::I32 => Some(4),
      Type::Array(typ, sz) => {
        let elt_sz = typ.size_in_bytes()?;
        let count = match sz {
          ArraySize::Known(n) => Some(*n),
          ArraySize::Unknown => None,
        }?;
        elt_sz.checked_mul(count)
      }
      Type::Ptr(_) => None,
      Type::GuestPtr(_, GuestPtrKind::Far) => Some(4),
      Type::GuestPtr(_, _) => Some(2),
      Type::Struct(r) => Some(r.size as usize),
      Type::Unknown => None,
    }
  }

  pub fn has_unknown_array_bound(&self) -> bool {
    match self {
      Type::Array(base, size) => matches!(size, ArraySize::Unknown) || base.has_unknown_array_bound(),
      Type::GuestPtr(base, _) | Type::Ptr(base) => base.has_unknown_array_bound(),
      _ => false,
    }
  }

  pub fn collapse_unknown_types_to_u32(&self) -> Type {
    match self {
      Type::Unknown => Type::U32,
      _ => self.clone(),
    }
  }
}

impl fmt::Display for Type {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      Type::Void => write!(f, "void"),
      Type::U8   => write!(f, "u8"),
      Type::U16  => write!(f, "u16"),
      Type::U32  => write!(f, "u32"),
      Type::I8   => write!(f, "i8"),
      Type::I16  => write!(f, "i16"),
      Type::I32  => write!(f, "i32"),
      Type::Array(typ, sz)  => {
        let mut dims = vec![sz];
        let mut base = typ.as_ref();
        while let Type::Array(inner, next) = base {
          dims.push(next);
          base = inner;
        }
        write!(f, "{}", base)?;
        for dim in dims {
          write!(f, "[")?;
          if let ArraySize::Known(n) = dim { write!(f, "{}", n)?; }
          write!(f, "]")?;
        }
        Ok(())
      }
      Type::Ptr(base)  => write!(f, "{}*", base),
      Type::GuestPtr(base, kind) => match kind {
        GuestPtrKind::Near => write!(f, "near<{}>", base),
        GuestPtrKind::NearSs => write!(f, "near_ss<{}>", base),
        GuestPtrKind::NearEs => write!(f, "near_es<{}>", base),
        GuestPtrKind::Far => write!(f, "far<{}>", base),
      },
      Type::Struct(r)  => write!(f, "struct_id_{}", r.idx),
      Type::Unknown => write!(f, "?unknown_type?"),
    }
  }
}


#[derive(Debug)]
pub struct TypeDatabase {
  structs: Vec<config::Struct>,
  basetypes: HashMap<String, Type>,
}

impl TypeDatabase {
  pub fn new() -> Self {
    let mut basetypes = HashMap::new();
    basetypes.insert("void".to_string(), Type::Void);
    basetypes.insert("u8".to_string(),   Type::U8);
    basetypes.insert("u16".to_string(),  Type::U16);
    basetypes.insert("u32".to_string(),  Type::U32);
    basetypes.insert("i8".to_string(),   Type::I8);
    basetypes.insert("i16".to_string(),  Type::I16);
    basetypes.insert("i32".to_string(),  Type::I32);

    Self { structs: vec![], basetypes }
  }

  pub fn append_struct(&mut self, s: &config::Struct) {
    let r = StructRef { idx: self.structs.len(), size: s.size };
    self.structs.push(s.clone());
    self.basetypes.insert(s.name.to_string(), Type::Struct(r));
  }

  pub fn lookup_struct(&self, r: StructRef) -> Option<&config::Struct> {
    self.structs.get(r.idx)
  }

  pub fn parse_type(&self, s: &str) -> Result<Type, String> {
    let s = s.trim();
    if let Some(typ) = self.basetypes.get(s) { return Ok(typ.clone()); }

    for (prefix, kind) in [("near_ss<", GuestPtrKind::NearSs), ("near_es<", GuestPtrKind::NearEs),
                           ("near<", GuestPtrKind::Near), ("far<", GuestPtrKind::Far)] {
      if s.starts_with(prefix) && s.ends_with('>') {
        let inner = &s[prefix.len()..s.len()-1];
        if inner.is_empty() { break; }
        return Ok(Type::GuestPtr(Box::new(self.parse_type(inner)?), kind));
      }
    }

    if let Some(open) = s.find('[') {
      if !s.ends_with(']') { return Err(format!("Failed to parse type: '{}'", s)); }
      let base = self.parse_type(&s[..open])?;
      let mut dims = vec![];
      let mut rest = &s[open..];
      while !rest.is_empty() {
        let after_open = rest.strip_prefix('[')
          .ok_or_else(|| format!("Invalid suffix after array dimension in type: '{}'", s))?;
        let end = after_open.find(']').ok_or_else(|| format!("Failed to parse type: '{}'", s))?;
        let dim = &after_open[..end];
        let size = if dim.is_empty() { ArraySize::Unknown } else {
          let n: usize = dim.parse().map_err(|_| format!("Invalid array bound '{}' in type '{}'", dim, s))?;
          if n == 0 { return Err(format!("Array bound must be positive in type '{}'", s)); }
          ArraySize::Known(n)
        };
        dims.push(size);
        rest = &after_open[end+1..];
        if !rest.is_empty() && !rest.starts_with('[') {
          return Err(format!("Invalid suffix after array dimension in type: '{}'", s));
        }
      }
      let mut typ = base;
      for size in dims.into_iter().rev() { typ = Type::Array(Box::new(typ), size); }
      return Ok(typ);
    }
    Err(format!("Failed to parse type: '{}'", s))
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn recursive_arrays_preserve_c_dimension_order_and_size() {
    let db = TypeDatabase::new();
    let typ = db.parse_type("u16[3][5]").unwrap();
    assert_eq!(typ, Type::Array(Box::new(Type::Array(Box::new(Type::U16), ArraySize::Known(5))), ArraySize::Known(3)));
    assert_eq!(typ.size_in_bytes(), Some(30));
    assert_eq!(typ.to_string(), "u16[3][5]");
  }

  #[test]
  fn guest_pointer_annotations_have_guest_widths() {
    let db = TypeDatabase::new();
    for annotation in ["near<u16>", "near_ss<u16>", "near_es<u16>"] {
      let typ = db.parse_type(annotation).unwrap();
      assert_eq!(typ.size_in_bytes(), Some(2));
    }
    assert_eq!(db.parse_type("far<u16>").unwrap().size_in_bytes(), Some(4));
    assert!(db.parse_type("near<>" ).is_err());
    assert!(db.parse_type("u16[2][x]").is_err());
    assert!(db.parse_type("u16[2]junk[3]").is_err());
  }
}
